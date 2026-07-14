//! Optional Touch ID gate for releasing an encrypted backup's keys.
//!
//! The backup password lives in the Keychain ([`crate::secret`]); when the user
//! turns on biometric unlock, reconstructing the decryptor first requires them to
//! authenticate (Touch ID, or the device passcode as fallback). It's **off by
//! default** and the gate is a no-op when disabled, so nothing changes unless the
//! user opts in. This wraps `LocalAuthentication` via `robius-authentication`
//! rather than hand-rolling `LAContext` FFI.
//!
//! Note: biometric-protected *Keychain* items (OS-enforced) are a later hardening
//! — this is an app-level gate in front of the existing Keychain read, which
//! still needs a properly signed build to work (see docs/signing.md).

use std::sync::atomic::{AtomicBool, Ordering};

use robius_authentication::{
    AndroidText, BiometricStrength, Context, PolicyBuilder, Text, WindowsText,
};

/// Whether the user enabled "require Touch ID". Set from the frontend setting via
/// the `set_biometric_required` command; read by the key-load paths.
static REQUIRED: AtomicBool = AtomicBool::new(false);

pub fn set_required(on: bool) {
    REQUIRED.store(on, Ordering::Relaxed);
}

pub fn is_required() -> bool {
    REQUIRED.load(Ordering::Relaxed)
}

/// Prompt the user to authenticate (the OS shows the Touch ID sheet). `reason` is
/// the shown prompt text. Blocks until the user responds. `Ok(())` on success
/// (biometric or the passcode fallback); `Err` on cancel/failure/unavailability.
pub fn authenticate(reason: &str) -> Result<(), String> {
    let policy = PolicyBuilder::new()
        .biometrics(Some(BiometricStrength::Strong))
        .password(true)
        .build()
        .ok_or_else(|| "auth policy unavailable".to_string())?;

    let text = Text {
        // Only `apple` is used on macOS; the others satisfy the cross-platform
        // struct (short, fixed strings so WindowsText::new never returns None).
        android: AndroidText {
            title: "Unlock backup",
            subtitle: None,
            description: None,
        },
        apple: reason,
        windows: WindowsText::new("Unlock backup", "Unlock backup")
            .expect("valid windows prompt text"),
    };

    Context::new(())
        .blocking_authenticate(text, &policy)
        .map_err(|e| format!("{e:?}"))
}

/// The gate used by the key-load paths: prompt only when the user turned biometric
/// unlock on. Returns `Ok(())` to proceed, `Err` to deny key access.
pub fn gate(reason: &str) -> Result<(), String> {
    if is_required() {
        authenticate(reason)
    } else {
        Ok(())
    }
}
