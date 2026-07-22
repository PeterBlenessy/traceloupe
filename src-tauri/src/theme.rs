//! Theme bridge — exposes the host OS's accent color to the frontend.
//!
//! TraceLoupe's UI follows the macOS accent color set in System Settings
//! (`NSColor.controlAccentColor`). We convert the sRGB color into oklch (the
//! format `src/index.css` uses for every token) so the frontend can drop it
//! into the `--accent-system-value` CSS variable without further math.
//!
//! On Windows / Linux we return `Ok(None)` and the frontend falls back to the
//! default accent baked into `index.css`.

#[cfg(target_os = "macos")]
use objc2::rc::autoreleasepool;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSColor, NSColorSpace};

/// Read the OS-level accent color and return it as an oklch CSS string.
///
/// Returns `Ok(Some("oklch(L% C H)"))` when the host exposes an accent we can
/// read, `Ok(None)` when the host has no readable accent (Windows, Linux), and
/// `Err` for genuine failures (color-space conversion failed, etc.).
#[tauri::command]
pub fn get_system_accent_color() -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        read_macos_accent()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

#[cfg(target_os = "macos")]
fn read_macos_accent() -> Result<Option<String>, String> {
    autoreleasepool(|_| {
        // `NSColor.controlAccentColor` returns a color in a device-dependent
        // color space. Coerce to sRGB so the channel values are well-defined
        // in [0.0, 1.0] before the oklch math.
        let accent = NSColor::controlAccentColor();
        let srgb_space = NSColorSpace::sRGBColorSpace();
        let srgb = accent
            .colorUsingColorSpace(&srgb_space)
            .ok_or_else(|| "Could not convert system accent to sRGB color space".to_string())?;

        let r = srgb.redComponent() as f32;
        let g = srgb.greenComponent() as f32;
        let b = srgb.blueComponent() as f32;

        Ok(Some(format_oklch(srgb_to_oklch(r, g, b))))
    })
}

/// Convert an sRGB triple (each channel in [0.0, 1.0]) to oklch.
///
/// Returns `(L, C, H)` where `L` is in [0.0, 1.0], `C` is typically in
/// [0.0, ~0.4] for in-gamut sRGB, and `H` is in degrees normalized to [0, 360).
///
/// Math follows Björn Ottosson's original Oklab paper (the same reference CSS
/// Color 4 uses for the `oklch()` color function), with a polar wrap to oklch.
// The matrix constants are quoted verbatim from the paper; they exceed f32
// precision but rounding them by hand would only obscure the source.
#[allow(clippy::excessive_precision)]
fn srgb_to_oklch(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    // 1. Gamma decompression: sRGB → linear sRGB
    let r_lin = srgb_to_linear(r);
    let g_lin = srgb_to_linear(g);
    let b_lin = srgb_to_linear(b);

    // 2. Linear sRGB → LMS (Ottosson M1 matrix)
    let l = 0.4122214708 * r_lin + 0.5363325363 * g_lin + 0.0514459929 * b_lin;
    let m = 0.2119034982 * r_lin + 0.6806995451 * g_lin + 0.1073969566 * b_lin;
    let s = 0.0883024619 * r_lin + 0.2817188376 * g_lin + 0.6299787005 * b_lin;

    // 3. Non-linearity: cube root (preserves sign for out-of-gamut input)
    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    // 4. LMS' → Oklab (Ottosson M2 matrix)
    let lab_l = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_;
    let lab_a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_;
    let lab_b = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_;

    // 5. Oklab → oklch (polar)
    let c = (lab_a * lab_a + lab_b * lab_b).sqrt();
    let mut h = lab_b.atan2(lab_a).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }

    // For near-grey colors the hue is numerically meaningless. Snap chroma
    // to zero so the formatter doesn't emit jittery hues for white/black.
    let c = if c < 1e-4 { 0.0 } else { c };
    let h = if c == 0.0 { 0.0 } else { h };

    (lab_l, c, h)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Format an `(L, C, H)` triple as an `oklch()` CSS string.
