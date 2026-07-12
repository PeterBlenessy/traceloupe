//! Cross-validation against tools/make_fixture_backup.py (independent Python
//! reference). Gated on SALVAGE_ENC_FIXTURE so it only runs when pointed at a
//! generated fixture; not part of the normal suite.
use salvage_core::crypto::BackupDecryptor;

#[test]
fn decrypts_python_generated_backup() {
    let Ok(dir) = std::env::var("SALVAGE_ENC_FIXTURE") else {
        eprintln!("skipping: set SALVAGE_ENC_FIXTURE to a fixture dir");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let dec = BackupDecryptor::open(&dir, "salvage-test").expect("open with correct password");

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
