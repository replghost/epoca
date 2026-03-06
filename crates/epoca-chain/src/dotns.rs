//! DOTNS (DOT Name Service) on-chain resolution.
//!
//! Resolves `.dot` domain names to IPFS content hashes by querying the
//! DOTNS content resolver contract on Asset Hub Paseo via `state_call`.
//!
//! Flow:
//! 1. ENS-style namehash of the domain (keccak256)
//! 2. ABI-encode the `contenthash(bytes32)` call
//! 3. SCALE-encode `ReviveApi::call()` parameters
//! 4. Send `state_call("ReviveApi_call", params)` via JSON-RPC HTTP
//! 5. Decode the response to extract the IPFS CID

use std::collections::HashMap;
use tiny_keccak::{Hasher, Keccak};

/// DOTNS content resolver contract on Asset Hub Paseo.
const CONTENT_RESOLVER: [u8; 20] = hex_addr("7756DF72CBc7f062e7403cD59e45fBc78bed1cD7");

/// Solidity function selector for `contenthash(bytes32)`.
const CONTENTHASH_SELECTOR: [u8; 4] = [0xbc, 0x1c, 0x58, 0xd1];

/// JSON-RPC endpoints for Asset Hub Paseo (tried in order).
const RPC_ENDPOINTS: &[&str] = &[
    "https://sys.ibp.network/asset-hub-paseo",
    "https://asset-hub-paseo.dotters.network",
];

/// IPFS gateway for fetching resolved content.
const IPFS_GATEWAY: &str = "https://ipfs.dotspark.app";

/// Compile-time hex string to 20-byte address.
const fn hex_addr(s: &str) -> [u8; 20] {
    let b = s.as_bytes();
    assert!(b.len() == 40, "address must be 40 hex chars");
    let mut out = [0u8; 20];
    let mut i = 0;
    while i < 20 {
        out[i] = (hex_nibble(b[i * 2]) << 4) | hex_nibble(b[i * 2 + 1]);
        i += 1;
    }
    out
}

const fn hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("invalid hex"),
    }
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    hasher.update(data);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

/// ENS-style namehash: `namehash("mytestapp.dot")`.
/// Split by `.`, reverse, iteratively hash.
fn namehash(domain: &str) -> [u8; 32] {
    if domain.is_empty() {
        return [0u8; 32];
    }
    let labels: Vec<&str> = domain.split('.').collect();
    let mut node = [0u8; 32];
    for label in labels.into_iter().rev() {
        let label_hash = keccak256(label.as_bytes());
        let mut buf = [0u8; 64];
        buf[..32].copy_from_slice(&node);
        buf[32..].copy_from_slice(&label_hash);
        node = keccak256(&buf);
    }
    node
}

/// ABI-encode `contenthash(bytes32 node)` call data.
fn encode_contenthash_call(node: &[u8; 32]) -> Vec<u8> {
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&CONTENTHASH_SELECTOR);
    data.extend_from_slice(node);
    data
}

/// SCALE-encode the parameters for `ReviveApi::call()` runtime API.
///
/// Parameters (in order, from runtime metadata):
///   origin: AccountId32 ([u8; 32])
///   dest: H160 ([u8; 20])
///   value: u128 (BalanceOf<T>, 16 bytes LE)
///   gas_limit: Option<Weight> where Weight { ref_time: Compact<u64>, proof_size: Compact<u64> }
///   storage_deposit_limit: Option<u128>
///   input_data: Vec<u8>
fn scale_encode_revive_call(dest: &[u8; 20], input_data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128 + input_data.len());

    // origin: AccountId32 (Alice — matches dot.li's dry-run convention)
    buf.extend_from_slice(&[
        0xd4, 0x35, 0x93, 0xc7, 0x15, 0xfd, 0xd3, 0x1c, 0x61, 0x14, 0x1a, 0xbd, 0x04, 0xa9,
        0x9f, 0xd6, 0x82, 0x2c, 0x85, 0x58, 0x85, 0x4c, 0xcd, 0xe3, 0x9a, 0x56, 0x84, 0xe7,
        0xa5, 0x6d, 0xa2, 0x7d,
    ]);

    // dest: H160
    buf.extend_from_slice(dest);

    // value: u128 = 0
    buf.extend_from_slice(&0u128.to_le_bytes());

    // gas_limit: Option<Weight> = Some(Weight { ref_time: MAX, proof_size: MAX })
    buf.push(0x01); // Some
    scale_compact_u64(&mut buf, u64::MAX); // ref_time
    scale_compact_u64(&mut buf, u64::MAX); // proof_size

    // storage_deposit_limit: Option<u128> = Some(u64::MAX as u128)
    buf.push(0x01); // Some
    buf.extend_from_slice(&(u64::MAX as u128).to_le_bytes());

    // input_data: Vec<u8> = compact_len ++ bytes
    scale_compact_len(&mut buf, input_data.len());
    buf.extend_from_slice(input_data);

    buf
}

