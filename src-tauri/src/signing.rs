//! Detect whether the running app has a stable (non-adhoc) code signature.
//!
//! Keychain items are ACL-bound to the app's signing identity; an adhoc/unsigned
//! dev build gets a fresh identity every rebuild and loses access to them (see
//! docs/signing.md). Both stable Keychain persistence and Touch ID need a real
//! signature, so the UI uses this to decide whether to offer/enable the biometric
//! gate rather than letting the user turn on something that can't work.

use std::process::Command;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningStatus {
    /// Stably signed with a real identity (not adhoc): Keychain items persist
    /// across rebuilds and Touch ID can work.
    pub signed: bool,
    /// Ad-hoc signed — the dev default; Keychain access is lost on each rebuild.
    pub adhoc: bool,
    /// The signing authority (certificate common name), when signed.
    pub identity: Option<String>,
}

impl SigningStatus {
    fn unknown() -> Self {
        SigningStatus {
            signed: false,
            adhoc: false,
            identity: None,
        }
    }
}

/// Inspect the running executable's code signature via `codesign`. `codesign`
/// prints its report to stderr: an `Authority=<cert>` line when signed with a real
/// identity, and `Signature=adhoc` / `flags=…(adhoc…)` for an ad-hoc signature.
pub fn status() -> SigningStatus {
    let Ok(exe) = std::env::current_exe() else {
        return SigningStatus::unknown();
    };
    let Ok(out) = Command::new("/usr/bin/codesign")
        .arg("-dvvv")
        .arg(&exe)
        .output()
    else {
        return SigningStatus::unknown();
    };
    let text = String::from_utf8_lossy(&out.stderr);

    let adhoc = text.contains("Signature=adhoc") || text.contains("(adhoc");
    let identity = text
        .lines()
        .find_map(|l| l.strip_prefix("Authority="))
        .map(|s| s.trim().to_string());
    SigningStatus {
        // A real, stable signature: a named authority and not ad-hoc.
        signed: identity.is_some() && !adhoc,
        adhoc,
        identity,
    }
}
