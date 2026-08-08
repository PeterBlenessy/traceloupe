//! Find an app's media columns by looking at the data, not by knowing the schema.
//!
//! Every app module names the columns its media lives in, which works right up
//! until the app ships a new schema. Then the column is gone, the hand-written
//! query finds nothing (or, worse, fails to prepare and takes the whole parse
//! with it — #360), and the app's photos silently stop appearing. That failure
//! is invisible: an empty gallery looks exactly like a device with no photos.
//!
//! This pass asks a different question. Rather than "is `ZMEDIALOCALPATH`
//! there?", it asks "which column in this database holds values that are
//! *actually media in this backup*?" — and answers it against the Manifest,
//! which is ground truth. A column whose values resolve to real files is a
//! media column whatever it happens to be called this year.
//!
//! Two shapes are recognised:
//!
//! - **Paths.** Text that looks like a media filename AND whose basename is a
//!   file the backup contains. The second half is what makes this safe to run
//!   automatically: a guess that resolves to nothing scores nothing.
//! - **Inline bytes.** A blob whose leading bytes are a JPEG/PNG/HEIC/GIF/MP4
//!   signature. Some apps (Threema) keep photos in the database rather than on
//!   disk, so there is no path to find at all.
//!
//! Validated on the public iOS 17 backup: run blind against every app module,
//! it independently rediscovered `ZWAMEDIAITEM.ZMEDIALOCALPATH` (WhatsApp) and
//! `ZATTACHMENT.ZNAME` (Viber) — the two columns that had been found by hand —
//! with resolve counts matching what those hand-written parsers produce.

use rusqlite::types::ValueRef;
use rusqlite::Connection;

/// How many rows of a column to look at before deciding. Enough to be sure, few
/// enough that scanning every column of every table stays cheap.
const SAMPLE: usize = 200;
/// A column has to be *mostly* media to count. A stray URL in a text column
/// that also holds message bodies is not a media column.
const MIN_RATIO: f64 = 0.5;
/// How many distinct message links a column must demonstrate before it is
/// believed. Below this, a coincidence is indistinguishable from a relationship.
const MIN_LINK_ROWS: usize = 3;

/// What a discovered column holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaShape {
    /// Text naming a file that exists in the backup.
    Path,
    /// Bytes of an image/video, stored in the database itself.
    Inline,
}

/// One column that appears to hold media, and the evidence for saying so.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Discovery {
    pub table: String,
    pub column: String,
    pub shape: MediaShape,
    /// Values sampled from the column.
    pub sampled: usize,
    /// Of those, how many looked like media.
    pub matched: usize,
    /// Of those, how many are genuinely present (a file in the backup, or bytes
    /// with a real signature). This is the number worth trusting.
    pub verified: usize,
}

impl Discovery {
    /// A one-line account of how this column was chosen, for the import record.
    /// Discovery must never be silent: an examiner needs to see that a file was
    /// attributed by inference and on what evidence, not just that it appeared.
    pub fn describe(&self) -> String {
        let shape = match self.shape {
            MediaShape::Path => "paths",
            MediaShape::Inline => "inline bytes",
        };
        format!(
            "{}.{} holds {shape} ({} of {} sampled verified)",
            self.table, self.column, self.verified, self.sampled
        )
    }
}

/// The extensions worth treating as gallery media.
const MEDIA_EXTS: [&str; 10] = [
    "jpg", "jpeg", "png", "heic", "heif", "gif", "webp", "mp4", "mov", "m4v",
];

/// Whether `name` ends in a media extension.
pub fn looks_like_media_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    MEDIA_EXTS.iter().any(|e| lower.ends_with(&format!(".{e}")))
}

/// The image/video format `bytes` starts with, if any.
///
/// Scans a short window rather than only offset 0. Core Data stores a binary
/// attribute with a one-byte prefix, so Threema's photos are a plain JPEG whose
/// signature begins at byte 1 — checking offset 0 alone reports "not an image"
/// for a perfectly good picture, which is exactly what an early version of this
/// did.
pub fn media_magic(bytes: &[u8]) -> Option<&'static str> {
    const WINDOW: usize = 8;
    for off in 0..=WINDOW.min(bytes.len()) {
        let b = &bytes[off..];
        if b.len() < 12 {
            break;
        }
        if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some("jpeg");
        }
        if b.starts_with(&[0x89, b'P', b'N', b'G']) {
            return Some("png");
        }
        if b.starts_with(b"GIF8") {
            return Some("gif");
        }
        if b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
            return Some("webp");
        }
        if &b[4..8] == b"ftyp" {
            return Some(match &b[8..12] {
                b"heic" | b"heix" | b"mif1" | b"msf1" => "heic",
                _ => "mp4",
            });
        }
    }
    None
}