/// SCALE compact encoding for a u64 value.
fn scale_compact_u64(buf: &mut Vec<u8>, val: u64) {
    if val < 64 {
        buf.push((val as u8) << 2);
    } else if val < 0x4000 {
        let v = ((val as u16) << 2) | 1;
        buf.extend_from_slice(&v.to_le_bytes());
    } else if val < 0x4000_0000 {
        let v = ((val as u32) << 2) | 2;
        buf.extend_from_slice(&v.to_le_bytes());
    } else {
        // Big integer mode: upper 6 bits = (byte_count - 4), lower 2 bits = 0b11
        // For u64, we need up to 8 bytes
        let bytes = val.to_le_bytes();
        let len = 8 - (val.leading_zeros() / 8) as usize;
        let len = len.max(4); // minimum 4 bytes in big mode
        let prefix = (((len - 4) as u8) << 2) | 3;
        buf.push(prefix);
        buf.extend_from_slice(&bytes[..len]);
    }
}

/// SCALE compact encoding for a length prefix.
fn scale_compact_len(buf: &mut Vec<u8>, n: usize) {
    if n < 64 {
        buf.push((n as u8) << 2);
    } else if n < 16384 {
        let v = ((n as u16) << 2) | 1;
        buf.extend_from_slice(&v.to_le_bytes());
    } else if n < 1_073_741_824 {
        let v = ((n as u32) << 2) | 2;
        buf.extend_from_slice(&v.to_le_bytes());
    } else {
        // Big mode — shouldn't happen for our use case
        panic!("compact encoding: value too large");
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        out.push(u8::from_str_radix(&s[i..i + 2], 16).ok()?);
    }
    Some(out)
}

/// Send a `state_call` JSON-RPC request via HTTP.
fn rpc_state_call(method: &str, params_hex: &str) -> Result<Vec<u8>, String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "state_call",
        "params": [method, params_hex]
    });
    let payload_str = payload.to_string();

    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(15)))
            .build(),
    );

    for endpoint in RPC_ENDPOINTS {
        log::info!("[dotns] trying RPC: {endpoint}");
        let result = agent
            .post(*endpoint)
            .content_type("application/json")
            .send(payload_str.as_bytes());
        match result {
            Ok(resp) => {
                let resp_bytes = match resp.into_body().read_to_vec() {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("[dotns] failed to read response from {endpoint}: {e}");
                        continue;
                    }
                };
                let body: serde_json::Value = match serde_json::from_slice(&resp_bytes) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("[dotns] failed to parse response from {endpoint}: {e}");
                        continue;
                    }
                };
                log::info!("[dotns] response: {body}");
                if let Some(err) = body.get("error") {
                    log::warn!("[dotns] RPC error from {endpoint}: {err}");
                    continue;
                }
                if let Some(result) = body.get("result").and_then(|v| v.as_str()) {
                    return hex_decode(result)
                        .ok_or_else(|| format!("invalid hex in RPC response: {result}"));
                }
                log::warn!("[dotns] unexpected response from {endpoint}: {body}");
                continue;
            }
            Err(e) => {
                log::warn!("[dotns] HTTP error for {endpoint}: {e}");
                continue;
            }
        }
    }

    Err("all RPC endpoints failed".into())
}

