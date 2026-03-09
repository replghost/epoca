//! CARv1 / UnixFS parser for .prod bundles.
//!
//! Parses a CARv1 archive (a sequential list of IPFS blocks) and walks the
//! dag-pb / UnixFS directory tree to produce a flat `HashMap<String, Vec<u8>>`
//! of filename → content, suitable for loading into a `ProdBundle`.

use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Returns `true` if the bytes look like a CARv1 file (CBOR header containing "roots").
pub fn is_car_file(data: &[u8]) -> bool {
    data.len() >= 20 && data[1..20].windows(5).any(|w| w == b"roots")
}

/// Parse a CARv1 file into a flat asset map.
///
/// Walks the dag-pb directory tree rooted at the first CIDv1 block and maps
/// filenames to their reassembled content bytes.
pub fn parse_car_to_assets(data: &[u8]) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut pos = 0;

    // 1. Read header: varint(header_len) + CBOR header
    let (header_len, n) = read_uvarint(&data[pos..])?;
    pos += n;
    if pos + header_len > data.len() {
        return Err("CAR header length exceeds data".into());
    }
    pos += header_len; // skip CBOR header

    // 2. Read all blocks: varint(block_len) + CID + block_data
    let mut blocks: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    let mut root_cid: Option<Vec<u8>> = None;

    while pos < data.len() {
        let (block_len, n) = read_uvarint(&data[pos..])?;
        pos += n;

        if pos + block_len > data.len() {
            break;
        }

        let block_start = pos;
        // Parse CID: version(varint) + codec(varint) + multihash
        let (version, n) = read_uvarint(&data[pos..])?;
        pos += n;
        let (_codec, n) = read_uvarint(&data[pos..])?;
        pos += n;
        // Multihash: hash_function(varint) + digest_size(varint) + digest
        let (hash_fn, n) = read_uvarint(&data[pos..])?;
        pos += n;
        let (digest_size, n) = read_uvarint(&data[pos..])?;
        pos += n;
        let digest_start = pos;
        if pos + digest_size > data.len() || pos + digest_size > block_start + block_len {
            return Err("CID digest extends beyond block boundary".into());
        }
        pos += digest_size;

        let cid_bytes = data[block_start..pos].to_vec();
        let block_data = data[pos..block_start + block_len].to_vec();

        // Verify block integrity: recompute hash and compare against CID digest.
        if hash_fn == 0x12 && digest_size == 32 {
            let expected = &data[digest_start..digest_start + 32];
            let actual = Sha256::digest(&block_data);
            if actual.as_slice() != expected {
                return Err(format!(
                    "CID integrity check failed: block hash mismatch (expected {}, got {})",
                    hex(&expected[..4]),
                    hex(&actual[..4]),
                ));
            }
        }

        if version == 1 && root_cid.is_none() {
            root_cid = Some(cid_bytes.clone());
        }

        blocks.insert(cid_bytes, block_data);
        pos = block_start + block_len;
    }

    log::info!("[car] parsed {} blocks", blocks.len());

    let root = root_cid.ok_or("CAR has no blocks")?;
    let root_block = blocks.get(&root).ok_or("root block not found in CAR")?;

    // 3. Parse root dag-pb node to get directory links
    let links = parse_dagpb_links(root_block);
    log::info!("[car] root directory has {} entries", links.len());

    // 4. For each link, reassemble content (recurse into subdirectories)
    const MAX_DEPTH: usize = 32;
    let mut assets = HashMap::new();
    for (name, cid_bytes, _size) in &links {
        match reassemble_file(&blocks, cid_bytes, MAX_DEPTH) {
            Ok(content) => {
                log::info!("[car] extracted: {name} ({} bytes)", content.len());
                assets.insert(name.clone(), content);
            }
            Err(_) => {
                if let Some(dir_block) = blocks.get(cid_bytes) {
                    let sub_links = parse_dagpb_links(dir_block);
                    if !sub_links.is_empty() {
                        extract_directory_recursive(&blocks, name, &sub_links, &mut assets, MAX_DEPTH - 1);
                    }
                }
            }
        }
    }

    if assets.is_empty() {
        return Err("CAR file contained no extractable assets".into());
    }

    Ok(assets)
}