/// Scan every column of `conn` for media, verifying paths with `present`.
///
/// `present` is handed a basename and answers whether the backup holds a file by
/// that name — the Manifest, in production. Nothing is reported on the strength
/// of appearances alone.
pub fn discover_media(conn: &Connection, present: &dyn Fn(&str) -> bool) -> Vec<Discovery> {
    let mut out = Vec::new();
    let Ok(mut stmt) = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'") else {
        return out;
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
        return out;
    };
    let tables: Vec<String> = rows.flatten().collect();

    for table in tables {
        // Core Data's own bookkeeping holds no user media and every row of it
        // would be sampled for nothing.
        if table.starts_with("Z_METADATA") || table.starts_with("sqlite_") {
            continue;
        }
        for column in columns_of(conn, &table) {
            if let Some(d) = score_column(conn, &table, &column, present) {
                out.push(d);
            }
        }
    }
    // Best evidence first, so a caller taking the top candidate takes the one
    // with the most verified rows behind it.
    out.sort_by_key(|d| std::cmp::Reverse(d.verified));
    out
}

fn columns_of(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info(\"{table}\")")) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

fn score_column(
    conn: &Connection,
    table: &str,
    column: &str,
    present: &dyn Fn(&str) -> bool,
) -> Option<Discovery> {
    let sql = format!(
        "SELECT \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL LIMIT {SAMPLE}"
    );
    let mut stmt = conn.prepare(&sql).ok()?;
    let mut rows = stmt.query([]).ok()?;

    let (mut sampled, mut path_hits, mut path_ok, mut blob_hits) = (0usize, 0usize, 0usize, 0usize);
    while let Ok(Some(r)) = rows.next() {
        match r.get_ref(0) {
            Ok(ValueRef::Text(b)) => {
                sampled += 1;
                let s = String::from_utf8_lossy(b);
                let base = s.rsplit(['/', '\\']).next().unwrap_or("");
                if looks_like_media_name(base) {
                    path_hits += 1;
                    if present(base) {
                        path_ok += 1;
                    }
                }
            }
            Ok(ValueRef::Blob(b)) => {
                sampled += 1;
                if media_magic(b).is_some() {
                    blob_hits += 1;
                }
            }
            _ => sampled += 1,
        }
    }
    if sampled == 0 {
        return None;
    }
    let ratio = |n: usize| n as f64 / sampled as f64;

    // Paths are only reported when at least one genuinely resolves. A column
    // full of filenames for media that is not in the backup (Threema's
    // ZFILENAME) describes something real but yields nothing to show, and
    // reporting it as a find would be a promise the gallery cannot keep.
    if path_hits > 0 && ratio(path_hits) >= MIN_RATIO && path_ok > 0 {
        return Some(Discovery {
            table: table.to_string(),
            column: column.to_string(),
            shape: MediaShape::Path,
            sampled,
            matched: path_hits,
            verified: path_ok,
        });
    }
    if blob_hits > 0 && ratio(blob_hits) >= MIN_RATIO {
        return Some(Discovery {
            table: table.to_string(),
            column: column.to_string(),
            shape: MediaShape::Inline,
            sampled,
            matched: blob_hits,
            verified: blob_hits,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(prefix: &[u8]) -> Vec<u8> {
        let mut v = prefix.to_vec();
        v.extend_from_slice(&[
            0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1,
        ]);
        v
    }

    /// Core Data prefixes a binary attribute, so a stored photo's signature does
    /// not start at byte 0. Checking only offset 0 called Threema's pictures
    /// "not an image" and found nothing at all.
    #[test]
    fn magic_is_found_behind_a_core_data_prefix() {
        assert_eq!(media_magic(&jpeg(&[])), Some("jpeg"));
        assert_eq!(media_magic(&jpeg(&[0x01])), Some("jpeg"));
        assert_eq!(media_magic(&jpeg(&[0x04, 0x0A])), Some("jpeg"));
        assert_eq!(media_magic(b"not an image at all, just text"), None);
    }

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE ZMEDIA (Z_PK INTEGER PRIMARY KEY, ZPATH TEXT, ZBODY TEXT, ZBLOB BLOB);",
        )
        .unwrap();
        c
    }

    /// The point of the pass: find the media column without being told its name.
    #[test]
    fn finds_a_path_column_by_its_values() {
        let c = db();
        for (i, p) in ["a/one.jpg", "a/two.jpg", "a/three.heic"]
            .iter()
            .enumerate()
        {
            c.execute(
                "INSERT INTO ZMEDIA (Z_PK, ZPATH, ZBODY) VALUES (?1, ?2, 'hello there')",
                rusqlite::params![i as i64, p],
            )
            .unwrap();
        }
        let present = |_: &str| true;
        let found = discover_media(&c, &present);
        assert_eq!(found.len(), 1, "only the path column, got {found:?}");
        assert_eq!(found[0].column, "ZPATH");
        assert_eq!(found[0].shape, MediaShape::Path);
        assert_eq!(found[0].verified, 3);
    }

    /// Filenames for media that is not in the backup are not a find. Reporting
    /// them would promise the gallery something it cannot show.
    #[test]
    fn a_path_column_that_resolves_to_nothing_is_not_reported() {
        let c = db();
        c.execute(
            "INSERT INTO ZMEDIA (Z_PK, ZPATH) VALUES (1, 'gone/missing.jpg')",
            [],
        )
        .unwrap();
        let absent = |_: &str| false;
        assert!(discover_media(&c, &absent).is_empty());
    }

    /// Photos kept in the database rather than on disk have no path to find.
    #[test]
    fn finds_inline_image_bytes() {
        let c = db();
        c.execute(
            "INSERT INTO ZMEDIA (Z_PK, ZBLOB) VALUES (1, ?1)",
            rusqlite::params![jpeg(&[0x01])],
        )
        .unwrap();
        let present = |_: &str| true;
        let found = discover_media(&c, &present);
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].column, "ZBLOB");
        assert_eq!(found[0].shape, MediaShape::Inline);
    }

    /// A column of message bodies must not be mistaken for media because one
    /// row happens to mention a filename.
    #[test]
    fn a_mostly_text_column_is_not_a_media_column() {
        let c = db();
        for (i, b) in ["see photo.jpg", "hello", "how are you", "fine thanks"]
            .iter()
            .enumerate()
        {
            c.execute(
                "INSERT INTO ZMEDIA (Z_PK, ZBODY) VALUES (?1, ?2)",
                rusqlite::params![i as i64, b],
            )
            .unwrap();
        }
        let present = |_: &str| true;
        assert!(discover_media(&c, &present).is_empty());
    }
}

