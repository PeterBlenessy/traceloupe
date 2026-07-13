//! Cross-validation against tools/make_fixture_backup.py (independent Python
//! reference). Gated on TRACELOUPE_ENC_FIXTURE so it only runs when pointed at a
//! generated fixture; not part of the normal suite.
use traceloupe_core::crypto::BackupDecryptor;

#[test]
fn decrypts_python_generated_backup() {
    let Ok(dir) = std::env::var("TRACELOUPE_ENC_FIXTURE") else {
        eprintln!("skipping: set TRACELOUPE_ENC_FIXTURE to a fixture dir");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let dec = BackupDecryptor::open(&dir, "traceloupe-test").expect("open with correct password");

    // Manifest.db decrypts to real SQLite (magic header).
    let manifest = dec.decrypt_manifest_db().expect("decrypt Manifest.db");
    assert_eq!(
        &manifest[..15],
        b"SQLite format 3",
        "not valid SQLite after decrypt"
    );

    // Write it out, open it, and decrypt every file it lists — proving the
    // per-file key path works against the independent generator.
    let tmp = std::env::temp_dir().join("xcheck_manifest.db");
    std::fs::write(&tmp, &manifest).unwrap();
    let conn = rusqlite::Connection::open(&tmp).unwrap();
    let mut stmt = conn
        .prepare("SELECT fileID, relativePath, file FROM Files ORDER BY relativePath")
        .unwrap();
    let rows: Vec<(String, String, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!rows.is_empty(), "no files in decrypted manifest");

    let mut checked = 0;
    for (file_id, rel, blob) in &rows {
        let pt = dec.decrypt_file(blob, file_id).expect("decrypt file");
        // The seeded DBs are SQLite; the PNGs start with the PNG magic.
        if rel.ends_with(".db") || rel.ends_with(".sqlitedb") || rel.ends_with(".storedata") {
            assert_eq!(&pt[..15], b"SQLite format 3", "{rel} not SQLite");
        } else if rel.ends_with(".png") {
            assert_eq!(&pt[..8], b"\x89PNG\r\n\x1a\n", "{rel} not PNG");
        }
        checked += 1;
    }
    eprintln!("cross-check OK: decrypted {checked} files from the Python fixture");
}

#[test]
fn parses_encrypted_camera_roll() {
    let Ok(dir) = std::env::var("TRACELOUPE_ENC_FIXTURE") else {
        eprintln!("skipping: set TRACELOUPE_ENC_FIXTURE to a fixture dir");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let dec = BackupDecryptor::open(&dir, "traceloupe-test").expect("open with correct password");
    let cache = std::env::temp_dir().join("xcheck_media_cache");
    let _ = std::fs::remove_dir_all(&cache);

    let assets = traceloupe_core::parsers::camera_roll::parse_camera_roll(&dir, Some(&dec), &cache)
        .expect("parse encrypted camera roll");
    let asset = assets
        .iter()
        .find(|a| a.relative_path.ends_with("IMG_0001.HEIC"))
        .expect("DCIM asset present");

    // Capture date recovered from the decrypted Photos.sqlite (700000000 + offset).
    assert_eq!(asset.taken_at, Some(1_678_307_200));

    // The thumbnail was decrypted into the cache and is a real image on disk.
    let thumb = asset.thumb_path.as_ref().expect("thumbnail resolved");
    let thumb_bytes = std::fs::read(thumb).expect("read decrypted thumb");
    assert_eq!(
        &thumb_bytes[..8],
        b"\x89PNG\r\n\x1a\n",
        "thumbnail not decrypted"
    );

    // The original stays encrypted on disk; its stored wrapped key + size
    // decrypt+trim it on demand into a valid HEIC (bytes 4..8 are the `ftyp`
    // box marker), with no CBC padding beyond the real length.
    let key = asset
        .decrypt_key
        .as_ref()
        .expect("wrapped key for encrypted asset");
    let size = asset.plain_size.map(|s| s as usize);
    let ct = std::fs::read(&asset.full_path).expect("read encrypted original");
    let full = dec
        .decrypt_bytes(key, &ct, size)
        .expect("decrypt original on demand");
    assert_eq!(&full[4..8], b"ftyp", "decrypted original is not a HEIC");
    if let Some(n) = size {
        assert_eq!(full.len(), n, "on-demand decrypt not trimmed to real size");
    }

    eprintln!("encrypted camera-roll OK: date, decrypted thumb, on-demand full image");
}
