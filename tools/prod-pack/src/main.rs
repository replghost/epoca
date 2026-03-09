use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: prod-pack <directory> [output.prod]");
        std::process::exit(1);
    }

    let dir = PathBuf::from(&args[1]);
    if !dir.is_dir() {
        eprintln!("Error: {} is not a directory", dir.display());
        std::process::exit(1);
    }

    let output = if args.len() > 2 {
        PathBuf::from(&args[2])
    } else {
        let stem = dir.file_name().unwrap_or("bundle".as_ref());
        PathBuf::from(stem).with_extension("prod")
    };

    let mut files = BTreeMap::new();
    collect_files(&dir, &dir, &mut files).expect("failed to collect files");

    if !files.contains_key("manifest.toml") {
        eprintln!("Warning: no manifest.toml found in bundle directory");
    }

    inject_asset_hashes(&mut files);

    let car_bytes = build_car(&files);
    std::fs::write(&output, &car_bytes).expect("failed to write output");
    eprintln!(
        "Wrote {} ({} bytes, {} files)",
        output.display(),
        car_bytes.len(),
        files.len()
    );
}

/// Compute SHA-256 and size for every file in the bundle, then append an
/// `[assets]` TOML section to `manifest.toml` so consumers can verify
/// individual files without re-hashing the whole archive.
fn inject_asset_hashes(files: &mut BTreeMap<String, Vec<u8>>) {
    // Build the [assets] section over all files except manifest.toml itself.
    let mut section = String::from("\n[assets]\n");
    for (path, data) in files.iter() {
        if path == "manifest.toml" {
            continue;
        }
        let hash = Sha256::digest(data);
        let hex_hash = hash
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let size = data.len() as u64;
        // Use quoted keys so paths with slashes are valid TOML.
        section.push_str(&format!(
            "[assets.\"{}\"]\nsha256 = \"{}\"\nsize = {}\n",
            path, hex_hash, size
        ));
    }

    // Append the section to the existing manifest content.
    let manifest = files.entry("manifest.toml".to_string()).or_default();
    manifest.extend_from_slice(section.as_bytes());
}

fn collect_files(
    base: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Skip symlinks to avoid escaping the bundle directory.
        if path.symlink_metadata()?.file_type().is_symlink() {
            eprintln!("Skipping symlink: {}", path.display());
            continue;
        }
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .to_string();
            // Skip non-bundle artifacts.
            if rel.ends_with(".prod") || rel.ends_with(".polkavm") && rel != "app.polkavm" {
                eprintln!("Skipping: {rel}");
                continue;
            }
            out.insert(rel, std::fs::read(&path)?);
        }
    }
    Ok(())
}

struct DirEntry {
    name: String,
    cid: Vec<u8>,
    size: u64,
}

struct Dir {
    files: Vec<(String, Vec<u8>, u64)>, // (name, cid, size)
    subdirs: BTreeMap<String, Dir>,
}

impl Dir {
    fn new() -> Self {
        Dir {
            files: Vec::new(),
            subdirs: BTreeMap::new(),
        }
    }
}

fn build_car(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut blocks: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut root = Dir::new();

    // Create raw leaf blocks for each file, zstd-compressed.
    // CID is over the compressed bytes so integrity checks work on the wire format.
    for (path, data) in files {
        let compressed = zstd::encode_all(data.as_slice(), 19).expect("zstd compress failed");
        // Only use compressed version if it's actually smaller.
        let (block_data, saved) = if compressed.len() < data.len() {
            let saved = data.len() - compressed.len();
            (compressed, saved)
        } else {
            (data.clone(), 0)
        };
        if saved > 0 {
            eprintln!(
                "  {path}: {} → {} bytes (zstd saved {})",
                data.len(),
                block_data.len(),
                saved
            );
        }
        let block_len = block_data.len() as u64;
        let cid = make_cid(0x55, &block_data); // raw codec
        blocks.push((cid.clone(), block_data));

        let parts: Vec<&str> = path.split('/').collect();
        let mut current = &mut root;
        for &part in &parts[..parts.len() - 1] {
            current = current
                .subdirs
                .entry(part.to_string())
                .or_insert_with(Dir::new);
        }
        let file_name = parts.last().unwrap().to_string();
        current.files.push((file_name, cid, block_len));
    }

    // Build directory dag-pb blocks bottom-up.
    let (root_cid, _) = build_dir_block(&root, &mut blocks);

    // Assemble CAR: header + blocks.
    // Root block must come first (parser uses first CIDv1 block as root).
    let header = encode_car_header(&root_cid);
    let mut car = Vec::new();
    write_uvarint(&mut car, header.len());
    car.extend_from_slice(&header);

    let root_idx = blocks.iter().position(|(cid, _)| *cid == root_cid).unwrap();
    let root_block = blocks.remove(root_idx);
    write_car_block(&mut car, &root_block.0, &root_block.1);
    for (cid, data) in &blocks {
        write_car_block(&mut car, cid, data);
    }

    car
}

fn build_dir_block(dir: &Dir, blocks: &mut Vec<(Vec<u8>, Vec<u8>)>) -> (Vec<u8>, u64) {
    let mut entries = Vec::new();
    let mut total_size: u64 = 0;

    // Subdirectories first (depth-first).
    for (name, subdir) in &dir.subdirs {
        let (sub_cid, sub_size) = build_dir_block(subdir, blocks);
        entries.push(DirEntry {
            name: name.clone(),
            cid: sub_cid,
            size: sub_size,
        });
        total_size += sub_size;
    }

    // Files.
    for (name, cid, size) in &dir.files {
        entries.push(DirEntry {
            name: name.clone(),
            cid: cid.clone(),
            size: *size,
        });
        total_size += size;
    }

    let pb = encode_dagpb_directory(&entries);
    let cid = make_cid(0x70, &pb); // dag-pb codec
    blocks.push((cid.clone(), pb));

    (cid, total_size)
}

