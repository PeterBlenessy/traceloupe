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

const SCHEMA_VERSION: i64 = 6;

/// A scan's SCOPE as a SQL predicate over `content_findings f`: `?1` is the
/// sources slug ('all' or a comma-joined service list), `?2`/`?3` the optional
/// range bounds. Findings with a NULL `occurred_at` (e.g. an undated note) stay
/// in scope — a range filter can't place them, and dropping them would hide
/// real findings.
///
/// Defined once because *every* view of "how many findings does this scan
/// have" must agree: [`AnalysisDb::list_findings_in_scope`] (the Findings
/// panel) and [`AnalysisDb::count_findings_in_scope`] (the live progress
/// counter) both build on it. Two hand-written copies of this predicate is
/// exactly how the panel and the progress bar drifted apart (#59).
/// Whether a finding has been dismissed. Written once: the SELECT, the
/// "hide dismissed" filter and the counts must agree, and three copies of a
/// correlated EXISTS is how they would stop agreeing.
/// The scope predicate plus a page request's filters. Shared by the page query
/// and the matching count, so the number the list produces and the number the
/// panel promises come from the same string — separately-derived counts and rows
/// are exactly what drifted in #59.
fn filtered_scope(q: &FindingQuery) -> String {
    let mut w = String::from(IN_SCOPE_PREDICATE);
    if !q.include_dismissed {
        w.push_str(&format!(" AND NOT {DISMISSED_EXPR}"));
    }
    if q.exclude_stale {
        w.push_str(" AND NOT f.stale");
    }
    if let Some(sev) = q.severity {
        // An integer from an enum-shaped field, never caller text.
        w.push_str(&format!(" AND f.severity = {}", sev as i64));
    }
    w
}

/// How the Findings panel orders a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FindingSort {
    /// Severity first, recency inside a band.
    #[default]
    Severity,
    Date,
}

/// A page request from the Findings panel: its filters and its order.
#[derive(Debug, Clone, Default)]
pub struct FindingQuery {
    /// Only this severity, or every severity when None.
    pub severity: Option<u8>,
    /// Dismissed findings are hidden unless the panel asks for them.
    pub include_dismissed: bool,
    pub sort: FindingSort,
    pub desc: bool,
    /// Order by conversation first, so the panel's grouped mode can build its
    /// headings from a window without holding the whole list.
    pub group_by_thread: bool,
    /// Drop findings whose source content is gone. The panel keeps them (the
    /// verdict still stands); the printable report leaves them out, because it
    /// quotes the text and there is none to quote.
    pub exclude_stale: bool,
}

/// What the filter pills display, counted in one query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FindingCounts {
    /// Not dismissed — the panel's default view.
    pub live: i64,
    /// Not dismissed AND not stale — what the printable report includes, so its
    /// "N more not shown" line can be computed without fetching everything.
    pub live_fresh: i64,
    pub dismissed: i64,
    pub serious: i64,
    pub harmful: i64,
    pub concerning: i64,
}

/// How many conversations the by-conversation chart draws before folding the
/// rest into a stated remainder.
pub const CONVERSATION_CHART_CAP: usize = 12;

/// How the report's time chart buckets its x-axis.
///
/// Chosen from the span the findings cover rather than fixed, so a two-week scan
/// and a ten-year one both produce roughly 10–30 bars. The axis names the unit,
/// because "Findings by month" and "Findings by day" are different claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeUnit {
    Day,
    Week,
    /// What an empty or single-instant scan falls back to.
    #[default]
    Month,
    Quarter,
    Year,
}

impl TimeUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            TimeUnit::Day => "day",
            TimeUnit::Week => "week",
            TimeUnit::Month => "month",
            TimeUnit::Quarter => "quarter",
            TimeUnit::Year => "year",
        }
    }

    /// The coarsest unit that still shows the shape of a span this long.
    fn for_span(seconds: i64) -> TimeUnit {
        const DAY: i64 = 86_400;
        match seconds {
            s if s <= 31 * DAY => TimeUnit::Day,
            s if s <= 210 * DAY => TimeUnit::Week,
            s if s <= 1095 * DAY => TimeUnit::Month,
            s if s <= 3650 * DAY => TimeUnit::Quarter,
            _ => TimeUnit::Year,
        }
    }

    /// SQLite expression yielding this unit's bucket key.
    ///
    /// Bucketed by the LOCAL calendar (`localtime`) — a message sent at 23:30 on
    /// the 31st belongs to that month as the reader lived it, not as UTC files
    /// it. The keys are `YYYY-MM-DD` (day, and a week's Monday), `YYYY-MM`,
    /// `YYYY-Qn` and `YYYY`; the view formats them for the locale, so no
    /// timestamp is re-interpreted in a second time zone on the way out.
    fn key_expr(self) -> &'static str {
        match self {
            TimeUnit::Day => "date(f.occurred_at, 'unixepoch', 'localtime')",
            // 'weekday 0' lands on the coming Sunday (or stays put on one), so
            // -6 days is that week's Monday — including when the finding itself
            // falls on a Monday.
            TimeUnit::Week => {
                "date(f.occurred_at, 'unixepoch', 'localtime', 'weekday 0', '-6 days')"
            }
            TimeUnit::Month => "strftime('%Y-%m', f.occurred_at, 'unixepoch', 'localtime')",
            TimeUnit::Quarter => {
                "strftime('%Y', f.occurred_at, 'unixepoch', 'localtime') || '-Q' ||
                 ((CAST(strftime('%m', f.occurred_at, 'unixepoch', 'localtime') AS INTEGER) + 2) / 3)"
            }
            TimeUnit::Year => "strftime('%Y', f.occurred_at, 'unixepoch', 'localtime')",
        }
    }
}

/// One bar of one chart, split two ways.
///
/// `confirmed[i]` and `unconfirmed[i]` are severity `i + 1` — index 0 is
/// concerning, 2 is serious. Confirmed means the cascade's strong tier agreed
/// with the sweep; unconfirmed means only the fast tier ever saw it, and the
/// chart hatches that portion rather than letting it borrow the same authority.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChartBucket {
    /// The bucket's identity: a date key, a category slug, or a thread
    /// identifier. Empty means "findings with no conversation" — notes.
    pub key: String,
    pub confirmed: [i64; 3],
    pub unconfirmed: [i64; 3],
}

impl ChartBucket {
    pub fn total(&self) -> i64 {
        self.confirmed.iter().chain(&self.unconfirmed).sum()
    }
}

/// Everything the report's charts draw, from one pass over the findings.
#[derive(Debug, Clone, Default)]
pub struct FindingAnalytics {
    /// What one bar of [`Self::over_time`] spans.
    pub unit: TimeUnit,
    /// Only the buckets that have findings — a run of empty months is a gap the
    /// view fills, so an absence of data can't arrive as an absence of bars.
    pub over_time: Vec<ChartBucket>,
    pub by_category: Vec<ChartBucket>,
    /// The busiest conversations, most findings first, capped at
    /// [`CONVERSATION_CHART_CAP`].
    pub by_conversation: Vec<ChartBucket>,
    /// Conversations past the cap, and their findings — stated, never dropped.
    pub other_conversations: i64,
    pub other_conversation_findings: i64,
    /// How many findings the charts describe, under the caller's filter.
    pub charted: i64,
    /// In scope but undated, so absent from [`Self::over_time`].
    pub undated: i64,
    /// Dismissed as false positives — excluded from every chart, reported so the
    /// reader can see how much the model got wrong.
    pub dismissed: i64,
}

