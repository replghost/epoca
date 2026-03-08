//! Pure Substrate extrinsic construction.
//!
//! No GPUI dependency, no async — only SCALE encoding and signing payload
//! construction. Suitable for use from any thread or context.
//!
//! References:
//! - SCALE compact encoding: <https://docs.substrate.io/reference/scale-codec/>
//! - Substrate extrinsic format v4: <https://docs.substrate.io/reference/transaction-format/>
//! - CheckMetadataHash signed extension: added in Polkadot runtime ≥ 1_001_000

/// SCALE compact encoding for u128 values.
///
/// Encoding modes:
/// - `n < 64`:      single byte `n << 2 | 0b00`
/// - `n < 2^14`:    two bytes LE `(n << 2 | 0b01) as u16`
/// - `n < 2^30`:    four bytes LE `(n << 2 | 0b10) as u32`
/// - `n >= 2^30`:   big-integer mode `[0b11 | (byte_count-4)<<2] ++ n as LE bytes`
pub fn compact_encode_u128(n: u128) -> Vec<u8> {
    if n < 64 {
        vec![(n as u8) << 2]
    } else if n < (1 << 14) {
        let v = ((n as u16) << 2) | 0b01;
        v.to_le_bytes().to_vec()
    } else if n < (1 << 30) {
        let v = ((n as u32) << 2) | 0b10;
        v.to_le_bytes().to_vec()
    } else {
        // Big-integer mode: prefix byte encodes (byte_count - 4) in bits [7:2],
        // mode bits [1:0] = 0b11.  Minimum 4 bytes after prefix.
        let bytes = n.to_le_bytes(); // 16 bytes, LE
        // Determine the minimum number of bytes needed to represent n.
        let needed = {
            let mut count = 16usize;
            while count > 1 && bytes[count - 1] == 0 {
                count -= 1;
            }
            count.max(4) // big-integer mode requires at least 4 bytes
        };
        let prefix = (((needed - 4) as u8) << 2) | 0b11;
        let mut out = Vec::with_capacity(1 + needed);
        out.push(prefix);
        out.extend_from_slice(&bytes[..needed]);
        out
    }
}

/// SCALE compact encoding for u64 values (delegates to the u128 version).
pub fn compact_encode(n: u64) -> Vec<u8> {
    compact_encode_u128(n as u128)
}

/// Encode a mortal era as 2 LE bytes.
///
/// `period` is rounded up to the next power of two, then clamped to `[4, 65536]`.
/// `block_number` is the current (or mortality checkpoint) block number.
///
/// The 16-bit encoded value carries:
/// - bits `[3:0]`: `trailing_zeros(period) - 1`   (period selector)
/// - bits `[15:4]`: quantized phase
pub fn encode_mortal_era(block_number: u64, period: u64) -> [u8; 2] {
    // Round period to the next power of two, clamped to [4, 65536].
    let period = period.next_power_of_two().clamp(4, 65536);
    let phase = block_number % period;
    let quantize_factor = (period >> 12).max(1);
    let quantized_phase = (phase / quantize_factor) * quantize_factor;
    // Encoded value fits in u16.
    let encoded =
        (period.trailing_zeros() as u64 - 1) | ((quantized_phase / quantize_factor) << 4);
    (encoded as u16).to_le_bytes()
}

/// All chain state required to build a signed extrinsic.
pub struct ExtrinsicParams {
    pub spec_version: u32,
    pub tx_version: u32,
    /// Genesis hash of the chain (32 bytes).
    pub genesis_hash: [u8; 32],
    /// Finalized block hash used as the mortality checkpoint (32 bytes).
    pub mortality_checkpoint: [u8; 32],
    /// Block number at the mortality checkpoint — used to encode the mortal era.
    pub block_number: u64,
    pub nonce: u64,
    /// Transaction tip (usually 0).
    pub tip: u128,
}

/// Build the "extra" bytes that travel between the signature and the call data
/// inside the extrinsic body, and also inside the signing payload.
///
/// Layout:
/// 1. Mortal era (2 bytes)
/// 2. Compact-encoded nonce
/// 3. Compact-encoded tip (u128)
/// 4. `0x00` — `CheckMetadataHash` mode = Disabled
pub fn build_extra(params: &ExtrinsicParams) -> Vec<u8> {
    let era = encode_mortal_era(params.block_number, 64);
    let mut extra = Vec::new();
    extra.extend_from_slice(&era);
    extra.extend_from_slice(&compact_encode(params.nonce));
    extra.extend_from_slice(&compact_encode_u128(params.tip));
    extra.push(0x00); // CheckMetadataHash::Disabled
    extra
}

