//! Native Notes parser (Phase 2): reads a decrypted `NoteStore.sqlite` directly
//! into the cache `notes` table, so Notes can be materialized natively — without
//! iLEAPP's `notes` module. Locate + decrypt the DB via the
//! [`crate::manifest::ManifestIndex`], then call [`parse_notes`].
//!
//! The modern Notes app (iOS 9+) stores everything in a Core Data schema:
//! `ZICCLOUDSYNCINGOBJECT` holds both notes and folders, and each note's body
//! lives in `ZICNOTEDATA.ZDATA` — a gzip-compressed protobuf. We inflate it and
//! walk the wire format to the note text (see [`note_text_from_protobuf`]); the
//! title/snippet/timestamps come straight from the note's columns.
//!
//! Core Data column names carry version-dependent suffixes (`ZCREATIONDATE1` vs
//! `ZCREATIONDATE3`, …), so we introspect the actual columns of
//! `ZICCLOUDSYNCINGOBJECT` and pick the first candidate that exists rather than
//! hard-coding one iOS version.
//!
//! provenance: reference (own implementation) from the reverse-engineered Notes
//! Core Data schema and the `NoteStoreProto` wire format.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::normalize::ImportReport;
use crate::Result;

/// Core Data counts time in seconds since 2001-01-01 UTC. Convert to Unix epoch
/// seconds; a 0/absent timestamp → None.
const MAC_EPOCH: f64 = 978_307_200.0;
fn coredata_to_unix(secs: Option<f64>) -> Option<i64> {
    match secs {
        Some(s) if s > 0.0 => Some((s + MAC_EPOCH) as i64),
        _ => None,
    }
}

/// Pick the first candidate column that actually exists in `cols`, else the
/// literal `NULL` so the generated SQL still parses on schemas that lack it.
fn col_or_null(cols: &HashSet<String>, candidates: &[&str]) -> String {
    for c in candidates {
        if cols.contains(*c) {
            return format!("n.{c}");
        }
    }
    "NULL".to_string()
}

/// Parse a decrypted `NoteStore.sqlite` into the cache `notes` table.
///
/// With `replace = false` it appends (for a fresh cache, like the normalizer).
/// With `replace = true` it clears the `notes` table first, **in the same
/// transaction as the re-insert**, so a partial re-import is atomic (a parse
/// failure rolls the delete back too).
pub fn parse_notes(
    note_store: &Path,
    cache: &CacheDb,
    report: &mut ImportReport,
    replace: bool,
) -> Result<()> {
    let src = Connection::open_with_flags(note_store, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    // A recognizable modern Notes DB has these two tables. If they're absent this
    // isn't a schema we understand — error so the caller falls back to iLEAPP.
    let cols = table_columns(&src, "ZICCLOUDSYNCINGOBJECT")?;
    if cols.is_empty() || table_columns(&src, "ZICNOTEDATA")?.is_empty() {
        return Err(crate::Error::Parse(
            "NoteStore.sqlite is not a recognized Notes schema".into(),
        ));
    }
    // ZNOTEDATA links a note object to its body blob; without it we can't find
    // note rows at all.
    if !cols.contains("ZNOTEDATA") {
        return Err(crate::Error::Parse(
            "NoteStore.sqlite has no ZNOTEDATA column".into(),
        ));
    }

    // Resolve the version-suffixed columns against this DB's actual schema.
    let title = col_or_null(&cols, &["ZTITLE1", "ZTITLE"]);
    let snippet = col_or_null(&cols, &["ZSNIPPET"]);
    let folder_fk = col_or_null(&cols, &["ZFOLDER"]);
    let created = col_or_null(
        &cols,
        &[
            "ZCREATIONDATE1",
            "ZCREATIONDATE3",
            "ZCREATIONDATE",
            "ZCREATIONDATE2",
        ],
    );
    let modified = col_or_null(
        &cols,
        &[
            "ZMODIFICATIONDATE1",
            "ZMODIFICATIONDATE3",
            "ZMODIFICATIONDATE",
            "ZMODIFICATIONDATE2",
        ],
    );
    let deleted = col_or_null(&cols, &["ZMARKEDFORDELETION"]);
    // Folders use ZTITLE2 for their name; join the note's ZFOLDER back to it.
    let folder_title = if cols.contains("ZTITLE2") {
        "f.ZTITLE2"
    } else {
        "NULL"
    };

    // One row per note: its columns + its folder's title + its (gzipped) body
    // blob. `WHERE n.ZNOTEDATA IS NOT NULL` selects note objects (folders and
    // accounts have no body data).
    let sql = format!(
        "SELECT {title}, {snippet}, {created}, {modified}, {deleted}, {folder_title}, d.ZDATA
         FROM ZICCLOUDSYNCINGOBJECT n
         LEFT JOIN ZICCLOUDSYNCINGOBJECT f ON f.Z_PK = {folder_fk}
         LEFT JOIN ZICNOTEDATA d ON d.Z_PK = n.ZNOTEDATA
         WHERE n.ZNOTEDATA IS NOT NULL"
    );

    let conn = cache.conn();
    let tx = conn.unchecked_transaction()?;
    if replace {
        tx.execute("DELETE FROM notes", [])?;
    }
    let mut stmt = src.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let title: Option<String> = r.get(0)?;
        let snippet: Option<String> = r.get(1)?;
        let created_at = coredata_to_unix(r.get::<_, Option<f64>>(2)?);
        let modified_at = coredata_to_unix(r.get::<_, Option<f64>>(3)?);
        let marked_deleted = r.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0;
        let folder_name: Option<String> = r.get(5)?;
        let zdata: Option<Vec<u8>> = r.get(6)?;

        let body_text = zdata
            .as_deref()
            .and_then(decode_note_body)
            .unwrap_or_default();
        // The note text repeats the title as its first line; drop it so the body
        // isn't a duplicate of the heading the UI already shows.
        let body_text = strip_leading_title(&body_text, title.as_deref());
        let body_html = if body_text.is_empty() {
            None
        } else {
            Some(text_to_html(&body_text))
        };
        // Prefer the stored snippet; otherwise derive one from the body.
        let snippet = snippet
            .filter(|s| !s.trim().is_empty())
            .or_else(|| derive_snippet(&body_text));
        // Notes in "Recently Deleted" have no folder row of their own; label them
        // so they're distinguishable rather than showing an empty folder.
        let folder = folder_name
            .filter(|s| !s.trim().is_empty())
            .or_else(|| marked_deleted.then(|| "Recently Deleted".to_string()));

        tx.execute(
            "INSERT INTO notes (folder, title, snippet, body_html, created_at, modified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![folder, title, snippet, body_html, created_at, modified_at],
        )?;
        report.notes += 1;
    }
    drop(rows);
    drop(stmt);
    tx.commit()?;
    Ok(())
}

