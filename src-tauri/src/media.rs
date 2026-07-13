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

/// Longest edge (px) for a grid thumbnail.
const THUMB_MAX_EDGE: u32 = 512;

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

    if !out.exists() && !run_sips(src, &out, want_thumb) {
        // Conversion failed (corrupt file, unknown format): fall back to the
        // original bytes so at least something is served.
        let bytes = std::fs::read(src).ok()?;
        return Some(Rendered {
            bytes,
            content_type: safe_content_type(src_mime),
        });
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