/// The column in `table` that points at a message, if there is one.
///
/// Core Data writes a relationship as a plain integer column, so a media table's
/// link to its message is just "some INTEGER column whose values are message row
/// ids". There is no metadata saying which — so this tests the candidates
/// against the ids we actually imported and takes the one that mostly hits.
///
/// A name like `ZMESSAGE` is a hint, not proof, and is used only to break ties:
/// the evidence is the overlap.
pub fn infer_message_fk(
    conn: &Connection,
    table: &str,
    known_ids: &std::collections::HashSet<i64>,
) -> Option<String> {
    if known_ids.is_empty() {
        return None;
    }
    let mut best: Option<(String, usize)> = None;
    for column in columns_of(conn, table) {
        // The row's own key is not a link to anything, and Core Data's
        // bookkeeping columns are constants. `Z_ENT` is the entity number —
        // every row of a table carries the same value, and if that number
        // happens to equal a message id, EVERY image in the table gets
        // attached to that one message. Which is exactly what it did.
        if ["Z_PK", "rowid", "Z_ENT", "Z_OPT"]
            .iter()
            .any(|c| column.eq_ignore_ascii_case(c))
        {
            continue;
        }
        let sql = format!(
            "SELECT \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL LIMIT {SAMPLE}"
        );
        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let Ok(rows) = stmt.query_map([], |r| r.get::<_, i64>(0)) else {
            continue;
        };
        let vals: Vec<i64> = rows.flatten().collect();
        if vals.is_empty() {
            continue;
        }
        let hits = vals.iter().filter(|v| known_ids.contains(v)).count();
        // Most of the column has to land on real messages. A counter or a size
        // column will hit a few ids by coincidence; a relationship hits nearly
        // all of them.
        if hits * 4 < vals.len() * 3 {
            continue;
        }
        // And it must not collapse. A real one-media-per-message relationship
        // is close to injective; a column that maps twenty rows onto two ids is
        // a category or a flag that happens to share values with message ids,
        // and using it would attribute a pile of photos to one message.
        let distinct: std::collections::HashSet<i64> = vals
            .iter()
            .copied()
            .filter(|v| known_ids.contains(v))
            .collect();
        if distinct.len() * 2 < hits {
            continue;
        }
        // One row of evidence is not evidence. A single-row table whose key
        // happens to equal a message id looks like a perfect relationship —
        // that is how a contact's avatar came to be attached to "Are you here
        // yet?". A relationship has to be demonstrated across several rows, and
        // if it cannot be, the media still reaches the gallery; it just is not
        // claimed to belong to a conversation.
        if hits < MIN_LINK_ROWS || distinct.len() < MIN_LINK_ROWS {
            continue;
        }
        let named = column.to_uppercase().contains("MESSAGE");
        let score = hits * 2 + usize::from(named);
        if best.as_ref().is_none_or(|(_, b)| score > *b) {
            best = Some((column, score));
        }
    }
    best.map(|(c, _)| c)
}

