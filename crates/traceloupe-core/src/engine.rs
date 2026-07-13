//! Locate the iLEAPP engine (architecture §9).
//!
//! Resolution order, most specific first:
//! 1. A dev "run from source" override (`python` + `ileapp.py`) — used to test
//!    against a live iLEAPP checkout.
//! 2. An explicit frozen-binary override (power-user setting / env).
//! 3. The frozen binary we downloaded on first use, under the app's data dir.
//!
//! If none resolve, the engine is not installed and the UI offers to fetch it.
//! This function is pure (no env/FS-global access) so it unit-tests cleanly;
//! the shell layer reads env vars and app paths and passes them in.

use std::path::{Path, PathBuf};

use crate::sidecar::EngineConfig;

/// A "run from source" override: (python interpreter, path to `ileapp.py`).
pub type SourceOverride = (PathBuf, PathBuf);

/// Resolve which iLEAPP to run, or `None` if it must still be installed.
pub fn resolve_engine(
    source_override: Option<SourceOverride>,
    binary_override: Option<PathBuf>,
    installed_binary: &Path,
) -> Option<EngineConfig> {
    if let Some((python, ileapp_py)) = source_override {
        if python.exists() && ileapp_py.exists() {
            return Some(EngineConfig::from_source(python, ileapp_py));
        }
    }
    if let Some(binary) = binary_override {
        if binary.exists() {
            return Some(EngineConfig::frozen(binary));
        }
    }
    if installed_binary.exists() {
        return Some(EngineConfig::frozen(installed_binary));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_source_override_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let py = tmp.path().join("python");
        let src = tmp.path().join("ileapp.py");
        std::fs::write(&py, "").unwrap();
        std::fs::write(&src, "").unwrap();

        let cfg =
            resolve_engine(Some((py.clone(), src.clone())), None, Path::new("/nope")).unwrap();
        assert_eq!(cfg.python.as_deref(), Some(py.as_path()));
        assert_eq!(cfg.program, src);
    }

    #[test]
    fn falls_back_to_installed_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let installed = tmp.path().join("ileapp");
        std::fs::write(&installed, "").unwrap();

        let cfg = resolve_engine(None, None, &installed).unwrap();
        assert_eq!(cfg.program, installed);
        assert!(cfg.python.is_none());
    }

    #[test]
    fn none_when_nothing_available() {
        // Overrides pointing at missing paths are ignored, not errors.
        let missing = PathBuf::from("/does/not/exist");
        assert!(resolve_engine(
            Some((missing.clone(), missing.clone())),
            Some(missing.clone()),
            &missing,
        )
        .is_none());
    }
}
