//! Backup discovery: enumerate Finder/MobileSync (or user-chosen) backup
//! directories and read their metadata from `Info.plist` / `Manifest.plist`.
//!
//! Discovery only reads the two small metadata plists — never `Manifest.db`
//! or file blobs — so it is fast and safe to run on every app launch.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{Error, Result};

/// Metadata for one backup directory. All fields are best-effort: a backup
/// with a missing or unreadable plist still appears, with `None` fields,
/// rather than being hidden from the user.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    /// Directory name — the device UDID for Finder-created backups.
    pub id: String,
    pub path: PathBuf,
    pub device_name: Option<String>,
    /// e.g. "iPhone12,3"
    pub product_type: Option<String>,
    /// iOS version, e.g. "17.5.1"
    pub product_version: Option<String>,
    pub serial_number: Option<String>,
    /// Unix epoch seconds.
    pub last_backup_date: Option<i64>,
    pub is_encrypted: Option<bool>,
}

/// Finder's default backup location for the current user.
pub fn default_backup_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library/Application Support/MobileSync/Backup"))
}

/// Enumerate backups under `root`. Returns `Error::PermissionDenied` when the
/// directory exists but is unreadable (the missing-Full-Disk-Access case) and
/// `Error::BackupDirNotFound` when it does not exist.
pub fn discover_backups(root: &Path) -> Result<Vec<BackupInfo>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::BackupDirNotFound { path: root.into() })
        }
        Err(e) => return Err(Error::io(root, e)),
    };

    let mut backups = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(root, e))?;
        let path = entry.path();
        if !path.is_dir() || !looks_like_backup(&path) {
            continue;
        }
        backups.push(read_backup_info(&path));
    }
    backups.sort_by_key(|b| std::cmp::Reverse(b.last_backup_date));
    Ok(backups)
}

/// A directory is treated as a backup if it carries any of the files every
/// iOS backup contains. Loose on purpose: partially copied backups should
/// still be listed so the user can see what they have.
pub fn looks_like_backup(dir: &Path) -> bool {
    ["Manifest.db", "Manifest.plist", "Info.plist"]
        .iter()
        .any(|f| dir.join(f).exists())
}

/// Discover backups the user may have meant by picking `path`: first the
/// backups *inside* it (the MobileSync/Backup root case), and if there are
/// none but `path` is itself a backup directory, that single backup. This
/// lets a folder picker accept either the backups root or one backup.
pub fn discover_at(path: &Path) -> Result<Vec<BackupInfo>> {
    let backups = discover_backups(path)?;
    if backups.is_empty() && looks_like_backup(path) {
        return Ok(vec![read_backup_info(path)]);
    }
    Ok(backups)
}

/// Read metadata for a single backup directory. Never fails: unreadable or
/// missing plists degrade to `None` fields (error isolation, architecture §12).
pub fn read_backup_info(dir: &Path) -> BackupInfo {
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let info = plist::Value::from_file(dir.join("Info.plist")).ok();
    let manifest = plist::Value::from_file(dir.join("Manifest.plist")).ok();

    let info_dict = info.as_ref().and_then(|v| v.as_dictionary());
    let manifest_dict = manifest.as_ref().and_then(|v| v.as_dictionary());
    // Manifest.plist carries a Lockdown dict with the same device fields as
    // Info.plist; used as fallback when Info.plist is absent.
    let lockdown = manifest_dict
        .and_then(|d| d.get("Lockdown"))
        .and_then(|v| v.as_dictionary());

    let get_string = |key_info: &str, key_lockdown: &str| -> Option<String> {
        info_dict
            .and_then(|d| d.get(key_info))
            .or_else(|| lockdown.and_then(|d| d.get(key_lockdown)))
            .and_then(|v| v.as_string())
            .map(str::to_owned)
    };

    let last_backup_date = info_dict
        .and_then(|d| d.get("Last Backup Date"))
        .or_else(|| manifest_dict.and_then(|d| d.get("Date")))
        .and_then(|v| v.as_date())
        // Convert via a Unix-epoch duration, not `OffsetDateTime::from` — a crafted
        // out-of-range plist date would panic that conversion (this fn is on the
        // discovery/launch path). Degrade to None instead of crashing.
        .and_then(|d| {
            std::time::SystemTime::from(d)
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|dur| dur.as_secs() as i64)
        });

    BackupInfo {
        id,
        path: dir.to_path_buf(),
        device_name: get_string("Device Name", "DeviceName"),
        product_type: get_string("Product Type", "ProductType"),
        product_version: get_string("Product Version", "ProductVersion"),
        serial_number: get_string("Serial Number", "SerialNumber"),
        last_backup_date,
        is_encrypted: manifest_dict
            .and_then(|d| d.get("IsEncrypted"))
            .and_then(|v| v.as_boolean()),
    }
}