/// Recursively extract files from a dag-pb directory subtree.
fn extract_directory_recursive(
    blocks: &HashMap<Vec<u8>, Vec<u8>>,
    prefix: &str,
    links: &[(String, Vec<u8>, u64)],
    assets: &mut HashMap<String, Vec<u8>>,
    depth: usize,
) {
    if depth == 0 {
        log::warn!("[car] max recursion depth reached at {prefix}");
        return;
    }
    for (name, cid_bytes, _size) in links {
        let full_path = format!("{prefix}/{name}");
        match reassemble_file(blocks, cid_bytes, depth - 1) {
            Ok(content) => {
                log::info!("[car] extracted: {full_path} ({} bytes)", content.len());
                assets.insert(full_path, content);
            }
            Err(_) => {
                if let Some(dir_block) = blocks.get(cid_bytes) {
                    let sub_links = parse_dagpb_links(dir_block);
                    if !sub_links.is_empty() {
                        extract_directory_recursive(blocks, &full_path, &sub_links, assets, depth - 1);
                    }
                }
            }
        }
    }
}

/// Parse dag-pb protobuf to extract PBLink entries: (name, CID bytes, tsize).
pub fn parse_dagpb_links(pb: &[u8]) -> Vec<(String, Vec<u8>, u64)> {
    let mut links = Vec::new();
    let mut pos = 0;

    while pos < pb.len() {
        let tag = pb[pos];
        let field_num = tag >> 3;
        let wire_type = tag & 0x7;
        pos += 1;

        if wire_type == 2 {
            let (length, n) = match read_uvarint(&pb[pos..]) {
                Ok(v) => v,
                Err(_) => break,
            };
            pos += n;
            if pos + length > pb.len() {
                break;
            }

            if field_num == 2 {
                // PBLink — parse inner fields
                let inner = &pb[pos..pos + length];
                let mut hash = Vec::new();
                let mut name = String::new();
                let mut tsize: u64 = 0;
                let mut ipos = 0;
                while ipos < inner.len() {
                    let itag = inner[ipos];
                    let inum = itag >> 3;
                    let iwire = itag & 0x7;
                    ipos += 1;
                    match iwire {
                        2 => {
                            let (ilen, n) = match read_uvarint(&inner[ipos..]) {
                                Ok(v) => v,
                                Err(_) => break,
                            };
                            ipos += n;
                            if ipos + ilen > inner.len() {
                                break;
                            }
                            match inum {
                                1 => hash = inner[ipos..ipos + ilen].to_vec(),
                                2 => {
                                    name = String::from_utf8_lossy(&inner[ipos..ipos + ilen])
                                        .to_string()
                                }
                                _ => {}
                            }
                            ipos += ilen;
                        }
                        0 => {
                            let (val, n) = match read_uvarint(&inner[ipos..]) {
                                Ok(v) => v,
                                Err(_) => break,
                            };
                            ipos += n;
                            if inum == 3 {
                                tsize = val as u64;
                            }
                        }
                        _ => break,
                    }
                }
                if !hash.is_empty() {
                    links.push((name, hash, tsize));
                }
            }

            pos += length;
        } else if wire_type == 0 {
            let (_, n) = match read_uvarint(&pb[pos..]) {
                Ok(v) => v,
                Err(_) => break,
            };
            pos += n;
        } else {
            break;
        }
    }

    links
}

/// UnixFS node type from the protobuf Type field.
#[derive(Debug, PartialEq)]
enum UnixFsType {
    Raw,
    Directory,
    File,
    Unknown,
}

/// Extract the UnixFS type from a dag-pb Data field.
fn unixfs_type(unixfs: &[u8]) -> UnixFsType {
    let mut pos = 0;
    while pos < unixfs.len() {
        let tag = unixfs[pos];
        let field_num = tag >> 3;
        let wire_type = tag & 0x7;
        pos += 1;
        if wire_type == 0 {
            let (val, n) = match read_uvarint(&unixfs[pos..]) {
                Ok(v) => v,
                Err(_) => return UnixFsType::Unknown,
            };
            pos += n;
            if field_num == 1 {
                return match val {
                    0 => UnixFsType::Raw,
                    1 => UnixFsType::Directory,
                    2 => UnixFsType::File,
                    _ => UnixFsType::Unknown,
                };
            }
        } else if wire_type == 2 {
            let (length, n) = match read_uvarint(&unixfs[pos..]) {
                Ok(v) => v,
                Err(_) => return UnixFsType::Unknown,
            };
            pos += n + length;
        } else {
            break;
        }
    }
    UnixFsType::Unknown
}

