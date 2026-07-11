//! iLEAPP sidecar runner (architecture §9).
//!
//! Spawns iLEAPP as a separate process against an encrypted backup and streams
//! its progress. iLEAPP decrypts the backup itself (spike finding — see
//! `docs/spike-ileapp.md`), so the invocation is just:
//!
//! ```text
//! ileapp -t itunes -i <backup> -o <out> --itunes_password <pw>
//! ```
//!
//! The password is passed as a CLI argument because that is the only interface
//! iLEAPP exposes for it. To keep it off the global process table where
//! avoidable, callers should prefer running the sidecar with a restricted
//! environment; a future iLEAPP with stdin password support would let us stop
//! passing it in argv at all.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::{Error, Result};

/// Where to find the iLEAPP engine and how to run it.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Path to the iLEAPP executable (our re-frozen binary), or a Python
    /// entrypoint when `python` is set.
    pub program: PathBuf,
    /// If set, `program` is a script run via this interpreter (dev mode:
    /// `python ileapp.py ...`). If `None`, `program` is the frozen binary.
    pub python: Option<PathBuf>,
}

impl EngineConfig {
    /// A frozen-binary config (production: downloaded, pinned iLEAPP).
    pub fn frozen(binary: impl Into<PathBuf>) -> Self {
        Self {
            program: binary.into(),
            python: None,
        }
    }

    /// A run-from-source config (dev/CI: `python /path/to/ileapp.py`).
    pub fn from_source(python: impl Into<PathBuf>, ileapp_py: impl Into<PathBuf>) -> Self {
        Self {
            program: ileapp_py.into(),
            python: Some(python.into()),
        }
    }

    fn command(&self) -> Command {
        match &self.python {
            Some(py) => {
                let mut c = Command::new(py);
                c.arg(&self.program);
                c
            }
            None => Command::new(&self.program),
        }
    }
}

/// Progress update parsed from iLEAPP's stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub current: u32,
    pub total: u32,
    /// The artifact module currently running, e.g. "sms".
    pub artifact: String,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.current as f32 / self.total as f32
        }
    }
}

/// Parse an iLEAPP progress line. iLEAPP emits, for each artifact:
/// `[478/583] sms [sms] artifact started`. Returns `None` for other lines.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let line = line.trim();
    if !line.starts_with('[') {
        return None;
    }
    let close = line.find(']')?;
    let (cur, total) = line[1..close].split_once('/')?;
    let current: u32 = cur.trim().parse().ok()?;
    let total: u32 = total.trim().parse().ok()?;
    // Only report at the "started" edge to avoid double-counting.
    if !line.ends_with("artifact started") {
        return None;
    }
    let rest = line[close + 1..].trim();
    let artifact = rest.split_whitespace().next().unwrap_or("").to_string();
    Some(Progress {
        current,
        total,
        artifact,
    })
}

/// A cooperative cancel flag shared with the caller.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Run iLEAPP against `backup_dir`, writing output under `out_dir`. Calls
/// `on_progress` for each artifact as it starts. Blocks until completion.
///
/// Returns the path to the produced `_lava_artifacts.db`.
pub fn run_import(
    cfg: &EngineConfig,
    backup_dir: &Path,
    password: &str,
    out_dir: &Path,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir).map_err(|e| Error::io(out_dir, e))?;

    let mut cmd = cfg.command();
    cmd.arg("-t")
        .arg("itunes")
        .arg("-i")
        .arg(backup_dir)
        .arg("-o")
        .arg(out_dir)
        .arg("--itunes_password")
        .arg(password)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        Error::EngineNotFound(format!("failed to spawn {}: {e}", cfg.program.display()))
    })?;

    // Stream stdout for progress. iLEAPP prints progress to stdout.
    let stdout = child.stdout.take().expect("piped stdout");
    let reader = BufReader::new(stdout);
    let mut tail: Vec<String> = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| Error::io(out_dir, e))?;
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        if let Some(p) = parse_progress(&line) {
            on_progress(p);
        }
        push_tail(&mut tail, line);
    }

    let status = child.wait().map_err(|e| Error::io(out_dir, e))?;
    if !status.success() {
        return Err(Error::EngineFailed {
            code: status.code().unwrap_or(-1),
            detail: tail.join("\n"),
        });
    }

    find_lava_db(out_dir)
}