/// The apps that were installed on the device, as bundle IDs (e.g.
/// `net.whatsapp.WhatsApp`), read from `Info.plist`'s "Installed Applications".
/// Sorted; empty if the key is absent or unreadable. Cheap — no decryption.
pub fn installed_apps(dir: &Path) -> Vec<String> {
    let Ok(info) = plist::Value::from_file(dir.join("Info.plist")) else {
        return Vec::new();
    };
    let mut apps: Vec<String> = info
        .as_dictionary()
        .and_then(|d| d.get("Installed Applications"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_string().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    apps.sort();
    apps
}

/// An installed app with the App Store metadata the backup carries for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppInfo {
    pub bundle_id: String,
    /// App Store display name (`itemName`), e.g. "Instagram".
    pub name: Option<String>,
    /// Seller / developer (`artistName`), e.g. "Instagram, Inc.".
    pub seller: Option<String>,
    /// Marketing version installed (`bundleShortVersionString`), e.g. "436.0.0".
    pub version: Option<String>,
    /// App Store category (`genre`), e.g. "Photo & Video".
    pub genre: Option<String>,
    /// The app's App Store release date, an RFC-3339 string (`releaseDate`).
    /// This is the app's original release, not the install/update date (which
    /// the backup doesn't expose) — labelled accordingly in the UI.
    pub released: Option<String>,
}

/// Installed apps with their per-app App Store metadata, read from
/// `Info.plist`'s "Applications" dict (each value carries an `iTunesMetadata`
/// **nested binary plist**). Falls back to bundle-id-only entries for apps
/// whose metadata is absent/unreadable. Sorted by bundle id; cheap, no
/// decryption. Info.plist itself is never encrypted.
pub fn installed_apps_meta(dir: &Path) -> Vec<AppInfo> {
    use std::collections::BTreeSet;

    let Ok(info) = plist::Value::from_file(dir.join("Info.plist")) else {
        return Vec::new();
    };
    let dict = info.as_dictionary();
    // `Applications` holds per-app App Store metadata, but only for apps with a
    // store receipt — it's a SUBSET. `Installed Applications` is the complete
    // list of bundle ids (system + sideloaded + metadata-less apps included).
    // Take the union so no installed app is dropped, and enrich from the
    // metadata dict where present.
    let apps_meta = dict
        .and_then(|d| d.get("Applications"))
        .and_then(|v| v.as_dictionary());
    let mut ids: BTreeSet<String> = dict
        .and_then(|d| d.get("Installed Applications"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_string().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if let Some(meta) = apps_meta {
        ids.extend(meta.keys().cloned());
    }

    let str_field = |md: &plist::Dictionary, key: &str| -> Option<String> {
        md.get(key)
            .and_then(|v| v.as_string())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };

    // BTreeSet already yields bundle ids sorted + de-duplicated.
    ids.into_iter()
        .map(|bundle_id| {
            let mut app = AppInfo {
                bundle_id: bundle_id.clone(),
                ..Default::default()
            };
            // iTunesMetadata is a Data blob holding a nested binary plist.
            let md = apps_meta
                .and_then(|m| m.get(&bundle_id))
                .and_then(|entry| entry.as_dictionary())
                .and_then(|d| d.get("iTunesMetadata"))
                .and_then(|v| v.as_data())
                .and_then(|bytes| plist::Value::from_reader(std::io::Cursor::new(bytes)).ok())
                .and_then(|v| v.into_dictionary());
            if let Some(md) = md {
                app.name = str_field(&md, "itemName");
                app.seller = str_field(&md, "artistName");
                app.version = str_field(&md, "bundleShortVersionString");
                app.genre = str_field(&md, "genre");
                app.released = str_field(&md, "releaseDate");
            }
            app
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::{Dictionary, Value};

    fn write_plist(path: &Path, value: Value) {
        value.to_file_binary(path).unwrap();
    }

    fn make_backup(root: &Path, id: &str, name: &str, encrypted: bool) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();

        let mut info = Dictionary::new();
        info.insert("Device Name".into(), Value::String(name.into()));
        info.insert("Product Type".into(), Value::String("iPhone12,3".into()));
        info.insert("Product Version".into(), Value::String("17.5.1".into()));
        info.insert("Serial Number".into(), Value::String("F2LXXXXXXXXX".into()));
        info.insert(
            "Last Backup Date".into(),
            Value::Date(std::time::SystemTime::UNIX_EPOCH.into()),
        );
        write_plist(&dir.join("Info.plist"), Value::Dictionary(info));

        let mut manifest = Dictionary::new();
        manifest.insert("IsEncrypted".into(), Value::Boolean(encrypted));
        write_plist(&dir.join("Manifest.plist"), Value::Dictionary(manifest));

        dir
    }

    #[test]
    fn discovers_and_parses_backups() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(
            tmp.path(),
            "00008030-000A1B2C3D4E5F",
            "Peter's iPhone",
            true,
        );
        // Non-backup noise must be ignored.
        std::fs::create_dir(tmp.path().join("not-a-backup")).unwrap();
        std::fs::write(tmp.path().join(".DS_Store"), b"").unwrap();

        let backups = discover_backups(tmp.path()).unwrap();
        assert_eq!(backups.len(), 1);
        let b = &backups[0];
        assert_eq!(b.id, "00008030-000A1B2C3D4E5F");
        assert_eq!(b.device_name.as_deref(), Some("Peter's iPhone"));
        assert_eq!(b.product_version.as_deref(), Some("17.5.1"));
        assert_eq!(b.is_encrypted, Some(true));
        assert_eq!(b.last_backup_date, Some(0));
    }

    #[test]
    fn lists_installed_apps_from_info_plist() {
        let tmp = tempfile::tempdir().unwrap();
        let mut info = Dictionary::new();
        info.insert(
            "Installed Applications".into(),
            Value::Array(vec![
                Value::String("net.whatsapp.WhatsApp".into()),
                Value::String("com.burbn.instagram".into()),
                Value::String("com.apple.mobilesafari".into()),
            ]),
        );
        write_plist(&tmp.path().join("Info.plist"), Value::Dictionary(info));

        let apps = installed_apps(tmp.path());
        assert_eq!(
            apps,
            vec![
                "com.apple.mobilesafari",
                "com.burbn.instagram",
                "net.whatsapp.WhatsApp",
            ]
        );
        // Missing key / no plist → empty, not an error.
        assert!(installed_apps(&tmp.path().join("nope")).is_empty());
    }

    #[test]
    fn missing_root_is_a_distinct_error() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        assert!(matches!(
            discover_backups(&missing),
            Err(Error::BackupDirNotFound { .. })
        ));
    }

    #[test]
    fn discover_at_accepts_root_or_single_backup() {
        let tmp = tempfile::tempdir().unwrap();
        // A backups root with one backup inside.
        make_backup(tmp.path(), "00008030-ROOT", "Root Phone", true);
        let via_root = discover_at(tmp.path()).unwrap();
        assert_eq!(via_root.len(), 1);
        assert_eq!(via_root[0].device_name.as_deref(), Some("Root Phone"));

        // Pointing directly at the single backup dir also resolves it.
        let backup_dir = tmp.path().join("00008030-ROOT");
        let via_single = discover_at(&backup_dir).unwrap();
        assert_eq!(via_single.len(), 1);
        assert_eq!(via_single[0].device_name.as_deref(), Some("Root Phone"));

        // A plain folder that is neither yields nothing (not an error).
        let empty = tmp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(discover_at(&empty).unwrap().is_empty());
    }

    #[test]
    fn unreadable_plists_degrade_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("deadbeef");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("Manifest.db"), b"not really sqlite").unwrap();
        std::fs::write(dir.join("Info.plist"), b"garbage").unwrap();

        let backups = discover_backups(tmp.path()).unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].device_name, None);
        assert_eq!(backups[0].is_encrypted, None);
    }

    #[test]
    fn newest_backup_sorts_first() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "aaa", "Old", false);
        let newer = tmp.path().join("bbb");
        std::fs::create_dir(&newer).unwrap();
        let mut info = Dictionary::new();
        info.insert(
            "Last Backup Date".into(),
            Value::Date(
                (std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000))
                    .into(),
            ),
        );
        write_plist(&newer.join("Info.plist"), Value::Dictionary(info));

        let backups = discover_backups(tmp.path()).unwrap();
        assert_eq!(backups[0].id, "bbb");
    }

    #[test]
    fn reads_app_metadata_from_itunes_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // A nested iTunesMetadata bplist (serialized to bytes, stored as Data).
        let mut md = Dictionary::new();
        md.insert("itemName".into(), Value::String("Instagram".into()));
        md.insert("artistName".into(), Value::String("Instagram, Inc.".into()));
        md.insert(
            "bundleShortVersionString".into(),
            Value::String("436.0.0".into()),
        );
        md.insert("genre".into(), Value::String("Photo & Video".into()));
        md.insert(
            "releaseDate".into(),
            Value::String("2010-10-06T08:12:41Z".into()),
        );
        let mut md_bytes: Vec<u8> = Vec::new();
        Value::Dictionary(md)
            .to_writer_binary(&mut md_bytes)
            .unwrap();

        // Only Instagram has metadata (a receipt). Safari is a system app that
        // appears in "Installed Applications" but NOT in "Applications".
        let mut app_entry = Dictionary::new();
        app_entry.insert("iTunesMetadata".into(), Value::Data(md_bytes));
        let mut apps = Dictionary::new();
        apps.insert("com.burbn.instagram".into(), Value::Dictionary(app_entry));

        let mut info = Dictionary::new();
        info.insert("Applications".into(), Value::Dictionary(apps));
        info.insert(
            "Installed Applications".into(),
            Value::Array(vec![
                Value::String("com.burbn.instagram".into()),
                Value::String("com.apple.mobilesafari".into()),
            ]),
        );
        write_plist(&dir.join("Info.plist"), Value::Dictionary(info));

        let out = installed_apps_meta(dir);
        assert_eq!(out.len(), 2, "the metadata-less system app must NOT be dropped");
        // Sorted by bundle id: mobilesafari, then instagram.
        let ig = out.iter().find(|a| a.bundle_id == "com.burbn.instagram").unwrap();
        assert_eq!(ig.name.as_deref(), Some("Instagram"));
        assert_eq!(ig.seller.as_deref(), Some("Instagram, Inc."));
        assert_eq!(ig.version.as_deref(), Some("436.0.0"));
        assert_eq!(ig.genre.as_deref(), Some("Photo & Video"));
        assert_eq!(ig.released.as_deref(), Some("2010-10-06T08:12:41Z"));
        // The system app (in the array, absent from Applications) still appears,
        // with only its bundle id.
        let safari = out.iter().find(|a| a.bundle_id == "com.apple.mobilesafari").unwrap();
        assert_eq!(safari.name, None);
    }
}