/// Build the signing payload for a Substrate extrinsic v4.
///
/// Layout: `call_data || extra || additional_signed`
///
/// Additional signed (only in signing payload, not in extrinsic body):
/// - `spec_version` (u32 LE)
/// - `tx_version` (u32 LE)
/// - `genesis_hash` (32 bytes)
/// - `mortality_checkpoint` (32 bytes)
/// - No extra byte for metadata hash because `CheckMetadataHash::Disabled` (0x00
///   mode byte in `extra`) means the hash is omitted from additional_signed.
///
/// If the assembled payload exceeds 256 bytes it is Blake2b-256 hashed before
/// returning, matching the Substrate signer convention.
pub fn build_signing_payload(call_data: &[u8], params: &ExtrinsicParams) -> Vec<u8> {
    let extra = build_extra(params);

    let mut payload = Vec::with_capacity(call_data.len() + extra.len() + 72);
    payload.extend_from_slice(call_data);
    payload.extend_from_slice(&extra);

    // Additional signed: spec_version, tx_version, genesis_hash, mortality_checkpoint.
    payload.extend_from_slice(&params.spec_version.to_le_bytes());
    payload.extend_from_slice(&params.tx_version.to_le_bytes());
    payload.extend_from_slice(&params.genesis_hash);
    payload.extend_from_slice(&params.mortality_checkpoint);

    // If the payload exceeds 256 bytes, hash it with Blake2b-256.
    if payload.len() > 256 {
        use blake2::Digest;
        let hash = blake2::Blake2b::<blake2::digest::consts::U32>::digest(&payload);
        hash.to_vec()
    } else {
        payload
    }
}