/// Column names of `table`, upper-cased. Empty if the table doesn't exist.
fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        set.insert(r.get::<_, String>(1)?.to_uppercase());
    }
    Ok(set)
}

/// gzip-inflate a `ZDATA` blob and extract the note's plain text. Returns None
/// when the blob isn't gzip or the protobuf has no text field.
fn decode_note_body(zdata: &[u8]) -> Option<String> {
    let mut buf = Vec::new();
    GzDecoder::new(zdata).read_to_end(&mut buf).ok()?;
    note_text_from_protobuf(&buf)
}

/// Walk the `NoteStoreProto` wire format to the note text: top-level field 2
/// (Document) → field 3 (Note) → field 2 (note_text, a UTF-8 string).
fn note_text_from_protobuf(buf: &[u8]) -> Option<String> {
    let document = first_len_delimited(buf, 2)?;
    let note = first_len_delimited(document, 3)?;
    let text = first_len_delimited(note, 2)?;
    Some(String::from_utf8_lossy(text).into_owned())
}

/// Scan a protobuf message for the first length-delimited (wire type 2) field
/// numbered `field`, returning its bytes. Bounds-checked against malformed input.
fn first_len_delimited(buf: &[u8], field: u64) -> Option<&[u8]> {
    let mut i = 0;
    while i < buf.len() {
        let (tag, n) = read_varint(&buf[i..])?;
        i += n;
        let field_number = tag >> 3;
        let wire_type = tag & 0b111;
        match wire_type {
            0 => {
                // varint
                let (_, n) = read_varint(&buf[i..])?;
                i += n;
            }
            1 => i = i.checked_add(8)?, // 64-bit
            5 => i = i.checked_add(4)?, // 32-bit
            2 => {
                let (len, n) = read_varint(&buf[i..])?;
                i += n;
                let len = len as usize;
                let end = i.checked_add(len)?;
                if end > buf.len() {
                    return None;
                }
                if field_number == field {
                    return Some(&buf[i..end]);
                }
                i = end;
            }
            _ => return None, // groups (3/4) unused here; bail on anything odd
        }
        if i > buf.len() {
            return None;
        }
    }
    None
}

/// Read a base-128 varint from the front of `buf`; returns (value, bytes_read).
fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0;
    for (i, &b) in buf.iter().enumerate() {
        if shift >= 64 {
            return None; // overlong varint
        }
        value |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
    }
    None // truncated
}

/// If `body` begins with the note's title line, remove it (the UI shows the
/// title separately). Also trims a leading blank line left behind.
fn strip_leading_title(body: &str, title: Option<&str>) -> String {
    let title = match title {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => return body.to_string(),
    };
    let first_line = body.lines().next().unwrap_or("").trim();
    if first_line == title {
        body.split_once('\n')
            .map_or("", |(_, rest)| rest)
            .to_string()
    } else {
        body.to_string()
    }
}