///
/// L is rendered as a percentage with 1 decimal, C with 3 decimals, H as an
/// integer (degrees).
fn format_oklch((l, c, h): (f32, f32, f32)) -> String {
    let l_pct = (l * 100.0 * 10.0).round() / 10.0;
    let c_round = (c * 1000.0).round() / 1000.0;
    let h_round = h.round() as i32;
    format!("oklch({l_pct}% {c_round} {h_round})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn white_is_pure_lightness() {
        let (l, c, _h) = srgb_to_oklch(1.0, 1.0, 1.0);
        assert!(approx(l, 1.0, 0.001), "white L should be ~1.0, got {l}");
        assert!(approx(c, 0.0, 0.001), "white C should be ~0.0, got {c}");
    }

    #[test]
    fn black_is_pure_darkness() {
        let (l, c, _h) = srgb_to_oklch(0.0, 0.0, 0.0);
        assert!(approx(l, 0.0, 0.001), "black L should be ~0.0, got {l}");
        assert!(approx(c, 0.0, 0.001), "black C should be ~0.0, got {c}");
    }

    #[test]
    fn pure_red_matches_reference() {
        // Reference values from CSS Color 4 / Ottosson's published tables.
        let (l, c, h) = srgb_to_oklch(1.0, 0.0, 0.0);
        assert!(approx(l, 0.628, 0.01), "red L ≈ 0.628, got {l}");
        assert!(approx(c, 0.258, 0.01), "red C ≈ 0.258, got {c}");
        assert!(approx(h, 29.23, 1.0), "red H ≈ 29.23°, got {h}");
    }

    #[test]
    fn pure_blue_matches_reference() {
        let (l, c, h) = srgb_to_oklch(0.0, 0.0, 1.0);
        // sRGB blue: L ≈ 0.452, C ≈ 0.313, H ≈ 264.05°
        assert!(approx(l, 0.452, 0.01), "blue L ≈ 0.452, got {l}");
        assert!(approx(c, 0.313, 0.01), "blue C ≈ 0.313, got {c}");
        assert!(approx(h, 264.05, 1.5), "blue H ≈ 264.05°, got {h}");
    }

    #[test]
    fn macos_default_blue_is_in_blue_hue_range() {
        // macOS default accent is roughly sRGB(0.0, 0.478, 1.0).
        let (l, c, h) = srgb_to_oklch(0.0, 0.478, 1.0);
        assert!(
            l > 0.5 && l < 0.75,
            "macOS blue L should be mid-light, got {l}"
        );
        assert!(c > 0.1, "macOS blue should be chromatic, got C={c}");
        assert!(
            h > 230.0 && h < 270.0,
            "macOS blue hue should land in the blue range, got {h}°"
        );
    }

    #[test]
    fn formatter_emits_expected_shape() {
        let s = format_oklch((0.628, 0.258, 29.23));
        assert_eq!(s, "oklch(62.8% 0.258 29)");
    }

    #[test]
    fn formatter_handles_zero_chroma() {
        let s = format_oklch((1.0, 0.0, 0.0));
        assert_eq!(s, "oklch(100% 0 0)");
    }

    #[test]
    fn hue_is_normalized_to_zero_360() {
        // Magenta-ish: should land in the high-300s, never negative.
        let (_l, _c, h) = srgb_to_oklch(1.0, 0.0, 0.5);
        assert!((0.0..360.0).contains(&h), "hue {h} out of [0, 360)");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn returns_none_on_non_macos() {
        let result = get_system_accent_color().expect("should not error on non-macos");
        assert!(result.is_none(), "non-macos should return Ok(None)");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn returns_oklch_string_on_macos() {
        // Runs against the actual host AppKit accent color: assert shape only.
        let result = get_system_accent_color().expect("should read accent on macOS");
        let s = result.expect("macOS should return Some(_)");
        assert!(
            s.starts_with("oklch("),
            "should start with oklch(, got: {s}"
        );
        assert!(s.ends_with(")"), "should end with ), got: {s}");
        assert!(s.contains("%"), "should contain a percentage, got: {s}");
    }
}