/// Decode the `ReviveApi::call()` SCALE response to extract the return data.
///
/// The response is a `ContractResult<ExecReturnValue>` which is roughly:
///   gas_consumed: Weight { ref_time: Compact<u64>, proof_size: Compact<u64> }
///   gas_required: Weight { ref_time: Compact<u64>, proof_size: Compact<u64> }
///   storage_deposit: StorageDeposit enum (1 byte tag + u128)
///   debug_message: Vec<u8> (compact len + bytes)
///   result: Result<ExecReturnValue, DispatchError>
///     Ok variant (0x00):
///       flags: u32 (4 bytes)
///       data: Vec<u8> (compact len + bytes)
fn decode_contract_result(response: &[u8]) -> Result<Vec<u8>, String> {
    let mut pos = 0;

    // gas_consumed: Weight { ref_time: Compact<u64>, proof_size: Compact<u64> }
    let (_, n) = decode_scale_compact(&response[pos..])?;
    pos += n;
    let (_, n) = decode_scale_compact(&response[pos..])?;
    pos += n;

    // gas_required: Weight { ref_time: Compact<u64>, proof_size: Compact<u64> }
    let (_, n) = decode_scale_compact(&response[pos..])?;
    pos += n;
    let (_, n) = decode_scale_compact(&response[pos..])?;
    pos += n;

    // storage_deposit: StorageDeposit enum (1 byte variant + u128)
    if pos + 17 > response.len() {
        return Err("response too short (storage_deposit)".into());
    }
    pos += 1 + 16;

    // pallet-revive adds extra fields not present in pallet-contracts:
    // - Option<Balance> (1 tag + 16 u128 if Some) — likely storage deposit limit
    // - Balance (16 bytes u128) — likely eth gas price or fee
    if pos >= response.len() {
        return Err("response too short (extra fields)".into());
    }
    let opt_tag = response[pos];
    pos += 1; // Option tag
    if opt_tag == 1 {
        pos += 16; // Some(u128)
    }
    pos += 16; // plain u128

    // debug_message: Vec<u8>
    let (msg_len, bytes_read) = decode_scale_compact(&response[pos..])?;
    pos += bytes_read + msg_len;

    // result: ExecReturnValue { flags: u32, data: Vec<u8> } (no Result wrapper in pallet-revive)
    if pos + 4 > response.len() {
        return Err("response too short (flags)".into());
    }
    pos += 4; // flags: u32

    // data: Vec<u8>
    let (data_len, bytes_read) = decode_scale_compact(&response[pos..])?;
    pos += bytes_read;

    if pos + data_len > response.len() {
        return Err(format!(
            "data extends beyond response (pos={pos}, data_len={data_len}, total={})",
            response.len()
        ));
    }

    Ok(response[pos..pos + data_len].to_vec())
}

/// Decode a SCALE compact-encoded integer, returning (value, bytes_consumed).
fn decode_scale_compact(data: &[u8]) -> Result<(usize, usize), String> {
    if data.is_empty() {
        return Err("empty data for compact decode".into());
    }
    let mode = data[0] & 0b11;
    match mode {
        0 => Ok(((data[0] >> 2) as usize, 1)),
        1 => {
            if data.len() < 2 {
                return Err("compact: need 2 bytes".into());
            }
            let v = u16::from_le_bytes([data[0], data[1]]) >> 2;
            Ok((v as usize, 2))
        }
        2 => {
            if data.len() < 4 {
                return Err("compact: need 4 bytes".into());
            }
            let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) >> 2;
            Ok((v as usize, 4))
        }
        3 => {
            // Big integer mode
            let bytes_needed = (data[0] >> 2) as usize + 4;
            if data.len() < 1 + bytes_needed {
                return Err("compact: big mode insufficient data".into());
            }
            let mut val: usize = 0;
            for i in (0..bytes_needed).rev() {
                val = (val << 8) | data[1 + i] as usize;
            }
            Ok((val, 1 + bytes_needed))
        }
        _ => unreachable!(),
    }
}

/// Decode ABI-encoded bytes return value from Solidity.
/// The contenthash function returns `bytes` which is ABI-encoded as:
///   offset (32 bytes, = 0x20)
///   length (32 bytes)
///   data (padded to 32-byte boundary)
fn decode_abi_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 64 {
        return Err(format!("ABI bytes too short: {} bytes", data.len()));
    }
    // First 32 bytes: offset (should be 0x20 = 32)
    // Next 32 bytes: length
    let len = u32::from_be_bytes([data[60], data[61], data[62], data[63]]) as usize;
    if 64 + len > data.len() {
        return Err(format!("ABI bytes: length {len} exceeds data"));
    }
    Ok(data[64..64 + len].to_vec())
}

/// Parse a contenthash value to extract an IPFS CIDv1.
/// The contenthash format follows EIP-1577:
///   0xe3 0x01 0x01 <multihash>  (IPFS, codec dag-pb, CIDv1)
///   0xe5 0x01 ...               (Swarm)
///
/// We decode the CID and return it as a base32-encoded string.
fn contenthash_to_cid(data: &[u8]) -> Result<String, String> {
    if data.is_empty() {
        return Err("empty contenthash".into());
    }

    // EIP-1577 contenthash uses multicodec varint prefix.
    // IPFS namespace = 0xe3 = 227, encoded as varint `e3 01` (2 bytes).
    // Swarm namespace = 0xe5 = 229, encoded as varint `e5 01` (2 bytes).
    let (codec, varint_len) = decode_unsigned_varint(data);
    match codec {
        0xe3 => {
            // IPFS — skip the namespace varint, rest is the CID
            let cid_bytes = &data[varint_len..];
            Ok(format!("b{}", base32_encode(cid_bytes)))
        }
        0xe5 => Err("Swarm contenthash not supported".into()),
        _ => {
            // Try treating the whole thing as raw CID bytes
            Ok(format!("b{}", base32_encode(data)))
        }
    }
}

