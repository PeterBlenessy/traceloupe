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
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use rusqlite::{Connection, OpenFlags};

use crate::cache::CacheDb;
use crate::crypto::{self, BackupDecryptor};
use crate::manifest::ManifestIndex;
use crate::normalize::ImportReport;
use crate::Result;

/// The Notes backup domain — where a note's Media/Previews images live.
const NOTES_DOMAIN: &str = "AppDomainGroup-group.com.apple.notes";

/// Lets the parser resolve a note's embedded image to its backup file (for a list
/// thumbnail). `None` when no Manifest is available (e.g. tests) — notes still
/// parse, just without image thumbnails.
pub struct NoteImageSource<'a> {
    pub index: &'a ManifestIndex,
    /// `Some` for an encrypted backup — the image blob is then ciphertext and its
    /// wrapped key is stored for on-demand decryption at view time.
    pub decryptor: Option<&'a BackupDecryptor>,
}

/// A note's first embedded image, resolved to a servable backup blob.
struct NoteImage {
    local_path: String,
    decrypt_key: Option<Vec<u8>>,
    plain_size: Option<i64>,
    mime: Option<String>,
}

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

/// Like [`col_or_null`] but builds `COALESCE(n.a, n.b, …)` over *every* candidate
/// that exists, so a present-but-NULL column can't shadow a populated sibling
/// (Core Data's suffixed date columns). Returns `"NULL"` if none exist.
fn coalesce_or_null(cols: &HashSet<String>, candidates: &[&str]) -> String {
    let present: Vec<String> = candidates
        .iter()
        .filter(|c| cols.contains(**c))
        .map(|c| format!("n.{c}"))
        .collect();
    match present.len() {
        0 => "NULL".to_string(),
        1 => present.into_iter().next().unwrap(),
        _ => format!("COALESCE({})", present.join(", ")),
    }
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
    images: Option<&NoteImageSource>,
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
    // Core Data keeps several suffixed date columns and only one is populated —
    // e.g. on a modern NoteStore ZCREATIONDATE1 exists but is entirely NULL while
    // the real value moved to ZCREATIONDATE3. Picking the first *existing* column
    // (col_or_null) would lock onto the NULL one, so COALESCE across all that
    // exist and let the populated sibling win.
    let created = coalesce_or_null(
        &cols,
        &[
            "ZCREATIONDATE1",
            "ZCREATIONDATE3",
            "ZCREATIONDATE",
            "ZCREATIONDATE2",
        ],
    );
    let modified = coalesce_or_null(
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

    // Password-protected (locked) notes: the body ciphertext is `ZICNOTEDATA.ZDATA`
    // (same column an unlocked note's gzip body uses), AES-GCM'd under a per-note key
    // that's wrapped by a PBKDF2 key from the note password. The salt/iterations/
    // wrapped-key are on the note *object* (n); the IV/tag are on the ZICNOTEDATA
    // row (d) — a common source of "unlock always fails" bugs.
    let protected = col_or_null(&cols, &["ZISPASSWORDPROTECTED"]);
    let salt = col_or_null(&cols, &["ZCRYPTOSALT"]);
    let iter = col_or_null(&cols, &["ZCRYPTOITERATIONCOUNT"]);
    let wrapped = col_or_null(&cols, &["ZCRYPTOWRAPPEDKEY"]);
    let hint = col_or_null(&cols, &["ZPASSWORDHINT"]);
    let ndata_cols = table_columns(&src, "ZICNOTEDATA")?;
    let iv = if ndata_cols.contains("ZCRYPTOINITIALIZATIONVECTOR") {
        "d.ZCRYPTOINITIALIZATIONVECTOR"
    } else {
        "NULL"
    };
    let tag = if ndata_cols.contains("ZCRYPTOTAG") {
        "d.ZCRYPTOTAG"
    } else {
        "NULL"
    };
    // Pinned-to-top flag (independent of lock state).
    let pinned = col_or_null(&cols, &["ZISPINNED"]);
    // Rich-content indicators: the checklist flag, and per-note counts of embedded
    // attachments (image/video vs total) via the attachment objects' ZNOTE back-ref.
    // Filter on ZTYPEUTI (only attachments carry it) rather than the version-specific
    // entity number. Absent-column schemas fall back to 0.
    let checklist = col_or_null(&cols, &["ZHASCHECKLIST"]);
    let (image_count_expr, attach_count_expr) =
        if cols.contains("ZTYPEUTI") && cols.contains("ZNOTE") {
            (
                "(SELECT COUNT(*) FROM ZICCLOUDSYNCINGOBJECT a WHERE a.ZNOTE = n.Z_PK \
              AND a.ZTYPEUTI IN ('public.png','public.jpeg','public.heic','public.avif',\
              'org.webmproject.webp','public.mpeg-4','com.apple.quicktime-movie'))",
                "(SELECT COUNT(*) FROM ZICCLOUDSYNCINGOBJECT a WHERE a.ZNOTE = n.Z_PK \
              AND a.ZTYPEUTI IS NOT NULL)",
            )
        } else {
            ("0", "0")
        };
    // Hashtag tags (iOS 15+): each tag is an inline-attachment token whose text is
    // in ZALTTEXT and whose note is ZNOTE1 (distinct from the media columns
    // ZTYPEUTI/ZNOTE). Pre-load note_pk → [tags]; empty when the columns are absent.
    let note_tags = load_note_tags(&src, &cols)?;

    // First embedded image per note, resolved to a backup blob for a thumbnail.
    // Empty when there's no Manifest (tests) or the schema lacks the columns.
    let note_images = load_note_images(&src, &cols, images);

    // One row per note: its columns + its folder's title + its body blob `d.ZDATA`
    // (gzip protobuf when unlocked, AES-GCM ciphertext when locked) + crypto params +
    // the note's own Z_PK (last), used to attach its tags.
    // `WHERE ZNOTEDATA IS NOT NULL` selects note objects (folders/accounts have none).
    let sql = format!(
        "SELECT {title}, {snippet}, {created}, {modified}, {deleted}, {folder_title}, d.ZDATA,
                {protected}, {wrapped}, {salt}, {iter}, {iv}, {tag}, {hint}, {pinned},
                {checklist}, {image_count_expr}, {attach_count_expr}, n.Z_PK
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
        let protected = r.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0;
        let crypto_wrapped: Option<Vec<u8>> = r.get(8)?;
        let crypto_salt: Option<Vec<u8>> = r.get(9)?;
        let crypto_iter: Option<i64> = r.get(10)?;
        let crypto_iv: Option<Vec<u8>> = r.get(11)?;
        let crypto_tag: Option<Vec<u8>> = r.get(12)?;
        let password_hint: Option<String> = r
            .get::<_, Option<String>>(13)?
            .filter(|s| !s.trim().is_empty());
        let pinned = r.get::<_, Option<i64>>(14)?.unwrap_or(0) != 0;
        let has_checklist = r.get::<_, Option<i64>>(15)?.unwrap_or(0) != 0;
        let image_count: i64 = r.get::<_, Option<i64>>(16)?.unwrap_or(0);
        let attachment_count: i64 = r.get::<_, Option<i64>>(17)?.unwrap_or(0);
        let note_pk: i64 = r.get(18)?;
        // Tags as a JSON array (None when the note has none), for the tag filter.
        let tags = note_tags
            .get(&note_pk)
            .filter(|v| !v.is_empty())
            .and_then(|v| serde_json::to_string(v).ok());

        // Notes in "Recently Deleted" have no folder row of their own; label them
        // so they're distinguishable rather than showing an empty folder.
        let folder = folder_name
            .filter(|s| !s.trim().is_empty())
            .or_else(|| marked_deleted.then(|| "Recently Deleted".to_string()));

        // A locked note has its body encrypted — withhold body/snippet and store
        // the crypto params (ciphertext = ZDATA, wrapped key + salt/iter, IV/tag) so
        // it can be unlocked on demand (never plaintext here).
        let locked = protected;
        if locked {
            tx.execute(
                "INSERT INTO notes
                    (folder, title, snippet, body_html, created_at, modified_at,
                     locked, password_hint, crypto_salt, crypto_iter, crypto_iv, crypto_tag, encrypted_data, pinned,
                     has_checklist, image_count, attachment_count, crypto_wrapped_key, tags)
                 VALUES (?1, ?2, NULL, NULL, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                rusqlite::params![
                    folder,
                    title,
                    created_at,
                    modified_at,
                    password_hint,
                    crypto_salt,
                    crypto_iter,
                    crypto_iv,
                    crypto_tag,
                    zdata,
                    pinned,
                    has_checklist as i64,
                    image_count,
                    attachment_count,
                    crypto_wrapped,
                    // Withhold tags too: a locked note's hashtag alt-text is stored
                    // in cleartext, so surfacing it would leak protected content.
                    None::<String>,
                ],
            )?;
            report.notes += 1;
            continue;
        }

        let body_text = zdata
            .as_deref()
            .and_then(decode_note_body)
            .unwrap_or_default();
        // The note text repeats the title as its first line; drop it so the body
        // isn't a duplicate of the heading the UI already shows.
        let body_text = strip_leading_title(&body_text, title.as_deref());
        // Stored as PLAIN TEXT (newlines preserved): the Notes view renders the
        // body in a `whitespace-pre-wrap` block as text, matching the iLEAPP path.
        // (Emitting HTML here would show literal `<br>`/`&amp;` in the UI.)
        let body = if body_text.is_empty() {
            None
        } else {
            Some(clean_note_text(&body_text))
        };
        // Prefer the stored snippet; otherwise derive one from the body.
        let snippet = snippet
            .filter(|s| !s.trim().is_empty())
            .or_else(|| derive_snippet(&body_text));

        // Rich HTML (formatting, lists, checklists) from the same protobuf; the UI
        // renders it when present, falling back to the plain body otherwise.
        let body_rich = zdata.as_deref().and_then(decode_note_rich);

        // Its first embedded image (for a list thumbnail), if resolved.
        let img = note_images.get(&note_pk);
        tx.execute(
            "INSERT INTO notes (folder, title, snippet, body_html, body_rich, created_at, modified_at, locked, pinned,
                                has_checklist, image_count, attachment_count, tags,
                                image_local_path, image_decrypt_key, image_plain_size, image_mime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                folder,
                title,
                snippet,
                body,
                body_rich,
                created_at,
                modified_at,
                pinned,
                has_checklist as i64,
                image_count,
                attachment_count,
                tags,
                img.map(|i| i.local_path.as_str()),
                img.and_then(|i| i.decrypt_key.as_deref()),
                img.and_then(|i| i.plain_size),
                img.and_then(|i| i.mime.as_deref()),
            ],
        )?;
        report.notes += 1;
    }
    drop(rows);
    drop(stmt);
    tx.commit()?;
    Ok(())
}

/// Decrypt a locked note's body from its stored crypto params + the note password.
/// Returns the plain text, or None on a wrong password / undecodable body (the
/// AES-GCM tag check makes a wrong password a clean failure). Never persisted.
pub fn decrypt_locked_note(
    password: &str,
    salt: &[u8],
    iterations: u32,
    iv: &[u8],
    tag: &[u8],
    encrypted_data: &[u8],
    wrapped_key: &[u8],
) -> Option<String> {
    // The decrypted blob is the same gzip-protobuf an unlocked note stores.
    let gz = crate::crypto::decrypt_note(
        password,
        salt,
        iterations,
        iv,
        tag,
        encrypted_data,
        wrapped_key,
    )
    .ok()?;
    let text = decode_note_body(&gz)?;
    Some(clean_note_text(&text))
}

/// Load `note_pk → [hashtag tags]` from the inline hashtag-attachment tokens.
/// Empty when the schema predates tags (iOS &lt; 15) or lacks the columns.
fn load_note_tags(
    conn: &Connection,
    cols: &HashSet<String>,
) -> Result<std::collections::HashMap<i64, Vec<String>>> {
    let mut map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    if !(cols.contains("ZTYPEUTI1") && cols.contains("ZNOTE1") && cols.contains("ZALTTEXT")) {
        return Ok(map);
    }
    let mut stmt = conn.prepare(
        "SELECT ZNOTE1, ZALTTEXT FROM ZICCLOUDSYNCINGOBJECT
         WHERE ZTYPEUTI1 = 'com.apple.notes.inlinetextattachment.hashtag'
           AND ZNOTE1 IS NOT NULL AND ZALTTEXT IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    // Best-effort per row: a wrongly-typed cell (SQLite is dynamically typed)
    // skips that tag rather than aborting the whole Notes import.
    while let Ok(Some(r)) = rows.next() {
        let (Ok(note_pk), Ok(tag)) = (r.get::<_, i64>(0), r.get::<_, String>(1)) else {
            continue;
        };
        let tag = tag.trim().to_string();
        if tag.is_empty() {
            continue;
        }
        let entry = map.entry(note_pk).or_default();
        if !entry.contains(&tag) {
            entry.push(tag);
        }
    }
    Ok(map)
}

/// Image UTIs we resolve for a note thumbnail (matches the `image_count` set).
const NOTE_IMAGE_UTIS: &str =
    "'public.jpeg','public.png','public.heic','public.avif','org.webmproject.webp'";

/// Resolve each note's FIRST embedded image (lowest attachment Z_PK) to a servable
/// backup blob for a list thumbnail. Empty when there's no Manifest or the schema
/// lacks the object columns. Best-effort per note (unresolved images are skipped).
fn load_note_images(
    conn: &Connection,
    cols: &HashSet<String>,
    images: Option<&NoteImageSource>,
) -> std::collections::HashMap<i64, NoteImage> {
    let mut map = std::collections::HashMap::new();
    let Some(src) = images else {
        return map;
    };
    if !(cols.contains("ZMEDIA")
        && cols.contains("ZACCOUNT1")
        && cols.contains("ZTYPEUTI")
        && cols.contains("ZNOTE"))
    {
        return map;
    }
    // First image attachment per note, joined to its media file record, its
    // account (for the on-disk Accounts/<uuid>/… path), and an optional
    // pre-rendered preview (Z_ENT 6, linked by ZATTACHMENT).
    let sql = format!(
        "SELECT a.ZNOTE, m.ZIDENTIFIER, m.ZFILENAME, acc.ZIDENTIFIER, p.ZIDENTIFIER
         FROM ZICCLOUDSYNCINGOBJECT a
         JOIN ZICCLOUDSYNCINGOBJECT m ON m.Z_PK = a.ZMEDIA
         JOIN ZICCLOUDSYNCINGOBJECT acc ON acc.Z_PK = a.ZACCOUNT1
         LEFT JOIN ZICCLOUDSYNCINGOBJECT p ON p.ZATTACHMENT = a.Z_PK AND p.Z_ENT = 6
         WHERE a.ZNOTE IS NOT NULL AND a.ZTYPEUTI IN ({NOTE_IMAGE_UTIS})
         ORDER BY a.ZNOTE, a.Z_PK"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return map;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return map;
    };
    while let Ok(Some(r)) = rows.next() {
        let Ok(note_pk) = r.get::<_, i64>(0) else {
            continue;
        };
        if map.contains_key(&note_pk) {
            continue; // keep the first image per note
        }
        let media_uuid = r.get::<_, Option<String>>(1).ok().flatten();
        let filename = r.get::<_, Option<String>>(2).ok().flatten();
        let Some(account) = r.get::<_, Option<String>>(3).ok().flatten() else {
            continue;
        };
        let preview_stem = r.get::<_, Option<String>>(4).ok().flatten();
        if let Some(img) = resolve_note_image(
            src,
            &account,
            media_uuid.as_deref(),
            filename.as_deref(),
            preview_stem.as_deref(),
        ) {
            map.insert(note_pk, img);
        }
    }
    map
}

/// Resolve a note image's candidate on-disk paths (pre-rendered preview first,
/// then the full-res original) against the Manifest; the first that exists wins.
fn resolve_note_image(
    src: &NoteImageSource,
    account: &str,
    media_uuid: Option<&str>,
    filename: Option<&str>,
    preview_stem: Option<&str>,
) -> Option<NoteImage> {
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    if let Some(stem) = preview_stem {
        candidates.push((
            format!("Accounts/{account}/Previews/{stem}.png"),
            Some("image/png".to_string()),
        ));
        candidates.push((
            format!("Accounts/{account}/Previews/{stem}.jpg"),
            Some("image/jpeg".to_string()),
        ));
    }
    if let (Some(media), Some(file)) = (media_uuid, filename) {
        candidates.push((
            format!("Accounts/{account}/Media/{media}/{file}"),
            mime_from_name(file),
        ));
    }
    for (rel, mime) in candidates {
        if let Ok(Some(entry)) = src.index.find(NOTES_DOMAIN, &rel) {
            let path: PathBuf = src.index.blob_path(&entry.file_id);
            let (decrypt_key, plain_size) = if src.decryptor.is_some() {
                match crypto::file_key_field(&entry.file_blob) {
                    Ok((k, s)) => (Some(k), s.and_then(|v| i64::try_from(v).ok())),
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };
            return Some(NoteImage {
                local_path: path.to_string_lossy().into_owned(),
                decrypt_key,
                plain_size,
                mime,
            });
        }
    }
    None
}

/// Best-effort image MIME from a filename extension (the protocol still renders
/// via sips regardless; this just labels the type / drives HEIC handling).
fn mime_from_name(name: &str) -> Option<String> {
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    Some(
        match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "heic" | "heif" => "image/heic",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => return None,
        }
        .to_string(),
    )
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
/// when the blob isn't gzip or the protobuf has no text field. Inflation is
/// capped so a crafted highly-compressible blob can't balloon to gigabytes and
/// OOM the process (real note bodies are kilobytes).
fn decode_note_body(zdata: &[u8]) -> Option<String> {
    const MAX_NOTE_BYTES: u64 = 64 * 1024 * 1024;
    let mut buf = Vec::new();
    GzDecoder::new(zdata)
        .take(MAX_NOTE_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    note_text_from_protobuf(&buf)
}

/// Like [`decode_note_body`] but returns rich HTML (formatting, lists,
/// checklists) from the note's attribute runs. None if it can't be decoded.
fn decode_note_rich(zdata: &[u8]) -> Option<String> {
    const MAX_NOTE_BYTES: u64 = 64 * 1024 * 1024;
    let mut buf = Vec::new();
    GzDecoder::new(zdata)
        .take(MAX_NOTE_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    note_html_from_protobuf(&buf)
}

/// Walk the `NoteStoreProto` wire format to the note text: top-level field 2
/// (Document) → field 3 (Note) → field 2 (note_text, a UTF-8 string).
fn note_text_from_protobuf(buf: &[u8]) -> Option<String> {
    let document = first_len_delimited(buf, 2)?;
    let note = first_len_delimited(document, 3)?;
    let text = first_len_delimited(note, 2)?;
    Some(String::from_utf8_lossy(text).into_owned())
}

/// A paragraph's block style (AttributeRun.paragraph_style.style_type).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Block {
    #[default]
    Body,
    Title,
    Heading,
    Subheading,
    Monospace,
    Bulleted,
    Numbered,
    /// Checklist item; the bool is its done/checked state.
    Checklist(bool),
}

/// Which HTML list a block belongs in (checklists render as a `<ul>` variant).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Ul,
    Ol,
    Check,
}

impl Block {
    fn list_kind(self) -> Option<ListKind> {
        match self {
            Block::Bulleted => Some(ListKind::Ul),
            Block::Numbered => Some(ListKind::Ol),
            Block::Checklist(_) => Some(ListKind::Check),
            _ => None,
        }
    }
}

/// Inline character styling of a run.
#[derive(Default, Clone)]
struct Inline {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    link: Option<String>,
}

/// Render the note body protobuf to sanitized rich HTML (headings, bold/italic/
/// underline/strike, bulleted/numbered lists, and checklists as checkboxes).
/// Returns None when there are no attribute runs (caller keeps the plain text).
fn note_html_from_protobuf(buf: &[u8]) -> Option<String> {
    let document = first_len_delimited(buf, 2)?;
    let note = first_len_delimited(document, 3)?;
    let text = String::from_utf8_lossy(first_len_delimited(note, 2)?).into_owned();
    let runs = all_len_delimited(note, 5);
    if runs.is_empty() {
        return None;
    }
    // Run `length` fields count UTF-16 code units, so slice the text in UTF-16.
    let utf16: Vec<u16> = text.encode_utf16().collect();

    let mut b = HtmlBuilder::default();
    let mut pos = 0usize;
    for run in runs {
        let len = first_varint(run, 1).unwrap_or(0) as usize;
        let end = pos.saturating_add(len).min(utf16.len());
        let slice = String::from_utf16_lossy(&utf16[pos..end]);
        pos = end;
        b.push_run(&slice, run_block(run), &run_inline(run));
    }
    b.finish();
    (!b.out.trim().is_empty()).then_some(b.out)
}

/// The block style of a run, from its `paragraph_style` (field 2): `style_type`
/// (field 1) and, for a checklist (100+3), the `checklist.done` flag.
fn run_block(run: &[u8]) -> Block {
    let Some(ps) = first_len_delimited(run, 2) else {
        return Block::Body;
    };
    // style_type is an int32; -1 (body) encodes as a 10-byte varint → cast down.
    let style_type = first_varint(ps, 1).map(|v| v as i64 as i32).unwrap_or(-1);
    match style_type {
        0 => Block::Title,
        1 => Block::Heading,
        2 => Block::Subheading,
        4 => Block::Monospace,
        100 | 101 => Block::Bulleted, // dotted / dashed both render as a bullet list
        102 => Block::Numbered,
        103 => {
            let done = first_len_delimited(ps, 5)
                .and_then(|cl| first_varint(cl, 2))
                .unwrap_or(0)
                != 0;
            Block::Checklist(done)
        }
        _ => Block::Body,
    }
}

/// The inline styling of a run: bold (font_weight f.5 or emphasis f.14 = 1/3),
/// italic (emphasis 2/3), underline (f.6), strikethrough (f.7), link (f.9).
fn run_inline(run: &[u8]) -> Inline {
    let emphasis = first_varint(run, 14);
    Inline {
        bold: first_varint(run, 5).is_some_and(|v| v != 0) || matches!(emphasis, Some(1) | Some(3)),
        italic: matches!(emphasis, Some(2) | Some(3)),
        underline: first_varint(run, 6).is_some_and(|v| v != 0),
        strike: first_varint(run, 7).is_some_and(|v| v != 0),
        link: first_len_delimited(run, 9)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .filter(|s| !s.is_empty()),
    }
}

/// Accumulates styled runs into HTML, reconstructing paragraphs at newlines and
/// grouping consecutive list items into `<ul>`/`<ol>`.
#[derive(Default)]
struct HtmlBuilder {
    out: String,
    line: String,
    line_block: Block,
    open_list: Option<ListKind>,
    /// Lines flushed so far — used to drop the note's leading Title line, which
    /// Apple stores as line 1 and the UI already shows as the note's heading.
    flushed: usize,
}

impl HtmlBuilder {
    /// Append a run: its text (which may contain newlines) with inline styling,
    /// flushing a paragraph at each newline.
    fn push_run(&mut self, text: &str, block: Block, inline: &Inline) {
        let segments: Vec<&str> = text.split('\n').collect();
        let last = segments.len() - 1;
        for (i, seg) in segments.into_iter().enumerate() {
            self.line_block = block;
            self.line.push_str(&inline_html(seg, inline));
            if i != last {
                self.flush_line();
            }
        }
    }

    fn flush_line(&mut self) {
        let block = self.line_block;
        let content = std::mem::take(&mut self.line);
        self.line_block = Block::default();
        let is_first = self.flushed == 0;
        self.flushed += 1;
        // Drop the leading title line (shown separately as the note heading).
        if is_first && block == Block::Title {
            return;
        }

        match block.list_kind() {
            Some(kind) => {
                if self.open_list != Some(kind) {
                    self.close_list();
                    self.out.push_str(match kind {
                        ListKind::Ul => "<ul>",
                        ListKind::Ol => "<ol>",
                        ListKind::Check => "<ul class=\"note-checklist\">",
                    });
                    self.open_list = Some(kind);
                }
                if let Block::Checklist(done) = block {
                    let checked = if done { " checked" } else { "" };
                    let cls = if done { " class=\"checked\"" } else { "" };
                    self.out.push_str(&format!(
                        "<li{cls}><input type=\"checkbox\" disabled{checked}> {content}</li>"
                    ));
                } else {
                    self.out.push_str(&format!("<li>{content}</li>"));
                }
            }
            None => {
                self.close_list();
                let (open, close) = match block {
                    Block::Title => ("<h1>", "</h1>"),
                    Block::Heading => ("<h2>", "</h2>"),
                    Block::Subheading => ("<h3>", "</h3>"),
                    Block::Monospace => ("<pre>", "</pre>"),
                    _ => ("<p>", "</p>"),
                };
                // Keep blank body lines as spacing rather than empty tags.
                if content.is_empty() && matches!(block, Block::Body) {
                    self.out.push_str("<p><br></p>");
                } else {
                    self.out.push_str(open);
                    self.out.push_str(&content);
                    self.out.push_str(close);
                }
            }
        }
    }

    fn close_list(&mut self) {
        if self.open_list.take().is_some() {
            self.out.push_str("</ul>");
        }
    }

    fn finish(&mut self) {
        if !self.line.is_empty() || self.line_block != Block::default() {
            self.flush_line();
        }
        self.close_list();
    }
}

/// Escape a text segment and wrap it in the run's inline tags. A link href is
/// only honored for http/https/mailto (else rendered as plain text), so note
/// content can't inject `javascript:` URLs into the rendered HTML.
fn inline_html(text: &str, style: &Inline) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut s = escape_html(text);
    if style.bold {
        s = format!("<strong>{s}</strong>");
    }
    if style.italic {
        s = format!("<em>{s}</em>");
    }
    if style.underline {
        s = format!("<u>{s}</u>");
    }
    if style.strike {
        s = format!("<s>{s}</s>");
    }
    if let Some(href) = style.link.as_deref() {
        let lower = href.to_ascii_lowercase();
        if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:")
        {
            s = format!("<a href=\"{}\">{s}</a>", escape_html(href));
        }
    }
    s
}

/// Minimal HTML-text escaping (the tag set we emit is fixed and trusted).
fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // The object-replacement char marks attachments/tables in the stream.
            '\u{fffc}' | '\r' => {}
            _ => out.push(c),
        }
    }
    out
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