/// Check if a dag-pb block is a UnixFS directory node.
fn is_directory_node(block: &[u8]) -> bool {
    if let Some(data) = extract_dagpb_data(block) {
        unixfs_type(&data) == UnixFsType::Directory
    } else {
        false
    }
}

/// zstd frame magic number.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// If data starts with the zstd magic number, decompress it; otherwise return as-is.
fn maybe_decompress_zstd(data: Vec<u8>) -> Result<Vec<u8>, String> {
    if data.len() >= 4 && data[..4] == ZSTD_MAGIC {
        zstd::decode_all(data.as_slice()).map_err(|e| format!("zstd decompress failed: {e}"))
    } else {
        Ok(data)
    }
}

/// Reassemble a file from its dag-pb block(s).
/// Handles single-chunk files, multi-chunk files, and raw leaves.
/// Returns Err for directory nodes (caller should recurse instead).
fn reassemble_file(
    blocks: &HashMap<Vec<u8>, Vec<u8>>,
    cid_bytes: &[u8],
    depth: usize,
) -> Result<Vec<u8>, String> {
    if depth == 0 {
        return Err("max recursion depth reached".into());
    }
    let block = blocks.get(cid_bytes).ok_or("block not found")?;

    if is_directory_node(block) {
        return Err("directory node".into());
    }

    let links = parse_dagpb_links(block);

    if links.is_empty() {
        // Leaf node — extract data from UnixFS protobuf
        if let Some(data) = extract_dagpb_data(block) {
            return maybe_decompress_zstd(extract_unixfs_data(&data)?);
        }
        // Raw leaf — the block IS the data (possibly zstd-compressed)
        return maybe_decompress_zstd(block.clone());
    }

    // Multi-chunk file — concatenate child blocks in order
    let mut result = Vec::new();
    for (_name, child_cid, _size) in &links {
        let chunk = reassemble_file(blocks, child_cid, depth - 1)?;
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}

/// Extract the Data field (field 1) from a dag-pb node.
fn extract_dagpb_data(pb: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0;
    while pos < pb.len() {
        let tag = pb[pos];
        let field_num = tag >> 3;
        let wire_type = tag & 0x7;
        pos += 1;
        if wire_type == 2 {
            let (length, n) = read_uvarint(&pb[pos..]).ok()?;
            pos += n;
            if field_num == 1 {
                return Some(pb[pos..pos + length].to_vec());
            }
            pos += length;
        } else if wire_type == 0 {
            let (_, n) = read_uvarint(&pb[pos..]).ok()?;
            pos += n;
        } else {
            break;
        }
    }
    None
}

/// Extract file data from a UnixFS protobuf.
/// UnixFS: field 1 = Type (varint), field 2 = Data (bytes), field 3 = filesize.
fn extract_unixfs_data(unixfs: &[u8]) -> Result<Vec<u8>, String> {
    let mut pos = 0;
    while pos < unixfs.len() {
        let tag = unixfs[pos];
        let field_num = tag >> 3;
        let wire_type = tag & 0x7;
        pos += 1;
        match wire_type {
            2 => {
                let (length, n) = read_uvarint(&unixfs[pos..]).map_err(|e| e.to_string())?;
                pos += n;
                if field_num == 2 {
                    return Ok(unixfs[pos..pos + length].to_vec());
                }
                pos += length;
            }
            0 => {
                let (_, n) = read_uvarint(&unixfs[pos..]).map_err(|e| e.to_string())?;
                pos += n;
            }
            _ => break,
        }
    }
    Err("no data in UnixFS node".into())
}

/// Format a byte slice as hex (for error messages).
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}


/// Read an unsigned varint (LEB128). Returns (value, bytes_consumed).
pub fn read_uvarint(data: &[u8]) -> Result<(usize, usize), String> {
    if data.is_empty() {
        return Err("empty data for uvarint".into());
    }
    let mut value: usize = 0;
    let mut shift = 0;
    for (i, &byte) in data.iter().enumerate() {
        value |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
        if shift > 63 {
            return Err("uvarint too long".into());
        }
    }
    Err("unterminated uvarint".into())
}