/// Decode an unsigned varint (LEB128).
fn decode_unsigned_varint(data: &[u8]) -> (u64, usize) {
    let mut value: u64 = 0;
    let mut shift = 0;
    for (i, &byte) in data.iter().enumerate() {
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return (value, i + 1);
        }
        shift += 7;
    }
    (value, data.len())
}

/// RFC 4648 base32 encoding (lowercase, no padding).
fn base32_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut result = String::new();
    let mut bits: u32 = 0;
    let mut num_bits: u32 = 0;
    for &byte in data {
        bits = (bits << 8) | byte as u32;
        num_bits += 8;
        while num_bits >= 5 {
            num_bits -= 5;
            result.push(ALPHABET[((bits >> num_bits) & 0x1f) as usize] as char);
        }
    }
    if num_bits > 0 {
        result.push(ALPHABET[((bits << (5 - num_bits)) & 0x1f) as usize] as char);
    }
    result
}

/// Resolve a `.dot` domain name to an IPFS CID via DOTNS on-chain lookup.
///
/// Returns the CID string (e.g. "bafybeig...") or an error.
pub fn resolve_dotns(name: &str) -> Result<String, String> {
    let domain = if name.ends_with(".dot") {
        name.to_string()
    } else {
        format!("{name}.dot")
    };

    log::info!("[dotns] resolving: {domain}");

    // 1. Compute namehash
    let node = namehash(&domain);
    log::info!("[dotns] namehash: {}", hex_encode(&node));

    // 2. ABI-encode contenthash(bytes32) call
    let call_data = encode_contenthash_call(&node);

    // 3. SCALE-encode ReviveApi::call() params
    let params = scale_encode_revive_call(&CONTENT_RESOLVER, &call_data);
    let params_hex = hex_encode(&params);

    // 4. RPC state_call
    let response = rpc_state_call("ReviveApi_call", &params_hex)?;
    log::info!("[dotns] got response: {} bytes", response.len());

    // 5. Decode ContractResult → return data
    let return_data = decode_contract_result(&response)?;
    log::info!("[dotns] contract return data: {} bytes", return_data.len());

    if return_data.is_empty() {
        return Err("domain not registered (empty return data)".into());
    }

    // 6. Decode ABI-encoded bytes
    let contenthash = decode_abi_bytes(&return_data)?;
    log::info!("[dotns] contenthash: {} bytes", contenthash.len());

    if contenthash.is_empty() {
        return Err("domain has no contenthash set".into());
    }

    // 7. Parse contenthash → CID
    let cid = contenthash_to_cid(&contenthash)?;
    log::info!("[dotns] resolved CID: {cid}");

    Ok(cid)
}

/// Fetch content from IPFS and return a map of filename → bytes.
///
/// Tries the IPFS gateway to fetch the CID. If the content is a single HTML file,
/// returns it as `{"index.html": bytes}`. If it's a directory listing, fetches
/// individual files.
pub fn fetch_ipfs(cid: &str) -> Result<HashMap<String, Vec<u8>>, String> {
    let url = format!("{IPFS_GATEWAY}/ipfs/{cid}/");
    log::info!("[dotns] fetching IPFS: {url}");

    // First, try fetching as a directory (with trailing slash)
    match fetch_ipfs_url(&url) {
        Ok((content_type, body)) => {
            if content_type.contains("text/html") && looks_like_directory_listing(&body) {
                // Parse directory listing and fetch individual files
                return fetch_ipfs_directory(cid, &body);
            }
            // Single file — treat as index.html
            let mut assets = HashMap::new();
            assets.insert("index.html".into(), body);
            return Ok(assets);
        }
        Err(_) => {}
    }

    // Fallback: fetch without trailing slash (single file)
    let url = format!("{IPFS_GATEWAY}/ipfs/{cid}");
    let (_, body) = fetch_ipfs_url(&url)?;
    let mut assets = HashMap::new();
    assets.insert("index.html".into(), body);
    Ok(assets)
}

fn fetch_ipfs_url(url: &str) -> Result<(String, Vec<u8>), String> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build(),
    );
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("IPFS fetch failed: {e}"))?;

    let content_type = resp
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body = resp.into_body()
        .read_to_vec()
        .map_err(|e| format!("IPFS read failed: {e}"))?;

    Ok((content_type, body))
}