/// Keep only the last N log lines, for error reporting.
fn push_tail(tail: &mut Vec<String>, line: String) {
    const MAX: usize = 20;
    tail.push(line);
    if tail.len() > MAX {
        tail.remove(0);
    }
}

/// iLEAPP writes into a timestamped subfolder `iLEAPP_Output_*/`. Find the
/// `_lava_artifacts.db` within `out_dir` (searching one level of subdirs).
pub fn find_lava_db(out_dir: &Path) -> Result<PathBuf> {
    let direct = out_dir.join("_lava_artifacts.db");
    if direct.exists() {
        return Ok(direct);
    }
    let entries = std::fs::read_dir(out_dir).map_err(|e| Error::io(out_dir, e))?;
    for entry in entries.flatten() {
        let candidate = entry.path().join("_lava_artifacts.db");
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(Error::NoEngineOutput {
        path: out_dir.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_started_progress_lines() {
        let p = parse_progress("[478/583] sms [sms] artifact started").unwrap();
        assert_eq!(
            p,
            Progress {
                current: 478,
                total: 583,
                artifact: "sms".into()
            }
        );
        assert!((p.fraction() - 478.0 / 583.0).abs() < 1e-6);
    }

    #[test]
    fn ignores_completed_and_other_lines() {
        assert!(parse_progress("[478/583] sms [sms] artifact completed").is_none());
        assert!(parse_progress("Detected encrypted iTunes backup").is_none());
        assert!(parse_progress("Report generation Completed.").is_none());
        assert!(parse_progress("").is_none());
    }

    #[test]
    fn handles_multiword_artifact_prefix() {
        let p =
            parse_progress("[1/583] get_rctManifest [rctAsyncStorageManifest] artifact started")
                .unwrap();
        assert_eq!(p.artifact, "get_rctManifest");
        assert_eq!((p.current, p.total), (1, 583));
    }

    #[test]
    fn find_lava_db_searches_timestamped_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("iLEAPP_Output_2026-07-11");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("_lava_artifacts.db"), b"x").unwrap();
        assert_eq!(
            find_lava_db(tmp.path()).unwrap(),
            sub.join("_lava_artifacts.db")
        );
    }

    #[test]
    fn find_lava_db_errors_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            find_lava_db(tmp.path()),
            Err(Error::NoEngineOutput { .. })
        ));
    }

    // Exercises the spawn/stream/exit path with a fake "engine" so it needs no
    // real iLEAPP: a shell script that emits progress lines then writes a lava
    // DB. Skipped on Windows (no /bin/sh).
    #[cfg(unix)]
    #[test]
    fn run_import_streams_progress_and_finds_output() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("fake_ileapp.sh");
        {
            let mut f = std::fs::File::create(&script).unwrap();
            // Args: -t itunes -i <backup> -o <out> --itunes_password <pw>
            writeln!(
                f,
                r#"#!/bin/sh
out=""
while [ $# -gt 0 ]; do
  case "$1" in -o) out="$2"; shift 2;; *) shift;; esac
done
echo "Detected encrypted iTunes backup"
echo "[1/2] contacts [contacts] artifact started"
echo "[2/2] sms [sms] artifact started"
sub="$out/iLEAPP_Output_test"
mkdir -p "$sub"
: > "$sub/_lava_artifacts.db"
echo "Report generation Completed."
"#
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let cfg = EngineConfig::frozen(&script);
        let out = tmp.path().join("out");
        let cancel = CancelToken::new();
        let mut seen = Vec::new();
        let lava = run_import(&cfg, tmp.path(), "pw", &out, &cancel, |p| seen.push(p)).unwrap();

        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].artifact, "sms");
        assert!(lava.ends_with("_lava_artifacts.db"));
        assert!(lava.exists());
    }

    #[cfg(unix)]
    #[test]
    fn run_import_reports_engine_failure() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("boom.sh");
        {
            let mut f = std::fs::File::create(&script).unwrap();
            writeln!(f, "#!/bin/sh\necho 'ImportError: broken freeze'\nexit 1").unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cfg = EngineConfig::frozen(&script);
        let err = run_import(
            &cfg,
            tmp.path(),
            "pw",
            &tmp.path().join("out"),
            &CancelToken::new(),
            |_| {},
        )
        .unwrap_err();
        match err {
            Error::EngineFailed { code, detail } => {
                assert_eq!(code, 1);
                assert!(detail.contains("broken freeze"));
            }
            other => panic!("expected EngineFailed, got {other:?}"),
        }
    }
}
