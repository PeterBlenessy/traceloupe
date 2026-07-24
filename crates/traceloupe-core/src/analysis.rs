//! Analysis store: the per-backup sidecar SQLite DB (`analysis.db`) for Safety
//! Scan (docs/CONTEXT.md §Safety Scan, ADR 0002, docs/plans/safety-scan-plan.md T3).
//!
//! Lives beside the parse cache (`caches/<backup_id>/analysis.db`) but has a
//! deliberately separate lifecycle: re-import atomically replaces `cache.db`,
//! while Content Findings here represent hours of local LLM compute and must
//! survive. Rows therefore carry *stable references* (thread identifier +
//! timestamp + text fingerprint), not just cache row ids; cache ids are cached
//! for cheap joins and re-resolved (or the row marked stale) after re-import.
//!
//! Nothing in this DB may contain raw message/note text except the model's
//! one-line `rationale` and summary texts; the audit log is content-free by
//! construction (identifier ranges, models, counts — never text).
//!
//! Timestamps are Unix epoch seconds (INTEGER). Migrations are tracked with
//! `PRAGMA user_version`, mirroring `cache.rs`.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::{Error, Result};

pub struct AnalysisDb {
    conn: Connection,
}

const SCHEMA_VERSION: i64 = 2;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- One row per Safety Scan run.
CREATE TABLE IF NOT EXISTS scans (
    id           INTEGER PRIMARY KEY,
    model        TEXT NOT NULL,             -- e.g. 'gemma-4-E4B-it-Q4_K_M'
    range_start  INTEGER,                   -- user time-range filter (unix s), NULL = open
    range_end    INTEGER,
    sources      TEXT NOT NULL DEFAULT 'all', -- 'all' | 'messages' | 'notes' (v2)
    status       TEXT NOT NULL,             -- 'running' | 'completed' | 'cancelled' | 'failed'
                                            -- | 'interrupted' (stranded 'running' repaired at open)
    started_at   INTEGER NOT NULL,
    finished_at  INTEGER,
    chunks_total INTEGER NOT NULL DEFAULT 0,
    chunks_done  INTEGER NOT NULL DEFAULT 0
);