const DISMISSED_EXPR: &str = "EXISTS(SELECT 1 FROM dismissals d
                WHERE d.fingerprint = f.fingerprint AND d.category = f.category)";

const IN_SCOPE_PREDICATE: &str = "(?1 = 'all'
     OR ((',' || ?1 || ',') LIKE '%,notes,%' AND f.source_kind = 'note')
     OR ((',' || ?1 || ',') LIKE '%,messages,%' AND f.source_kind = 'message')
     OR (f.source_kind = 'message' AND f.service IS NOT NULL
         AND (',' || ?1 || ',') LIKE ('%,' || f.service || ',%')))
 AND (?2 IS NULL OR f.occurred_at IS NULL OR f.occurred_at >= ?2)
 AND (?3 IS NULL OR f.occurred_at IS NULL OR f.occurred_at <= ?3)";

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
    rechecked         INTEGER NOT NULL DEFAULT 0,  -- 1 = confirmed by the cascade's strong tier (v3)
    service           TEXT,                 -- message thread's service (iMessage/SMS/TikTok…); NULL for notes/legacy (v5)
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
    flagged       INTEGER NOT NULL DEFAULT 0, -- sweep produced ≥1 finding (v4): the
                                             -- DURABLE cascade re-check set, immune to
                                             -- a sibling window's re-check deleting the
                                             -- shared item's finding
    classified_at INTEGER NOT NULL
);