#[cfg(test)]
mod fk_tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE ZIMAGEDATA (Z_PK INTEGER PRIMARY KEY, ZMESSAGE INTEGER,
                 ZWIDTH INTEGER, ZDATA BLOB);",
        )
        .unwrap();
        for (pk, msg, w) in [(1, 41, 512), (2, 42, 433), (3, 43, 319)] {
            c.execute(
                "INSERT INTO ZIMAGEDATA (Z_PK, ZMESSAGE, ZWIDTH) VALUES (?1, ?2, ?3)",
                rusqlite::params![pk, msg, w],
            )
            .unwrap();
        }
        c
    }

    /// The link is found from the values, not the name — Core Data gives no
    /// metadata saying which integer column is a relationship.
    #[test]
    fn finds_the_message_relationship_by_overlap() {
        let ids: std::collections::HashSet<i64> = [41, 42, 43, 44].into_iter().collect();
        assert_eq!(
            infer_message_fk(&db(), "ZIMAGEDATA", &ids).as_deref(),
            Some("ZMESSAGE")
        );
    }

    /// A column of unrelated numbers must not be mistaken for the link. ZWIDTH
    /// holds plausible-looking integers and would attach photos to whichever
    /// messages happened to share an id with a pixel count.
    #[test]
    fn a_column_of_unrelated_numbers_is_not_a_link() {
        // Only ZWIDTH's values are "known", so ZMESSAGE cannot win on overlap.
        let ids: std::collections::HashSet<i64> = [512].into_iter().collect();
        assert_eq!(infer_message_fk(&db(), "ZIMAGEDATA", &ids), None);
    }

    /// `Z_ENT` is the same number on every row of a Core Data table. When that
    /// number happened to be a message id, every image in the table was
    /// attached to that single message — eight photos on one line of a
    /// conversation. A constant is never a relationship.
    #[test]
    fn a_constant_core_data_column_is_never_the_link() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE ZIMAGEDATA (Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER,
                 ZMESSAGE INTEGER, ZDATA BLOB);",
        )
        .unwrap();
        for pk in 1..=8 {
            // ZMESSAGE unpopulated, exactly as Threema ships it.
            c.execute(
                "INSERT INTO ZIMAGEDATA (Z_PK, Z_ENT, ZMESSAGE) VALUES (?1, 12, NULL)",
                rusqlite::params![pk],
            )
            .unwrap();
        }
        // 12 IS a real message id — the coincidence that caused the bug.
        let ids: std::collections::HashSet<i64> = (1..=200).collect();
        assert_eq!(
            infer_message_fk(&c, "ZIMAGEDATA", &ids),
            None,
            "no column says which message these belong to, so nothing may be claimed"
        );
    }

    /// A lone coincidental match is not a relationship. A contact avatar whose
    /// table key happened to equal a message id was attached to that message,
    /// which read as "someone sent this picture" when nobody had.
    #[test]
    fn a_single_coincidental_match_is_not_a_link() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE ZCONTACT (Z_PK INTEGER PRIMARY KEY, ZIMAGEDATA BLOB);")
            .unwrap();
        c.execute("INSERT INTO ZCONTACT (Z_PK) VALUES (2)", [])
            .unwrap();
        let ids: std::collections::HashSet<i64> = (1..=200).collect();
        assert_eq!(infer_message_fk(&c, "ZCONTACT", &ids), None);
    }

    /// Nothing to match against means no guessing.
    #[test]
    fn no_known_ids_means_no_link() {
        assert_eq!(
            infer_message_fk(&db(), "ZIMAGEDATA", &std::collections::HashSet::new()),
            None
        );
    }
}