fn write_car_block(car: &mut Vec<u8>, cid: &[u8], data: &[u8]) {
    write_uvarint(car, cid.len() + data.len());
    car.extend_from_slice(cid);
    car.extend_from_slice(data);
}

// --- CID / multihash ---

fn make_cid(codec: u8, data: &[u8]) -> Vec<u8> {
    let digest = Sha256::digest(data);
    let mut cid = Vec::with_capacity(36);
    cid.push(0x01); // CIDv1
    cid.push(codec);
    cid.push(0x12); // sha-256
    cid.push(0x20); // 32 bytes
    cid.extend_from_slice(&digest);
    cid
}

// --- Unsigned varint (LEB128) ---

fn write_uvarint(buf: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

// --- CAR header (CBOR) ---

fn encode_car_header(root_cid: &[u8]) -> Vec<u8> {
    // CBOR: {"roots": [<cid_bytes>], "version": 1}
    // "roots" placed first so is_car_file() heuristic finds it in first 20 bytes.
    let mut h = Vec::new();
    h.push(0xa2); // map(2)

    // "roots"
    h.push(0x65); // text(5)
    h.extend_from_slice(b"roots");
    h.push(0x81); // array(1)
    encode_cbor_bytes(&mut h, root_cid);

    // "version"
    h.push(0x67); // text(7)
    h.extend_from_slice(b"version");
    h.push(0x01); // uint(1)

    h
}

fn encode_cbor_bytes(buf: &mut Vec<u8>, data: &[u8]) {
    let len = data.len();
    if len < 24 {
        buf.push(0x40 | len as u8);
    } else if len < 256 {
        buf.push(0x58);
        buf.push(len as u8);
    } else {
        buf.push(0x59);
        buf.push((len >> 8) as u8);
        buf.push(len as u8);
    }
    buf.extend_from_slice(data);
}

// --- Protobuf encoding (dag-pb / UnixFS) ---

fn encode_dagpb_directory(entries: &[DirEntry]) -> Vec<u8> {
    let mut pb = Vec::new();

    // Field 2 (repeated): PBLink
    for entry in entries {
        let inner = encode_pblink(&entry.cid, &entry.name, entry.size);
        pb.push(0x12); // tag: field 2, wire type 2
        write_uvarint(&mut pb, inner.len());
        pb.extend_from_slice(&inner);
    }

    // Field 1: Data (UnixFS directory)
    let unixfs = encode_unixfs_directory();
    pb.push(0x0a); // tag: field 1, wire type 2
    write_uvarint(&mut pb, unixfs.len());
    pb.extend_from_slice(&unixfs);

    pb
}

fn encode_pblink(cid: &[u8], name: &str, tsize: u64) -> Vec<u8> {
    let mut inner = Vec::new();

    // Field 1: Hash (CID)
    inner.push(0x0a); // tag: field 1, wire type 2
    write_uvarint(&mut inner, cid.len());
    inner.extend_from_slice(cid);

    // Field 2: Name
    inner.push(0x12); // tag: field 2, wire type 2
    write_uvarint(&mut inner, name.len());
    inner.extend_from_slice(name.as_bytes());

    // Field 3: Tsize
    inner.push(0x18); // tag: field 3, wire type 0
    write_uvarint(&mut inner, tsize as usize);

    inner
}

fn encode_unixfs_directory() -> Vec<u8> {
    // UnixFS: Type = 1 (Directory)
    vec![0x08, 0x01] // field 1, wire type 0, varint 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_parser() {
        let mut files = BTreeMap::new();
        files.insert(
            "manifest.toml".to_string(),
            br#"[app]
id = "com.test.roundtrip"
name = "Round Trip"
version = "0.1.0"
app_type = "spa"

[webapp]
entry = "index.html"

[permissions]
sign = true
"#
            .to_vec(),
        );
        files.insert(
            "assets/index.html".to_string(),
            b"<h1>hello</h1>".to_vec(),
        );

        let car = build_car(&files);

        // Verify is_car_file detects it
        assert!(epoca_sandbox::car::is_car_file(&car));

        // Parse through the real parser
        let bundle = epoca_sandbox::bundle::ProdBundle::from_bytes(&car).unwrap();
        assert_eq!(bundle.manifest.app.name, "Round Trip");
        assert_eq!(bundle.manifest.app.app_type, "spa");
        assert!(bundle.program_bytes.is_none());
        assert_eq!(
            bundle.assets.get("index.html").map(|v| v.as_slice()),
            Some(b"<h1>hello</h1>".as_slice())
        );
        let perms = bundle.manifest.permissions.unwrap();
        assert!(perms.sign);
    }

    #[test]
    fn tampered_block_rejected() {
        let mut files = BTreeMap::new();
        files.insert(
            "manifest.toml".to_string(),
            br#"[app]
id = "com.test.tamper"
name = "Tamper"
version = "0.1.0"
app_type = "spa"

[webapp]
entry = "index.html"
"#
            .to_vec(),
        );
        files.insert("assets/index.html".to_string(), b"original".to_vec());

        let mut car = build_car(&files);

        // Corrupt a byte in the block data region (after the header).
        // Find "original" in the CAR bytes and flip a byte.
        let needle = b"original";
        let offset = car
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("should find 'original' in CAR");
        car[offset] ^= 0xff;

        let result = epoca_sandbox::car::parse_car_to_assets(&car);
        assert!(result.is_err(), "tampered CAR should fail integrity check");
        let err = result.unwrap_err();
        assert!(
            err.contains("integrity check failed"),
            "error should mention integrity: {err}"
        );
    }
}