-- Scan report + per-flagged-thread summaries (plan T6).
CREATE TABLE IF NOT EXISTS summaries (
    scan_id    INTEGER NOT NULL REFERENCES scans(id),
    kind       TEXT NOT NULL,                -- 'report' | 'thread'
    thread_ref TEXT NOT NULL DEFAULT '',     -- threads.identifier for kind='thread'
    content    TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    digest     TEXT NOT NULL DEFAULT '',     -- sha256 of the findings that produced
                                             -- this text; '' = pre-digest row
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
    /// The message thread's service (iMessage/SMS/TikTok…); None for notes. Lets
    /// a scan scoped to specific services count/list exactly its findings.
    pub service: Option<String>,
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
    /// 1 = confirmed by the cascade's strong tier (E4B re-checked and kept it);
    /// 0 = seen only by the fast sweep tier (E2B), unconfirmed.
    pub rechecked: bool,
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
            // v3: the cascade's strong tier marks confirmed findings so a
            // later chunk's re-check clear can't wipe an earlier chunk's
            // confirmation of a shared (overlapping) item.
            let has_rechecked = conn
                .prepare("PRAGMA table_info(content_findings)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "rechecked");
            if !has_rechecked {
                conn.execute(
                    "ALTER TABLE content_findings ADD COLUMN rechecked INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            // v4: durable "sweep flagged this chunk" marker on chunk_progress,
            // so the cascade re-check set survives a sibling window's re-check
            // deleting a shared item's finding (recomputing the set from live
            // findings could otherwise drop a chunk mid-cascade on resume).
            let has_flagged = conn
                .prepare("PRAGMA table_info(chunk_progress)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "flagged");
            if !has_flagged {
                conn.execute(
                    "ALTER TABLE chunk_progress ADD COLUMN flagged INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            // v5: the message thread's service on each finding (iMessage/SMS/
            // TikTok…), so a scan scoped to a subset of services can count/list
            // exactly its findings. NULL for notes and for findings created
            // before this column existed (they only match 'all'/'messages').
            let has_service = conn
                .prepare("PRAGMA table_info(content_findings)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "service");
            if !has_service {
                conn.execute("ALTER TABLE content_findings ADD COLUMN service TEXT", [])?;
            }
            // v6: a digest of the findings a summary was written from, so a
            // re-scan can reuse the text when those findings haven't changed
            // instead of paying a model call per thread every time (#43).
            // Existing rows keep '' — which never matches a real digest, so
            // they simply re-summarize once.
            let has_digest = conn
                .prepare("PRAGMA table_info(summaries)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "digest");
            if !has_digest {
                conn.execute(
                    "ALTER TABLE summaries ADD COLUMN digest TEXT NOT NULL DEFAULT ''",
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
    pub fn resume_scan(&self, scan_id: i64, _model: &str) -> Result<()> {
        // One scan at a time: any *other* stranded row is repaired first.
        self.repair_stranded_scans()?;
        // Deliberately does NOT touch `model`: the row already records what
        // ran (including a completed cascade's "e2b→e4b" receipt), and
        // overwriting it with the resume's sweep model would erase that
        // provenance. Resume continues the same scan, so its recorded tier
        // stands.
        let n = self.conn.execute(
            "UPDATE scans SET status = 'running', finished_at = NULL, chunks_done = 0
             WHERE id = ?1 AND status != 'completed'",
            params![scan_id],
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
    /// latest fingerprint wins. `flagged` marks that this (sweep) chunk
    /// produced ≥1 finding — the durable cascade re-check set (see v4 schema).
    pub fn record_chunk(
        &self,
        scan_id: i64,
        chunk_key: &str,
        fingerprint: &str,
        status: ChunkStatus,
        flagged: bool,
        at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chunk_progress (chunk_key, fingerprint, scan_id, status, flagged, classified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(chunk_key) DO UPDATE SET
               fingerprint = excluded.fingerprint, scan_id = excluded.scan_id,
               status = excluded.status, flagged = excluded.flagged,
               classified_at = excluded.classified_at",
            params![chunk_key, fingerprint, scan_id, status.as_str(), flagged, at],
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
                    fingerprint, category, severity, rationale, service, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                    f.service,
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
        self.query_findings("?1 IS NULL OR f.scan_id = ?1", params![scan_id])
    }

    /// Findings within a scan's SCOPE — its sources ('all'|'messages'|'notes')
    /// and optional time range — regardless of which run classified them. This
    /// is what a scan's detail view shows: because classification is cached per
    /// chunk across scans, a finding "belongs to" the first run that saw it, but
    /// every scan whose scope covers it must surface it (see [`Self::list_scans`]).
    /// A finding with a NULL `occurred_at` (e.g. an undated note) is kept — a
    /// range filter can't place it, and dropping it would hide real findings.
    pub fn list_findings_in_scope(
        &self,
        sources: &str,
        range_start: Option<i64>,
        range_end: Option<i64>,
    ) -> Result<Vec<FindingRow>> {
        self.query_findings(IN_SCOPE_PREDICATE, params![sources, range_start, range_end])
    }

    /// One page of a scan's findings, filtered and ordered by SQLite rather than
    /// by the view (#65).
    ///
    /// The panel used to receive every finding and derive the visible list in
    /// JavaScript — ~3 MB of JSON at the 8800 findings seen in practice, re-sent
    /// and re-derived on every invalidation. Ordering has to happen here anyway
    /// for paging to mean anything: a page is only well-defined relative to a
    /// total order.
    ///
    /// `stale` findings are deliberately still returned. Their source content is
    /// gone but the verdict stands, and hiding them would silently shrink a past
    /// scan's results.
    pub fn list_findings_in_scope_page(
        &self,
        sources: &str,
        range_start: Option<i64>,
        range_end: Option<i64>,
        q: &FindingQuery,
        offset: i64,
        limit: i64,
    ) -> Result<Vec<FindingRow>> {
        let where_clause = filtered_scope(q);
        // Built from enums and integers, never from caller text.
        let dir = if q.desc { "DESC" } else { "ASC" };
        let mut tail = String::from("ORDER BY ");
        if q.group_by_thread {
            // Notes have no thread; they gather at the end under their own
            // heading, which is where the grouped view has always put them.
            tail.push_str("f.thread_identifier IS NULL, f.thread_identifier, ");
        }
        match q.sort {
            FindingSort::Severity => {
                // Severity first, recency inside a band — the same order the
                // panel produced with `severity * 1e12 + occurred_at`.
                tail.push_str(&format!("f.severity {dir}, f.occurred_at {dir}"));
            }
            FindingSort::Date => tail.push_str(&format!("f.occurred_at {dir}")),
        }
        // A total order, so paging is well-defined. Findings routinely share a
        // severity AND a timestamp, and SQL leaves the order among ties
        // unspecified — today's plan happens to return them by rowid, but adding
        // an index over (severity, occurred_at) would be enough to change that,
        // and then a row sits on two pages while another sits on none.
        tail.push_str(&format!(", f.id {dir} LIMIT {limit} OFFSET {offset}"));
        self.query_findings_tail(
            &where_clause,
            &tail,
            params![sources, range_start, range_end],
        )
    }

    /// How many findings the current filter matches — the virtualizer's row
    /// count. Same predicate as the page, so the list can't run out early or
    /// leave a gap at the end.
    pub fn count_findings_matching(
        &self,
        sources: &str,
        range_start: Option<i64>,
        range_end: Option<i64>,
        q: &FindingQuery,
    ) -> Result<i64> {
        Ok(self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM content_findings f WHERE {}",
                filtered_scope(q)
            ),
            params![sources, range_start, range_end],
            |r| r.get(0),
        )?)
    }

    /// The numbers the filter pills show, in one round trip.
    ///
    /// Counted with the same predicate the page query uses, so a pill can never
    /// promise rows the list won't produce — the drift #59 was about.
    pub fn count_findings_breakdown(
        &self,
        sources: &str,
        range_start: Option<i64>,
        range_end: Option<i64>,
    ) -> Result<FindingCounts> {
        let sql = format!(
            "SELECT
               COUNT(*) FILTER (WHERE NOT {DISMISSED_EXPR}),
               COUNT(*) FILTER (WHERE NOT {DISMISSED_EXPR} AND NOT f.stale),
               COUNT(*) FILTER (WHERE {DISMISSED_EXPR}),
               COUNT(*) FILTER (WHERE NOT {DISMISSED_EXPR} AND f.severity = 3),
               COUNT(*) FILTER (WHERE NOT {DISMISSED_EXPR} AND f.severity = 2),
               COUNT(*) FILTER (WHERE NOT {DISMISSED_EXPR} AND f.severity = 1)
             FROM content_findings f
             WHERE {IN_SCOPE_PREDICATE}"
        );
        let c = self
            .conn
            .query_row(&sql, params![sources, range_start, range_end], |r| {
                Ok(FindingCounts {
                    live: r.get(0)?,
                    live_fresh: r.get(1)?,
                    dismissed: r.get(2)?,
                    serious: r.get(3)?,
                    harmful: r.get(4)?,
                    concerning: r.get(5)?,
                })
            })?;
        Ok(c)
    }

    /// How many LIVE findings are in a scan's scope — the same predicate as
    /// [`Self::list_findings_in_scope`] plus the two filters the Findings panel
    /// applies client-side (`!dismissed && !stale`), so this number is exactly
    /// what the panel displays. The live scan progress reports it, which is why
    /// it must not drift from the list query: both build on
    /// [`IN_SCOPE_PREDICATE`].
    pub fn count_findings_in_scope(
        &self,
        sources: &str,
        range_start: Option<i64>,
        range_end: Option<i64>,
    ) -> Result<usize> {
        Ok(self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM content_findings f
                 WHERE ({IN_SCOPE_PREDICATE})
                   AND f.stale = 0
                   AND NOT EXISTS(SELECT 1 FROM dismissals d
                                  WHERE d.fingerprint = f.fingerprint
                                    AND d.category = f.category)"
            ),
            params![sources, range_start, range_end],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// The report's charts, counted in SQL over EVERY finding the filter matches
    /// (#66).
    ///
    /// The rendered list is capped — 500 rows in the panel, 100 in the narrative
    /// — and a chart built from either cap would quietly describe a subset while
    /// looking like it described the whole scan. So the aggregates are computed
    /// here, and from [`filtered_scope`]: the same predicate the page query uses,
    /// so a chart can never describe a different population than the list beneath
    /// it.
    ///
    /// Every bar carries its severity split AND its confirmation split. The
    /// cascade re-checks only some findings with the strong tier, so a bar's
    /// unconfirmed portion is drawn hatched — a scan run without the cascade then
    /// *looks* less certain instead of being silently so.
    pub fn finding_analytics(
        &self,
        sources: &str,
        range_start: Option<i64>,
        range_end: Option<i64>,
        q: &FindingQuery,
    ) -> Result<FindingAnalytics> {
        let scope = filtered_scope(q);

        // The bucket unit comes from the span the findings actually cover, so a
        // three-week scan and a ten-year one both produce a readable axis.
        // Undated findings are counted here and excluded from the time chart:
        // they cannot be placed on an axis, and dropping them silently would
        // leave the chart's total disagreeing with the list's.
        let (min_at, max_at, undated, charted) = self.conn.query_row(
            &format!(
                "SELECT MIN(f.occurred_at), MAX(f.occurred_at),
                        COUNT(*) FILTER (WHERE f.occurred_at IS NULL),
                        COUNT(*)
                 FROM content_findings f WHERE {scope}"
            ),
            params![sources, range_start, range_end],
            |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )?;
        let unit = match (min_at, max_at) {
            (Some(a), Some(b)) => TimeUnit::for_span(b - a),
            // Nothing dated to measure; the axis is empty either way.
            _ => TimeUnit::Month,
        };

        let over_time = self.bucket_counts(
            &format!("({scope}) AND f.occurred_at IS NOT NULL"),
            unit.key_expr(),
            "ORDER BY k",
            params![sources, range_start, range_end],
        )?;
        let by_category = self.bucket_counts(
            &scope,
            "f.category",
            "ORDER BY total DESC, k",
            params![sources, range_start, range_end],
        )?;
        // Notes have no thread; they gather under one empty key, which the view
        // labels, rather than one bar per note.
        let mut by_conversation = self.bucket_counts(
            &scope,
            "COALESCE(f.thread_identifier, '')",
            "ORDER BY total DESC, k",
            params![sources, range_start, range_end],
        )?;

        // A scan can touch hundreds of conversations. The chart shows the busiest
        // and folds the rest into a stated remainder — an unbounded bar chart is
        // an unbounded payload, and neither is readable.
        let mut other_conversations = 0;
        let mut other_conversation_findings = 0;
        if by_conversation.len() > CONVERSATION_CHART_CAP {
            for b in by_conversation.split_off(CONVERSATION_CHART_CAP) {
                other_conversations += 1;
                other_conversation_findings += b.total();
            }
        }

        // Dismissed is reported whatever the current filter is: the charts leave
        // false positives out, and the count is how the report says so.
        let dismissed = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM content_findings f
                 WHERE ({IN_SCOPE_PREDICATE}) AND {DISMISSED_EXPR}"
            ),
            params![sources, range_start, range_end],
            |r| r.get::<_, i64>(0),
        )?;

        Ok(FindingAnalytics {
            unit,
            over_time,
            by_category,
            by_conversation,
            other_conversations,
            other_conversation_findings,
            charted,
            undated,
            dismissed,
        })
    }

    /// One grouped count query, parameterised by what makes a bucket. Every chart
    /// shares it, so the severity and confirmation splits cannot mean one thing
    /// on the time axis and another by category.
    fn bucket_counts(
        &self,
        where_clause: &str,
        key_expr: &str,
        tail: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<ChartBucket>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {key_expr} AS k,
                    COUNT(*) FILTER (WHERE f.severity = 1 AND f.rechecked),
                    COUNT(*) FILTER (WHERE f.severity = 2 AND f.rechecked),
                    COUNT(*) FILTER (WHERE f.severity = 3 AND f.rechecked),
                    COUNT(*) FILTER (WHERE f.severity = 1 AND NOT f.rechecked),
                    COUNT(*) FILTER (WHERE f.severity = 2 AND NOT f.rechecked),
                    COUNT(*) FILTER (WHERE f.severity = 3 AND NOT f.rechecked),
                    COUNT(*) AS total
             FROM content_findings f
             WHERE {where_clause}
             GROUP BY k
             {tail}"
        ))?;
        let rows = stmt.query_map(params, |r| {
            Ok(ChartBucket {
                key: r.get(0)?,
                confirmed: [r.get(1)?, r.get(2)?, r.get(3)?],
                unconfirmed: [r.get(4)?, r.get(5)?, r.get(6)?],
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Shared body for the finding-list queries: the same SELECT + severity-desc
    /// ordering, parameterised only by the WHERE predicate.
    fn query_findings(
        &self,
        where_clause: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<FindingRow>> {
        self.query_findings_tail(
            where_clause,
            "ORDER BY f.severity DESC, f.occurred_at DESC",
            params,
        )
    }

    /// The row query, with the caller supplying everything after `WHERE` —
    /// ordering, and a `LIMIT`/`OFFSET` when it wants one page rather than all
    /// of them. One SELECT for both, so a paged read and a whole read can never
    /// disagree about what a finding row contains.
    fn query_findings_tail(
        &self,
        where_clause: &str,
        tail: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<FindingRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT f.id, f.scan_id, f.source_kind, f.source_id, f.thread_identifier,
                    f.occurred_at, f.fingerprint, f.category, f.severity, f.rationale,
                    f.stale, f.created_at, f.rechecked,
                    {DISMISSED_EXPR}
             FROM content_findings f
             WHERE {where_clause}
             {tail}",
        ))?;
        let rows = stmt.query_map(params, |r| {
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
                r.get::<_, bool>(13)?,
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
                rechecked,
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
                rechecked,
                created_at,
            });
        }
        Ok(out)
    }

    /// The chunk keys whose SWEEP produced a finding (`flagged = 1`) — the
    /// cascade's re-check set. Durable and resume-stable: derived from the
    /// sweep-time marker on chunk_progress, NOT from live findings (which a
    /// sibling window's re-check can delete), so an interrupted cascade never
    /// loses a chunk that still needs the strong tier. Only sweep rows carry
    /// `flagged = 1`; `#recheck` rows are always 0.
    pub fn flagged_chunk_keys(&self) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT chunk_key FROM chunk_progress WHERE flagged = 1")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new();
        for k in rows {
            out.insert(k?);
        }
        Ok(out)
    }

    /// Update a scan's recorded model — the cascade stamps "e2b→e4b" once the
    /// re-check phase actually ran, so the receipt says what judged what.
    pub fn set_model(&self, scan_id: i64, model: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE scans SET model = ?2 WHERE id = ?1",
            params![scan_id, model],
        )?;
        Ok(())
    }

    /// Apply the cascade strong tier's verdict for one chunk (#35), ATOMICALLY:
    /// in a single transaction it (1) removes the chunk items' *sweep* findings
    /// (rechecked = 0) so the strong tier's silence overrules a weak-tier false
    /// positive, (2) inserts the strong tier's verdicts marked rechecked = 1,
    /// and (3) records the `<chunk_key>#recheck` checkpoint. All-or-nothing is
    /// the whole point: a crash mid-way must not leave an item with its sweep
    /// finding deleted but no checkpoint, which resume would read as "cleared"
    /// and never re-check — silently dropping a finding the strong tier might
    /// have confirmed.
    ///
    /// The `rechecked = 0` scope on the sweep-clear is what makes overlapping
    /// windows safe: a later chunk re-checking a shared item deletes only the
    /// remaining sweep verdicts, never an earlier chunk's confirmation
    /// (rechecked = 1) of that same item.
    pub fn apply_recheck(
        &mut self,
        scan_id: i64,
        chunk_key: &str,
        chunk_fingerprint: &str,
        item_fingerprints: &[String],
        findings: &[NewFinding],
        at: i64,
    ) -> Result<()> {
        for f in findings {
            if !(1..=3).contains(&f.severity) {
                return Err(Error::Invalid(format!(
                    "severity {} out of range",
                    f.severity
                )));
            }
        }
        let tx = self.conn.transaction()?;
        // 1. Drop this chunk's items' unconfirmed (sweep) findings.
        for fp in item_fingerprints {
            tx.execute(
                "DELETE FROM content_findings WHERE fingerprint = ?1 AND rechecked = 0",
                params![fp],
            )?;
        }
        // 2. Insert the strong tier's verdicts (rechecked = 1). Collapse a
        //    duplicate (kind, fp, category) — including another window's
        //    confirmation of the same item+category — into one row.
        for f in findings {
            tx.execute(
                "DELETE FROM content_findings
                 WHERE source_kind = ?1 AND fingerprint = ?2 AND category = ?3",
                params![f.source_kind.as_str(), f.fingerprint, f.category.as_str()],
            )?;
            tx.execute(
                "INSERT INTO content_findings
                   (scan_id, source_kind, source_id, thread_identifier, occurred_at,
                    fingerprint, category, severity, rationale, service, rechecked, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11)",
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
                    f.service,
                    at
                ],
            )?;
        }
        // 3. Checkpoint + progress, in the SAME transaction.
        let recheck_key = format!("{chunk_key}#recheck");
        tx.execute(
            "INSERT INTO chunk_progress (chunk_key, fingerprint, scan_id, status, classified_at)
             VALUES (?1, ?2, ?3, 'done', ?4)
             ON CONFLICT(chunk_key) DO UPDATE SET
               fingerprint = excluded.fingerprint, scan_id = excluded.scan_id,
               status = excluded.status, classified_at = excluded.classified_at",
            params![recheck_key, chunk_fingerprint, scan_id, at],
        )?;
        tx.execute(
            "UPDATE scans SET chunks_done = chunks_done + 1 WHERE id = ?1",
            params![scan_id],
        )?;
        tx.commit()?;
        Ok(())
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
    ///
    /// Findings are counted by SCOPE (the scan's sources + time range), NOT by
    /// which run first classified each chunk. Classification is cached per chunk
    /// across scans, so a re-scan reuses chunks and attributes no new findings to
    /// its own id — counting by scan_id then makes a re-scan of already-covered
    /// data look "Clean". Counting by scope means every scan shows the findings
    /// that fall within it, so two scans over the same data agree.
    pub fn list_scans(&self, limit: i64) -> Result<Vec<ScanListRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.model, s.range_start, s.range_end, s.sources, s.status,
                    s.started_at, s.finished_at,
                    coalesce(count(f.id), 0),
                    coalesce(sum(f.severity = 3), 0),
                    coalesce(sum(f.severity = 2), 0),
                    coalesce(sum(f.severity = 1), 0)
             FROM scans s
             LEFT JOIN content_findings f ON f.stale = 0
                AND (s.sources = 'all'
                     OR ((',' || s.sources || ',') LIKE '%,notes,%'
                         AND f.source_kind = 'note')
                     OR ((',' || s.sources || ',') LIKE '%,messages,%'
                         AND f.source_kind = 'message')
                     OR (f.source_kind = 'message' AND f.service IS NOT NULL
                         AND (',' || s.sources || ',')
                             LIKE ('%,' || f.service || ',%')))
                AND (s.range_start IS NULL OR f.occurred_at IS NULL
                     OR f.occurred_at >= s.range_start)
                AND (s.range_end IS NULL OR f.occurred_at IS NULL
                     OR f.occurred_at <= s.range_end)
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

    /// Store a summary. `digest` fingerprints the findings the text was written
    /// from (see [`Self::find_summary_by_digest`]); pass `""` when there is
    /// nothing to key on (e.g. the fixed clean-scan report).
    pub fn set_summary(
        &self,
        scan_id: i64,
        kind: &str,
        thread_ref: &str,
        content: &str,
        digest: &str,
        at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO summaries
                 (scan_id, kind, thread_ref, content, digest, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![scan_id, kind, thread_ref, content, digest, at],
        )?;
        Ok(())
    }

    /// The most recent summary of `kind`/`thread_ref` written from exactly the
    /// same findings, from ANY scan — the re-scan cache. Summaries are keyed per
    /// scan, but the prose only depends on the findings, so an unchanged thread
    /// can reuse an earlier run's text instead of paying another model call
    /// (#43). An empty `digest` never matches (that's the "unknown" marker on
    /// pre-v6 rows and on the fixed clean report).
    pub fn find_summary_by_digest(
        &self,
        kind: &str,
        thread_ref: &str,
        digest: &str,
    ) -> Result<Option<String>> {
        if digest.is_empty() {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT content FROM summaries
                 WHERE kind = ?1 AND thread_ref = ?2 AND digest = ?3
                 ORDER BY created_at DESC LIMIT 1",
                params![kind, thread_ref, digest],
                |r| r.get(0),
            )
            .optional()?)
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
    // Kept next to the other migration-sensitive tests: every shipped DB is at
    // an older user_version, so the ALTER path — not the fresh CREATE — is what
    // real users run.
    /// How large a findings payload actually gets, for the IPC audit (#65).
    /// Ignored: a measurement, not an assertion. Run with:
    ///
    /// ```text
    /// cargo test -p traceloupe-core findings_payload_size -- --ignored --nocapture
    /// ```
    ///
    /// Synthetic rows sized like real ones (a sentence-length rationale) — no
    /// real backup involved.
    /// Seed `n` findings across two threads and three severities.
    fn seeded_findings(n: usize) -> AnalysisDb {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let rows: Vec<NewFinding> = (0..n)
            .map(|i| NewFinding {
                source_kind: if i % 4 == 3 {
                    SourceKind::Note
                } else {
                    SourceKind::Message
                },
                source_id: Some(i as i64),
                thread_identifier: if i % 4 == 3 {
                    None
                } else {
                    Some(format!("chat{}", i % 2))
                },
                // Deliberate ties: many findings share a severity AND a
                // timestamp, which is what makes the id tie-break load-bearing.
                occurred_at: Some(1_700_000_000 + (i as i64 % 7)),
                fingerprint: format!("fp{i:05}"),
                category: Category::ScamFraud,
                severity: (i % 3 + 1) as u8,
                rationale: "why".into(),
                service: Some("iMessage".into()),
            })
            .collect();
        db.replace_findings(scan, &rows, 1).unwrap();
        db
    }

    #[test]
    fn pages_partition_the_list_exactly_once() {
        // Rows sharing a sort key must not drift between pages — one fetched
        // twice, another never. The seed makes many findings share a severity and
        // a timestamp so the order is decided by the id tie-break rather than by
        // luck. (SQLite's current plan is incidentally stable for these ties, so
        // this asserts the property rather than demonstrating the failure.)
        let db = seeded_findings(97);
        let q = FindingQuery {
            sort: FindingSort::Severity,
            desc: true,
            ..Default::default()
        };
        let whole: Vec<String> = db
            .list_findings_in_scope_page("all", None, None, &q, 0, 1000)
            .unwrap()
            .into_iter()
            .map(|f| f.fingerprint)
            .collect();
        assert_eq!(whole.len(), 97);

        let mut paged = Vec::new();
        for offset in (0..97).step_by(10) {
            paged.extend(
                db.list_findings_in_scope_page("all", None, None, &q, offset, 10)
                    .unwrap()
                    .into_iter()
                    .map(|f| f.fingerprint),
            );
        }
        assert_eq!(paged, whole, "pages must reproduce the full order exactly");
        let unique: std::collections::HashSet<_> = paged.iter().collect();
        assert_eq!(unique.len(), 97, "no row appears on two pages");
    }

    #[test]
    fn the_pills_cannot_promise_rows_the_list_will_not_produce() {
        // Counts and rows come from the same predicate. When they were derived
        // separately — a SQL count against a JavaScript filter — they drifted,
        // which is what #59 was about.
        let db = seeded_findings(60);
        db.set_dismissed("fp00000", Category::ScamFraud, true, 1)
            .unwrap();
        db.set_dismissed("fp00001", Category::ScamFraud, true, 1)
            .unwrap();
        let counts = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(counts.dismissed, 2);
        assert_eq!(counts.live, 58);

        let page = |severity, include_dismissed| {
            db.list_findings_in_scope_page(
                "all",
                None,
                None,
                &FindingQuery {
                    severity,
                    include_dismissed,
                    ..Default::default()
                },
                0,
                1000,
            )
            .unwrap()
            .len() as i64
        };
        assert_eq!(page(None, false), counts.live);
        assert_eq!(page(None, true), counts.live + counts.dismissed);
        assert_eq!(page(Some(3), false), counts.serious);
        assert_eq!(page(Some(2), false), counts.harmful);
        assert_eq!(page(Some(1), false), counts.concerning);
    }

    #[test]
    fn grouping_orders_a_thread_contiguously_with_notes_last() {
        // The grouped view builds its headings from a window, so every finding
        // for a conversation must be adjacent in the total order — otherwise a
        // heading reappears further down the list.
        let db = seeded_findings(40);
        let rows = db
            .list_findings_in_scope_page(
                "all",
                None,
                None,
                &FindingQuery {
                    group_by_thread: true,
                    desc: true,
                    ..Default::default()
                },
                0,
                1000,
            )
            .unwrap();
        let threads: Vec<Option<String>> =
            rows.iter().map(|f| f.thread_identifier.clone()).collect();
        let mut seen = Vec::new();
        for t in &threads {
            if seen.last() != Some(t) {
                assert!(!seen.contains(t), "thread {t:?} appears in two blocks");
                seen.push(t.clone());
            }
        }
        assert_eq!(threads.last().unwrap(), &None, "notes gather at the end");
    }

    #[test]
    #[ignore = "measurement, not an assertion — run with --ignored --nocapture"]
    fn findings_payload_size_by_count() {
        for n in [100usize, 1_000, 8_000] {
            // Approximate the JSON the command serialises: the DTO's fields with
            // realistic lengths. Rationale dominates, so model it honestly.
            let per_row = format!(
                r#"{{"id":123456,"sourceKind":"message","sourceId":987654,"threadId":4242,"threadIdentifier":"+46701234567","service":"iMessage","occurredAt":1700000000,"fingerprint":"{}","category":"harassment-bullying","severity":2,"rationale":"{}","stale":false,"dismissed":false,"rechecked":true}}"#,
                "a".repeat(64),
                "Repeated insults directed at the recipient over several messages.",
            );
            let bytes = per_row.len() * n + n; // + separators
            println!(
                "findings payload: {n} findings -> ~{:.1} KB of JSON",
                bytes as f64 / 1024.0
            );
        }
    }

    #[test]
    fn v5_database_upgrades_and_keeps_its_summaries() {
        use rusqlite::Connection;
        let conn = Connection::open_in_memory().unwrap();
        // A realistic pre-v6 store: the full schema, but `summaries` back in
        // its old shape (no digest) and the version pinned to 5.
        conn.execute_batch(super::SCHEMA_V1).unwrap();
        conn.execute_batch(
            "DROP TABLE summaries;
             CREATE TABLE summaries (
                 scan_id INTEGER NOT NULL REFERENCES scans(id),
                 kind TEXT NOT NULL,
                 thread_ref TEXT NOT NULL DEFAULT '',
                 content TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 PRIMARY KEY (scan_id, kind, thread_ref));
             INSERT INTO scans (id, model, status, started_at)
                 VALUES (1, 'm', 'completed', 100);
             INSERT INTO summaries (scan_id, kind, thread_ref, content, created_at)
                 VALUES (1, 'report', '', 'An older run''s report.', 101);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 5i64).unwrap();

        let db = super::AnalysisDb::init(conn).expect("v5 store must migrate");
        assert_eq!(db.schema_version().unwrap(), super::SCHEMA_VERSION);
        // The pre-existing summary survives the ALTER…
        assert_eq!(
            db.get_summary(1, "report", "").unwrap().as_deref(),
            Some("An older run's report."),
        );
        // …and its blank digest never satisfies a cache lookup, so it simply
        // re-summarizes once instead of being reused for unrelated findings.
        assert_eq!(
            db.find_summary_by_digest("report", "", "any-digest")
                .unwrap(),
            None,
        );
    }

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
            service: Some("iMessage".into()),
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
        // Resume: same row back to running, finish cleared, model PRESERVED
        // (resume must not overwrite a completed cascade's "e2b→e4b" receipt).
        db.resume_scan(scan, "m2").unwrap();
        let row = db.scan_by_id(scan).unwrap().unwrap();
        assert_eq!(row.status, "running");
        assert_eq!(row.finished_at, None);
        assert_eq!(row.model, "m", "resume keeps the recorded model");
        // A completed scan is not resumable, and no second row ever appeared.
        db.finish_scan(scan, ScanStatus::Completed, 200).unwrap();
        assert!(db.resume_scan(scan, "m2").is_err());
        assert_eq!(db.list_scans(50).unwrap().len(), 1);
    }

    #[test]
    fn apply_recheck_overrules_sweep_and_preserves_confirmations() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        // Sweep flagged fp1 (scam) and fp2 (self-harm); fp1 also dismissed.
        db.replace_findings(
            scan,
            &[
                finding("fp1", Category::ScamFraud),
                finding("fp2", Category::SelfHarm),
            ],
            101,
        )
        .unwrap();
        db.set_dismissed("fp1", Category::ScamFraud, true, 102)
            .unwrap();

        // Chunk A (items fp1, fp2): strong tier confirms fp1/scam, clears fp2.
        db.apply_recheck(
            scan,
            "A",
            "fpA",
            &["fp1".into(), "fp2".into()],
            &[finding("fp1", Category::ScamFraud)],
            103,
        )
        .unwrap();
        let rows = db.list_findings(None).unwrap();
        assert_eq!(rows.len(), 1, "fp2 sweep finding overruled and removed");
        assert_eq!(rows[0].fingerprint, "fp1");
        assert!(rows[0].dismissed, "dismissal survives the re-check");
        assert!(db.chunk_is_done("A#recheck", "fpA").unwrap());

        // Chunk B shares item fp1 (overlap) and the strong tier is SILENT on
        // it there — must NOT wipe chunk A's confirmation (rechecked = 1).
        db.apply_recheck(scan, "B", "fpB", &["fp1".into()], &[], 104)
            .unwrap();
        let rows = db.list_findings(None).unwrap();
        assert_eq!(
            rows.len(),
            1,
            "confirmed fp1 survives a sibling window's silence"
        );
        assert_eq!(rows[0].fingerprint, "fp1");
    }

    #[test]
    fn flagged_chunk_keys_are_durable_across_a_recheck_delete() {
        // The cascade re-check set must NOT be recomputed from live findings —
        // a sibling window's re-check can delete a shared item's finding, and
        // if a crash then interrupts, the still-un-re-checked chunk must remain
        // in the set (verification Finding A). The durable `flagged` marker on
        // chunk_progress is what guarantees this.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        // Sweep flagged chunks X and Y (both produced a finding).
        db.record_chunk(scan, "X", "fpX", ChunkStatus::Done, true, 101)
            .unwrap();
        db.record_chunk(scan, "Y", "fpY", ChunkStatus::Done, true, 101)
            .unwrap();
        db.replace_findings(scan, &[finding("shared", Category::ScamFraud)], 101)
            .unwrap();
        assert_eq!(
            db.flagged_chunk_keys().unwrap(),
            ["X".to_string(), "Y".to_string()].into_iter().collect()
        );
        // Y is re-checked and the strong tier clears the shared finding.
        db.apply_recheck(scan, "Y", "fpY", &["shared".into()], &[], 102)
            .unwrap();
        assert!(db.list_findings(None).unwrap().is_empty());
        // The re-check set is UNCHANGED — X is still flagged, so a resume here
        // re-checks it (its #recheck checkpoint was never written).
        assert_eq!(
            db.flagged_chunk_keys().unwrap(),
            ["X".to_string(), "Y".to_string()].into_iter().collect()
        );
        assert!(db.chunk_is_done("Y#recheck", "fpY").unwrap());
        assert!(!db.chunk_is_done("X#recheck", "fpX").unwrap());
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
    fn findings_are_scoped_not_owned_by_scan_id() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        // Scan A (all sources) classifies one MESSAGE and one NOTE finding.
        let a = db.begin_scan("m", (None, None), "all", 100).unwrap();
        let msg = finding("fp-msg", Category::ScamFraud); // message, @1_700_000_000
        let mut note = finding("fp-note", Category::SelfHarm);
        note.source_kind = SourceKind::Note;
        note.thread_identifier = None;
        note.occurred_at = Some(1_700_000_500);
        db.replace_findings(a, &[msg, note], 101).unwrap();

        // Scan B (notes only) runs later; its chunks were reused from A, so it
        // owns NO finding rows of its own — the bug this scoping fixes.
        let b = db.begin_scan("m", (None, None), "notes", 200).unwrap();

        // Scope surfaces findings by sources, regardless of which scan owns them.
        let in_b = db.list_findings_in_scope("notes", None, None).unwrap();
        assert_eq!(in_b.len(), 1);
        assert_eq!(in_b[0].fingerprint, "fp-note");
        let msgs = db.list_findings_in_scope("messages", None, None).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].fingerprint, "fp-msg");

        // list_scans counts by scope: B (notes) shows the note, A (all) shows
        // both — even though scan A owns every finding row.
        let rows = db.list_scans(50).unwrap();
        assert_eq!(rows.iter().find(|r| r.id == a).unwrap().findings, 2);
        assert_eq!(rows.iter().find(|r| r.id == b).unwrap().findings, 1);

        // A time range that excludes both keeps them out of scope.
        let none = db
            .list_findings_in_scope("all", Some(1_800_000_000), None)
            .unwrap();
        assert_eq!(none.len(), 0);
    }

    /// The count the live progress bar reports must equal what the Findings
    /// panel renders: the same scope predicate, minus dismissed and stale (#59).
    #[test]
    fn scope_count_matches_what_the_panel_renders() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let a = db.begin_scan("m", (None, None), "all", 100).unwrap();
        let msg = finding("fp-msg", Category::ScamFraud); // iMessage, @1_700_000_000
        let mut note = finding("fp-note", Category::SelfHarm);
        note.source_kind = SourceKind::Note;
        note.thread_identifier = None;
        note.service = None;
        note.occurred_at = Some(1_700_000_500);
        let mut undated = finding("fp-undated", Category::ThreatViolence);
        undated.source_kind = SourceKind::Note;
        undated.occurred_at = None;
        undated.service = None;
        db.replace_findings(a, &[msg, note, undated], 101).unwrap();

        // Every scope the panel can ask for, counted the same way it lists.
        for (sources, start, end) in [
            ("all", None, None),
            ("notes", None, None),
            ("messages", None, None),
            ("iMessage", None, None),
            ("iMessage,notes", None, None),
            ("tiktok", None, None),
            ("all", Some(1_700_000_400), None),
            ("all", Some(1_800_000_000), None),
            ("all", None, Some(1_600_000_000)),
        ] {
            assert_eq!(
                db.count_findings_in_scope(sources, start, end).unwrap(),
                db.list_findings_in_scope(sources, start, end)
                    .unwrap()
                    .iter()
                    .filter(|f| !f.dismissed && !f.stale)
                    .count(),
                "scope ({sources}, {start:?}, {end:?}) must count what it lists",
            );
        }
        // Sanity: 'all' really does see all three, undated note included.
        assert_eq!(db.count_findings_in_scope("all", None, None).unwrap(), 3);

        // Dismissed and stale drop out — of both the count and the panel.
        db.set_dismissed("fp-msg", Category::ScamFraud, true, 102)
            .unwrap();
        db.set_stale("fp-note", true).unwrap();
        assert_eq!(db.count_findings_in_scope("all", None, None).unwrap(), 1);
        assert_eq!(
            db.list_findings_in_scope("all", None, None)
                .unwrap()
                .iter()
                .filter(|f| !f.dismissed && !f.stale)
                .count(),
            1
        );

        // Re-classifying already-flagged content transfers ownership to a new
        // scan (replace_findings deletes + re-inserts) — the scope count must
        // not move, and must not depend on who owns the row.
        let b = db.begin_scan("m", (None, None), "all", 200).unwrap();
        db.replace_findings(b, &[finding("fp-msg", Category::ScamFraud)], 201)
            .unwrap();
        assert_eq!(db.count_findings_in_scope("all", None, None).unwrap(), 1);
    }

    #[test]
    fn chunk_resume_is_fingerprint_sensitive() {
        let db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.record_chunk(scan, "thread1:0", "abc", ChunkStatus::Done, false, 101)
            .unwrap();
        assert!(db.chunk_is_done("thread1:0", "abc").unwrap());
        // Content changed → chunk must be re-classified.
        assert!(!db.chunk_is_done("thread1:0", "def").unwrap());
        // Skipped chunks never count as done.
        db.record_chunk(scan, "thread1:1", "xyz", ChunkStatus::Skipped, false, 102)
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
        db.record_chunk(scan, "k", "fp", ChunkStatus::Done, false, 101)
            .unwrap();
        db.audit(scan, 101, "chunk_classified", "chunk=k verdicts=0")
            .unwrap();
        assert!(db.finish_scan(scan, ScanStatus::Running, 102).is_err());
        db.finish_scan(scan, ScanStatus::Completed, 103).unwrap();
        db.set_summary(scan, "report", "", "Nothing flagged.", "", 104)
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
        db.record_chunk(scan, "k", "fp", ChunkStatus::Done, false, 101)
            .unwrap();
        db.audit(scan, 101, "chunk_classified", "chunk=k").unwrap();
        db.set_summary(scan, "report", "", "Nothing flagged.", "", 104)
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
                service: Some("iMessage".into()),
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

    // ---- report charts (#66) -------------------------------------------------

    /// Seed findings at chosen instants, one per timestamp.
    fn dated(db: &mut AnalysisDb, scan: i64, at: &[Option<i64>]) {
        let rows: Vec<NewFinding> = at
            .iter()
            .enumerate()
            .map(|(i, t)| NewFinding {
                occurred_at: *t,
                fingerprint: format!("t{i:05}"),
                ..finding(&format!("t{i:05}"), Category::ScamFraud)
            })
            .collect();
        db.replace_findings(scan, &rows, 1).unwrap();
    }

    /// 2024-03-11 is a Monday; 2024-03-17 the Sunday that closes its week.
    const MON_2024_03_11: i64 = 1_710_115_200;
    const DAY: i64 = 86_400;

    #[test]
    fn charts_count_every_finding_not_the_page_the_report_renders() {
        // The trap #66 names: the panel renders at most 500 rows and the
        // narrative at most 100. A chart built from either would describe a
        // subset while looking like it described the scan.
        let db = seeded_findings(600);
        let q = FindingQuery::default();
        let page = db
            .list_findings_in_scope_page("all", None, None, &q, 0, 500)
            .unwrap();
        let a = db.finding_analytics("all", None, None, &q).unwrap();

        assert_eq!(page.len(), 500, "page is capped");
        assert_eq!(a.charted, 600, "charts are not");
        let by_cat: i64 = a.by_category.iter().map(|b| b.total()).sum();
        let by_conv: i64 = a.by_conversation.iter().map(|b| b.total()).sum();
        let over_time: i64 = a.over_time.iter().map(|b| b.total()).sum();
        assert_eq!(by_cat, 600);
        assert_eq!(by_conv + a.other_conversation_findings, 600);
        assert_eq!(over_time, 600 - a.undated);
    }

    #[test]
    fn the_bucket_unit_follows_the_span_the_findings_cover() {
        for (span_days, want, bars) in [
            (10_i64, TimeUnit::Day, 11_usize),
            (120, TimeUnit::Week, 18),
            (900, TimeUnit::Month, 31),
            (2500, TimeUnit::Quarter, 28),
            (7000, TimeUnit::Year, 20),
        ] {
            let mut db = AnalysisDb::open_in_memory().unwrap();
            let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
            // Twenty findings spread evenly across the span, plus its endpoint.
            let mut at: Vec<Option<i64>> = (0..20)
                .map(|i| Some(MON_2024_03_11 + i * span_days * DAY / 20))
                .collect();
            at.push(Some(MON_2024_03_11 + span_days * DAY));
            dated(&mut db, scan, &at);

            let a = db
                .finding_analytics("all", None, None, &FindingQuery::default())
                .unwrap();
            assert_eq!(a.unit, want, "span of {span_days} days");
            // The point of adapting: the axis stays readable at every range.
            assert!(
                a.over_time.len() <= bars,
                "{span_days} days produced {} bars",
                a.over_time.len()
            );
            assert!(!a.over_time.is_empty());
        }
    }

    #[test]
    fn a_week_bucket_runs_monday_through_sunday() {
        // `weekday 0, -6 days` is exactly the idiom that lands a week early when
        // the finding itself falls on a Monday.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let mut at: Vec<Option<i64>> = (0..7)
            .map(|d| Some(MON_2024_03_11 + d * DAY + 12 * 3600))
            .collect();
        // …and one in the following week, which must not join them.
        at.push(Some(MON_2024_03_11 + 7 * DAY + 12 * 3600));
        // Force week bucketing by spanning ~4 months.
        at.push(Some(MON_2024_03_11 + 120 * DAY));
        dated(&mut db, scan, &at);

        let a = db
            .finding_analytics("all", None, None, &FindingQuery::default())
            .unwrap();
        assert_eq!(a.unit, TimeUnit::Week);
        let first = &a.over_time[0];
        assert_eq!(first.key, "2024-03-11", "the week is keyed by its Monday");
        assert_eq!(first.total(), 7, "Monday through Sunday, and no more");
        assert_eq!(a.over_time[1].key, "2024-03-18");
    }

    #[test]
    fn undated_findings_are_left_off_the_axis_and_counted_out_loud() {
        // They cannot be placed on a timeline. Dropping them quietly would leave
        // the chart's total disagreeing with the list's, which is how a reader
        // learns to distrust both.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        dated(
            &mut db,
            scan,
            &[
                Some(MON_2024_03_11),
                Some(MON_2024_03_11 + DAY),
                None,
                None,
                None,
            ],
        );

        let a = db
            .finding_analytics("all", None, None, &FindingQuery::default())
            .unwrap();
        assert_eq!(a.charted, 5);
        assert_eq!(a.undated, 3);
        assert_eq!(a.over_time.iter().map(|b| b.total()).sum::<i64>(), 2);
        // The other charts keep them: only the time axis can't place them.
        assert_eq!(a.by_category.iter().map(|b| b.total()).sum::<i64>(), 5);
    }

    #[test]
    fn every_bar_splits_confirmed_from_unconfirmed() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let rows: Vec<NewFinding> = (0..6)
            .map(|i| NewFinding {
                severity: (i % 3 + 1) as u8,
                occurred_at: Some(MON_2024_03_11 + i),
                ..finding(&format!("c{i}"), Category::ThreatViolence)
            })
            .collect();
        db.replace_findings(scan, &rows, 1).unwrap();
        // The cascade confirmed the first three.
        db.conn()
            .execute(
                "UPDATE content_findings SET rechecked = 1
                 WHERE fingerprint IN ('c0','c1','c2')",
                [],
            )
            .unwrap();

        let a = db
            .finding_analytics("all", None, None, &FindingQuery::default())
            .unwrap();
        let cat = &a.by_category[0];
        // c0/c3 are severity 1, c1/c4 severity 2, c2/c5 severity 3 — one
        // confirmed and one unconfirmed in each band.
        assert_eq!(cat.confirmed, [1, 1, 1]);
        assert_eq!(cat.unconfirmed, [1, 1, 1]);
        assert_eq!(cat.total(), 6);
    }

    #[test]
    fn dismissals_leave_the_charts_but_stay_in_the_disclosure() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let rows: Vec<NewFinding> = (0..4)
            .map(|i| NewFinding {
                occurred_at: Some(MON_2024_03_11 + i),
                ..finding(&format!("d{i}"), Category::SelfHarm)
            })
            .collect();
        db.replace_findings(scan, &rows, 1).unwrap();
        db.set_dismissed("d0", Category::SelfHarm, true, 2).unwrap();

        let a = db
            .finding_analytics("all", None, None, &FindingQuery::default())
            .unwrap();
        assert_eq!(a.charted, 3, "the dismissed one is not drawn");
        assert_eq!(a.dismissed, 1, "but the report can say so");
        assert_eq!(a.by_category[0].total(), 3);
    }

    #[test]
    fn the_conversation_chart_is_capped_and_says_what_it_folded() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        // 20 conversations; the busiest get progressively more findings, so the
        // ranking is unambiguous.
        let mut rows = Vec::new();
        for c in 0..20u32 {
            for n in 0..=c {
                rows.push(NewFinding {
                    thread_identifier: Some(format!("chat{c:02}")),
                    occurred_at: Some(MON_2024_03_11 + i64::from(n)),
                    ..finding(&format!("k{c:02}-{n:02}"), Category::ScamFraud)
                });
            }
        }
        let total: i64 = rows.len() as i64;
        db.replace_findings(scan, &rows, 1).unwrap();

        let a = db
            .finding_analytics("all", None, None, &FindingQuery::default())
            .unwrap();
        assert_eq!(a.by_conversation.len(), CONVERSATION_CHART_CAP);
        assert_eq!(a.by_conversation[0].key, "chat19", "busiest first");
        assert_eq!(a.other_conversations, 20 - CONVERSATION_CHART_CAP as i64);
        // Nothing is lost between the bars and the remainder.
        let drawn: i64 = a.by_conversation.iter().map(|b| b.total()).sum();
        assert_eq!(drawn + a.other_conversation_findings, total);
        assert_eq!(a.charted, total);
    }

    #[test]
    fn charts_and_list_answer_to_the_same_filter() {
        // filtered_scope() is shared on purpose: a chart that counted a different
        // population than the list beneath it is #59 all over again.
        let db = seeded_findings(90);
        for q in [
            FindingQuery::default(),
            FindingQuery {
                severity: Some(3),
                ..Default::default()
            },
            FindingQuery {
                include_dismissed: true,
                ..Default::default()
            },
            FindingQuery {
                exclude_stale: true,
                severity: Some(1),
                ..Default::default()
            },
        ] {
            let listed = db.count_findings_matching("all", None, None, &q).unwrap();
            let charted = db.finding_analytics("all", None, None, &q).unwrap().charted;
            assert_eq!(listed, charted, "{q:?}");
        }
    }
}
