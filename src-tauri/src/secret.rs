//! Secure storage for encrypted-backup passwords, via the macOS Keychain.
//!
//! An encrypted backup's password is needed again after import — to reopen the
//! backup in a later session and to decrypt full-resolution photos on demand
//! (the grid uses thumbnails decrypted at import, but originals stay encrypted).
//! Rather than hold it in plaintext or re-prompt, we stash it in the Keychain,
//! keyed by backup id, and reconstruct the decryptor from it when needed.

use keyring::Entry;

/// Keychain service name; entries are per-backup under this service.
const SERVICE: &str = "se.addable.traceloupe.backup-password";

fn entry(backup_id: &str) -> Option<Entry> {
    Entry::new(SERVICE, backup_id).ok()
}

/// Store (or replace) the password for `backup_id`.
pub fn store(backup_id: &str, password: &str) -> Result<(), String> {
    entry(backup_id)
        .ok_or_else(|| "keychain unavailable".to_string())?
        .set_password(password)
        .map_err(|e| e.to_string())
}

/// Retrieve the password for `backup_id`, if one was stored. A missing entry is a
/// quiet `None`; a genuine Keychain failure (access denied — e.g. an unsigned dev
/// build whose signature the item's ACL doesn't recognize) is logged to stderr so
/// the "keys silently didn't load" case is diagnosable.
pub fn get(backup_id: &str) -> Option<String> {
    match entry(backup_id)?.get_password() {
        Ok(p) => Some(p),
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            eprintln!("keychain read failed for backup {backup_id}: {e}");
            None
        }
    }
}

/// Remove any stored password for `backup_id` (best effort).
pub fn delete(backup_id: &str) {
    if let Some(e) = entry(backup_id) {
        let _ = e.delete_credential();
    }
}
