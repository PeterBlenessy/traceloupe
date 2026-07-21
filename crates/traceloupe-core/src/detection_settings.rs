//! Persisted Security Check settings (see CONTEXT.md decisions log).
//!
//! These are app preferences, not cache data: they must survive a re-import,
//! so they live in a small JSON file under the app data dir rather than in the
//! cache DB. Consent state is recorded here too, so the first-run and
//! first-fetch dialogs are shown exactly once.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Which analyzer modules the Passive Check runs. Default apps-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PassiveScope {
    /// Bundle-id + manifest AppDomain matching only (near-zero false positives).
    #[default]
    AppsOnly,
    /// Everything an Explicit Scan runs (message/URL/domain content included).
    Full,
}

/// A yes/no/unasked consent tri-state so first-run dialogs fire exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Consent {
    #[default]
    Unasked,
    Granted,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DetectionSettings {
    /// Master switch for the Passive Check running inside import.
    pub passive_enabled: bool,
    pub passive_scope: PassiveScope,
    /// Consent for the Passive Check (asked at first launch after the feature
    /// ships). `passive_enabled` is only honored once this is `Granted`.
    pub passive_consent: Consent,
    /// Fetch fresh indicator feeds at the start of an Explicit Scan.
    pub auto_update_indicators: bool,
    /// Consent for network fetches of indicator feeds (asked before the first
    /// fetch). Fetching only happens once this is `Granted`.
    pub fetch_consent: Consent,
    /// Optional local folder of custom indicator files (`.stix`/`.stix2`/`.yaml`)
    /// merged into the scan alongside the bundled feeds (researcher mode).
    ///
    /// (Shortened-URL expansion is deliberately NOT a setting here: it is a
    /// per-link, per-use action with a per-backup opt-out stored in the backup's
    /// own cache, so it never lives in these global settings.)
    pub custom_indicator_dir: Option<String>,
}

impl Default for DetectionSettings {
    fn default() -> Self {
        DetectionSettings {
            passive_enabled: true,
            passive_scope: PassiveScope::default(),
            passive_consent: Consent::Unasked,
            auto_update_indicators: true,
            fetch_consent: Consent::Unasked,
            custom_indicator_dir: None,
        }
    }
}

impl DetectionSettings {
    fn file(app_data: &Path) -> PathBuf {
        app_data.join("detection-settings.json")
    }

    /// Load settings, returning defaults if the file is absent. A corrupt file
    /// is an error the caller can surface rather than silently resetting.
    pub fn load(app_data: &Path) -> Result<Self> {
        let path = Self::file(app_data);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| Error::Parse(format!("detection-settings.json: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::io(&path, e)),
        }
    }

    pub fn save(&self, app_data: &Path) -> Result<()> {
        std::fs::create_dir_all(app_data).map_err(|e| Error::io(app_data, e))?;
        let path = Self::file(app_data);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())
            .map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| Error::io(&path, e))
    }

    /// Whether the Passive Check should run during import right now.
    pub fn passive_active(&self) -> bool {
        self.passive_enabled && self.passive_consent == Consent::Granted
    }

    /// Whether an Explicit Scan may fetch fresh feeds right now.
    pub fn may_fetch(&self) -> bool {
        self.auto_update_indicators && self.fetch_consent == Consent::Granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_conservative_until_consent() {
        let s = DetectionSettings::default();
        // Enabled by preference, but gated on consent.
        assert!(s.passive_enabled);
        assert!(!s.passive_active());
        assert!(s.auto_update_indicators);
        assert!(!s.may_fetch());
        assert_eq!(s.passive_scope, PassiveScope::AppsOnly);
    }

    #[test]
    fn consent_unlocks_behavior() {
        let mut s = DetectionSettings::default();
        s.passive_consent = Consent::Granted;
        s.fetch_consent = Consent::Granted;
        assert!(s.passive_active());
        assert!(s.may_fetch());
        // Master switches still win.
        s.passive_enabled = false;
        s.auto_update_indicators = false;
        assert!(!s.passive_active());
        assert!(!s.may_fetch());
    }

    #[test]
    fn round_trips_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = DetectionSettings::default();
        s.passive_scope = PassiveScope::Full;
        s.passive_consent = Consent::Denied;
        s.save(tmp.path()).unwrap();
        let loaded = DetectionSettings::load(tmp.path()).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn missing_file_is_defaults_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            DetectionSettings::load(tmp.path()).unwrap(),
            DetectionSettings::default()
        );
    }

    #[test]
    fn unknown_fields_and_partial_files_use_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("detection-settings.json"),
            r#"{"passiveScope":"full","future_field":true}"#,
        )
        .unwrap();
        let s = DetectionSettings::load(tmp.path()).unwrap();
        assert_eq!(s.passive_scope, PassiveScope::Full);
        // Unspecified fields fall back to defaults.
        assert!(s.auto_update_indicators);
    }
}