/// Encode the final signed extrinsic as a `0x`-prefixed hex string.
///
/// Wire format:
/// ```text
/// compact_length || version_byte || address || signature || extra || call_data
/// ```
///
/// - `compact_length`: SCALE compact-encoded length of everything after it.
/// - `0x84`: extrinsic version 4 (`0x04`) with the signed bit set (`0x80`).
/// - `MultiAddress::Id`: `0x00` prefix + 32-byte public key.
/// - `MultiSignature::Sr25519`: `0x01` prefix + 64-byte signature.
/// - `extra`: mortal era, compact nonce, compact tip, CheckMetadataHash mode.
/// - `call_data`: SCALE-encoded pallet call.
pub fn encode_signed_extrinsic(
    call_data: &[u8],
    public_key: &[u8; 32],
    signature: &[u8; 64],
    params: &ExtrinsicParams,
) -> String {
    let extra = build_extra(params);

    // Build the body (everything after the compact length prefix).
    let mut body = Vec::new();

    // Extrinsic version 4 | signed bit.
    body.push(0x84);

    // MultiAddress::Id (variant 0) + 32-byte pubkey.
    body.push(0x00);
    body.extend_from_slice(public_key);

    // MultiSignature::Sr25519 (variant 1) + 64-byte signature.
    body.push(0x01);
    body.extend_from_slice(signature);

    // Extra signed extensions.
    body.extend_from_slice(&extra);

    // Call data.
    body.extend_from_slice(call_data);

    // Prepend compact-encoded length of body.
    let mut out = compact_encode(body.len() as u64);
    out.extend_from_slice(&body);

    // Hex-encode with 0x prefix.
    let mut hex = String::with_capacity(2 + out.len() * 2);
    hex.push_str("0x");
    for byte in &out {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // compact_encode / compact_encode_u128
    // -------------------------------------------------------------------------

    #[test]
    fn compact_encode_single_byte_mode() {
        // Values < 64 fit in one byte: value << 2 | 0b00.
        assert_eq!(compact_encode(0), vec![0x00]);
        assert_eq!(compact_encode(1), vec![0x04]);
        assert_eq!(compact_encode(63), vec![0xfc]);
    }

    #[test]
    fn compact_encode_two_byte_mode() {
        // 64 <= n < 16384 → 2 bytes LE.
        // 64 → (64 << 2) | 1 = 257 = 0x0101 LE → [0x01, 0x01]
        assert_eq!(compact_encode(64), vec![0x01, 0x01]);
        // 16383 → (16383 << 2) | 1 = 65533 = 0xfffd LE → [0xfd, 0xff]
        assert_eq!(compact_encode(16383), vec![0xfd, 0xff]);
    }

    #[test]
    fn compact_encode_four_byte_mode() {
        // 16384 <= n < 2^30 → 4 bytes LE.
        // 16384 → (16384 << 2) | 2 = 65538 = 0x00010002 LE → [0x02, 0x00, 0x01, 0x00]
        assert_eq!(compact_encode(16384), vec![0x02, 0x00, 0x01, 0x00]);
        // 2^30 - 1 = 1073741823 → ((2^30-1) << 2) | 2 = 4294967294 = 0xfffffffe LE
        assert_eq!(
            compact_encode((1u64 << 30) - 1),
            vec![0xfe, 0xff, 0xff, 0xff]
        );
    }

    #[test]
    fn compact_encode_big_integer_mode() {
        // 2^30 = 1073741824 — first value that requires big-integer mode.
        // byte_count for 2^30: needs 5 bytes (0x40000000 = 4 bytes but value
        // doesn't fit in 30-bit field, so minimum 4 in big mode).
        // 2^30 = 0x40_00_00_00: 4 bytes LE, but big mode minimum is 4 bytes.
        // prefix = (4-4)<<2 | 3 = 0x03
        let enc = compact_encode(1u64 << 30);
        assert_eq!(enc[0], 0x03); // prefix: (0 extra bytes beyond 4) | 0b11
        // The next 4 bytes are 1073741824 LE = [0x00, 0x00, 0x00, 0x40]
        assert_eq!(&enc[1..], &[0x00, 0x00, 0x00, 0x40]);
    }

    #[test]
    fn compact_encode_u128_big_value() {
        // u128 value that doesn't fit in u64 — exercises the u128-specific path.
        let n: u128 = (u64::MAX as u128) + 1; // 2^64
        let enc = compact_encode_u128(n);
        assert_eq!(enc[0] & 0b11, 0b11, "must be big-integer mode");
        // Decode manually: strip prefix, read LE bytes.
        let byte_count = ((enc[0] >> 2) as usize) + 4;
        let mut val = 0u128;
        for i in (0..byte_count).rev() {
            val = (val << 8) | enc[1 + i] as u128;
        }
        assert_eq!(val, n);
    }

    // -------------------------------------------------------------------------
    // encode_mortal_era
    // -------------------------------------------------------------------------

    #[test]
    fn mortal_era_known_value() {
        // With block_number=100, period=64 (already a power of two):
        //   period = 64, phase = 100 % 64 = 36
        //   quantize_factor = max(64 >> 12, 1) = 1
        //   quantized_phase = 36
        //   encoded = (trailing_zeros(64) - 1) | (36 << 4)
        //           = (6 - 1) | 576
        //           = 5 | 576 = 581 = 0x0245 LE → [0x45, 0x02]
        let era = encode_mortal_era(100, 64);
        let encoded_val = u16::from_le_bytes(era);
        // period selector bits [3:0] = trailing_zeros(64) - 1 = 5
        assert_eq!(encoded_val & 0x0f, 5, "period selector bits incorrect");
        // phase bits [15:4] = quantized_phase / quantize_factor = 36
        assert_eq!(encoded_val >> 4, 36, "phase bits incorrect");
    }

    #[test]
    fn mortal_era_period_clamped_to_min() {
        // period=1 → next_power_of_two=1 → clamp to 4.
        let era = encode_mortal_era(0, 1);
        let encoded_val = u16::from_le_bytes(era);
        // trailing_zeros(4) = 2, so period selector = 2 - 1 = 1
        assert_eq!(encoded_val & 0x0f, 1, "period selector for period=4");
    }

    #[test]
    fn mortal_era_period_clamped_to_max() {
        // period=131072 → next_power_of_two=131072 → clamp to 65536.
        let era = encode_mortal_era(0, 131072);
        let encoded_val = u16::from_le_bytes(era);
        // trailing_zeros(65536) = 16, selector = 16 - 1 = 15
        assert_eq!(encoded_val & 0x0f, 15, "period selector for period=65536");
    }

    // -------------------------------------------------------------------------
    // build_signing_payload
    // -------------------------------------------------------------------------

    #[test]
    fn signing_payload_structure() {
        let params = ExtrinsicParams {
            spec_version: 1_000_000,
            tx_version: 25,
            genesis_hash: [0xab; 32],
            mortality_checkpoint: [0xcd; 32],
            block_number: 100,
            nonce: 0,
            tip: 0,
        };
        // Simple 2-byte call data (e.g. Balances.transfer_keep_alive with no args
        // would be longer — here we just want a deterministic short payload).
        let call_data = [0x04u8, 0x00u8];

        let payload = build_signing_payload(&call_data, &params);

        // Payload must not be empty.
        assert!(!payload.is_empty());

        // For short call_data the payload will be < 256 bytes, so it should
        // NOT be hashed — verify it starts with the call_data bytes.
        assert_eq!(&payload[..2], &call_data, "payload should begin with call_data");

        // After call_data comes extra:
        //   era (2) + compact_nonce (1 for 0) + compact_tip (1 for 0) + 0x00 = 5 bytes
        // Then additional_signed: spec_version(4) + tx_version(4) + genesis(32) + checkpoint(32)
        let extra_len = 2 + 1 + 1 + 1; // era + nonce + tip + CheckMetadataHash
        let additional_signed_len = 4 + 4 + 32 + 32;
        let expected_len = call_data.len() + extra_len + additional_signed_len;
        assert_eq!(payload.len(), expected_len, "unexpected payload length");
    }

    #[test]
    fn signing_payload_hashed_when_large() {
        let params = ExtrinsicParams {
            spec_version: 1_000_000,
            tx_version: 25,
            genesis_hash: [0x11; 32],
            mortality_checkpoint: [0x22; 32],
            block_number: 0,
            nonce: 0,
            tip: 0,
        };
        // 300-byte call data forces total > 256 → payload should be hashed to 32 bytes.
        let call_data = vec![0x42u8; 300];
        let payload = build_signing_payload(&call_data, &params);
        assert_eq!(payload.len(), 32, "hashed payload must be exactly 32 bytes");
    }

    // -------------------------------------------------------------------------
    // encode_signed_extrinsic
    // -------------------------------------------------------------------------

    #[test]
    fn signed_extrinsic_format() {
        let params = ExtrinsicParams {
            spec_version: 1_000_000,
            tx_version: 25,
            genesis_hash: [0x00; 32],
            mortality_checkpoint: [0x00; 32],
            block_number: 0,
            nonce: 3,
            tip: 0,
        };
        let call_data = [0x04u8, 0x00u8];
        let public_key = [0xaau8; 32];
        let signature = [0xbbu8; 64];

        let hex = encode_signed_extrinsic(&call_data, &public_key, &signature, &params);

        assert!(hex.starts_with("0x"), "must have 0x prefix");

        let raw = hex_decode_local(&hex[2..]).expect("valid hex");

        // Decode compact length prefix.
        let (body_len, prefix_bytes) = decode_compact_local(&raw);
        let body = &raw[prefix_bytes..];
        assert_eq!(body.len(), body_len, "compact length must match actual body");

        // Check version byte.
        assert_eq!(body[0], 0x84, "version byte must be 0x84 (v4 | signed)");

        // Check MultiAddress variant.
        assert_eq!(body[1], 0x00, "MultiAddress::Id variant byte");
        assert_eq!(&body[2..34], &[0xaa; 32], "public key");

        // Check MultiSignature variant.
        assert_eq!(body[34], 0x01, "MultiSignature::Sr25519 variant byte");
        assert_eq!(&body[35..99], &[0xbb; 64], "signature");

        // extra starts at offset 99
        let extra_start = 99usize;
        // era: 2 bytes, nonce=3 compact (1 byte: 3<<2|0 = 0x0c), tip=0 (1 byte: 0x00), mode=0x00
        let era = encode_mortal_era(0, 64);
        assert_eq!(&body[extra_start..extra_start + 2], &era, "era bytes");
        assert_eq!(body[extra_start + 2], 0x0c, "compact nonce for 3");
        assert_eq!(body[extra_start + 3], 0x00, "compact tip 0");
        assert_eq!(body[extra_start + 4], 0x00, "CheckMetadataHash::Disabled");

        // call_data at the end
        let call_start = extra_start + 5;
        assert_eq!(&body[call_start..], &call_data, "call_data at end");
    }

    // -------------------------------------------------------------------------
    // Local helpers for tests only
    // -------------------------------------------------------------------------

    fn hex_decode_local(s: &str) -> Option<Vec<u8>> {
        if s.len() % 2 != 0 {
            return None;
        }
        let mut out = Vec::with_capacity(s.len() / 2);
        for i in (0..s.len()).step_by(2) {
            out.push(u8::from_str_radix(&s[i..i + 2], 16).ok()?);
        }
        Some(out)
    }

    /// Decode a SCALE compact integer from the front of `data`.
    /// Returns `(value, bytes_consumed)`.
    fn decode_compact_local(data: &[u8]) -> (usize, usize) {
        let mode = data[0] & 0b11;
        match mode {
            0 => ((data[0] >> 2) as usize, 1),
            1 => {
                let v = u16::from_le_bytes([data[0], data[1]]) >> 2;
                (v as usize, 2)
            }
            2 => {
                let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) >> 2;
                (v as usize, 4)
            }
            _ => {
                let byte_count = ((data[0] >> 2) as usize) + 4;
                let mut val: usize = 0;
                for i in (0..byte_count).rev() {
                    val = (val << 8) | data[1 + i] as usize;
                }
                (val, 1 + byte_count)
            }
        }
    }
}