/// A short plain-text snippet: the first non-empty line, capped at 120 chars.
fn derive_snippet(body: &str) -> Option<String> {
    let line = body.lines().map(str::trim).find(|l| !l.is_empty())?;
    let snippet: String = line.chars().take(120).collect();
    Some(snippet)
}

/// Turn the note's plain text into minimal safe HTML: escape metacharacters and
/// map newlines to `<br>` so paragraphs survive. (Rich formatting — bold, lists,
/// attachments — lives in the protobuf's attribute runs, decoded in a later pass.)
fn text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("<br>"),
            // Object-replacement char marks attachments/tables in the text stream;
            // drop it so it doesn't render as a stray glyph.
            '\u{fffc}' => {}
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    /// Encode `bytes` as a length-delimited (wire type 2) field.
    fn field_bytes(field: u64, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, (field << 3) | 2);
        put_varint(&mut out, bytes.len() as u64);
        out.extend_from_slice(bytes);
        out
    }
    fn put_varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    /// Build a gzipped NoteStoreProto whose note text is `text`.
    fn make_zdata(text: &str) -> Vec<u8> {
        let note = field_bytes(2, text.as_bytes());
        let document = field_bytes(3, &note);
        let store = field_bytes(2, &document);
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&store).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn extracts_text_through_the_protobuf_layers() {
        let zdata = make_zdata("Shopping\nMilk\nEggs");
        let body = decode_note_body(&zdata).unwrap();
        assert_eq!(body, "Shopping\nMilk\nEggs");
    }

    #[test]
    fn malformed_protobuf_yields_none_not_panic() {
        // Truncated varints / random bytes must not panic the field walker.
        assert!(note_text_from_protobuf(&[0xff, 0xff, 0xff]).is_none());
        assert!(note_text_from_protobuf(&[]).is_none());
        assert!(decode_note_body(b"not gzip").is_none());
    }

    #[test]
    fn strips_title_and_builds_html() {
        let body = strip_leading_title("Shopping\nMilk\nEggs", Some("Shopping"));
        assert_eq!(body, "Milk\nEggs");
        assert_eq!(text_to_html("a < b\nc & d"), "a &lt; b<br>c &amp; d");
    }

    fn make_note_store(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("NoteStore.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZICNOTEDATA (Z_PK INTEGER PRIMARY KEY, ZNOTE INTEGER, ZDATA BLOB);
             CREATE TABLE ZICCLOUDSYNCINGOBJECT (
                Z_PK INTEGER PRIMARY KEY, ZTITLE1 TEXT, ZTITLE2 TEXT, ZSNIPPET TEXT,
                ZFOLDER INTEGER, ZNOTEDATA INTEGER,
                ZCREATIONDATE1 REAL, ZMODIFICATIONDATE1 REAL, ZMARKEDFORDELETION INTEGER);
             -- A folder object (ZTITLE2 set, no note data).
             INSERT INTO ZICCLOUDSYNCINGOBJECT (Z_PK, ZTITLE2) VALUES (1, 'Groceries');",
        )
        .unwrap();
        // A note in the Groceries folder. Core Data time: unix 1_700_000_000 =
        // 721692800 seconds since 2001.
        let zdata = make_zdata("Shopping\nMilk\nEggs");
        conn.execute(
            "INSERT INTO ZICNOTEDATA (Z_PK, ZNOTE, ZDATA) VALUES (5, 10, ?1)",
            rusqlite::params![zdata],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ZICCLOUDSYNCINGOBJECT
                (Z_PK, ZTITLE1, ZSNIPPET, ZFOLDER, ZNOTEDATA, ZCREATIONDATE1, ZMODIFICATIONDATE1, ZMARKEDFORDELETION)
             VALUES (10, 'Shopping', NULL, 1, 5, 721692800.0, 721692900.0, 0)",
            [],
        )
        .unwrap();
        db
    }

    #[test]
    fn parses_a_note_from_note_store() {
        let tmp = tempfile::tempdir().unwrap();
        let db = make_note_store(tmp.path());
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();

        parse_notes(&db, &cache, &mut report, false).unwrap();
        assert_eq!(report.notes, 1);

        let c = cache.conn();
        let (folder, title, snippet, body, created): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
        ) = c
            .query_row(
                "SELECT folder, title, snippet, body_html, created_at FROM notes",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(folder.as_deref(), Some("Groceries"));
        assert_eq!(title.as_deref(), Some("Shopping"));
        // Title stripped from the body; derived snippet from the first body line.
        assert_eq!(body.as_deref(), Some("Milk<br>Eggs"));
        assert_eq!(snippet.as_deref(), Some("Milk"));
        assert_eq!(created, 1_700_000_000);
    }

    #[test]
    fn non_notes_schema_errors_so_caller_can_fall_back() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("random.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE foo (a INTEGER);").unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        assert!(parse_notes(&db, &cache, &mut report, false).is_err());
        assert_eq!(report.notes, 0);
    }
}