-- A Content Finding: one model verdict attached to one message or note.
-- `source_id` is the *current* cache row id (fast joins, refreshed after
-- re-import); identity across re-imports is (source_kind, thread_identifier,
-- occurred_at, fingerprint).
CREATE TABLE IF NOT EXISTS content_findings (
    id                INTEGER PRIMARY KEY,
    scan_id           INTEGER NOT NULL REFERENCES scans(id),
    source_kind       TEXT NOT NULL,        -- 'message' | 'note'
    source_id         INTEGER,              -- cache row id; NULL/stale after re-import until re-resolved
    thread_identifier TEXT,                 -- threads.identifier (messages) — stable across imports
    occurred_at       INTEGER,              -- messages.sent_at / notes.modified_at
    fingerprint       TEXT NOT NULL,        -- sha256 hex of the normalized source text
    category          TEXT NOT NULL,        -- Forensic 9 slug (see Category)
    severity          INTEGER NOT NULL CHECK (severity BETWEEN 1 AND 3),
    rationale         TEXT NOT NULL,        -- the model's one-line justification
    stale             INTEGER NOT NULL DEFAULT 0,  -- fingerprint no longer matches the cache row
    created_at        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_findings_scan ON content_findings(scan_id);
CREATE INDEX IF NOT EXISTS idx_findings_source ON content_findings(source_kind, fingerprint);
CREATE INDEX IF NOT EXISTS idx_findings_category ON content_findings(category, severity DESC);

-- False-positive dismissals. Keyed by (fingerprint, category) — NOT finding row
-- id — so a dismissal survives re-scans and re-imports (plan T8 AC).
CREATE TABLE IF NOT EXISTS dismissals (
    fingerprint  TEXT NOT NULL,
    category     TEXT NOT NULL,
    dismissed_at INTEGER NOT NULL,
    PRIMARY KEY (fingerprint, category)
);

-- Per-Chunk classification progress. One row per chunk_key (latest state);
-- resume skips chunks whose status is 'done' with an unchanged fingerprint,
-- which also gives incremental re-scan for free.
CREATE TABLE IF NOT EXISTS chunk_progress (
    chunk_key     TEXT PRIMARY KEY,          -- stable chunker-assigned key
    fingerprint   TEXT NOT NULL,             -- sha256 of the chunk's normalized text
    scan_id       INTEGER NOT NULL REFERENCES scans(id),
    status        TEXT NOT NULL,             -- 'done' | 'skipped'
    classified_at INTEGER NOT NULL
);

-- Scan report + per-flagged-thread summaries (plan T6).
CREATE TABLE IF NOT EXISTS summaries (
    scan_id    INTEGER NOT NULL REFERENCES scans(id),
    kind       TEXT NOT NULL,                -- 'report' | 'thread'
    thread_ref TEXT NOT NULL DEFAULT '',     -- threads.identifier for kind='thread'
    content    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (scan_id, kind, thread_ref)
);

-- Content-free audit log: what a scan did, never what the text said.
CREATE TABLE IF NOT EXISTS audit_log (
    id      INTEGER PRIMARY KEY,
    scan_id INTEGER NOT NULL REFERENCES scans(id),
    at      INTEGER NOT NULL,
    event   TEXT NOT NULL,                   -- 'scan_started' | 'chunk_classified' | ...
    detail  TEXT NOT NULL DEFAULT ''         -- ranges/counts/model — free of source text
);
CREATE INDEX IF NOT EXISTS idx_audit_scan ON audit_log(scan_id, at);
"#;

/// The Forensic 9 taxonomy (docs/CONTEXT.md). Slugs are the wire/storage format.
/// `Ord` follows declaration order, used only to key eval score maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    ThreatViolence,
    HarassmentBullying,
    SexualContent,
    GroomingExploitation,
    SelfHarm,
    HateIdentity,
    CoerciveControl,
    ScamFraud,
    DrugsIllegal,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::ThreatViolence,
        Category::HarassmentBullying,
        Category::SexualContent,
        Category::GroomingExploitation,
        Category::SelfHarm,
        Category::HateIdentity,
        Category::CoerciveControl,
        Category::ScamFraud,
        Category::DrugsIllegal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::ThreatViolence => "threat-violence",
            Category::HarassmentBullying => "harassment-bullying",
            Category::SexualContent => "sexual-content",
            Category::GroomingExploitation => "grooming-exploitation",
            Category::SelfHarm => "self-harm",
            Category::HateIdentity => "hate-identity",
            Category::CoerciveControl => "coercive-control",
            Category::ScamFraud => "scam-fraud",
            Category::DrugsIllegal => "drugs-illegal",
        }
    }

    pub fn parse(s: &str) -> Option<Category> {
        Category::ALL.iter().copied().find(|c| c.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl ScanStatus {
    fn as_str(self) -> &'static str {
        match self {
            ScanStatus::Running => "running",
            ScanStatus::Completed => "completed",
            ScanStatus::Cancelled => "cancelled",
            ScanStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Message,
    Note,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Message => "message",
            SourceKind::Note => "note",
        }
    }

    pub fn parse(s: &str) -> Option<SourceKind> {
        match s {
            "message" => Some(SourceKind::Message),
            "note" => Some(SourceKind::Note),
            _ => None,
        }
    }
}

/// A Content Finding to insert (the write shape; `list_findings` returns rows).
#[derive(Debug, Clone)]
pub struct NewFinding {
    pub source_kind: SourceKind,
    pub source_id: Option<i64>,
    pub thread_identifier: Option<String>,
    pub occurred_at: Option<i64>,
    pub fingerprint: String,
    pub category: Category,
    pub severity: u8,
    pub rationale: String,
}

#[derive(Debug, Clone)]
pub struct FindingRow {
    pub id: i64,
    pub scan_id: i64,
    pub source_kind: SourceKind,
    pub source_id: Option<i64>,
    pub thread_identifier: Option<String>,
    pub occurred_at: Option<i64>,
    pub fingerprint: String,
    pub category: Category,
    pub severity: u8,
    pub rationale: String,
    pub stale: bool,
    pub dismissed: bool,
    pub created_at: i64,
}

/// One row of the `scans` table (see SCHEMA_V1 for column semantics).
#[derive(Debug, Clone)]
pub struct ScanRow {
    pub id: i64,
    pub model: String,
    pub range_start: Option<i64>,
    pub range_end: Option<i64>,
    /// Which content the scan covered: 'all' | 'messages' | 'notes'.
    pub sources: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub chunks_total: i64,
    pub chunks_done: i64,
}

/// A scan for the history list: the fields a user cares about (period, when,
/// status, model) plus its live finding counts. No `chunks` — that's internal.
#[derive(Debug, Clone)]
pub struct ScanListRow {
    pub id: i64,
    pub model: String,
    pub range_start: Option<i64>,
    pub range_end: Option<i64>,
    /// Which content the scan covered: 'all' | 'messages' | 'notes'.
    pub sources: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub findings: i64,
    /// Live (non-stale) finding counts split by severity, for the history
    /// row's badge: 3 = serious, 2 = harmful, 1 = concerning.
    pub serious: i64,
    pub harmful: i64,
    pub concerning: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStatus {
    Done,
    Skipped,
}

impl ChunkStatus {
    fn as_str(self) -> &'static str {
        match self {
            ChunkStatus::Done => "done",
            ChunkStatus::Skipped => "skipped",
        }
    }
}

impl AnalysisDb {
    /// Open (creating and migrating as needed) the analysis DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// In-memory DB for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Unlike the cache, findings are NOT cheap to rebuild after a crash —
        // FULL keeps every committed verdict durable at the cost of an fsync
        // per commit, which is noise next to seconds-per-chunk inference.
        conn.pragma_update(None, "synchronous", "FULL")?;
        // A UI write (dismiss) will land while a scan commit is in flight once
        // T7 opens a second connection; wait instead of failing SQLITE_BUSY.
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(SCHEMA_V1)?;
        }
        // Additive migrations go here (mirroring cache.rs); never downgrade a
        // newer store.
        if version < SCHEMA_VERSION {
            // v2: which content a scan covered ('all'|'messages'|'notes'), so
            // the history can label it and "Resume" can re-run the same scope.
            let has_sources = conn
                .prepare("PRAGMA table_info(scans)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "sources");
            if !has_sources {
                conn.execute(
                    "ALTER TABLE scans ADD COLUMN sources TEXT NOT NULL DEFAULT 'all'",
                    [],
                )?;
            }
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // ---- scans ----

    pub fn begin_scan(
        &self,
        model: &str,
        range: (Option<i64>, Option<i64>),
        sources: &str,
        started_at: i64,
    ) -> Result<i64> {
        // Backstop repair for scans stranded 'running' (normally already done
        // at backup open via repair_stranded_scans): one scan at a time means
        // any 'running' row at begin is by definition dead.
        self.repair_stranded_scans()?;
        self.conn.execute(
            "INSERT INTO scans (model, range_start, range_end, sources, status, started_at)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
            params![model, range.0, range.1, sources, started_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Reopen a non-completed scan for a resumed run: the SAME row goes back
    /// to 'running', so one logical scan attempt keeps one identity across
    /// stops and interruptions — findings and progress accumulate on it
    /// instead of scattering over a chain of rows. A new row is only ever
    /// created by an explicit new scan (begin_scan). The model is updated in
    /// case the user switched tiers between runs.
    pub fn resume_scan(&self, scan_id: i64, model: &str) -> Result<()> {
        // One scan at a time: any *other* stranded row is repaired first.
        self.repair_stranded_scans()?;
        let n = self.conn.execute(
            "UPDATE scans SET status = 'running', finished_at = NULL, chunks_done = 0,
                    model = ?2
             WHERE id = ?1 AND status != 'completed'",
            params![scan_id, model],
        )?;
        if n == 0 {
            return Err(Error::Invalid(format!(
                "scan {scan_id} is not resumable (missing or completed)"
            )));
        }
        Ok(())
    }

    /// Repair scans stranded 'running' by a crash or kill: mark them
    /// 'interrupted'. Called when a backup becomes active (this process
    /// provably has no scan in flight then), so the stored state never claims
    /// a scan is running longer than necessary. `finished_at` stays NULL —
    /// the actual death time is unknown and won't be invented. Returns the
    /// number of rows repaired.
    ///
    /// Caveat (accepted, same as the begin-time backstop): a second app
    /// instance sharing this DB with a genuinely live scan would be
    /// mislabeled — single-instance is the supported model.
    pub fn repair_stranded_scans(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE scans SET status = 'interrupted' WHERE status = 'running'",
            [],
        )?)
    }

    pub fn set_chunks_total(&self, scan_id: i64, total: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE scans SET chunks_total = ?2 WHERE id = ?1",
            params![scan_id, total],
        )?;
        Ok(())
    }

    pub fn finish_scan(&self, scan_id: i64, status: ScanStatus, finished_at: i64) -> Result<()> {
        if status == ScanStatus::Running {
            return Err(Error::Invalid("finish_scan with status 'running'".into()));
        }
        self.conn.execute(
            "UPDATE scans SET status = ?2, finished_at = ?3 WHERE id = ?1",
            params![scan_id, status.as_str(), finished_at],
        )?;
        Ok(())
    }

    // ---- chunk progress / resume ----

    /// Record a chunk as classified (or skipped). Upserts on chunk_key so the
    /// latest fingerprint wins.
    pub fn record_chunk(
        &self,
        scan_id: i64,
        chunk_key: &str,
        fingerprint: &str,
        status: ChunkStatus,
        at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chunk_progress (chunk_key, fingerprint, scan_id, status, classified_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(chunk_key) DO UPDATE SET
               fingerprint = excluded.fingerprint, scan_id = excluded.scan_id,
               status = excluded.status, classified_at = excluded.classified_at",
            params![chunk_key, fingerprint, scan_id, status.as_str(), at],
        )?;
        self.conn.execute(
            "UPDATE scans SET chunks_done = chunks_done + 1 WHERE id = ?1",
            params![scan_id],
        )?;
        Ok(())
    }

    /// Count a chunk toward `chunks_done` without touching chunk_progress —
    /// the reused-chunk path, so a resumed scan's persisted progress is honest.
    pub fn bump_chunks_done(&self, scan_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE scans SET chunks_done = chunks_done + 1 WHERE id = ?1",
            params![scan_id],
        )?;
        Ok(())
    }

    /// Rows written/re-confirmed by `scan_id` — the accurate per-run findings
    /// count (a message flagged by two overlapping windows is one row).
    pub fn count_scan_findings(&self, scan_id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM content_findings WHERE scan_id = ?1",
            params![scan_id],
            |r| r.get(0),
        )?)
    }

    /// True when `chunk_key` was already classified with this exact content —
    /// the resume/incremental check (plan T5).
    pub fn chunk_is_done(&self, chunk_key: &str, fingerprint: &str) -> Result<bool> {
        let hit: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM chunk_progress
                 WHERE chunk_key = ?1 AND fingerprint = ?2 AND status = 'done'",
                params![chunk_key, fingerprint],
                |r| r.get(0),
            )
            .optional()?;
        Ok(hit.is_some())
    }

    // ---- findings ----

    /// Insert findings for one classified chunk in a single transaction,
    /// clearing any previous findings that carry the same source fingerprints
    /// (re-classification of changed/re-scanned content replaces, not
    /// duplicates).
    pub fn replace_findings(
        &mut self,
        scan_id: i64,
        findings: &[NewFinding],
        at: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for f in findings {
            if !(1..=3).contains(&f.severity) {
                return Err(Error::Invalid(format!(
                    "severity {} out of range",
                    f.severity
                )));
            }
            tx.execute(
                "DELETE FROM content_findings
                 WHERE source_kind = ?1 AND fingerprint = ?2 AND category = ?3",
                params![f.source_kind.as_str(), f.fingerprint, f.category.as_str()],
            )?;
            tx.execute(
                "INSERT INTO content_findings
                   (scan_id, source_kind, source_id, thread_identifier, occurred_at,
                    fingerprint, category, severity, rationale, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    scan_id,
                    f.source_kind.as_str(),
                    f.source_id,
                    f.thread_identifier,
                    f.occurred_at,
                    f.fingerprint,
                    f.category.as_str(),
                    f.severity,
                    f.rationale,
                    at
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Findings, dismissed included (callers filter); severity-descending
    /// within category, newest first. `scan_id` restricts to one scan's
    /// findings (the per-scan history view); None returns every scan's.
    pub fn list_findings(&self, scan_id: Option<i64>) -> Result<Vec<FindingRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.scan_id, f.source_kind, f.source_id, f.thread_identifier,
                    f.occurred_at, f.fingerprint, f.category, f.severity, f.rationale,
                    f.stale, f.created_at,
                    EXISTS(SELECT 1 FROM dismissals d
                           WHERE d.fingerprint = f.fingerprint AND d.category = f.category)
             FROM content_findings f
             WHERE ?1 IS NULL OR f.scan_id = ?1
             ORDER BY f.severity DESC, f.occurred_at DESC",
        )?;
        let rows = stmt.query_map(params![scan_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, u8>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, bool>(10)?,
                r.get::<_, i64>(11)?,
                r.get::<_, bool>(12)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                scan_id,
                kind,
                source_id,
                thread_identifier,
                occurred_at,
                fingerprint,
                cat,
                severity,
                rationale,
                stale,
                created_at,
                dismissed,
            ) = row?;
            let source_kind = SourceKind::parse(&kind)
                .ok_or_else(|| Error::Invalid(format!("bad source_kind '{kind}'")))?;
            let category = Category::parse(&cat)
                .ok_or_else(|| Error::Invalid(format!("bad category '{cat}'")))?;
            out.push(FindingRow {
                id,
                scan_id,
                source_kind,
                source_id,
                thread_identifier,
                occurred_at,
                fingerprint,
                category,
                severity,
                rationale,
                stale,
                dismissed,
                created_at,
            });
        }
        Ok(out)
    }

    /// Dismiss (or un-dismiss) a finding as a false positive. Keyed by
    /// fingerprint + category so it outlives re-scans and re-imports.
    pub fn set_dismissed(
        &self,
        fingerprint: &str,
        category: Category,
        dismissed: bool,
        at: i64,
    ) -> Result<()> {
        if dismissed {
            self.conn.execute(
                "INSERT OR REPLACE INTO dismissals (fingerprint, category, dismissed_at)
                 VALUES (?1, ?2, ?3)",
                params![fingerprint, category.as_str(), at],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM dismissals WHERE fingerprint = ?1 AND category = ?2",
                params![fingerprint, category.as_str()],
            )?;
        }
        Ok(())
    }

    /// Mark findings stale/fresh by fingerprint set — run after re-import when
    /// re-resolving cache row ids (plan T3 AC: stale-flagged, never deleted).
    pub fn set_stale(&self, fingerprint: &str, stale: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE content_findings SET stale = ?2 WHERE fingerprint = ?1",
            params![fingerprint, stale],
        )?;
        Ok(())
    }

    /// Refresh the cached cache-row id for all findings with `fingerprint`.
    pub fn set_source_id(&self, fingerprint: &str, source_id: Option<i64>) -> Result<()> {
        self.conn.execute(
            "UPDATE content_findings SET source_id = ?2 WHERE fingerprint = ?1",
            params![fingerprint, source_id],
        )?;
        Ok(())
    }

    /// The most recent scan row (any status) — the UI's "what happened last".
    pub fn latest_scan(&self) -> Result<Option<ScanRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, model, range_start, range_end, sources, status, started_at,
                        finished_at, chunks_total, chunks_done
                 FROM scans ORDER BY id DESC LIMIT 1",
                [],
                |r| {
                    Ok(ScanRow {
                        id: r.get(0)?,
                        model: r.get(1)?,
                        range_start: r.get(2)?,
                        range_end: r.get(3)?,
                        sources: r.get(4)?,
                        status: r.get(5)?,
                        started_at: r.get(6)?,
                        finished_at: r.get(7)?,
                        chunks_total: r.get(8)?,
                        chunks_done: r.get(9)?,
                    })
                },
            )
            .optional()?)
    }

    /// A specific scan by id, for viewing a past scan's report.
    pub fn scan_by_id(&self, id: i64) -> Result<Option<ScanRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, model, range_start, range_end, sources, status, started_at,
                        finished_at, chunks_total, chunks_done
                 FROM scans WHERE id = ?1",
                params![id],
                |r| {
                    Ok(ScanRow {
                        id: r.get(0)?,
                        model: r.get(1)?,
                        range_start: r.get(2)?,
                        range_end: r.get(3)?,
                        sources: r.get(4)?,
                        status: r.get(5)?,
                        started_at: r.get(6)?,
                        finished_at: r.get(7)?,
                        chunks_total: r.get(8)?,
                        chunks_done: r.get(9)?,
                    })
                },
            )
            .optional()?)
    }

    /// Remove a scan and everything scoped to it — findings, per-chunk progress,
    /// summaries, and audit rows. Every child must go before the `scans` row
    /// itself: `foreign_keys` is ON, and each of these tables (audit_log
    /// included) has `scan_id REFERENCES scans(id)`, so leaving any behind makes
    /// the final delete fail. Dismissals are keyed by fingerprint (not scan) and
    /// are left intact so a re-scan still honours them.
    pub fn delete_scan(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM content_findings WHERE scan_id = ?1",
            params![id],
        )?;
        self.conn
            .execute("DELETE FROM chunk_progress WHERE scan_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM summaries WHERE scan_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM audit_log WHERE scan_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM scans WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Past scans, newest first, each with its live (non-stale) finding counts
    /// (total + per severity) — for the scan-history list.
    pub fn list_scans(&self, limit: i64) -> Result<Vec<ScanListRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.model, s.range_start, s.range_end, s.sources, s.status,
                    s.started_at, s.finished_at,
                    coalesce(count(f.id), 0),
                    coalesce(sum(f.severity = 3), 0),
                    coalesce(sum(f.severity = 2), 0),
                    coalesce(sum(f.severity = 1), 0)
             FROM scans s
             LEFT JOIN content_findings f ON f.scan_id = s.id AND f.stale = 0
             GROUP BY s.id ORDER BY s.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(ScanListRow {
                id: r.get(0)?,
                model: r.get(1)?,
                range_start: r.get(2)?,
                range_end: r.get(3)?,
                sources: r.get(4)?,
                status: r.get(5)?,
                started_at: r.get(6)?,
                finished_at: r.get(7)?,
                findings: r.get(8)?,
                serious: r.get(9)?,
                harmful: r.get(10)?,
                concerning: r.get(11)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// All summaries for a scan as (kind, thread_ref, content).
    pub fn list_summaries(&self, scan_id: i64) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, thread_ref, content FROM summaries
             WHERE scan_id = ?1 ORDER BY kind, thread_ref",
        )?;
        let rows = stmt.query_map(params![scan_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ---- summaries ----

    pub fn set_summary(
        &self,
        scan_id: i64,
        kind: &str,
        thread_ref: &str,
        content: &str,
        at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO summaries (scan_id, kind, thread_ref, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![scan_id, kind, thread_ref, content, at],
        )?;
        Ok(())
    }

    pub fn get_summary(
        &self,
        scan_id: i64,
        kind: &str,
        thread_ref: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT content FROM summaries
                 WHERE scan_id = ?1 AND kind = ?2 AND thread_ref = ?3",
                params![scan_id, kind, thread_ref],
                |r| r.get(0),
            )
            .optional()?)
    }

    // ---- audit log ----

    /// Append a content-free audit event. Callers must never pass source text
    /// in `detail` — ranges, counts, and model names only.
    pub fn audit(&self, scan_id: i64, at: i64, event: &str, detail: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit_log (scan_id, at, event, detail) VALUES (?1, ?2, ?3, ?4)",
            params![scan_id, at, event, detail],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(fp: &str, cat: Category) -> NewFinding {
        NewFinding {
            source_kind: SourceKind::Message,
            source_id: Some(42),
            thread_identifier: Some("chat123".into()),
            occurred_at: Some(1_700_000_000),
            fingerprint: fp.into(),
            category: cat,
            severity: 2,
            rationale: "test rationale".into(),
        }
    }

    #[test]
    fn schema_opens_and_stamps_version() {
        let db = AnalysisDb::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn finding_roundtrip_and_replacement() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db
            .begin_scan("gemma-4-E4B", (None, None), "all", 100)
            .unwrap();
        db.replace_findings(scan, &[finding("fp1", Category::ThreatViolence)], 101)
            .unwrap();
        // Re-classifying the same content replaces, never duplicates.
        let scan2 = db
            .begin_scan("gemma-4-E4B", (None, None), "all", 200)
            .unwrap();
        db.replace_findings(scan2, &[finding("fp1", Category::ThreatViolence)], 201)
            .unwrap();
        let rows = db.list_findings(None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scan_id, scan2);
        assert_eq!(rows[0].category, Category::ThreatViolence);
        assert!(!rows[0].dismissed);
    }

    #[test]
    fn dismissal_survives_rescan() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.replace_findings(scan, &[finding("fp1", Category::ScamFraud)], 101)
            .unwrap();
        db.set_dismissed("fp1", Category::ScamFraud, true, 102)
            .unwrap();
        // New scan re-inserts the same finding — dismissal must still apply.
        let scan2 = db.begin_scan("m", (None, None), "all", 200).unwrap();
        db.replace_findings(scan2, &[finding("fp1", Category::ScamFraud)], 201)
            .unwrap();
        let rows = db.list_findings(None).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].dismissed);
        // But a different category on the same message is NOT dismissed.
        db.replace_findings(scan2, &[finding("fp1", Category::ThreatViolence)], 202)
            .unwrap();
        let rows = db.list_findings(None).unwrap();
        let dismissed: Vec<bool> = rows.iter().map(|r| r.dismissed).collect();
        assert_eq!(rows.len(), 2);
        assert!(dismissed.contains(&true) && dismissed.contains(&false));
    }

    #[test]
    fn resume_reopens_the_same_row_never_a_new_one() {
        let db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.finish_scan(scan, ScanStatus::Cancelled, 150).unwrap();
        // Resume: same row back to running, finish cleared, model updated.
        db.resume_scan(scan, "m2").unwrap();
        let row = db.scan_by_id(scan).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.finished_at, None);
        assert_eq!(row.model, "m2");
        // A completed scan is not resumable, and no second row ever appeared.
        db.finish_scan(scan, ScanStatus::Completed, 200).unwrap();
        assert!(db.resume_scan(scan, "m2").is_err());
        assert_eq!(db.list_scans(50).unwrap().len(), 1);
    }

    #[test]
    fn repair_marks_stranded_scans_interrupted() {
        let db = AnalysisDb::open_in_memory().unwrap();
        let stranded = db.begin_scan("m", (None, None), "all", 100).unwrap();
        // Simulate a kill: the scan never finishes; the app reopens the backup.
        assert_eq!(db.repair_stranded_scans().unwrap(), 1);
        let (status, finished): (String, Option<i64>) = db
            .conn()
            .query_row(
                "SELECT status, finished_at FROM scans WHERE id = ?1",
                params![stranded],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "interrupted");
        assert_eq!(finished, None);
        // Idempotent: nothing left to repair.
        assert_eq!(db.repair_stranded_scans().unwrap(), 0);
    }

    #[test]
    fn list_findings_filters_by_scan() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan1 = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.replace_findings(scan1, &[finding("fp1", Category::ScamFraud)], 101)
            .unwrap();
        let scan2 = db.begin_scan("m", (None, None), "all", 200).unwrap();
        db.replace_findings(scan2, &[finding("fp2", Category::SelfHarm)], 201)
            .unwrap();
        assert_eq!(db.list_findings(None).unwrap().len(), 2);
        let only1 = db.list_findings(Some(scan1)).unwrap();
        assert_eq!(only1.len(), 1);
        assert_eq!(only1[0].fingerprint, "fp1");
        let only2 = db.list_findings(Some(scan2)).unwrap();
        assert_eq!(only2.len(), 1);
        assert_eq!(only2[0].fingerprint, "fp2");
    }

    #[test]
    fn list_scans_reports_model_and_severity_split() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db
            .begin_scan("gemma-4-E4B", (None, None), "all", 100)
            .unwrap();
        let mut serious = finding("fp1", Category::ThreatViolence);
        serious.severity = 3;
        let harmful = finding("fp2", Category::ScamFraud); // severity 2
        let mut concerning = finding("fp3", Category::SelfHarm);
        concerning.severity = 1;
        db.replace_findings(scan, &[serious, harmful, concerning], 101)
            .unwrap();
        db.finish_scan(scan, ScanStatus::Completed, 102).unwrap();
        // A stale finding must not count toward any bucket.
        db.set_stale("fp3", true).unwrap();

        let rows = db.list_scans(50).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model, "gemma-4-E4B");
        assert_eq!(rows[0].sources, "all");
        assert_eq!(rows[0].findings, 2);
        assert_eq!(
            (rows[0].serious, rows[0].harmful, rows[0].concerning),
            (1, 1, 0)
        );
    }

    #[test]
    fn chunk_resume_is_fingerprint_sensitive() {
        let db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.record_chunk(scan, "thread1:0", "abc", ChunkStatus::Done, 101)
            .unwrap();
        assert!(db.chunk_is_done("thread1:0", "abc").unwrap());
        // Content changed → chunk must be re-classified.
        assert!(!db.chunk_is_done("thread1:0", "def").unwrap());
        // Skipped chunks never count as done.
        db.record_chunk(scan, "thread1:1", "xyz", ChunkStatus::Skipped, 102)
            .unwrap();
        assert!(!db.chunk_is_done("thread1:1", "xyz").unwrap());
    }

    #[test]
    fn severity_range_enforced() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        let mut bad = finding("fp1", Category::SelfHarm);
        bad.severity = 4;
        assert!(db.replace_findings(scan, &[bad], 101).is_err());
    }

    #[test]
    fn stale_flag_and_source_id_refresh() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.replace_findings(scan, &[finding("fp1", Category::SelfHarm)], 101)
            .unwrap();
        db.set_stale("fp1", true).unwrap();
        db.set_source_id("fp1", None).unwrap();
        let rows = db.list_findings(None).unwrap();
        assert!(rows[0].stale);
        assert_eq!(rows[0].source_id, None);
        db.set_source_id("fp1", Some(99)).unwrap();
        db.set_stale("fp1", false).unwrap();
        let rows = db.list_findings(None).unwrap();
        assert!(!rows[0].stale);
        assert_eq!(rows[0].source_id, Some(99));
    }

    #[test]
    fn stale_running_scan_repaired_at_next_begin() {
        let db = AnalysisDb::open_in_memory().unwrap();
        let dead = db.begin_scan("m", (None, None), "all", 100).unwrap();
        // Simulate a crash: never finished. The next begin_scan repairs it.
        let live = db.begin_scan("m", (None, None), "all", 200).unwrap();
        let (dead_status, dead_finished): (String, Option<i64>) = db
            .conn()
            .query_row(
                "SELECT status, finished_at FROM scans WHERE id = ?1",
                params![dead],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        // Marked 'interrupted', and no invented finish time — when it actually
        // died is unknown.
        assert_eq!(dead_status, "interrupted");
        assert_eq!(dead_finished, None);
        let live_status: String = db
            .conn()
            .query_row(
                "SELECT status FROM scans WHERE id = ?1",
                params![live],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live_status, "running");
    }

    #[test]
    fn scan_lifecycle_and_summary() {
        let db = AnalysisDb::open_in_memory().unwrap();
        let scan = db
            .begin_scan("gemma-4-E2B", (Some(1000), Some(2000)), "all", 100)
            .unwrap();
        db.set_chunks_total(scan, 10).unwrap();
        db.record_chunk(scan, "k", "fp", ChunkStatus::Done, 101)
            .unwrap();
        db.audit(scan, 101, "chunk_classified", "chunk=k verdicts=0")
            .unwrap();
        assert!(db.finish_scan(scan, ScanStatus::Running, 102).is_err());
        db.finish_scan(scan, ScanStatus::Completed, 103).unwrap();
        db.set_summary(scan, "report", "", "Nothing flagged.", 104)
            .unwrap();
        assert_eq!(
            db.get_summary(scan, "report", "").unwrap().as_deref(),
            Some("Nothing flagged.")
        );
        assert_eq!(db.get_summary(scan, "thread", "x").unwrap(), None);
    }

    #[test]
    fn delete_scan_removes_all_children() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        // A fully-populated scan: a finding, a chunk row, an audit row, and a
        // summary — one row in every table that references scans(id). With
        // foreign_keys ON, delete_scan must clear all of them (the audit_log
        // row is the one that used to be left behind and blocked the delete).
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.record_chunk(scan, "k", "fp", ChunkStatus::Done, 101)
            .unwrap();
        db.audit(scan, 101, "chunk_classified", "chunk=k").unwrap();
        db.set_summary(scan, "report", "", "Nothing flagged.", 104)
            .unwrap();
        db.replace_findings(
            scan,
            &[NewFinding {
                source_kind: SourceKind::Message,
                source_id: Some(1),
                thread_identifier: Some("t".into()),
                occurred_at: Some(100),
                fingerprint: "fp".into(),
                category: Category::ScamFraud,
                severity: 2,
                rationale: "x".into(),
            }],
            105,
        )
        .unwrap();

        // A second scan is left untouched, proving the delete is scoped by id.
        let keep = db.begin_scan("m", (None, None), "all", 200).unwrap();
        db.audit(keep, 201, "scan_started", "").unwrap();

        db.delete_scan(scan).unwrap();

        assert!(db.scan_by_id(scan).unwrap().is_none());
        for (table, col) in [
            ("content_findings", "scan_id"),
            ("chunk_progress", "scan_id"),
            ("summaries", "scan_id"),
            ("audit_log", "scan_id"),
        ] {
            let n: i64 = db
                .conn()
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE {col} = ?1"),
                    params![scan],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0, "{table} still had rows for the deleted scan");
        }
        // The other scan and its audit row survive.
        assert!(db.scan_by_id(keep).unwrap().is_some());
        let kept_audit: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM audit_log WHERE scan_id = ?1",
                params![keep],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept_audit, 1);
    }
}
