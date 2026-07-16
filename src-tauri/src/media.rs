//! Native media rendering for the gallery (macOS).
//!
//! iLEAPP does not thumbnail or transcode images — its own code notes HEIC is
//! "future work" — and iOS photos are overwhelmingly HEIC, which webviews
//! don't render portably. So we render natively with macOS `sips` (built-in,
//! ImageIO-backed, no extra dependency): downscaled JPEG thumbnails for the
//! grid, and HEIC→JPEG for full view. Results are cached, so each image is
//! converted at most once (architecture §8: thumbnails on demand, cache-once).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Longest edge (px) for a grid thumbnail.
const THUMB_MAX_EDGE: u32 = 512;

/// Monotonic counter for unique per-render temp filenames (so concurrent `sips`
/// renders of the same id write to distinct temps and rename atomically).
static SIPS_SEQ: AtomicU64 = AtomicU64::new(0);

/// Rendered bytes plus the Content-Type to serve them with.
pub struct Rendered {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// A Content-Type value taken from the (untrusted) backup, made safe to put in a
/// response header. A malformed MIME — header-injection bytes, non-ASCII, over-
/// long — would otherwise make the response builder's `.body()` fail (and the
/// old `.unwrap()` panic). Falls back to a generic binary type.
pub fn safe_content_type(mime: Option<&str>) -> String {
    let generic = "application/octet-stream";
    match mime {
        Some(m)
            if !m.is_empty()
                && m.len() <= 128
                && m.bytes().all(|b| b.is_ascii_graphic() || b == b' ') =>
        {
            m.to_string()
        }
        _ => generic.to_string(),
    }
}

/// Whether `filename`'s extension is a raster image `sips` can transcode. Used to
/// recognize an image attachment when its stored MIME is NULL (common for sms.db)
/// or its on-disk path is an extension-less decrypted temp.
pub fn has_image_extension(filename: Option<&str>) -> bool {
    const IMAGE_EXTS: &[&str] = &[
        ".jpg", ".jpeg", ".png", ".gif", ".heic", ".heif", ".webp", ".tiff", ".tif", ".bmp",
    ];
    filename
        .map(|f| f.to_ascii_lowercase())
        .is_some_and(|f| IMAGE_EXTS.iter().any(|ext| f.ends_with(ext)))
}

/// Content-Type for INLINE serving of a non-image attachment. Only audio/video are
/// safe to hand to the webview inline; anything else (text/html, image/svg+xml,
/// application/javascript, …) is forced to a download type so attacker-supplied
/// attachment content can't execute as a document in the custom-scheme origin (the
/// app ships without a strict CSP). Images take the transcode/render path instead.
pub fn inline_media_content_type(mime: Option<&str>) -> String {
    let is_av = mime
        .map(|m| {
            m.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .is_some_and(|m| m.starts_with("audio/") || m.starts_with("video/"));
    if is_av {
        safe_content_type(mime)
    } else {
        "application/octet-stream".to_string()
    }
}

fn is_heic(src: &Path, mime: Option<&str>) -> bool {
    if let Some(m) = mime {
        let m = m.to_ascii_lowercase();
        if m.contains("heic") || m.contains("heif") {
            return true;
        }
    }
    matches!(
        src.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("heic" | "heif")
    )
}

/// Render a media item for serving. `want_thumb` selects a downscaled JPEG
/// thumbnail; otherwise the full image (HEIC transcoded to JPEG, other formats
/// served as-is). `cache_dir` holds the converted files. Returns `None` if the
/// source is unreadable and no conversion could be produced.
pub fn render(
    src: &Path,
    cache_dir: &Path,
    id: i64,
    want_thumb: bool,
    src_mime: Option<&str>,
) -> Option<Rendered> {
    let heic = is_heic(src, src_mime);

    // Non-HEIC full image: serve the original bytes directly.
    if !want_thumb && !heic {
        let bytes = std::fs::read(src).ok()?;
        return Some(Rendered {
            bytes,
            content_type: safe_content_type(src_mime),
        });
    }

    // Otherwise produce (and cache) a JPEG via sips.
    let _ = std::fs::create_dir_all(cache_dir);
    let suffix = if want_thumb { "thumb" } else { "full" };
    let out: PathBuf = cache_dir.join(format!("{id}.{suffix}.jpg"));

    if !out.exists() {
        // Render to a unique temp, then atomically rename into place — so two
        // concurrent requests for the same id (grid + lightbox, strict-mode
        // double-invoke) can't read a half-written JPEG from `out`.
        let seq = SIPS_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = cache_dir.join(format!("{id}.{suffix}.{seq}.partial.jpg"));
        if !run_sips(src, &tmp, want_thumb) {
            let _ = std::fs::remove_file(&tmp);
            // Conversion failed (corrupt file, unknown format): fall back to the
            // original bytes so at least something is served.
            let bytes = std::fs::read(src).ok()?;
            return Some(Rendered {
                bytes,
                content_type: safe_content_type(src_mime),
            });
        }
        // The cached JPEG can be decrypted plaintext of an encrypted photo —
        // restrict it to the owner (sips writes it world-readable by default)
        // BEFORE it's visible at `out`, matching the decrypt-at-rest handling.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        // Atomic on the same filesystem. If a concurrent request won the race, the
        // rename overwrites `out` with byte-identical content (harmless); on any
        // rename error, drop our temp and read whatever `out` the winner produced.
        if std::fs::rename(&tmp, &out).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    let bytes = std::fs::read(&out).ok()?;
    Some(Rendered {
        bytes,
        content_type: "image/jpeg".to_string(),
    })
}

/// Invoke macOS `sips` to write a JPEG copy of `src` at `out`, downscaling to
/// `THUMB_MAX_EDGE` for thumbnails. Returns whether a file was produced.
fn run_sips(src: &Path, out: &Path, thumb: bool) -> bool {
    let mut cmd = Command::new("/usr/bin/sips");
    cmd.arg("-s").arg("format").arg("jpeg");
    if thumb {
        cmd.arg("-Z").arg(THUMB_MAX_EDGE.to_string());
    }
    cmd.arg(src).arg("--out").arg(out);
    matches!(cmd.output(), Ok(o) if o.status.success()) && out.exists()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    // 1x1 PNG, used as a seed to produce a real HEIC via sips.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x08, 0x08, 0x02, 0x00, 0x00, 0x00, 0x4b,
        0x6d, 0x29, 0xdc, 0x00, 0x00, 0x00, 0x16, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x64,
        0x60, 0xf8, 0xcf, 0xc0, 0xc0, 0xc0, 0xc0, 0xf0, 0x9f, 0x01, 0x00, 0x0f, 0xf5, 0x03, 0xfd,
        0x9e, 0x9a, 0x54, 0x8b, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60,
        0x82,
    ];

    fn jpeg_magic(bytes: &[u8]) -> bool {
        bytes.starts_with(&[0xFF, 0xD8, 0xFF])
    }

    #[test]
    fn image_extension_detects_common_types() {
        assert!(has_image_extension(Some("IMG_0001.HEIC")));
        assert!(has_image_extension(Some("photo.jpg")));
        assert!(has_image_extension(Some("a.png")));
        assert!(!has_image_extension(Some("clip.mov")));
        assert!(!has_image_extension(Some("doc.pdf")));
        assert!(!has_image_extension(None));
    }

    #[test]
    fn safe_content_type_rejects_bad_mime() {
        assert_eq!(safe_content_type(Some("image/png")), "image/png");
        assert_eq!(safe_content_type(Some("video/mp4")), "video/mp4");
        // Header injection, empty, non-ASCII, and None all fall back.
        assert_eq!(
            safe_content_type(Some("image/png\r\nX-Evil: 1")),
            "application/octet-stream"
        );
        assert_eq!(safe_content_type(Some("")), "application/octet-stream");
        assert_eq!(
            safe_content_type(Some("tekst/密")),
            "application/octet-stream"
        );
        assert_eq!(safe_content_type(None), "application/octet-stream");
    }

    #[test]
    fn transcodes_heic_to_jpeg_for_full_and_thumb() {
        let tmp = tempfile::tempdir().unwrap();
        let png = tmp.path().join("src.png");
        std::fs::write(&png, TINY_PNG).unwrap();
        let heic = tmp.path().join("photo.heic");
        // Skip gracefully if this box's sips can't write HEIC.
        if !run_sips_heic(&png, &heic) {
            eprintln!("skipping: sips cannot produce HEIC here");
            return;
        }

        let cache = tmp.path().join("thumbs");
        // Full HEIC → JPEG.
        let full = render(&heic, &cache, 1, false, Some("image/heic")).unwrap();
        assert_eq!(full.content_type, "image/jpeg");
        assert!(jpeg_magic(&full.bytes), "full is not JPEG");

        // Thumbnail → JPEG, cached on disk.
        let thumb = render(&heic, &cache, 1, true, Some("image/heic")).unwrap();
        assert_eq!(thumb.content_type, "image/jpeg");
        assert!(jpeg_magic(&thumb.bytes));
        assert!(cache.join("1.thumb.jpg").exists());
    }

    #[test]
    fn serves_png_full_image_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let png = tmp.path().join("a.png");
        std::fs::write(&png, TINY_PNG).unwrap();
        let r = render(
            &png,
            &tmp.path().join("thumbs"),
            2,
            false,
            Some("image/png"),
        )
        .unwrap();
        assert_eq!(r.content_type, "image/png");
        assert_eq!(r.bytes, TINY_PNG); // original bytes, no transcode
    }

    fn run_sips_heic(src: &Path, out: &Path) -> bool {
        matches!(
            Command::new("/usr/bin/sips")
                .args(["-s", "format", "heic"])
                .arg(src)
                .arg("--out")
                .arg(out)
                .output(),
            Ok(o) if o.status.success()
        ) && out.exists()
    }
}