fn looks_like_directory_listing(body: &[u8]) -> bool {
    // IPFS directory listings contain links to files
    let s = std::str::from_utf8(body).unwrap_or("");
    s.contains("Index of") || s.contains("<a href=")
}

fn fetch_ipfs_directory(cid: &str, listing_html: &[u8]) -> Result<HashMap<String, Vec<u8>>, String> {
    let html = std::str::from_utf8(listing_html).map_err(|e| format!("invalid UTF-8: {e}"))?;
    let mut assets = HashMap::new();

    // Parse simple anchor tags to extract filenames
    for segment in html.split("<a href=\"") {
        if let Some(end) = segment.find('"') {
            let filename = &segment[..end];
            // Skip parent directory links and anchors
            if filename.is_empty()
                || filename == ".."
                || filename == "/"
                || filename.starts_with('?')
                || filename.starts_with('#')
            {
                continue;
            }
            let clean = filename.trim_start_matches("./").trim_end_matches('/');
            if clean.is_empty() || clean.contains("..") {
                continue;
            }

            let file_url = format!("{IPFS_GATEWAY}/ipfs/{cid}/{clean}");
            log::info!("[dotns] fetching: {clean}");
            match fetch_ipfs_url(&file_url) {
                Ok((_, body)) => {
                    assets.insert(clean.to_string(), body);
                }
                Err(e) => {
                    log::warn!("[dotns] failed to fetch {clean}: {e}");
                }
            }
        }
    }

    if assets.is_empty() {
        return Err("directory listing contained no files".into());
    }

    Ok(assets)
}

/// Full resolution pipeline: DOTNS lookup → IPFS fetch → asset map.
pub fn resolve_and_fetch(name: &str) -> Result<HashMap<String, Vec<u8>>, String> {
    let cid = resolve_dotns(name)?;
    fetch_ipfs(&cid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namehash_empty() {
        assert_eq!(namehash(""), [0u8; 32]);
    }

    #[test]
    fn test_namehash_dot() {
        // namehash("dot") should be keccak256(zeros32 ++ keccak256("dot"))
        let expected_label = keccak256(b"dot");
        let mut buf = [0u8; 64];
        buf[32..].copy_from_slice(&expected_label);
        let expected = keccak256(&buf);
        assert_eq!(namehash("dot"), expected);
    }

    #[test]
    fn test_hex_roundtrip() {
        let data = vec![0xde, 0xad, 0xbe, 0xef];
        let hex = hex_encode(&data);
        assert_eq!(hex, "0xdeadbeef");
        assert_eq!(hex_decode(&hex).unwrap(), data);
    }

    #[test]
    #[ignore] // requires network
    fn test_resolve_mytestapp() {
        let cid = resolve_dotns("mytestapp").expect("DOTNS resolution failed");
        assert_eq!(cid, "bafybeigcglpcphjr7nb3ykpt7yalgkjte43pkqswhi2ioajqwpw2khikda");
    }

    #[test]
    #[ignore] // requires network
    fn test_full_pipeline_mytestapp() {
        let assets = resolve_and_fetch("mytestapp").expect("resolve_and_fetch failed");
        eprintln!("Fetched {} assets:", assets.len());
        for (name, data) in &assets {
            eprintln!("  {name}: {} bytes", data.len());
        }
        assert!(assets.contains_key("index.html"), "missing index.html");
    }

    #[test]
    fn test_encoding_matches_dotli() {
        // Exact params captured from dot.li's state_call for mytestapp.dot
        let expected = "0xd43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d7756df72cbc7f062e7403cd59e45fbc78bed1cd7000000000000000000000000000000000113ffffffffffffffff13ffffffffffffffff01ffffffffffffffff000000000000000090bc1c58d1ea3cb49a7f22581a2b768fdfd30be01a398514934d65b60e158ee9ee93c20894";

        let node = namehash("mytestapp.dot");
        let call_data = encode_contenthash_call(&node);
        let params = scale_encode_revive_call(&CONTENT_RESOLVER, &call_data);
        let actual = hex_encode(&params);

        assert_eq!(actual, expected, "encoding mismatch:\nactual:   {actual}\nexpected: {expected}");
    }

    #[test]
    fn test_scale_compact() {
        let mut buf = Vec::new();
        scale_compact_len(&mut buf, 0);
        assert_eq!(buf, vec![0x00]);

        buf.clear();
        scale_compact_len(&mut buf, 1);
        assert_eq!(buf, vec![0x04]);

        buf.clear();
        scale_compact_len(&mut buf, 36);
        assert_eq!(buf, vec![0x90]); // 36 << 2 = 144 (single-byte mode, 36 < 64)
    }
}