/// All length-delimited (wire type 2) fields numbered `field`, in order — for
/// repeated messages like `attribute_run`. Bounds-checked against malformed input.
fn all_len_delimited(buf: &[u8], field: u64) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let Some((tag, n)) = read_varint(&buf[i..]) else {
            break;
        };
        i += n;
        let field_number = tag >> 3;
        match tag & 0b111 {
            0 => {
                let Some((_, n)) = read_varint(&buf[i..]) else {
                    break;
                };
                i += n;
            }
            1 => match i.checked_add(8) {
                Some(x) => i = x,
                None => break,
            },
            5 => match i.checked_add(4) {
                Some(x) => i = x,
                None => break,
            },
            2 => {
                let Some((len, n)) = read_varint(&buf[i..]) else {
                    break;
                };
                i += n;
                let Some(end) = i.checked_add(len as usize) else {
                    break;
                };
                if end > buf.len() {
                    break;
                }
                if field_number == field {
                    out.push(&buf[i..end]);
                }
                i = end;
            }
            _ => break,
        }
    }
    out
}

/// The first varint (wire type 0) field numbered `field`. Bounds-checked.
fn first_varint(buf: &[u8], field: u64) -> Option<u64> {
    let mut i = 0;
    while i < buf.len() {
        let (tag, n) = read_varint(&buf[i..])?;
        i += n;
        let field_number = tag >> 3;
        match tag & 0b111 {
            0 => {
                let (v, n) = read_varint(&buf[i..])?;
                if field_number == field {
                    return Some(v);
                }
                i += n;
            }
            1 => i = i.checked_add(8)?,
            5 => i = i.checked_add(4)?,
            2 => {
                let (len, n) = read_varint(&buf[i..])?;
                i += n;
                i = i.checked_add(len as usize)?;
                if i > buf.len() {
                    return None;
                }
            }
            _ => return None,
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

/// Clean the note's extracted text for display: drop the object-replacement
/// characters that mark attachments/tables in the stream, and carriage returns.
/// Newlines are preserved — the UI renders the body as plain text in a
/// `whitespace-pre-wrap` block. (Rich formatting — bold, lists, attachments —
/// lives in the protobuf's attribute runs, decoded in a later pass.)
fn clean_note_text(text: &str) -> String {
    text.chars()
        .filter(|&c| c != '\u{fffc}' && c != '\r')
        .collect()
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

    /// Encode a varint (wire type 0) field.
    fn field_varint(field: u64, v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, field << 3);
        put_varint(&mut out, v);
        out
    }

    /// Build one AttributeRun: text length + paragraph style_type (+ checklist done)
    /// (+ bold).
    fn make_run(length: u64, style_type: u64, checklist_done: Option<bool>, bold: bool) -> Vec<u8> {
        let mut ps = field_varint(1, style_type);
        if let Some(done) = checklist_done {
            let cl = field_varint(2, done as u64);
            ps.extend(field_bytes(5, &cl));
        }
        let mut r = field_varint(1, length); // run length (UTF-16 units)
        r.extend(field_bytes(2, &ps)); // paragraph_style
        if bold {
            r.extend(field_varint(5, 1)); // font_weight → bold
        }
        r
    }

    #[test]
    fn renders_rich_html_headings_checklist_and_escapes() {
        // "Title\nHeading\n<b> & bold\nMilk\nEggs" with per-line runs.
        let text = "Title\nHeading\n<b> & bold\nMilk\nEggs";
        let mut note = field_bytes(2, text.as_bytes());
        // Each AttributeRun is field 5 (repeated) on the Note.
        for run in [
            make_run(6, 0, None, false),          // "Title\n"      (title, dropped)
            make_run(8, 1, None, false),          // "Heading\n"    (heading)
            make_run(11, 4294967295, None, true), // "<b> & bold\n" bold body (-1)
            make_run(5, 103, Some(true), false),  // "Milk\n"       checklist done
            make_run(4, 103, Some(false), false), // "Eggs"        checklist
        ] {
            note.extend(field_bytes(5, &run));
        }
        let document = field_bytes(3, &note);
        let store = field_bytes(2, &document);
        let html = note_html_from_protobuf(&store).unwrap();

        assert!(!html.contains("Title"), "leading title line dropped");
        assert!(
            html.contains("<h2>Heading</h2>"),
            "heading rendered: {html}"
        );
        assert!(
            html.contains("<ul class=\"note-checklist\">"),
            "checklist list: {html}"
        );
        assert!(
            html.contains("<input type=\"checkbox\" disabled checked> Milk"),
            "checked item: {html}"
        );
        assert!(
            html.contains("<input type=\"checkbox\" disabled> Eggs"),
            "unchecked item: {html}"
        );
        // Bold run's text is HTML-escaped inside <strong>.
        assert!(
            html.contains("<strong>&lt;b&gt; &amp; bold</strong>"),
            "escaped bold: {html}"
        );
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
    fn strips_title_and_cleans_text() {
        let body = strip_leading_title("Shopping\nMilk\nEggs", Some("Shopping"));
        assert_eq!(body, "Milk\nEggs");
        // Plain text preserved (newlines kept); attachment markers / CRs dropped.
        assert_eq!(clean_note_text("a < b\nc\u{fffc}\r & d"), "a < b\nc & d");
    }

    fn make_note_store(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("NoteStore.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZICNOTEDATA (Z_PK INTEGER PRIMARY KEY, ZNOTE INTEGER, ZDATA BLOB);
             CREATE TABLE ZICCLOUDSYNCINGOBJECT (
                Z_PK INTEGER PRIMARY KEY, ZTITLE1 TEXT, ZTITLE2 TEXT, ZSNIPPET TEXT,
                ZFOLDER INTEGER, ZNOTEDATA INTEGER, ZISPINNED INTEGER,
                ZCREATIONDATE1 REAL, ZMODIFICATIONDATE1 REAL, ZMARKEDFORDELETION INTEGER,
                ZTYPEUTI1 TEXT, ZNOTE1 INTEGER, ZALTTEXT TEXT);
             -- A folder object (ZTITLE2 set, no note data).
             INSERT INTO ZICCLOUDSYNCINGOBJECT (Z_PK, ZTITLE2) VALUES (1, 'Groceries');
             -- Two hashtag inline-attachment tokens on note 10 (one repeated).
             INSERT INTO ZICCLOUDSYNCINGOBJECT (Z_PK, ZTYPEUTI1, ZNOTE1, ZALTTEXT)
                VALUES (20, 'com.apple.notes.inlinetextattachment.hashtag', 10, '#shopping'),
                       (21, 'com.apple.notes.inlinetextattachment.hashtag', 10, '#errands'),
                       (22, 'com.apple.notes.inlinetextattachment.hashtag', 10, '#shopping');",
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
                (Z_PK, ZTITLE1, ZSNIPPET, ZFOLDER, ZNOTEDATA, ZISPINNED, ZCREATIONDATE1, ZMODIFICATIONDATE1, ZMARKEDFORDELETION)
             VALUES (10, 'Shopping', NULL, 1, 5, 1, 721692800.0, 721692900.0, 0)",
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

        parse_notes(&db, &cache, &mut report, false, None).unwrap();
        assert_eq!(report.notes, 1);

        let c = cache.conn();
        let (folder, title, snippet, body, created, pinned): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ) = c
            .query_row(
                "SELECT folder, title, snippet, body_html, created_at, pinned FROM notes",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(folder.as_deref(), Some("Groceries"));
        assert_eq!(title.as_deref(), Some("Shopping"));
        // Hashtag tags: deduped, in first-seen order.
        let tags: Option<String> = c
            .query_row("SELECT tags FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tags.as_deref(), Some(r##"["#shopping","#errands"]"##));
        // Title stripped from the body; plain text with newlines preserved.
        assert_eq!(body.as_deref(), Some("Milk\nEggs"));
        assert_eq!(snippet.as_deref(), Some("Milk"));
        assert_eq!(created, 1_700_000_000);
        assert_eq!(pinned, 1, "ZISPINNED should carry through to the cache");
    }

    #[test]
    fn parses_and_decrypts_a_password_protected_note() {
        use aes_gcm::aead::Aead;
        use aes_gcm::aes::Aes128;
        use aes_gcm::{aead::consts::U16, AesGcm, KeyInit, Nonce};

        let password = "letmein";
        let salt = b"sixteen-byte-slt";
        let iters = 1000u32;
        // Real locked notes use a 16-byte IV; exercise that GCM path.
        let iv = [3u8; 16];
        // The plaintext body is the same gzip-protobuf an unlocked note stores.
        let body_gz = make_zdata("Secret\ntop secret");
        // A random per-note key encrypts the body; the KEK (from the password)
        // wraps that key (RFC 3394), mirroring Apple's real ladder.
        let note_key = [0x5au8; 16];
        let sealed = AesGcm::<Aes128, U16>::new_from_slice(&note_key)
            .unwrap()
            .encrypt(Nonce::<U16>::from_slice(&iv), body_gz.as_slice())
            .unwrap();
        let (ct, tag) = sealed.split_at(sealed.len() - 16);
        let kek = pbkdf2::pbkdf2_hmac_array::<sha2::Sha256, 16>(password.as_bytes(), salt, iters);
        let mut wrapped = [0u8; 24];
        aes_kw::KekAes128::from(kek)
            .wrap(&note_key, &mut wrapped)
            .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("NoteStore.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZICNOTEDATA (
                Z_PK INTEGER PRIMARY KEY, ZNOTE INTEGER, ZDATA BLOB,
                ZCRYPTOINITIALIZATIONVECTOR BLOB, ZCRYPTOTAG BLOB);
             CREATE TABLE ZICCLOUDSYNCINGOBJECT (
                Z_PK INTEGER PRIMARY KEY, ZTITLE1 TEXT, ZTITLE2 TEXT, ZSNIPPET TEXT,
                ZFOLDER INTEGER, ZNOTEDATA INTEGER, ZCREATIONDATE1 REAL, ZMODIFICATIONDATE1 REAL,
                ZMARKEDFORDELETION INTEGER, ZISPASSWORDPROTECTED INTEGER,
                ZCRYPTOSALT BLOB, ZCRYPTOITERATIONCOUNT INTEGER, ZCRYPTOWRAPPEDKEY BLOB,
                ZPASSWORDHINT TEXT);",
        )
        .unwrap();
        // A locked note: body ciphertext + IV/tag live on the ZICNOTEDATA row;
        // salt/iterations/wrapped-key live on the note object (real layout).
        conn.execute(
            "INSERT INTO ZICNOTEDATA (Z_PK, ZNOTE, ZDATA, ZCRYPTOINITIALIZATIONVECTOR, ZCRYPTOTAG)
             VALUES (1, 10, ?1, ?2, ?3)",
            rusqlite::params![ct, iv.as_slice(), tag],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ZICCLOUDSYNCINGOBJECT
                (Z_PK, ZTITLE1, ZNOTEDATA, ZISPASSWORDPROTECTED, ZCRYPTOSALT,
                 ZCRYPTOITERATIONCOUNT, ZCRYPTOWRAPPEDKEY, ZPASSWORDHINT,
                 ZCREATIONDATE1, ZMODIFICATIONDATE1)
             VALUES (10, 'Locked', 1, 1, ?1, ?2, ?3, 'my hint', 721692800.0, 721692900.0)",
            rusqlite::params![salt.as_slice(), iters, wrapped.as_slice()],
        )
        .unwrap();

        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        parse_notes(&db, &cache, &mut report, false, None).unwrap();
        assert_eq!(report.notes, 1);

        // Stored locked, with the hint, and NO plaintext body at rest.
        let (id, locked, body, hint): (i64, i64, Option<String>, Option<String>) = cache
            .conn()
            .query_row(
                "SELECT id, locked, body_html, password_hint FROM notes",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(locked, 1);
        assert_eq!(body, None);
        assert_eq!(hint.as_deref(), Some("my hint"));

        // The unlock path recovers the body with the right password, and only it.
        let (salt, iter, iv, tag, enc, wrapped) =
            crate::query::note_crypto(&cache, id).unwrap().unwrap();
        let iterations = u32::try_from(iter).unwrap();
        assert_eq!(
            decrypt_locked_note(password, &salt, iterations, &iv, &tag, &enc, &wrapped).as_deref(),
            Some("Secret\ntop secret"),
        );
        assert!(
            decrypt_locked_note("nope", &salt, iterations, &iv, &tag, &enc, &wrapped).is_none()
        );
    }

    #[test]
    fn non_notes_schema_errors_so_caller_can_fall_back() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("random.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE foo (a INTEGER);").unwrap();
        let cache = CacheDb::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        assert!(parse_notes(&db, &cache, &mut report, false, None).is_err());
        assert_eq!(report.notes, 0);
    }
}
