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

const SCHEMA_VERSION: i64 = 16;

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
/// The severity tier the default view hides. ONE definition: the page query,
/// its matching count, and the live progress counter must all agree about what
/// a reviewer will see, and separately-derived predicates are exactly what
/// drifted in #59.
const LOW_SEVERITY_EXPR: &str = "f.severity < 2";

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
    // An explicit severity filter beats the floor: asking for "concerning"
    // and getting nothing back would be a UI that argues with itself.
    if !q.include_low && q.severity.is_none() {
        w.push_str(&format!(" AND NOT ({LOW_SEVERITY_EXPR})"));
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

/// The ORDER BY a page of findings uses, without LIMIT/OFFSET.
///
/// **Defined once**, because both a page and a finding's *rank within* that page
/// order have to agree exactly. Returning to a specific finding (#224) means
/// counting how many rows sort before it, and a rank computed under even
/// slightly different ordering scrolls to the wrong row — the same "one rule,
/// two copies" mistake that #218 was.
///
/// Built from enums and integers, never from caller text.
fn order_by(q: &FindingQuery) -> String {
    let dir = if q.desc { "DESC" } else { "ASC" };
    let mut tail = String::from("ORDER BY ");
    if q.group_by_thread {
        // Notes have no thread; they gather at the end under their own heading,
        // which is where the grouped view has always put them.
        tail.push_str("f.thread_identifier IS NULL, f.thread_identifier, ");
    }
    match q.sort {
        FindingSort::Severity => {
            // Severity first, recency inside a band.
            tail.push_str(&format!("f.severity {dir}, f.occurred_at {dir}"));
        }
        FindingSort::Date => tail.push_str(&format!("f.occurred_at {dir}")),
    }
    // A total order, so paging is well-defined. Findings routinely share a
    // severity AND a timestamp, and SQL leaves ties unspecified — today's plan
    // happens to return them by rowid, but an index over (severity,
    // occurred_at) would be enough to change that, and then a row sits on two
    // pages while another sits on none.
    tail.push_str(&format!(", f.id {dir}"));
    tail
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
    /// Show severity-1 ("concerning") findings, which the default view hides.
    ///
    /// Measured on both tiers, EVERY false alarm the classifier produced on
    /// ordinary conversation was severity 1, and no labelled positive in the
    /// fixture set expects severity 1 — so this tier is where the noise lives
    /// and none of the signal does
    /// (docs/validation/safety-scan-validation.md). They are hidden, never
    /// deleted: a reviewer who wants maximum sensitivity turns them back on,
    /// and the count is always shown so the choice is visible.
    pub include_low: bool,
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
    /// Not dismissed AND at or above the severity floor — exactly the default
    /// view. These numbers are what the pills promise, so they must not count
    /// rows the list then refuses to produce; `concerning` below is the
    /// severity-1 tier the floor hides, and is what the "show them" affordance
    /// is labelled with.
    pub live: i64,
    /// Not dismissed AND not stale — what the printable report includes, so its
    /// "N more not shown" line can be computed without fetching everything.
    pub live_fresh: i64,
    /// Dismissed AND at or above the floor — what "Show dismissed" will
    /// actually produce, for the same reason `live` is floored.
    pub dismissed: i64,
    /// Live, not-stale findings whose flagged text has never been revealed —
    /// the app's unread count. Dismissing implies reading (the control lives
    /// inside the expansion), so a dismissed finding is never unread.
    pub unread: i64,
    pub serious: i64,
    pub harmful: i64,
    pub concerning: i64,
}

/// How many conversations the by-conversation chart draws before folding the
/// rest into a stated remainder.
pub const CONVERSATION_CHART_CAP: usize = 12;

/// The earliest instant a finding can honestly claim: 2007-01-01, before which
/// no iPhone existed to send the message.
///
/// This is what BOUNDS the time chart. The bucket unit is chosen from the span
/// the findings cover, and `Year` is the coarsest unit there is — so a single
/// finding carrying a decoded-wrong timestamp (Apple stores seconds since 2001;
/// read as Unix time that lands in 1970, and a zeroed field lands there too)
/// would stretch the axis across half a century and squash every real finding
/// into the last bar. Findings outside the window are counted as undatable
/// alongside the ones with no timestamp at all, which is what they are.
///
/// With the window closed, the axis can never exceed ~40 bars: the span is at
/// most 2007→now, which selects `Year`, which yields one bar per year.
pub const TIMELINE_START: i64 = 1_167_609_600;

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
    /// In scope but impossible to place on a timeline — no timestamp, or one
    /// outside [`TIMELINE_START`]..now. Absent from [`Self::over_time`] and
    /// present everywhere else.
    pub undated: i64,
    /// How many findings the charts LEFT OUT as false positives, under the
    /// caller's own filter — zero when the caller asked for dismissed findings,
    /// because then nothing was left out. It is what the disclosure beside the
    /// charts claims, so it has to mean exactly that.
    pub dismissed: i64,
}

const DISMISSED_EXPR: &str = "EXISTS(SELECT 1 FROM finding_verdicts v
                WHERE v.fingerprint = f.fingerprint AND v.category = f.category
                  AND v.verdict = 'dismissed')";

const IN_SCOPE_PREDICATE: &str = "(
     -- One conversation. EQUALITY, not LIKE: a thread identifier is a phone
     -- number or an email and may contain SQL wildcards ('_' in an address
     -- matches any character), so pattern-matching it would silently pull in
     -- other people's conversations.
     (?1 LIKE 'thread:%' AND f.source_kind = 'message'
      AND f.thread_identifier = substr(?1, 8))
     OR (?1 NOT LIKE 'thread:%' AND (?1 = 'all'
     OR ((',' || ?1 || ',') LIKE '%,notes,%' AND f.source_kind = 'note')
     OR ((',' || ?1 || ',') LIKE '%,messages,%' AND f.source_kind = 'message')
     OR (f.source_kind = 'message' AND f.service IS NOT NULL
         AND (',' || ?1 || ',') LIKE ('%,' || f.service || ',%')))))
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
    chunks_done  INTEGER NOT NULL DEFAULT 0,
    -- Why a run ended the way it did, when that needs saying (v9). Only 'failed'
    -- carries one: cancelled and interrupted explain themselves. Without it the
    -- history could say a scan failed and nothing more, which is what the user
    -- could already see.
    error        TEXT,
    -- Triage coverage (v16), NULL for batch scans and for triage scans made
    -- before this column existed. A triage scan reads a RANKED SUBSET in
    -- depth, so the honest report has to say how much it read and how much it
    -- did not — otherwise a budget or a Stop leaves silence that reads as
    -- "clean". These are the TriageOutcome counters, stored because the live
    -- event that carried them is gone the moment the scan ends.
    censused     INTEGER,
    candidates   INTEGER,
    deep_scanned INTEGER,
    unconfirmed  INTEGER
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
    sender            TEXT,                 -- who said it ('me' for the device owner);
                                            -- NULL on rows written before v11
    content_key       TEXT,                 -- normalized identity of SHORT content
                                            -- (safety_scan::content_key); NULL when the
                                            -- text is too long to recur, so a content
                                            -- rule can never match it
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
-- What the USER decided about a finding, and whether they have read it.
-- Keyed by (fingerprint, category) — NOT finding row id — so a verdict survives
-- re-scans and re-imports (plan T8 AC).
--
-- One table rather than "dismissals" plus a "seen" sibling: two places
-- answering "what happened to this finding" is the shape that produced the two
-- REPORT_FINDINGS_CAP constants and the mock that drifted from the SQL.
CREATE TABLE IF NOT EXISTS finding_verdicts (
    fingerprint TEXT NOT NULL,
    category    TEXT NOT NULL,
    verdict     TEXT,                          -- 'dismissed' | NULL
    reason      TEXT,                          -- why, when dismissed
    seen_at     INTEGER,                       -- NULL = nobody has looked
    -- Who put this row here. NULL = nobody decided anything, the finding was
    -- merely READ (mark_seen). 'person' = a decision, which no rule may
    -- overrule and no rule removal may undo. 'rule' = a standing rule, which
    -- removing that rule takes back.
    --
    -- A boolean cannot express this: a NULL verdict means "merely read" after
    -- mark_seen but "explicitly kept" after set_verdict, and a rule must cover
    -- the first and never the second.
    origin      TEXT,
    at          INTEGER NOT NULL,
    PRIMARY KEY (fingerprint, category)
);
CREATE INDEX IF NOT EXISTS idx_verdict ON finding_verdicts(verdict);

-- Standing "this is fine" rules: a dismissal the user chose to apply to a whole
-- conversation or category rather than one finding.
--
-- Scope is deliberately NOT "sender": a finding carries thread_identifier,
-- category and service, but the sender lives on the message in cache.db — a
-- different database. For a one-to-one chat "this conversation" is the same
-- thing; for a group it is broader, which the UI says out loud.
--
-- `category` bounds the rule to ONE Forensic 9 category (#394). Every rule made
-- from v10 on sets it. NULL means a rule from before, when a conversation rule
-- silenced every category at every severity — kept at that breadth because
-- which category the user was looking at when they clicked was never recorded,
-- and deleting the rule would resurface findings they deliberately set aside.
-- The rules panel labels those so they can be re-made deliberately.
--
-- UNIQUE includes category, so one conversation carries one rule per category.
CREATE TABLE IF NOT EXISTS suppressions (
    id         INTEGER PRIMARY KEY,
    -- 'thread' | 'category' | 'content+sender' | 'content+any'.
    -- For the content scopes `value` is a normalized content key, not a thread.
    scope      TEXT NOT NULL,
    value      TEXT NOT NULL,
    category   TEXT,                           -- NULL = pre-v10, every category
    sender     TEXT NOT NULL DEFAULT '',       -- '' = any sender; else the handle
    reason     TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(scope, value, category, sender)
);

-- Triage census (#459): one cheap embedding score per message, so the scan can
-- rank where to spend the expensive classifier. `sender` is copied in because
-- the unit of inference is (conversation, sender), not the conversation — a
-- group chat with one abuser and nine ordinary people averages to nothing but
-- stratifies to a clear signal.
--
-- `score` is the max cosine similarity to any selected-category prototype:
-- higher = more like known harm. It is advisory, never a finding.
CREATE TABLE IF NOT EXISTS census (
    source_id         INTEGER NOT NULL,      -- messages.id
    thread_identifier TEXT NOT NULL,
    sender            TEXT NOT NULL,         -- '' when unknown
    occurred_at       INTEGER,
    score             REAL NOT NULL,
    embedded_at       INTEGER NOT NULL,
    -- The message's durable identity (chunker::message_fingerprint). Row ids
    -- are cache-local and change on re-import while this store survives it, so
    -- a census row is only trusted for a message whose fingerprint still
    -- matches; '' (pre-v15 rows) never matches and is re-embedded.
    fingerprint       TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (source_id)
);
CREATE INDEX IF NOT EXISTS idx_census_cell
    ON census(thread_identifier, sender);

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
    /// Who sent the message — `me` for the device owner, otherwise the handle
    /// the parser resolved. `None` for notes and for rows written before v11.
    ///
    /// The chunker has always had this (`ChunkItem::sender`); it was simply
    /// dropped when the finding was built. Without it a group-chat finding
    /// cannot say who spoke, and a rule cannot mean "from this person" —
    /// which is why the schema comment used to record sender scope as
    /// impossible (#402).
    pub sender: Option<String>,
    /// Normalized identity of the flagged text, from
    /// `safety_scan::content_key`. `None` when the text is too long to recur,
    /// which is exactly when no content rule should ever match it (#404).
    pub content_key: Option<String>,
}

/// A standing rule as the panel shows it, including what it is currently
/// swallowing.
#[derive(Debug, Clone)]
pub struct SuppressionRow {
    pub scope: String,
    pub value: String,
    /// `None` for a pre-v10 rule covering every category.
    pub category: Option<String>,
    /// `""` when the rule is not bound to one person.
    pub sender: String,
    pub reason: Option<String>,
    /// Live count of findings this rule is dismissing right now.
    pub hits: i64,
}

/// One message's census score to insert.
#[derive(Debug, Clone)]
pub struct CensusRow {
    pub source_id: i64,
    pub thread_identifier: String,
    pub sender: String,
    pub occurred_at: Option<i64>,
    pub score: f64,
    /// The message's durable identity — what makes the row survivable across
    /// re-imports (the id alone is not; see the census schema comment).
    pub fingerprint: String,
}

/// A ranked triage cell: one (conversation, sender) pair and its evidence.
#[derive(Debug, Clone)]
pub struct TriageCell {
    pub thread_identifier: String,
    pub sender: String,
    pub total: i64,
    /// Messages at or above the hot threshold — the primary rank key.
    pub hot: i64,
    pub mean: f64,
    pub peak: f64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// Score slope per day; positive = escalating over time.
    pub trajectory: f64,
}

/// One message the triage worklist has selected for focused classification,
/// with the cell evidence that ranked it.
#[derive(Debug, Clone)]
pub struct WorkItem {
    pub source_id: i64,
    pub thread_identifier: String,
    pub sender: String,
    pub score: f64,
    /// Hot-message count of this message's (thread, sender) cell — why it ranks
    /// where it does.
    pub cell_hot: i64,
    /// The scored message's identity when it was embedded. The deep-scan only
    /// acts on this item if the message the id resolves to STILL has this
    /// fingerprint — otherwise the row predates a re-import and points at
    /// whatever message inherited the id.
    pub fingerprint: String,
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
    /// Who sent it; `None` for notes and pre-v11 rows.
    pub sender: Option<String>,
    /// Normalized identity of the flagged text; `None` when it is too long to
    /// recur, or on pre-v12 rows. No content rule can match a `None`.
    pub content_key: Option<String>,
    pub stale: bool,
    pub dismissed: bool,
    /// 1 = confirmed by the cascade's strong tier (E4B re-checked and kept it);
    /// 0 = seen only by the fast sweep tier (E2B), unconfirmed.
    pub rechecked: bool,
    pub created_at: i64,
}

/// What a triage scan read, and what it left unread — the honest coverage a
/// ranked-subset scan owes its reader. `unscanned` is derived rather than
/// stored, so the two can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriageCoverage {
    pub censused: usize,
    pub candidates: usize,
    pub deep_scanned: usize,
    pub unconfirmed: usize,
}

impl TriageCoverage {
    /// Candidates a budget or a stop left unread.
    pub fn unscanned(&self) -> usize {
        self.candidates.saturating_sub(self.deep_scanned)
    }
}

/// One row of the `scans` table (see SCHEMA_V1 for column semantics).
#[derive(Debug, Clone)]
pub struct ScanRow {
    /// Why a failed run failed. `None` for every other status.
    pub error: Option<String>,
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
    /// Triage coverage; `None` for batch scans and pre-v16 rows.
    pub coverage: Option<TriageCoverage>,
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
    /// Why a failed run failed — what the history's warning badge says on hover.
    pub error: Option<String>,
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

/// Whether finding `f` falls inside scan `s`'s scope — its sources and time range.
///
/// **Defined once, on purpose.** This decides both what a scan *counts* and what
/// survives when a scan is *deleted*, and having those written separately is
/// exactly how #218 happened: counting moved to scope while deletion stayed keyed
/// on `scan_id`, so removing an old scan destroyed findings that newer scans were
/// still displaying. Two copies of a rule are a rule that will disagree with
/// itself.
///
/// Requires the aliases `s` (scans) and `f` (content_findings).
const SCOPE_PREDICATE: &str = "
    (s.sources = 'all'
     OR ((',' || s.sources || ',') LIKE '%,notes,%' AND f.source_kind = 'note')
     OR ((',' || s.sources || ',') LIKE '%,messages,%' AND f.source_kind = 'message')
     OR (f.source_kind = 'message' AND f.service IS NOT NULL
         AND (',' || s.sources || ',') LIKE ('%,' || f.service || ',%')))
    AND (s.range_start IS NULL OR f.occurred_at IS NULL OR f.occurred_at >= s.range_start)
    AND (s.range_end IS NULL OR f.occurred_at IS NULL OR f.occurred_at <= s.range_end)
";

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
            // v7: what the user decided about a finding, plus whether they
            // have read it, in one place (#169). The old `dismissals` table
            // folds in; see the schema comment for why it is one table.
            let has_verdicts: bool = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'finding_verdicts'",
                [],
                |r| r.get::<_, i64>(0),
            )? > 0;
            if !has_verdicts {
                conn.execute_batch(
                    "CREATE TABLE finding_verdicts (
                         fingerprint TEXT NOT NULL,
                         category    TEXT NOT NULL,
                         verdict     TEXT,
                         reason      TEXT,
                         seen_at     INTEGER,
                         at          INTEGER NOT NULL,
                         PRIMARY KEY (fingerprint, category)
                     );
                     CREATE INDEX IF NOT EXISTS idx_verdict
                        ON finding_verdicts(verdict);",
                )?;
                let had_dismissals: bool = conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'dismissals'",
                    [],
                    |r| r.get::<_, i64>(0),
                )? > 0;
                if had_dismissals {
                    // A past dismissal carries dismissed_at across as seen_at:
                    // the user decided something about it, which is truer than
                    // calling every historical verdict unread.
                    conn.execute_batch(
                        "INSERT OR IGNORE INTO finding_verdicts
                             (fingerprint, category, verdict, seen_at, at)
                         SELECT fingerprint, category, 'dismissed',
                                dismissed_at, dismissed_at
                         FROM dismissals;
                         DROP TABLE dismissals;",
                    )?;
                }
            }
            // v8: standing suppression rules (#169). See the schema comment for
            // why the scopes are conversation and category rather than sender.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS suppressions (
                     id         INTEGER PRIMARY KEY,
                     scope      TEXT NOT NULL,
                     value      TEXT NOT NULL,
                     reason     TEXT,
                     created_at INTEGER NOT NULL,
                     UNIQUE(scope, value)
                 );",
            )?;
            // v9: why a failed run failed (#171). The engine had the error in
            // hand and dropped it.
            let has_error = conn
                .prepare("PRAGMA table_info(scans)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "error");
            if !has_error {
                conn.execute("ALTER TABLE scans ADD COLUMN error TEXT", [])?;
            }
            // v10: a rule is scoped to a CATEGORY as well (#394). Before this,
            // the thread arm of `apply_suppressions` joined on the thread alone,
            // so "this conversation is fine" pre-dismissed every future finding
            // of every category from that number.
            //
            // A rebuild, not an ALTER: the old UNIQUE(scope, value) is exactly
            // what has to go — it would forbid the second rule on the same
            // conversation for a different category, which is the whole point.
            let has_category = conn
                .prepare("PRAGMA table_info(suppressions)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "category");
            if !has_category {
                // Existing rules carry NULL = "every category", the breadth they
                // were created with. Narrowing them is impossible — which
                // category the user was looking at when they clicked was never
                // recorded — and deleting them would resurface findings someone
                // deliberately set aside. So they are grandfathered, and the
                // rules panel labels them so they can be re-made deliberately.
                conn.execute_batch(
                    "CREATE TABLE suppressions_v10 (
                         id         INTEGER PRIMARY KEY,
                         scope      TEXT NOT NULL,
                         value      TEXT NOT NULL,
                         category   TEXT,
                         reason     TEXT,
                         created_at INTEGER NOT NULL,
                         UNIQUE(scope, value, category)
                     );
                     INSERT INTO suppressions_v10
                         (id, scope, value, category, reason, created_at)
                     SELECT id, scope, value, NULL, reason, created_at
                     FROM suppressions;
                     DROP TABLE suppressions;
                     ALTER TABLE suppressions_v10 RENAME TO suppressions;",
                )?;
            }
            // v11: who sent the flagged message (#402). The chunker always had
            // it; the finding simply never carried it, so a group-chat finding
            // could not say who spoke and no rule could mean "from this person".
            // Existing rows stay NULL — the sender was never recorded and cannot
            // be reconstructed here — and NULL must never match a sender-scoped
            // rule, or an unknown sender would silently inherit someone else's.
            let has_sender = conn
                .prepare("PRAGMA table_info(content_findings)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "sender");
            if !has_sender {
                conn.execute("ALTER TABLE content_findings ADD COLUMN sender TEXT", [])?;
            }
            // v12: content-scoped rules (#404). Two pieces — the identity a
            // rule matches on, stored per finding so the match is a plain
            // indexed comparison, and the sender a rule is bounded to.
            let has_key = conn
                .prepare("PRAGMA table_info(content_findings)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "content_key");
            if !has_key {
                // Existing findings keep NULL: the flagged text is not in this
                // database, so the key cannot be recomputed here. A content
                // rule simply never matches them, which is the safe direction.
                conn.execute(
                    "ALTER TABLE content_findings ADD COLUMN content_key TEXT",
                    [],
                )?;
            }
            let has_supp_sender = conn
                .prepare("PRAGMA table_info(suppressions)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "sender");
            if !has_supp_sender {
                // Rebuild again: the UNIQUE has to grow a column, and existing
                // thread/category rules are sender-agnostic ('').
                conn.execute_batch(
                    "CREATE TABLE suppressions_v12 (
                         id         INTEGER PRIMARY KEY,
                         scope      TEXT NOT NULL,
                         value      TEXT NOT NULL,
                         category   TEXT,
                         sender     TEXT NOT NULL DEFAULT '',
                         reason     TEXT,
                         created_at INTEGER NOT NULL,
                         UNIQUE(scope, value, category, sender)
                     );
                     INSERT INTO suppressions_v12
                         (id, scope, value, category, sender, reason, created_at)
                     SELECT id, scope, value, category, '', reason, created_at
                     FROM suppressions;
                     DROP TABLE suppressions;
                     ALTER TABLE suppressions_v12 RENAME TO suppressions;",
                )?;
            }
            // v13: which verdicts a rule made (#406), so removing a rule can
            // take back exactly what it dismissed and nothing a person decided.
            let has_by_rule = conn
                .prepare("PRAGMA table_info(finding_verdicts)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "origin");
            if !has_by_rule {
                // Verdicts written before this stay 0 — indistinguishable from a
                // hand decision now, so they are treated as one. Conservative:
                // removing an old rule leaves its old dismissals in place rather
                // than resurfacing something the user may have judged themselves.
                conn.execute("ALTER TABLE finding_verdicts ADD COLUMN origin TEXT", [])?;
                // Existing verdicts are indistinguishable from a hand decision
                // now, so they are treated as one: removing an old rule leaves
                // its old dismissals rather than resurfacing something the user
                // may have judged themselves.
                conn.execute(
                    "UPDATE finding_verdicts SET origin = 'person' WHERE verdict IS NOT NULL",
                    [],
                )?;
            }
            // v14: the triage census (#459). A cheap per-message score store,
            // created empty; a scan populates it. No backfill — old backups
            // simply have no census until their next scan.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS census (
                     source_id         INTEGER NOT NULL,
                     thread_identifier TEXT NOT NULL,
                     sender            TEXT NOT NULL,
                     occurred_at       INTEGER,
                     score             REAL NOT NULL,
                     embedded_at       INTEGER NOT NULL,
                     PRIMARY KEY (source_id)
                 );
                 CREATE INDEX IF NOT EXISTS idx_census_cell
                     ON census(thread_identifier, sender);",
            )?;
            // v15: census rows carry the message fingerprint. analysis.db
            // survives re-import but cache row ids do not, so a census row is
            // only trusted when the fingerprint still matches the message the
            // id resolves to. Old rows keep '' and are simply re-embedded.
            let has_census_fp = conn
                .prepare("PRAGMA table_info(census)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .any(|c| c == "fingerprint");
            if !has_census_fp {
                conn.execute(
                    "ALTER TABLE census ADD COLUMN fingerprint TEXT NOT NULL DEFAULT ''",
                    [],
                )?;
            }
            // v16: triage coverage on the scan row. Additive and nullable —
            // batch scans have none, and pre-v16 triage scans keep NULL (the
            // UI shows no coverage line rather than inventing one).
            let scan_cols: Vec<String> = conn
                .prepare("PRAGMA table_info(scans)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(|c| c.ok())
                .collect();
            for col in ["censused", "candidates", "deep_scanned", "unconfirmed"] {
                if !scan_cols.iter().any(|c| c == col) {
                    conn.execute(&format!("ALTER TABLE scans ADD COLUMN {col} INTEGER"), [])?;
                }
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
        // One row per CONFIGURATION, not per run (#171). Running the same scope
        // again updates that row rather than adding a twin: the history is a
        // list of the scans you have set up, and `audit_log` keeps the record of
        // when each actually ran.
        //
        // Without this, showing the scope as the row's title would produce rows
        // with identical titles differing only by a date, which is worse than
        // the date-as-title it replaces. `resume_scan` already worked this way.
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM scans
                 WHERE sources = ?1
                   AND range_start IS ?2
                   AND range_end IS ?3
                 ORDER BY id DESC LIMIT 1",
                params![sources, range.0, range.1],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(id) = existing {
            // A re-run starts over: the previous outcome must not linger on a
            // row that is running again, and a stale `error` would keep the
            // warning badge on a scan that is currently fine.
            self.conn.execute(
                // Coverage goes too. It describes what the PREVIOUS run read,
                // and leaving it would let a re-run — or a Full read scan over
                // the same scope, which never records coverage — inherit a
                // claim that places were left unread. That is the exact
                // inversion the coverage line exists to prevent.
                "UPDATE scans
                    SET model = ?2, status = 'running', started_at = ?3,
                        finished_at = NULL, error = NULL,
                        chunks_total = 0, chunks_done = 0,
                        censused = NULL, candidates = NULL,
                        deep_scanned = NULL, unconfirmed = NULL
                  WHERE id = ?1",
                params![id, model, started_at],
            )?;
            return Ok(id);
        }

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
            // Coverage is cleared for the same reason as in begin_scan: it
            // describes a finished read, and the resumed run re-records it.
            "UPDATE scans SET status = 'running', finished_at = NULL, chunks_done = 0,
                              censused = NULL, candidates = NULL,
                              deep_scanned = NULL, unconfirmed = NULL
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
        self.finish_scan_with(scan_id, status, finished_at, None)
    }

    /// Finish a scan, recording why when there is anything to say.
    ///
    /// Only a failure carries a reason. Cancelled and interrupted explain
    /// themselves — you stopped it, or the app closed — and inventing text for
    /// them would be noise. A failure without one leaves the history saying "it
    /// failed", which the user could already see.
    pub fn finish_scan_with(
        &self,
        scan_id: i64,
        status: ScanStatus,
        finished_at: i64,
        error: Option<&str>,
    ) -> Result<()> {
        if status == ScanStatus::Running {
            return Err(Error::Invalid("finish_scan with status 'running'".into()));
        }
        self.conn.execute(
            "UPDATE scans SET status = ?2, finished_at = ?3, error = ?4 WHERE id = ?1",
            params![scan_id, status.as_str(), finished_at, error],
        )?;
        Ok(())
    }

    /// Store a triage scan's coverage on its row, so the report can state what
    /// was and was not read long after the live event is gone. Called at the
    /// end of a triage scan INCLUDING a cancelled one — a partial scan is
    /// exactly when the numbers matter most.
    pub fn record_triage_coverage(&self, scan_id: i64, c: TriageCoverage) -> Result<()> {
        self.conn.execute(
            "UPDATE scans SET censused = ?2, candidates = ?3, deep_scanned = ?4,
                              unconfirmed = ?5
             WHERE id = ?1",
            params![
                scan_id,
                c.censused as i64,
                c.candidates as i64,
                c.deep_scanned as i64,
                c.unconfirmed as i64
            ],
        )?;
        Ok(())
    }

    // ---- chunk progress / resume ----

    // ---- triage census (#459) ----

    /// One scored message from the cheap embedding pass.
    ///
    /// Bulk-insert these with [`Self::record_census`]. The score is advisory —
    /// it decides where the expensive classifier looks, never what a finding is.
    ///
    /// A cell is (thread_identifier, sender). Scanning re-embeds only messages
    /// whose id is not already present, so a resumed or repeated census is
    /// incremental for free.
    pub fn record_census(&mut self, rows: &[CensusRow], at: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO census
                     (source_id, thread_identifier, sender, occurred_at, score, embedded_at, fingerprint)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(source_id) DO UPDATE SET
                     score = excluded.score, embedded_at = excluded.embedded_at,
                     fingerprint = excluded.fingerprint",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.source_id,
                    r.thread_identifier,
                    r.sender,
                    r.occurred_at,
                    r.score,
                    at,
                    r.fingerprint
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// (id, fingerprint) pairs that already have a census score, so a scan
    /// embeds only the rest. The fingerprint is part of the key on purpose: a
    /// re-import reshuffles row ids under this surviving store, and skipping on
    /// the id alone would leave a NEW message unscored because an unrelated old
    /// row happens to share its id. Pre-v15 rows ('' fingerprint) never match.
    pub fn census_scored(&self) -> Result<std::collections::HashSet<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source_id, fingerprint FROM census")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// The triage worklist: (conversation, sender) cells ranked by evidence,
    /// most suspicious first.
    ///
    /// Ranking is by COUNT of hot messages, not by the single maximum — one
    /// stray high score is noise, a sender with many is signal, and this is the
    /// heavy-tail insight that makes triage beat a uniform sweep. `hot` counts
    /// messages at or above `threshold`; ties break on the mean so a denser
    /// cell sorts first.
    ///
    /// `trajectory` is the slope of score over time within the cell (per day),
    /// so a slow-burn pattern that never spikes but steadily climbs — grooming,
    /// coercive control — still surfaces. It is advisory context, not part of
    /// the sort, because a single number cannot rank two different kinds of
    /// evidence; the caller decides how to weigh a high count against a rising
    /// trajectory.
    /// The deep-scan worklist: which messages get the expensive focused
    /// classifier, in what order, within a budget.
    ///
    /// This is what turns "~40 h to focus-scan everything" into a bounded run.
    /// Only messages at or above `threshold` are candidates — the census has
    /// already decided the rest are not worth a classifier call. Among those,
    /// ordering is by the containing cell's evidence first (a hot message in a
    /// dense (conversation, sender) cell before an equally-hot one that stands
    /// alone), then by the message's own score. `budget` caps how many are
    /// returned; None runs the lot.
    ///
    /// The order is the point: spend a fixed classifier budget on the most
    /// concentrated harm, which in heavy-tailed data is where it lives. A
    /// caller that exhausts the budget knows exactly what was NOT scanned (the
    /// tail below the cut) and can report it honestly rather than as "clean".
    pub fn triage_worklist(&self, threshold: f64, budget: Option<usize>) -> Result<Vec<WorkItem>> {
        // Cell rank = count of hot messages in the (thread, sender) cell. A
        // correlated subquery gives each candidate its cell's hot-count, so the
        // ORDER BY can put dense cells first without a join to triage_cells.
        let sql = "
            SELECT c.source_id, c.thread_identifier, c.sender, c.score,
                   (SELECT COUNT(*) FROM census h
                    WHERE h.thread_identifier = c.thread_identifier
                      AND h.sender = c.sender
                      AND h.score >= ?1) AS cell_hot,
                   c.fingerprint
            FROM census c
            WHERE c.score >= ?1
            ORDER BY cell_hot DESC, c.score DESC, c.source_id ASC";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![threshold], |r| {
            Ok(WorkItem {
                source_id: r.get(0)?,
                thread_identifier: r.get(1)?,
                sender: r.get(2)?,
                score: r.get(3)?,
                cell_hot: r.get(4)?,
                fingerprint: r.get(5)?,
            })
        })?;
        let mut out: Vec<WorkItem> = rows.filter_map(|r| r.ok()).collect();
        if let Some(b) = budget {
            out.truncate(b);
        }
        Ok(out)
    }

    /// How many messages the census holds above `threshold` — the full deep-scan
    /// demand, so a caller can report "scanned N of M candidates" when a budget
    /// cuts the worklist short, and say what the cut leaves unread.
    pub fn triage_candidate_count(&self, threshold: f64) -> Result<usize> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM census WHERE score >= ?1",
            params![threshold],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    pub fn triage_cells(&self, threshold: f64) -> Result<Vec<TriageCell>> {
        let mut stmt = self.conn.prepare(
            "SELECT thread_identifier, sender,
                    COUNT(*)                              AS total,
                    SUM(CASE WHEN score >= ?1 THEN 1 ELSE 0 END) AS hot,
                    AVG(score)                            AS mean,
                    MAX(score)                            AS peak,
                    MIN(occurred_at)                      AS first_at,
                    MAX(occurred_at)                      AS last_at
             FROM census
             GROUP BY thread_identifier, sender
             ORDER BY hot DESC, mean DESC",
        )?;
        let mut cells: Vec<TriageCell> = stmt
            .query_map(params![threshold], |r| {
                Ok(TriageCell {
                    thread_identifier: r.get(0)?,
                    sender: r.get(1)?,
                    total: r.get(2)?,
                    hot: r.get(3)?,
                    mean: r.get(4)?,
                    peak: r.get(5)?,
                    first_at: r.get(6)?,
                    last_at: r.get(7)?,
                    trajectory: 0.0,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        // Trajectory needs the per-message points, computed per cell rather than
        // in SQL because a least-squares slope over (day, score) is clearer here
        // and the point count per cell is small.
        for c in &mut cells {
            c.trajectory = self.cell_trajectory(&c.thread_identifier, &c.sender)?;
        }
        Ok(cells)
    }

    /// Least-squares slope of score against day-offset within one cell, per day.
    /// Zero when there is one point or no time spread — no trend to report.
    fn cell_trajectory(&self, thread: &str, sender: &str) -> Result<f64> {
        let mut stmt = self.conn.prepare(
            "SELECT occurred_at, score FROM census
             WHERE thread_identifier = ?1 AND sender = ?2 AND occurred_at IS NOT NULL
             ORDER BY occurred_at",
        )?;
        let pts: Vec<(i64, f64)> = stmt
            .query_map(params![thread, sender], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        if pts.len() < 2 {
            return Ok(0.0);
        }
        let day = 86_400.0;
        let base = pts[0].0 as f64;
        let xs: Vec<f64> = pts.iter().map(|(t, _)| (*t as f64 - base) / day).collect();
        let ys: Vec<f64> = pts.iter().map(|(_, s)| *s).collect();
        let n = xs.len() as f64;
        let mean_x = xs.iter().sum::<f64>() / n;
        let mean_y = ys.iter().sum::<f64>() / n;
        let mut num = 0.0;
        let mut den = 0.0;
        for (x, y) in xs.iter().zip(&ys) {
            num += (x - mean_x) * (y - mean_y);
            den += (x - mean_x) * (x - mean_x);
        }
        Ok(if den == 0.0 { 0.0 } else { num / den })
    }

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
                    fingerprint, category, severity, rationale, service, sender,
                    content_key, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                    f.sender,
                    f.content_key,
                    at
                ],
            )?;
        }
        tx.commit()?;
        // A rule set yesterday must cover what today's scan turns up, or it only
        // ever applied to the findings that happened to exist when it was made.
        self.apply_suppressions(at)?;
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
        let tail = format!("{} LIMIT {limit} OFFSET {offset}", order_by(q));
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

    /// Where a finding sits in the current filter and order — its row index, or
    /// None when the filter excludes it.
    ///
    /// This is what lets "Back to Safety Scan" return to the finding somebody
    /// started from rather than to the top of the list (#224). The findings panel
    /// is virtualized and paged, so it needs an index, not an id.
    ///
    /// `ROW_NUMBER()` over the *same* `order_by(q)` the page query uses, rather
    /// than a hand-written "count rows that sort before this one". That
    /// comparison would need to reproduce the lexicographic order across three
    /// keys with NULLs in two of them, and any disagreement scrolls to the wrong
    /// row. Sharing the ordering string makes disagreement impossible.
    pub fn finding_rank(
        &self,
        sources: &str,
        range_start: Option<i64>,
        range_end: Option<i64>,
        q: &FindingQuery,
        finding_id: i64,
    ) -> Result<Option<i64>> {
        let sql = format!(
            "SELECT rn FROM (
                 SELECT f.id AS fid,
                        ROW_NUMBER() OVER ({order}) - 1 AS rn
                 FROM content_findings f
                 WHERE {where_clause}
             ) WHERE fid = ?4",
            order = order_by(q),
            where_clause = filtered_scope(q),
        );
        Ok(self
            .conn
            .query_row(
                &sql,
                params![sources, range_start, range_end, finding_id],
                |r| r.get(0),
            )
            .optional()?)
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
               COUNT(*) FILTER (WHERE NOT {DISMISSED_EXPR} AND NOT ({LOW_SEVERITY_EXPR})),
               COUNT(*) FILTER (WHERE NOT {DISMISSED_EXPR} AND NOT f.stale
                                AND NOT ({LOW_SEVERITY_EXPR})),
               COUNT(*) FILTER (WHERE {DISMISSED_EXPR} AND NOT ({LOW_SEVERITY_EXPR})),
               COUNT(*) FILTER (WHERE NOT {DISMISSED_EXPR} AND NOT f.stale
                                AND NOT ({LOW_SEVERITY_EXPR})
                                AND NOT EXISTS(SELECT 1 FROM finding_verdicts v
                                               WHERE v.fingerprint = f.fingerprint
                                                 AND v.category = f.category
                                                 AND v.seen_at IS NOT NULL)),
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
                    unread: r.get(3)?,
                    serious: r.get(4)?,
                    harmful: r.get(5)?,
                    concerning: r.get(6)?,
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
                // Same floor as the panel's default view: a live counter that
                // counted severity-1 findings would promise a number the list
                // then refuses to show.
                "SELECT COUNT(*) FROM content_findings f
                 WHERE ({IN_SCOPE_PREDICATE})
                   AND f.stale = 0
                   AND NOT ({LOW_SEVERITY_EXPR})
                   AND NOT {DISMISSED_EXPR}"
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
        now: i64,
    ) -> Result<FindingAnalytics> {
        let scope = filtered_scope(q);
        // A timestamp outside the window a backup can possibly cover is not a
        // date, it is a decode failure — and one of them ruins the axis for
        // everything else, because the unit is chosen from the span. See
        // [`TIMELINE_START`]. `now + a day` of slack absorbs clock skew without
        // admitting timestamps from next century.
        let datable = format!(
            "f.occurred_at IS NOT NULL AND f.occurred_at >= {TIMELINE_START} \
             AND f.occurred_at <= {}",
            now + 86_400
        );

        // The bucket unit comes from the span the findings actually cover, so a
        // three-week scan and a ten-year one both produce a readable axis.
        // Findings that cannot be placed on it are counted here and excluded:
        // dropping them silently would leave the chart's total disagreeing with
        // the list's.
        let (min_at, max_at, undated, charted) = self.conn.query_row(
            &format!(
                "SELECT MIN(CASE WHEN {datable} THEN f.occurred_at END),
                        MAX(CASE WHEN {datable} THEN f.occurred_at END),
                        COUNT(*) FILTER (WHERE NOT ({datable})),
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
            // Nothing datable to measure; the axis is empty either way.
            _ => TimeUnit::Month,
        };

        let over_time = self.bucket_counts(
            &format!("({scope}) AND {datable}"),
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

        // How many findings the charts LEFT OUT as false positives — not how many
        // dismissals exist. Two things made the old count say something untrue:
        // it ignored the severity filter, so narrowing to Serious still reported
        // every dismissal; and when the panel's "Show dismissed" is on the
        // findings are IN the charts, while the disclosure beside them went on
        // claiming they had been left out. Counted with the caller's own filter,
        // and zero when nothing was excluded, the sentence is true in every state.
        let dismissed = if q.include_dismissed {
            0
        } else {
            // Only `include_dismissed` flips — everything else, the severity
            // floor included, must stay the caller's, or this number describes
            // a different set of rows than the charts beside it.
            let shown_but_dismissed = FindingQuery {
                include_dismissed: true,
                ..q.clone()
            };
            self.conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM content_findings f
                     WHERE ({}) AND {DISMISSED_EXPR}",
                    filtered_scope(&shown_but_dismissed)
                ),
                params![sources, range_start, range_end],
                |r| r.get::<_, i64>(0),
            )?
        };

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
                    f.sender, f.content_key, f.stale, f.created_at, f.rechecked,
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
                r.get::<_, Option<String>>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, bool>(12)?,
                r.get::<_, i64>(13)?,
                r.get::<_, bool>(14)?,
                r.get::<_, bool>(15)?,
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
                sender,
                content_key,
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
                sender,
                content_key,
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
                    fingerprint, category, severity, rationale, service, sender,
                    content_key, rechecked, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13)",
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
                    f.sender,
                    f.content_key,
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
        self.set_verdict(
            fingerprint,
            category,
            dismissed.then_some("dismissed"),
            None,
            at,
        )
    }

    /// Record what the user decided about a finding, keeping what is already
    /// known about it.
    ///
    /// `ON CONFLICT DO UPDATE` rather than `INSERT OR REPLACE`: replace would
    /// drop `seen_at`, so dismissing something would forget it had been read —
    /// and undismissing would then mark it unread, which is not what undoing a
    /// verdict means.
    pub fn set_verdict(
        &self,
        fingerprint: &str,
        category: Category,
        verdict: Option<&str>,
        reason: Option<&str>,
        at: i64,
    ) -> Result<()> {
        self.conn.execute(
            // origin = 'person': this is a decision, whichever way it went.
            // Keeping a finding explicitly is as much a decision as dismissing
            // it, and no standing rule may overrule either.
            "INSERT INTO finding_verdicts
                 (fingerprint, category, verdict, reason, origin, at)
             VALUES (?1, ?2, ?3, ?4, 'person', ?5)
             ON CONFLICT(fingerprint, category) DO UPDATE SET
                 verdict = excluded.verdict,
                 reason  = excluded.reason,
                 origin  = 'person',
                 at      = excluded.at",
            params![fingerprint, category.as_str(), verdict, reason, at],
        )?;
        Ok(())
    }

    /// Dismiss everything a standing rule covers, now and in future.
    ///
    /// A rule is a dismissal the user chose to apply broadly: "this whole
    /// conversation is fine", "this category is noise". It is stored, and it is
    /// applied by DISMISSING the matching findings rather than by hiding them
    /// behind a second predicate.
    ///
    /// That is the safety property. A hidden finding is invisible; a dismissed
    /// one is counted, reachable behind "Show dismissed", and carries the reason
    /// that dismissed it. The case this app exists to catch is a conversation
    /// that was safe until it wasn't — so a rule must never make a finding
    /// disappear, only pre-judge it visibly.
    /// `category` bounds the rule to one Forensic 9 category. It is not
    /// optional for new rules — `None` recreates the pre-v10 breadth where a
    /// conversation rule silenced everything, and only the migration is
    /// allowed to leave it unset.
    ///
    /// `sender` bounds the rule to one person and belongs to the
    /// `content+sender` scope; `None` (stored as `''`) means any sender. It is
    /// never NULL, so the UNIQUE index still tells two rules on the same
    /// content from different people apart.
    pub fn add_suppression(
        &self,
        scope: &str,
        value: &str,
        category: &str,
        sender: Option<&str>,
        reason: Option<&str>,
        at: i64,
    ) -> Result<usize> {
        self.conn.execute(
            "INSERT INTO suppressions (scope, value, category, sender, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(scope, value, category, sender)
                 DO UPDATE SET reason = excluded.reason",
            params![scope, value, category, sender.unwrap_or(""), reason, at],
        )?;
        self.apply_suppressions(at)
    }
}

/// How a rule matches a finding. ONE definition: `apply_suppressions` uses it
/// to dismiss, and `list_suppressions` uses it to count what each rule is
/// currently swallowing. Two copies of this predicate would drift, and the
/// panel would report a number the engine does not act on.
const RULE_MATCH: &str = "\
    ((s.scope = 'thread'   AND s.value = f.thread_identifier) \
  OR (s.scope = 'category' AND s.value = f.category) \
  OR (s.scope = 'content+any' \
      AND f.content_key IS NOT NULL AND s.value = f.content_key) \
  OR (s.scope = 'content+sender' \
      AND f.content_key IS NOT NULL AND s.value = f.content_key \
      AND f.sender IS NOT NULL AND s.sender = f.sender)) \
 AND (s.category IS NULL OR s.category = f.category)";

impl AnalysisDb {
    /// Dismiss every finding a rule covers that is not already judged.
    ///
    /// Called when a rule is created and again whenever findings are written,
    /// so a rule made today still covers what tomorrow's scan turns up. A
    /// verdict a person made is left alone: a rule must not overwrite a
    /// decision the user made by hand. A row that exists only because the
    /// finding was READ (`mark_seen` writes one with a NULL verdict) is not a
    /// decision, and a rule still covers it — testing for the row rather than
    /// for a verdict put every finding the reviewer had opened permanently
    /// beyond every rule's reach (#406).
    ///
    /// The four scopes, narrow to broad:
    ///
    /// - `content+sender` — this exact short content, from this person. The one
    ///   the widening offer defaults to: grandmother's ❤️ is covered, an
    ///   identical ❤️ from a stranger is not, because the sender differs.
    /// - `content+any` — this content from anyone.
    /// - `thread` — this conversation.
    /// - `category` — everywhere.
    ///
    /// Two `IS NOT NULL` guards carry real weight rather than defending against
    /// SQL quirks:
    ///
    /// - **A finding with no `content_key` is never matched by a content rule.**
    ///   That is every finding written before v12, and every finding whose text
    ///   was too long to generalize. Both should be unreachable by a rule keyed
    ///   on content.
    /// - **A finding with no `sender` never matches a sender-scoped rule.** A
    ///   finding whose sender was never recorded must not inherit an exemption
    ///   somebody made for a different person (#402).
    ///
    /// Two constraints beyond the scope match, both load-bearing:
    ///
    /// - **The rule's category must match the finding's.** Without it, a
    ///   conversation rule made about a heart emoji pre-dismissed a
    ///   threat-violence finding from the same number months later (#394).
    /// - **Severity 3 is never auto-dismissed.** A standing rule is a blanket
    ///   judgement made before the finding existed, and the most serious tier
    ///   is where a blanket judgement is least defensible. The reviewer can
    ///   still dismiss such a finding by hand — that is a decision about
    ///   something they have actually seen.
    pub fn apply_suppressions(&self, at: i64) -> Result<usize> {
        Ok(self.conn.execute(
            &format!(
                "INSERT INTO finding_verdicts
                     (fingerprint, category, verdict, reason, origin, at)
                 SELECT f.fingerprint, f.category, 'dismissed',
                        COALESCE(s.reason, 'Matched a rule you set'), 'rule', ?1
                 FROM content_findings f
                 JOIN suppressions s ON {RULE_MATCH}
                 WHERE f.severity < 3
                   AND NOT EXISTS (SELECT 1 FROM finding_verdicts v
                                   WHERE v.fingerprint = f.fingerprint
                                     AND v.category = f.category
                                     AND v.origin = 'person')
                 ON CONFLICT(fingerprint, category) DO UPDATE SET
                     verdict = excluded.verdict,
                     reason  = excluded.reason,
                     origin  = 'rule',
                     at      = excluded.at
                 WHERE finding_verdicts.origin IS NOT 'person'"
            ),
            params![at],
        )?)
    }

    /// Every standing rule, newest first, as (scope, value, category, reason).
    /// A `None` category is a pre-v10 rule covering every category — the UI
    /// must say so rather than presenting it as an ordinary narrow rule.
    /// `hits` is what the rule is dismissing RIGHT NOW, counted with the same
    /// predicate the engine acts on. A rule with zero is either stale or was
    /// never needed, and the panel says so — a standing rule nobody can see the
    /// effect of is the shape this whole feature is trying to avoid.
    pub fn list_suppressions(&self) -> Result<Vec<SuppressionRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.scope, s.value, s.category, s.sender, s.reason,
                    (SELECT COUNT(*) FROM content_findings f
                     WHERE {RULE_MATCH}
                       AND EXISTS (SELECT 1 FROM finding_verdicts v
                                   WHERE v.fingerprint = f.fingerprint
                                     AND v.category = f.category
                                     AND v.verdict = 'dismissed'))
             FROM suppressions s ORDER BY s.created_at DESC"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok(SuppressionRow {
                scope: r.get(0)?,
                value: r.get(1)?,
                category: r.get(2)?,
                sender: r.get(3)?,
                reason: r.get(4)?,
                hits: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Drop a rule. Findings it already dismissed keep their verdict — undoing a
    /// rule is not the same as re-opening every judgement it made, and silently
    /// resurrecting a hundred findings would be its own surprise.
    /// `category` identifies WHICH rule on that conversation to drop; `None`
    /// targets a grandfathered pre-v10 rule (the row whose category IS NULL),
    /// never every rule on the conversation.
    pub fn remove_suppression(
        &self,
        scope: &str,
        value: &str,
        category: Option<&str>,
        sender: Option<&str>,
        at: i64,
    ) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM suppressions
             WHERE scope = ?1 AND value = ?2
               AND category IS ?3
               AND sender = ?4",
            params![scope, value, category, sender.unwrap_or("")],
        )?;
        // Take back every rule-made verdict, then re-apply what is left. A
        // finding two rules covered stays dismissed; one only this rule covered
        // comes back. Verdicts a person made carry origin = 'person' and are
        // never touched — undoing YOUR rule is not undoing YOUR judgement.
        //
        // The row is updated rather than deleted so `seen_at` survives: a
        // finding you had already read must not become unread because a rule
        // was removed.
        let before: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM finding_verdicts WHERE verdict = 'dismissed'",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "UPDATE finding_verdicts
             SET verdict = NULL, reason = NULL, origin = NULL, at = ?1
             WHERE origin = 'rule'",
            params![at],
        )?;
        self.apply_suppressions(at)?;
        let after: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM finding_verdicts WHERE verdict = 'dismissed'",
            [],
            |r| r.get(0),
        )?;
        Ok((before - after).max(0) as usize)
    }

    /// Mark a finding read — the first time its flagged text is revealed.
    ///
    /// Idempotent and one-way: `seen_at` records the FIRST look, and nothing
    /// un-sees a finding. Collapsing the row is not un-reading it.
    pub fn mark_seen(&self, fingerprint: &str, category: Category, at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO finding_verdicts (fingerprint, category, seen_at, at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(fingerprint, category) DO UPDATE SET
                 seen_at = COALESCE(finding_verdicts.seen_at, excluded.seen_at)",
            params![fingerprint, category.as_str(), at],
        )?;
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
                        finished_at, chunks_total, chunks_done, error,
                        censused, candidates, deep_scanned, unconfirmed
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
                        error: r.get(10)?,
                        // All four are written together, so `censused` alone
                        // decides whether the row carries coverage. Every
                        // column is read with `?`: a read error here must
                        // propagate, not degrade into zeros — "0 of 0 read in
                        // depth" suppresses the not-checked caveat entirely,
                        // which is the one failure this line exists to
                        // prevent.
                        coverage: match r.get::<_, Option<i64>>(11)? {
                            None => None,
                            Some(censused) => Some(TriageCoverage {
                                censused: censused as usize,
                                candidates: r.get::<_, Option<i64>>(12)?.unwrap_or(0) as usize,
                                deep_scanned: r.get::<_, Option<i64>>(13)?.unwrap_or(0) as usize,
                                unconfirmed: r.get::<_, Option<i64>>(14)?.unwrap_or(0) as usize,
                            }),
                        },
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
                        finished_at, chunks_total, chunks_done, error,
                        censused, candidates, deep_scanned, unconfirmed
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
                        error: r.get(10)?,
                        // All four are written together, so `censused` alone
                        // decides whether the row carries coverage. Every
                        // column is read with `?`: a read error here must
                        // propagate, not degrade into zeros — "0 of 0 read in
                        // depth" suppresses the not-checked caveat entirely,
                        // which is the one failure this line exists to
                        // prevent.
                        coverage: match r.get::<_, Option<i64>>(11)? {
                            None => None,
                            Some(censused) => Some(TriageCoverage {
                                censused: censused as usize,
                                candidates: r.get::<_, Option<i64>>(12)?.unwrap_or(0) as usize,
                                deep_scanned: r.get::<_, Option<i64>>(13)?.unwrap_or(0) as usize,
                                unconfirmed: r.get::<_, Option<i64>>(14)?.unwrap_or(0) as usize,
                            }),
                        },
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
    /// Delete a scan, keeping everything another scan still accounts for.
    ///
    /// A finding's `scan_id` records which run *first classified its chunk*, not
    /// which runs display it — classification is cached per chunk, so a re-scan
    /// over covered data attributes nothing to its own id. Deleting by `scan_id`
    /// therefore destroyed rows later scans were counting by scope, and took the
    /// cached classification with them, so a re-scan had to pay the model cost
    /// again to rediscover findings that were already known.
    ///
    /// So findings and chunk progress are **repointed** to a surviving scan, and
    /// only findings that no remaining scope covers are removed. `foreign_keys`
    /// is ON, which is why repointing must happen before the scan row goes.
    pub fn delete_scan(&self, id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        // Per-scan OUTPUT genuinely belongs to the scan: a report and its
        // per-thread summaries describe that run, and the audit log records it.
        tx.execute("DELETE FROM summaries WHERE scan_id = ?1", params![id])?;
        tx.execute("DELETE FROM audit_log WHERE scan_id = ?1", params![id])?;

        // Who inherits provenance for rows outliving this scan: the newest
        // remaining one, arbitrarily but stably. The column is provenance, not
        // ownership — `chunk_is_done` keys on chunk_key + fingerprint, not on it.
        let heir: Option<i64> = tx
            .query_row(
                "SELECT id FROM scans WHERE id != ?1 ORDER BY id DESC LIMIT 1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(heir) = heir {
            tx.execute(
                "UPDATE content_findings SET scan_id = ?2 WHERE scan_id = ?1",
                params![id, heir],
            )?;
            tx.execute(
                "UPDATE chunk_progress SET scan_id = ?2 WHERE scan_id = ?1",
                params![id, heir],
            )?;
            tx.execute("DELETE FROM scans WHERE id = ?1", params![id])?;

            // Then drop only what no surviving scope accounts for — otherwise
            // narrowing down to one scan would leave findings nothing displays.
            tx.execute(
                &format!(
                    "DELETE FROM content_findings WHERE id IN (
                         SELECT f.id FROM content_findings f
                         WHERE NOT EXISTS (SELECT 1 FROM scans s WHERE {SCOPE_PREDICATE})
                     )"
                ),
                [],
            )?;
        } else {
            // The last scan: nothing can cover these and nothing can reference
            // the scan row, so they go with it.
            tx.execute("DELETE FROM content_findings", [])?;
            tx.execute("DELETE FROM chunk_progress", [])?;
            tx.execute("DELETE FROM scans WHERE id = ?1", params![id])?;
        }

        tx.commit()?;
        Ok(())
    }

    /// The old ownership-based delete, test-only: it exists so the regression
    /// test can demonstrate the bug it replaced rather than assert against a
    /// remembered description of it.
    #[cfg(test)]
    fn delete_scan_by_ownership(&self, id: i64) -> Result<()> {
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
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.id, s.model, s.range_start, s.range_end, s.sources, s.status,
                    s.started_at, s.finished_at,
                    coalesce(count(f.id), 0),
                    coalesce(sum(f.severity = 3), 0),
                    coalesce(sum(f.severity = 2), 0),
                    coalesce(sum(f.severity = 1), 0),
                    s.error
             FROM scans s
             LEFT JOIN content_findings f ON f.stale = 0 AND {SCOPE_PREDICATE}
             GROUP BY s.id ORDER BY s.id DESC LIMIT ?1"
        ))?;
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
                error: r.get(12)?,
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
    /// Whether a scan row was produced by the TRIAGE pipeline. The scans table
    /// has no kind column; the triage command stamps a `triage_mode` audit row
    /// at start, and that stamp is the discriminator. Callers use it to refuse
    /// resuming a triage row through the batch engine (different pipeline,
    /// different semantics, same table).
    pub fn scan_is_triage(&self, scan_id: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM audit_log WHERE scan_id = ?1 AND event = 'triage_mode'",
            params![scan_id],
            |r| r.get::<_, i64>(0),
        )? > 0)
    }

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
                sender: None,
                content_key: None,
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
            // The seed spans every severity; paging must partition all of it.
            include_low: true,
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

    /// A conversation-scoped scan owns ONLY its conversation's findings.
    ///
    /// The panel, the pills, the charts and the live counter all resolve a
    /// scan's scope from its `sources` string, so if that string matched more
    /// than the scan covered, a scan of one thread would report other people's
    /// findings as its own.
    fn census(id: i64, thread: &str, sender: &str, at: i64, score: f64) -> CensusRow {
        CensusRow {
            source_id: id,
            thread_identifier: thread.into(),
            sender: sender.into(),
            occurred_at: Some(at),
            score,
            fingerprint: format!("fp{id}"),
        }
    }

    /// The worklist spends a budget on the densest harm first, and reports
    /// exactly what the budget left unscanned.
    /// The coverage a triage scan owes its reader must survive the scan: the
    /// live event that carries it is gone the moment the run ends, and a
    /// report opened tomorrow still has to say what was NOT read.
    #[test]
    fn triage_coverage_survives_the_scan() {
        let db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "messages", 1).unwrap();
        // A batch scan (or a triage scan that never recorded) has none — the
        // UI must be able to tell "no coverage" from "zero coverage".
        assert!(db.scan_by_id(scan).unwrap().unwrap().coverage.is_none());

        db.record_triage_coverage(
            scan,
            TriageCoverage {
                censused: 8_000,
                candidates: 120,
                deep_scanned: 40,
                unconfirmed: 3,
            },
        )
        .unwrap();
        let c = db.scan_by_id(scan).unwrap().unwrap().coverage.unwrap();
        assert_eq!(c.censused, 8_000);
        assert_eq!(c.candidates, 120);
        assert_eq!(c.deep_scanned, 40);
        assert_eq!(c.unconfirmed, 3);
        assert_eq!(
            c.unscanned(),
            80,
            "the unread tail is derived, so it can never contradict the pair"
        );
        // latest_scan reads the same row through a different query.
        assert_eq!(db.latest_scan().unwrap().unwrap().coverage, Some(c));
    }

    /// A re-run must not inherit the previous run's coverage. `begin_scan`
    /// reuses the row for a scope, so without clearing, a Full read scan (which
    /// records no coverage at all) would render the earlier triage run's "the
    /// rest were not checked" — claiming a scan that read EVERYTHING left
    /// places unread, the exact inversion this line exists to prevent.
    #[test]
    fn a_re_run_does_not_inherit_the_previous_coverage() {
        let db = AnalysisDb::open_in_memory().unwrap();
        let id = db.begin_scan("m", (None, None), "messages", 1).unwrap();
        db.record_triage_coverage(
            id,
            TriageCoverage {
                censused: 12_480,
                candidates: 210,
                deep_scanned: 180,
                unconfirmed: 12,
            },
        )
        .unwrap();
        assert!(db.scan_by_id(id).unwrap().unwrap().coverage.is_some());

        // The same scope again — the SAME row (that is the #171 design).
        let again = db.begin_scan("m", (None, None), "messages", 2).unwrap();
        assert_eq!(again, id, "same scope reuses the row");
        assert!(
            db.scan_by_id(id).unwrap().unwrap().coverage.is_none(),
            "a re-run starts with no coverage claim"
        );

        // And a resume clears it too: the resumed run re-records what it read.
        db.record_triage_coverage(
            id,
            TriageCoverage {
                censused: 1,
                candidates: 1,
                deep_scanned: 1,
                unconfirmed: 0,
            },
        )
        .unwrap();
        db.finish_scan(id, ScanStatus::Cancelled, 3).unwrap();
        db.resume_scan(id, "m").unwrap();
        assert!(
            db.scan_by_id(id).unwrap().unwrap().coverage.is_none(),
            "a resumed run starts with no coverage claim"
        );
    }

    /// A pre-v16 store must migrate without losing its scans, and its old rows
    /// simply have no coverage (never a fabricated zero).
    #[test]
    fn a_pre_coverage_store_migrates_and_reports_no_coverage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("analysis.db");
        {
            // A REAL store, rewound to the v15 shape: the scans table rebuilt
            // without the coverage columns (SQLite cannot DROP COLUMN from
            // this table — its DDL carries comments), everything else intact,
            // so the migration runs the same path a real upgrade does.
            let db = AnalysisDb::open(&path).unwrap();
            db.begin_scan("gemma", (None, None), "all", 1).unwrap();
            let conn = db.conn();
            conn.execute_batch(
                "CREATE TABLE scans_v15 AS
                     SELECT id, model, range_start, range_end, sources, status,
                            started_at, finished_at, chunks_total, chunks_done, error
                     FROM scans;
                 DROP TABLE scans;
                 ALTER TABLE scans_v15 RENAME TO scans;",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 15i64).unwrap();
        }
        let db = AnalysisDb::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let row = db.scan_by_id(1).unwrap().expect("the old scan survives");
        assert!(
            row.coverage.is_none(),
            "an old row has no coverage — not a zero one"
        );
        // And the migrated store can store coverage from here on.
        db.record_triage_coverage(
            1,
            TriageCoverage {
                censused: 10,
                candidates: 4,
                deep_scanned: 1,
                unconfirmed: 0,
            },
        )
        .unwrap();
        assert_eq!(
            db.scan_by_id(1)
                .unwrap()
                .unwrap()
                .coverage
                .unwrap()
                .unscanned(),
            3
        );
    }

    #[test]
    fn triage_worklist_ranks_dense_cells_first_and_honours_budget() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let mut rows = Vec::new();
        // A dense cell: 4 hot messages.
        for i in 0..4 {
            rows.push(census(10 + i, "dense", "y", 1000 + i, 0.80));
        }
        // A lone hot message in a sparse cell, HIGHER individual score.
        rows.push(census(20, "sparse", "z", 1000, 0.95));
        // Cold chatter, never a candidate.
        for i in 0..5 {
            rows.push(census(30 + i, "cold", "w", 1000 + i, 0.20));
        }
        db.record_census(&rows, 1).unwrap();

        // Full candidate demand excludes the cold five.
        assert_eq!(db.triage_candidate_count(0.65).unwrap(), 5);

        let full = db.triage_worklist(0.65, None).unwrap();
        assert_eq!(full.len(), 5, "cold messages are not candidates");
        // The dense cell's messages come first, even though the sparse one
        // scores higher individually — density outranks a lone spike.
        assert!(full[..4].iter().all(|w| w.thread_identifier == "dense"));
        assert_eq!(full[4].thread_identifier, "sparse");

        // A budget of 3 keeps the top of that order and drops the rest.
        let capped = db.triage_worklist(0.65, Some(3)).unwrap();
        assert_eq!(capped.len(), 3);
        assert!(capped.iter().all(|w| w.thread_identifier == "dense"));
        // The caller can see 3 of 5 were scanned — the other 2 are the honest
        // "not deep-scanned" tail.
        assert_eq!(db.triage_candidate_count(0.65).unwrap() - capped.len(), 2);
    }

    /// The threshold is the census dial: raising it shrinks the candidate set,
    /// so a Precise mode does less deep-scan work than a Thorough one.
    #[test]
    fn a_higher_threshold_yields_fewer_candidates() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let rows: Vec<_> = (0..10)
            .map(|i| census(i, "t", "s", 1000 + i, 0.50 + 0.03 * i as f64))
            .collect();
        db.record_census(&rows, 1).unwrap();
        let low = db.triage_worklist(0.55, None).unwrap().len();
        let high = db.triage_worklist(0.70, None).unwrap().len();
        assert!(high < low, "a higher census threshold scans fewer messages");
    }

    /// The heavy-tail insight: a cell with MANY hot messages ranks above one
    /// with a single higher spike. One stray high score is noise; density is
    /// signal, and ranking by count is what makes triage beat a uniform sweep.
    #[test]
    fn triage_ranks_by_density_not_by_the_single_peak() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let mut rows = vec![
            // "spiker": one very high score among clean ones.
            census(1, "spiker", "x", 1000, 0.95),
        ];
        for i in 0..5 {
            rows.push(census(10 + i, "spiker", "x", 1000 + i, 0.30));
        }
        // "dense": four moderately-hot messages, lower peak.
        for i in 0..4 {
            rows.push(census(20 + i, "dense", "y", 1000 + i, 0.72));
        }
        db.record_census(&rows, 1).unwrap();

        let cells = db.triage_cells(0.65).unwrap();
        assert_eq!(
            cells[0].thread_identifier, "dense",
            "density outranks a lone spike"
        );
        assert!(
            cells[0].peak < cells[1].peak,
            "even though its peak is lower"
        );
        assert_eq!(cells[0].hot, 4);
        assert_eq!(cells[1].hot, 1);
    }

    /// The group-chat case from Peter: one abuser among many ordinary
    /// participants. The unit of ranking is (conversation, sender), so the
    /// abuser's cell is separable even though the thread average is mild.
    #[test]
    fn one_hot_sender_in_a_crowd_is_ranked_separately() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let mut rows = Vec::new();
        // Nine ordinary senders, one message each, all cold.
        for i in 0..9 {
            rows.push(census(
                100 + i,
                "groupchat",
                &format!("ordinary{i}"),
                1000 + i,
                0.25,
            ));
        }
        // One abuser, several hot.
        for i in 0..4 {
            rows.push(census(200 + i, "groupchat", "abuser", 1000 + i, 0.80));
        }
        db.record_census(&rows, 1).unwrap();

        let cells = db.triage_cells(0.65).unwrap();
        assert_eq!(cells[0].sender, "abuser");
        assert_eq!(cells[0].thread_identifier, "groupchat");
        assert_eq!(cells[0].hot, 4);
        // Every other cell in the same thread is a separate, cold row.
        assert!(cells[1..].iter().all(|c| c.hot == 0));
        assert!(
            cells
                .iter()
                .filter(|c| c.thread_identifier == "groupchat")
                .count()
                >= 10,
            "the thread is split per sender, not pooled"
        );
    }

    /// A slow burn that never spikes but climbs steadily has a positive
    /// trajectory — the escalation signal the pattern categories need.
    #[test]
    fn a_rising_cell_has_positive_trajectory() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let day = 86_400;
        let mut rows = Vec::new();
        for i in 0..10i64 {
            // 0.30 climbing to ~0.55 over ten days, never crossing 0.65.
            rows.push(census(
                i,
                "burn",
                "s",
                1000 + i * day,
                0.30 + 0.025 * i as f64,
            ));
        }
        // A flat cell for contrast.
        for i in 0..10i64 {
            rows.push(census(100 + i, "flat", "s", 1000 + i * day, 0.40));
        }
        db.record_census(&rows, 1).unwrap();

        let cells = db.triage_cells(0.65).unwrap();
        let burn = cells
            .iter()
            .find(|c| c.thread_identifier == "burn")
            .unwrap();
        let flat = cells
            .iter()
            .find(|c| c.thread_identifier == "flat")
            .unwrap();
        assert_eq!(burn.hot, 0, "it never crosses the threshold");
        assert!(
            burn.trajectory > 0.01,
            "but it is clearly rising: {}",
            burn.trajectory
        );
        assert!(flat.trajectory.abs() < 1e-6, "the flat cell is flat");
    }

    /// The census is incremental: a re-scan embeds only new messages.
    #[test]
    fn census_reports_which_ids_are_already_scored() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        db.record_census(
            &[
                census(1, "t", "s", 1000, 0.5),
                census(2, "t", "s", 1001, 0.6),
            ],
            1,
        )
        .unwrap();
        let seen = db.census_scored().unwrap();
        let ids: std::collections::HashSet<i64> = seen.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&1) && ids.contains(&2) && !ids.contains(&3));
        // The fingerprint rides the pair: a row only matches its own identity.
        assert!(seen.contains(&(1, "fp1".to_string())));
        assert!(!seen.contains(&(1, "fp-other".to_string())));
    }

    #[test]
    fn a_conversation_scope_matches_only_that_conversation() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db
            .begin_scan("m", (None, None), "thread:+15550001", 1)
            .unwrap();
        db.replace_findings(
            scan,
            &[
                NewFinding {
                    thread_identifier: Some("+15550001".into()),
                    service: Some("iMessage".into()),
                    ..finding("mine", Category::ScamFraud)
                },
                NewFinding {
                    thread_identifier: Some("+15550002".into()),
                    service: Some("iMessage".into()),
                    ..finding("theirs", Category::ScamFraud)
                },
            ],
            100,
        )
        .unwrap();

        let rows = db
            .list_findings_in_scope("thread:+15550001", None, None)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].fingerprint, "mine");
        assert_eq!(
            db.count_findings_in_scope("thread:+15550001", None, None)
                .unwrap(),
            1,
            "the live counter agrees with the list"
        );
        // And a whole-backup scope still sees both.
        assert_eq!(
            db.list_findings_in_scope("all", None, None).unwrap().len(),
            2
        );
    }

    /// A thread identifier is a phone number or an email, and an email may
    /// contain `_`, which LIKE treats as "any character". Matching the scope by
    /// pattern would quietly pull in a different person's conversation.
    #[test]
    fn a_thread_scope_does_not_wildcard_match_a_similar_identifier() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db
            .begin_scan("m", (None, None), "thread:a_b@x.com", 1)
            .unwrap();
        db.replace_findings(
            scan,
            &[
                NewFinding {
                    thread_identifier: Some("a_b@x.com".into()),
                    ..finding("exact", Category::ScamFraud)
                },
                NewFinding {
                    thread_identifier: Some("aXb@x.com".into()),
                    ..finding("wildcard-victim", Category::ScamFraud)
                },
            ],
            100,
        )
        .unwrap();
        let rows = db
            .list_findings_in_scope("thread:a_b@x.com", None, None)
            .unwrap();
        assert_eq!(rows.len(), 1, "only the exact identifier");
        assert_eq!(rows[0].fingerprint, "exact");
    }

    /// The `NOT LIKE 'thread:%'` guard on the other arms is load-bearing, not
    /// belt-and-braces: an identifier containing a comma makes the wrapped slug
    /// contain `,notes,`, and without the guard a conversation scan would adopt
    /// every note in the backup. TikTok identifiers already carry colons, so
    /// "identifiers are simple" is not a safe assumption.
    #[test]
    fn a_conversation_scope_never_adopts_notes_via_its_identifier() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let odd = "a,notes,b";
        let scan = db
            .begin_scan("m", (None, None), &format!("thread:{odd}"), 1)
            .unwrap();
        db.replace_findings(
            scan,
            &[
                NewFinding {
                    source_kind: SourceKind::Note,
                    thread_identifier: None,
                    service: None,
                    ..finding("a-note", Category::SelfHarm)
                },
                NewFinding {
                    thread_identifier: Some(odd.into()),
                    ..finding("the-thread", Category::ScamFraud)
                },
            ],
            100,
        )
        .unwrap();
        let rows = db
            .list_findings_in_scope(&format!("thread:{odd}"), None, None)
            .unwrap();
        assert_eq!(rows.len(), 1, "the note is not part of a conversation");
        assert_eq!(rows[0].fingerprint, "the-thread");
    }

    /// The severity floor (#445). Measured on both tiers, every false alarm
    /// the classifier produced on ordinary conversation was severity 1, and no
    /// labelled positive expects severity 1 — so the default view hides that
    /// tier. Hidden, never deleted: the count is always available and one flag
    /// brings them back.
    #[test]
    fn the_default_view_hides_severity_one_but_still_counts_it() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let rows: Vec<NewFinding> = (1..=3)
            .map(|sev| NewFinding {
                severity: sev,
                ..finding(&format!("s{sev}"), Category::ScamFraud)
            })
            .collect();
        db.replace_findings(scan, &rows, 100).unwrap();

        let page = |include_low| {
            db.list_findings_in_scope_page(
                "all",
                None,
                None,
                &FindingQuery {
                    include_low,
                    ..FindingQuery::default()
                },
                0,
                100,
            )
            .unwrap()
        };
        let shown: Vec<u8> = page(false).iter().map(|f| f.severity).collect();
        assert!(!shown.contains(&1), "severity 1 is hidden by default");
        assert_eq!(shown.len(), 2);
        assert_eq!(page(true).len(), 3, "and one flag brings it back");

        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(c.live, 2, "the pill promises only what the list produces");
        assert_eq!(c.concerning, 1, "but the hidden one is still counted");

        // The live progress counter is a separate query; it must agree, or the
        // ticker promises findings the panel then refuses to show (#59).
        assert_eq!(db.count_findings_in_scope("all", None, None).unwrap(), 2);

        // Asking for severity 1 explicitly beats the floor — a filter that
        // returned nothing would be a UI arguing with itself.
        let explicit = db
            .list_findings_in_scope_page(
                "all",
                None,
                None,
                &FindingQuery {
                    severity: Some(1),
                    ..FindingQuery::default()
                },
                0,
                100,
            )
            .unwrap();
        assert_eq!(explicit.len(), 1);
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
        // One of the two dismissed rows is severity 1, below the floor, so the
        // pill promises the one the list will actually produce.
        assert_eq!(counts.dismissed, 1);
        // `live` is the DEFAULT view, which hides severity 1; the two together
        // are every undismissed finding in the seed.
        assert_eq!(counts.live + counts.concerning, 58);

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
            sender: None,
            content_key: None,
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
        let scan2 = db.begin_scan("m", (None, None), "notes", 200).unwrap();
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
        // A different SCOPE: begin_scan reuses the row for a configuration it
        // already has, so "all" twice would be one scan (#171).
        let scan2 = db.begin_scan("m", (None, None), "notes", 200).unwrap();
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

    /// A finding's rank must be the index the paged list actually puts it at —
    /// under every filter and both sort orders. If the two disagree, returning to
    /// a finding scrolls to the wrong row, which is worse than not scrolling.
    ///
    /// Checked by asking for the rank and then fetching a one-row page at that
    /// offset: the row that comes back must be the finding. That compares the two
    /// queries against each other rather than against my belief about the order.
    #[test]
    fn a_findings_rank_matches_the_page_it_lands_on() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();

        // Deliberately messy: shared severities, shared timestamps, a NULL
        // timestamp, notes with no thread — every tie-break and NULL the
        // ordering has to cope with.
        let new_f = |sev: u8, at: Option<i64>, kind: SourceKind, thread: Option<&str>, fp: &str| {
            NewFinding {
                source_kind: kind,
                source_id: Some(1),
                thread_identifier: thread.map(|t| t.to_string()),
                occurred_at: at,
                fingerprint: fp.into(),
                category: Category::ScamFraud,
                severity: sev,
                rationale: "x".into(),
                service: Some("iMessage".into()),
                sender: None,
                content_key: None,
            }
        };
        let findings = vec![
            new_f(3, Some(500), SourceKind::Message, Some("t1"), "a"),
            new_f(3, Some(500), SourceKind::Message, Some("t1"), "b"),
            new_f(2, Some(400), SourceKind::Message, Some("t2"), "c"),
            new_f(1, None, SourceKind::Note, None, "d"),
            new_f(2, Some(400), SourceKind::Note, None, "e"),
        ];
        db.replace_findings(scan, &findings, 105).unwrap();

        for sort in [FindingSort::Severity, FindingSort::Date] {
            for desc in [true, false] {
                for group in [true, false] {
                    let q = FindingQuery {
                        severity: None,
                        include_dismissed: true,
                        // This test is about rank/order, not the default
                        // view's severity floor — it wants every row.
                        include_low: true,
                        sort,
                        desc,
                        group_by_thread: group,
                        exclude_stale: false,
                    };
                    let all = db
                        .list_findings_in_scope_page("all", None, None, &q, 0, 100)
                        .unwrap();
                    assert_eq!(all.len(), 5);
                    for (i, row) in all.iter().enumerate() {
                        let rank = db
                            .finding_rank("all", None, None, &q, row.id)
                            .unwrap()
                            .unwrap_or_else(|| panic!("no rank for finding {}", row.id));
                        assert_eq!(
                            rank, i as i64,
                            "rank disagrees with the page order (sort={sort:?} desc={desc} group={group})"
                        );
                        // And the page at that offset really is this row.
                        let one = db
                            .list_findings_in_scope_page("all", None, None, &q, rank, 1)
                            .unwrap();
                        assert_eq!(one[0].id, row.id);
                    }
                }
            }
        }
    }

    /// A finding the current filter excludes has no rank, so the caller can say
    /// so instead of scrolling somewhere arbitrary.
    #[test]
    fn a_filtered_out_finding_has_no_rank() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.replace_findings(
            scan,
            &[NewFinding {
                source_kind: SourceKind::Message,
                source_id: Some(1),
                thread_identifier: Some("t".into()),
                occurred_at: Some(500),
                fingerprint: "a".into(),
                category: Category::ScamFraud,
                severity: 1,
                rationale: "x".into(),
                service: Some("iMessage".into()),
                sender: None,
                content_key: None,
            }],
            105,
        )
        .unwrap();
        // The row is severity 1, which the default view hides, so this test
        // asks for it explicitly before checking how a FILTER affects rank.
        let seen_all = FindingQuery {
            include_low: true,
            ..FindingQuery::default()
        };
        let id = db
            .list_findings_in_scope_page("all", None, None, &seen_all, 0, 10)
            .unwrap()[0]
            .id;

        // Filtering to a severity it does not have removes it from the order.
        let q = FindingQuery {
            severity: Some(3),
            include_low: true,
            ..FindingQuery::default()
        };
        assert_eq!(db.finding_rank("all", None, None, &q, id).unwrap(), None);
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
        // Simulate a crash: never finished. Beginning a DIFFERENT scan repairs
        // it — a different scope, or begin_scan would reuse the same row and
        // there would be no stranded row left to repair.
        let live = db.begin_scan("m", (None, None), "notes", 200).unwrap();
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

    /// Deleting an older scan must not change what a newer, overlapping scan
    /// counts — the bug in #218.
    ///
    /// Findings are counted by SCOPE, so a finding classified by scan 1 is also
    /// displayed by scan 2 when scan 2's sources and range contain it. Deleting
    /// by `scan_id` therefore destroyed rows scan 2 was showing, and took the
    /// cached classification with them.
    ///
    /// The old implementation is kept as `delete_scan_by_ownership` so this test
    /// demonstrates the bug rather than describing it.
    #[test]
    fn deleting_an_older_scan_keeps_what_a_newer_one_counts() {
        let finding = || NewFinding {
            source_kind: SourceKind::Message,
            source_id: Some(1),
            thread_identifier: Some("t".into()),
            occurred_at: Some(1_000),
            fingerprint: "fp1".into(),
            category: Category::ScamFraud,
            severity: 2,
            rationale: "x".into(),
            service: Some("iMessage".into()),
            sender: None,
            content_key: None,
        };

        // Two scans over the SAME data. The first classifies and owns the rows;
        // the second reuses the cached chunk, so nothing is attributed to it.
        let build = || {
            let mut db = AnalysisDb::open_in_memory().unwrap();
            let old = db.begin_scan("m", (None, None), "all", 100).unwrap();
            db.record_chunk(old, "k", "fp1", ChunkStatus::Done, true, 101)
                .unwrap();
            db.replace_findings(old, &[finding()], 105).unwrap();
            let new = db
                .begin_scan("m", (Some(1), Some(9_999)), "all", 200)
                .unwrap();
            (db, old, new)
        };

        let count_for = |db: &AnalysisDb, id: i64| {
            db.list_scans(50)
                .unwrap()
                .into_iter()
                .find(|r| r.id == id)
                .map(|r| r.findings)
        };

        // Both scans count the one finding, by scope.
        let (db, old, new) = build();
        assert_eq!(count_for(&db, old), Some(1));
        assert_eq!(count_for(&db, new), Some(1), "scope counting, not scan_id");

        // THE BUG: the ownership-based delete drops the newer scan's count too.
        let (db, old, new) = build();
        db.delete_scan_by_ownership(old).unwrap();
        assert_eq!(
            count_for(&db, new),
            Some(0),
            "this is the bug being fixed — if it is no longer 0, the old \
             implementation has changed and this test no longer demonstrates anything"
        );

        // THE FIX: scope-aware delete leaves the newer scan intact.
        let (db, old, new) = build();
        db.delete_scan(old).unwrap();
        assert!(
            db.scan_by_id(old).unwrap().is_none(),
            "the scan itself is gone"
        );
        assert_eq!(
            count_for(&db, new),
            Some(1),
            "the surviving scan still counts the finding it displays"
        );

        // And the cached classification survives, so a re-scan does not have to
        // pay the model cost again to rediscover what was already known.
        assert!(
            db.chunk_is_done("k", "fp1").unwrap(),
            "chunk_progress must survive — it is keyed on chunk_key+fingerprint, \
             not on the scan that happened to write it"
        );
    }

    /// Deleting the LAST scan leaves nothing orphaned.
    #[test]
    fn deleting_the_last_scan_removes_its_findings() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let only = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.record_chunk(only, "k", "fp", ChunkStatus::Done, false, 101)
            .unwrap();
        db.replace_findings(
            only,
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
                sender: None,
                content_key: None,
            }],
            105,
        )
        .unwrap();

        db.delete_scan(only).unwrap();
        for table in ["content_findings", "chunk_progress", "scans"] {
            let n: i64 = db
                .conn()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} must be empty after the last scan is deleted");
        }
    }

    /// A finding outside every surviving scope is removed rather than stranded.
    #[test]
    fn deleting_a_scan_drops_findings_no_survivor_covers() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        // Scan over everything, with a NOTE finding.
        let broad = db.begin_scan("m", (None, None), "all", 100).unwrap();
        db.replace_findings(
            broad,
            &[NewFinding {
                source_kind: SourceKind::Note,
                source_id: Some(7),
                thread_identifier: None,
                occurred_at: Some(500),
                fingerprint: "fpn".into(),
                category: Category::SelfHarm,
                severity: 3,
                rationale: "x".into(),
                service: None,
                sender: None,
                content_key: None,
            }],
            105,
        )
        .unwrap();
        // The only survivor covers messages, so the note is in nobody's scope.
        let messages_only = db.begin_scan("m", (None, None), "messages", 200).unwrap();

        db.delete_scan(broad).unwrap();
        let n: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM content_findings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "a note finding survives no messages-only scope");
        assert_eq!(
            db.list_scans(50)
                .unwrap()
                .into_iter()
                .find(|r| r.id == messages_only)
                .map(|r| r.findings),
            Some(0)
        );
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
                sender: None,
                content_key: None,
            }],
            105,
        )
        .unwrap();

        // A second scan is left untouched, proving the delete is scoped by id.
        // A different SCOPE, so this is genuinely a second scan: begin_scan
        // reuses the row for a configuration it already has (#171).
        let keep = db.begin_scan("m", (None, None), "notes", 200).unwrap();
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
    /// A fixed clock, so the timeline window is the same on every machine.
    const NOW: i64 = 1_760_000_000; // 2025-10-09

    #[test]
    fn charts_count_every_finding_not_the_page_the_report_renders() {
        // The trap #66 names: the panel renders at most 500 rows and the
        // narrative at most 100. A chart built from either would describe a
        // subset while looking like it described the scan.
        let db = seeded_findings(600);
        // Every severity, so the cap is what limits the page rather than the
        // default view's floor — capping is what this test is about.
        let q = FindingQuery {
            include_low: true,
            ..FindingQuery::default()
        };
        let page = db
            .list_findings_in_scope_page("all", None, None, &q, 0, 500)
            .unwrap();
        let a = db.finding_analytics("all", None, None, &q, NOW).unwrap();

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
            // Anchored so the span ENDS at the clock: a timestamp in the future
            // is not datable (see TIMELINE_START), which is the point of the
            // window and would otherwise silently shorten these spans.
            let start = NOW - span_days * DAY;
            let mut at: Vec<Option<i64>> = (0..20)
                .map(|i| Some(start + i * span_days * DAY / 20))
                .collect();
            at.push(Some(NOW));
            dated(&mut db, scan, &at);

            let a = db
                .finding_analytics("all", None, None, &FindingQuery::default(), NOW)
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
            .finding_analytics("all", None, None, &FindingQuery::default(), NOW)
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
            .finding_analytics("all", None, None, &FindingQuery::default(), NOW)
            .unwrap();
        assert_eq!(a.charted, 5);
        assert_eq!(a.undated, 3);
        assert_eq!(a.over_time.iter().map(|b| b.total()).sum::<i64>(), 2);
        // The other charts keep them: only the time axis can't place them.
        assert_eq!(a.by_category.iter().map(|b| b.total()).sum::<i64>(), 5);
    }

    #[test]
    fn one_bad_timestamp_cannot_stretch_the_axis_across_a_century() {
        // Apple stores seconds since 2001; read as Unix time that lands in 1970,
        // and a zeroed column lands there too. The bucket unit is chosen from
        // the span, and Year is the coarsest unit there is — so before the
        // window, ONE such finding turned a year of real findings into a single
        // bar at the right-hand edge, with fifty-odd empty ones beside it.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let mut at: Vec<Option<i64>> = (0..10)
            .map(|i| Some(NOW - 300 * DAY + i * 30 * DAY))
            .collect();
        at.push(Some(0)); // 1970: a zeroed column
        at.push(Some(978_307_200)); // 2001: the Core Data epoch read as Unix time
        at.push(Some(NOW + 400 * DAY)); // and one in the future
        dated(&mut db, scan, &at);

        let a = db
            .finding_analytics("all", None, None, &FindingQuery::default(), NOW)
            .unwrap();

        assert_eq!(a.charted, 13, "every finding is still counted");
        assert_eq!(a.undated, 3, "the three that cannot be placed say so");
        assert_eq!(
            a.unit,
            TimeUnit::Month,
            "the unit follows the ten real findings, not the outliers"
        );
        assert!(
            a.over_time.len() <= 11,
            "the axis stays readable: {} buckets",
            a.over_time.len()
        );
        // And the population still reconciles.
        assert_eq!(
            a.over_time.iter().map(|b| b.total()).sum::<i64>() + a.undated,
            a.charted
        );
    }

    #[test]
    fn the_time_axis_is_bounded_by_the_window_not_by_a_render_guard() {
        // The whole point of TIMELINE_START: with the window closed, the widest
        // possible span is 2007→now, which selects Year, which is one bar per
        // year. No caller can produce an axis that needs truncating.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let at: Vec<Option<i64>> = (0..40)
            .map(|i| Some(TIMELINE_START + i * (NOW - TIMELINE_START) / 40))
            .collect();
        dated(&mut db, scan, &at);

        let a = db
            .finding_analytics("all", None, None, &FindingQuery::default(), NOW)
            .unwrap();
        assert_eq!(a.unit, TimeUnit::Year);
        let years = (NOW - TIMELINE_START) / (365 * DAY) + 2;
        assert!(
            (a.over_time.len() as i64) <= years,
            "{} buckets for a {years}-year window",
            a.over_time.len()
        );
        assert_eq!(a.undated, 0);
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

        // Every severity band is the point of this assertion, so the chart is
        // asked for all of them rather than the default view's top two.
        let a = db
            .finding_analytics(
                "all",
                None,
                None,
                &FindingQuery {
                    include_low: true,
                    ..FindingQuery::default()
                },
                NOW,
            )
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
            .finding_analytics("all", None, None, &FindingQuery::default(), NOW)
            .unwrap();
        assert_eq!(a.charted, 3, "the dismissed one is not drawn");
        assert_eq!(a.dismissed, 1, "but the report can say so");
        assert_eq!(a.by_category[0].total(), 3);
    }

    #[test]
    fn the_dismissed_count_is_what_the_charts_left_out_and_nothing_else() {
        // The disclosure beside the charts reads "N dismissed as false positives
        // and left out of every chart". That sentence has to be true in every
        // state the panel can reach — including "Show dismissed", where they are
        // not left out at all, and a severity filter, which the count used to
        // ignore.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let rows: Vec<NewFinding> = (0..6)
            .map(|i| NewFinding {
                severity: (i % 3 + 1) as u8,
                occurred_at: Some(MON_2024_03_11 + i),
                ..finding(&format!("x{i}"), Category::SelfHarm)
            })
            .collect();
        db.replace_findings(scan, &rows, 1).unwrap();
        // One severity-1 (x0) and one severity-3 (x2) dismissed.
        db.set_dismissed("x0", Category::SelfHarm, true, 2).unwrap();
        db.set_dismissed("x2", Category::SelfHarm, true, 2).unwrap();

        // This case is explicitly about a severity-1 dismissal versus a
        // severity-3 one, so it asks for every severity.
        let all = db
            .finding_analytics(
                "all",
                None,
                None,
                &FindingQuery {
                    include_low: true,
                    ..FindingQuery::default()
                },
                NOW,
            )
            .unwrap();
        assert_eq!((all.charted, all.dismissed), (4, 2));

        // Filtered to serious: only the serious dismissal was left out.
        let serious = db
            .finding_analytics(
                "all",
                None,
                None,
                &FindingQuery {
                    severity: Some(3),
                    ..Default::default()
                },
                NOW,
            )
            .unwrap();
        assert_eq!(
            (serious.charted, serious.dismissed),
            (1, 1),
            "a severity filter narrows what was left out, not just what was drawn"
        );

        // Showing dismissed: they are in the charts, so nothing was left out.
        let shown = db
            .finding_analytics(
                "all",
                None,
                None,
                &FindingQuery {
                    include_dismissed: true,
                    // Same "every severity" basis as the `all` case above, so
                    // the 6 here is the same 6.
                    include_low: true,
                    ..Default::default()
                },
                NOW,
            )
            .unwrap();
        assert_eq!(
            (shown.charted, shown.dismissed),
            (6, 0),
            "nothing is 'left out' when the caller asked for it"
        );
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
            .finding_analytics("all", None, None, &FindingQuery::default(), NOW)
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
                include_low: false,
                ..Default::default()
            },
            FindingQuery {
                exclude_stale: true,
                severity: Some(1),
                ..Default::default()
            },
        ] {
            let listed = db.count_findings_matching("all", None, None, &q).unwrap();
            let charted = db
                .finding_analytics("all", None, None, &q, NOW)
                .unwrap()
                .charted;
            assert_eq!(listed, charted, "{q:?}");
        }
    }
    #[test]
    fn re_running_a_scope_updates_its_row_instead_of_adding_one() {
        // The history is a list of the scans you have set up, not of every time
        // one ran — otherwise showing the scope as the title produces rows with
        // identical titles differing only by a date.
        let db = AnalysisDb::open_in_memory().unwrap();
        let first = db.begin_scan("m", (None, None), "messages", 100).unwrap();
        db.finish_scan(first, ScanStatus::Completed, 150).unwrap();

        let again = db.begin_scan("m", (None, None), "messages", 300).unwrap();
        assert_eq!(again, first, "the same configuration is the same row");
        assert_eq!(db.list_scans(10).unwrap().len(), 1);

        let row = db.scan_by_id(first).unwrap().unwrap();
        assert_eq!(row.status, "running", "it is running again");
        assert_eq!(row.started_at, 300, "and the date is the LATEST run");
        assert_eq!(row.finished_at, None, "the old outcome does not linger");

        // A different scope is a different scan.
        let other = db.begin_scan("m", (None, None), "notes", 400).unwrap();
        assert_ne!(other, first);
        assert_eq!(db.list_scans(10).unwrap().len(), 2);
    }

    #[test]
    fn re_running_clears_a_previous_failure() {
        // The warning badge is driven by `error`. Leaving it set would mark a
        // scan that is currently running fine as failed.
        let db = AnalysisDb::open_in_memory().unwrap();
        let id = db.begin_scan("m", (None, None), "messages", 100).unwrap();
        db.finish_scan_with(id, ScanStatus::Failed, 150, Some("server died"))
            .unwrap();
        assert!(db.scan_by_id(id).unwrap().unwrap().error.is_some());

        db.begin_scan("m", (None, None), "messages", 300).unwrap();
        assert_eq!(
            db.scan_by_id(id).unwrap().unwrap().error,
            None,
            "a re-run starts clean"
        );
    }

    #[test]
    fn a_failed_scan_records_why() {
        // The badge in the history promises a reason on hover. Before this the
        // engine had the error in hand and dropped it, so the only honest
        // tooltip would have been "it failed" — which the status already said.
        let db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.finish_scan_with(
            scan,
            ScanStatus::Failed,
            200,
            Some("model server exited before the first chunk"),
        )
        .unwrap();

        let row = db.scan_by_id(scan).unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert_eq!(
            row.error.as_deref(),
            Some("model server exited before the first chunk")
        );
    }

    #[test]
    fn only_a_failure_carries_a_reason() {
        // Cancelled and interrupted explain themselves — you stopped it, or the
        // app closed. Inventing text for them would be noise in a tooltip.
        let db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.finish_scan(scan, ScanStatus::Cancelled, 200).unwrap();
        assert_eq!(db.scan_by_id(scan).unwrap().unwrap().error, None);
    }

    /// Reading a finding must not put it out of a rule's reach. `mark_seen`
    /// writes a verdict row with a NULL verdict, so a guard that tested for the
    /// ROW rather than for a decision made every finding the reviewer had
    /// opened permanently invisible to every standing rule — including the rule
    /// they had just made from that very finding.
    #[test]
    fn a_rule_covers_a_finding_the_reviewer_has_already_read() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(scan, &[finding("fp1", Category::ScamFraud)], 100)
            .unwrap();
        db.mark_seen("fp1", Category::ScamFraud, 150).unwrap();
        let n = db
            .add_suppression("category", "scam-fraud", "scam-fraud", None, None, 200)
            .unwrap();
        assert_eq!(
            n, 1,
            "a rule must cover a finding the reviewer has looked at"
        );
    }

    #[test]
    fn a_rule_dismisses_rather_than_hides() {
        // The safety property. A conversation marked fine today may not be fine
        // next month — this app exists to catch exactly that — so a rule must
        // never make a finding vanish. It pre-judges it VISIBLY: counted as
        // dismissed, reachable behind "Show dismissed", carrying the reason.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let rows: Vec<NewFinding> = (0..3)
            .map(|i| NewFinding {
                thread_identifier: Some(if i == 0 { "safe-chat" } else { "other" }.into()),
                ..finding(&format!("r{i}"), Category::ScamFraud)
            })
            .collect();
        db.replace_findings(scan, &rows, 100).unwrap();

        let n = db
            .add_suppression(
                "thread",
                "safe-chat",
                "scam-fraud",
                None,
                Some("Work group, all jokes"),
                200,
            )
            .unwrap();
        assert_eq!(n, 1, "one existing finding matched");

        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(c.live, 2, "the rule removed it from the default view");
        assert_eq!(c.dismissed, 1, "but it is counted, not gone");
        let reason: String = db
            .conn()
            .query_row(
                "SELECT reason FROM finding_verdicts WHERE fingerprint = 'r0'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "Work group, all jokes", "and says why");
    }

    #[test]
    fn a_rule_covers_findings_that_did_not_exist_when_it_was_made() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        db.add_suppression(
            "category",
            "scam-fraud",
            "scam-fraud",
            None,
            Some("Junk texts"),
            100,
        )
        .unwrap();

        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(
            scan,
            &[
                finding("new1", Category::ScamFraud),
                finding("new2", Category::SelfHarm),
            ],
            200,
        )
        .unwrap();

        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(
            (c.live, c.dismissed),
            (1, 1),
            "a rule set before the scan still applies to what the scan found"
        );
    }

    #[test]
    fn a_rule_never_overwrites_a_decision_made_by_hand() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(scan, &[finding("fp1", Category::ScamFraud)], 100)
            .unwrap();
        // The user looked at it and kept it, explicitly.
        db.set_verdict(
            "fp1",
            Category::ScamFraud,
            None,
            Some("Checked, it is real"),
            150,
        )
        .unwrap();

        db.add_suppression(
            "category",
            "scam-fraud",
            "scam-fraud",
            None,
            Some("Junk"),
            200,
        )
        .unwrap();

        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(
            c.live, 1,
            "a broad rule must not overrule a specific judgement"
        );
        let reason: String = db
            .conn()
            .query_row("SELECT reason FROM finding_verdicts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reason, "Checked, it is real");
    }

    #[test]
    fn removing_a_rule_takes_back_what_it_dismissed_but_not_your_own_judgement() {
        // The earlier contract left them dismissed, reasoning that resurrecting
        // a hundred findings SILENTLY would be its own surprise. The silence was
        // the problem, not the resurrection: a rule whose effect outlives it
        // leaves a blind spot with nothing left pointing at it. Removal now
        // takes back exactly what the rule dismissed and reports the number.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(scan, &[finding("fp1", Category::ScamFraud)], 100)
            .unwrap();
        db.add_suppression("category", "scam-fraud", "scam-fraud", None, None, 200)
            .unwrap();
        assert_eq!(
            db.count_findings_breakdown("all", None, None).unwrap().live,
            0
        );

        let back = db
            .remove_suppression("category", "scam-fraud", Some("scam-fraud"), None, 300)
            .unwrap();
        assert_eq!(back, 1, "and it says how many came back");
        assert!(db.list_suppressions().unwrap().is_empty());
        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(c.dismissed, 0, "the rule's verdict went with the rule");
        assert_eq!(c.live, 1);
    }

    /// The half that must NOT move: a decision made by hand outlives every
    /// rule, because removing your rule is not undoing your judgement.
    #[test]
    fn removing_a_rule_leaves_a_dismissal_you_made_yourself() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(scan, &[finding("mine", Category::ScamFraud)], 100)
            .unwrap();
        db.set_verdict(
            "mine",
            Category::ScamFraud,
            Some("dismissed"),
            Some("I checked"),
            150,
        )
        .unwrap();
        db.add_suppression("category", "scam-fraud", "scam-fraud", None, None, 200)
            .unwrap();

        let back = db
            .remove_suppression("category", "scam-fraud", Some("scam-fraud"), None, 300)
            .unwrap();
        assert_eq!(back, 0, "nothing of yours was taken back");
        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(c.dismissed, 1);
        let reason: Option<String> = db
            .conn()
            .query_row(
                "SELECT reason FROM finding_verdicts WHERE fingerprint = 'mine'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason.as_deref(), Some("I checked"), "your words, still");
    }

    /// A finding two rules cover stays dismissed when only one is removed.
    #[test]
    fn removing_one_of_two_overlapping_rules_keeps_the_finding_dismissed() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(
            scan,
            &[NewFinding {
                thread_identifier: Some("gran".into()),
                ..finding("both", Category::ScamFraud)
            }],
            100,
        )
        .unwrap();
        db.add_suppression("category", "scam-fraud", "scam-fraud", None, None, 200)
            .unwrap();
        db.add_suppression("thread", "gran", "scam-fraud", None, None, 200)
            .unwrap();

        let back = db
            .remove_suppression("thread", "gran", Some("scam-fraud"), None, 300)
            .unwrap();
        assert_eq!(back, 0, "the category rule still covers it");
        assert_eq!(
            db.count_findings_breakdown("all", None, None)
                .unwrap()
                .dismissed,
            1
        );
    }

    /// What the panel reports must be what the engine acts on.
    #[test]
    fn a_rule_reports_how_many_it_is_swallowing() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let rows: Vec<NewFinding> = (0..3)
            .map(|i| NewFinding {
                thread_identifier: Some(if i < 2 { "gran" } else { "other" }.into()),
                ..finding(&format!("f{i}"), Category::ScamFraud)
            })
            .collect();
        db.replace_findings(scan, &rows, 100).unwrap();
        db.add_suppression("thread", "gran", "scam-fraud", None, None, 200)
            .unwrap();
        db.add_suppression("thread", "nobody", "scam-fraud", None, None, 200)
            .unwrap();

        let rules = db.list_suppressions().unwrap();
        let by_value: std::collections::HashMap<_, _> =
            rules.iter().map(|r| (r.value.as_str(), r.hits)).collect();
        assert_eq!(by_value["gran"], 2);
        assert_eq!(
            by_value["nobody"], 0,
            "a rule that has never matched shows 0"
        );
    }

    #[test]
    fn seen_is_recorded_once_and_never_taken_back() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(scan, &[finding("fp1", Category::SelfHarm)], 100)
            .unwrap();
        let counts = |db: &AnalysisDb| db.count_findings_breakdown("all", None, None).unwrap();

        assert_eq!(counts(&db).unread, 1, "nobody has looked yet");
        db.mark_seen("fp1", Category::SelfHarm, 200).unwrap();
        assert_eq!(counts(&db).unread, 0);

        // A second look does not move the first: seen_at is when you FIRST read
        // it, and re-opening a row is not new information.
        db.mark_seen("fp1", Category::SelfHarm, 999).unwrap();
        let first: i64 = db
            .conn()
            .query_row("SELECT seen_at FROM finding_verdicts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(first, 200);
    }

    #[test]
    fn dismissing_does_not_forget_that_a_finding_was_read() {
        // The trap in writing a verdict: INSERT OR REPLACE drops seen_at, so
        // dismissing would mark something unread again — and undismissing would
        // leave it unread, which is not what undoing a verdict means.
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(scan, &[finding("fp1", Category::SelfHarm)], 100)
            .unwrap();

        db.mark_seen("fp1", Category::SelfHarm, 200).unwrap();
        db.set_dismissed("fp1", Category::SelfHarm, true, 300)
            .unwrap();
        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!((c.dismissed, c.unread), (1, 0), "dismissed, and still read");

        db.set_dismissed("fp1", Category::SelfHarm, false, 400)
            .unwrap();
        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(
            (c.dismissed, c.live, c.unread),
            (0, 1, 0),
            "undismissing restores it to the list WITHOUT marking it unread"
        );
    }

    /// Three findings with the same content from different people, so the
    /// sender is the only thing that can distinguish them. This is the whole
    /// point of #404: grandmother's ❤️ is covered, a stranger's identical ❤️ is
    /// not.
    fn hearts_from(db: &mut AnalysisDb) -> i64 {
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        let rows: Vec<NewFinding> = [("gran", "gran-fp"), ("stranger", "str-fp")]
            .iter()
            .map(|(who, fp)| NewFinding {
                thread_identifier: Some((*who).into()),
                sender: Some((*who).into()),
                content_key: Some("\u{2764}".into()),
                ..finding(fp, Category::HarassmentBullying)
            })
            .collect();
        db.replace_findings(scan, &rows, 100).unwrap();
        scan
    }

    fn live_fingerprints(db: &AnalysisDb) -> Vec<String> {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT fingerprint FROM content_findings f
                 WHERE NOT EXISTS (SELECT 1 FROM finding_verdicts v
                                   WHERE v.fingerprint = f.fingerprint
                                     AND v.verdict = 'dismissed')
                 ORDER BY fingerprint",
            )
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn a_content_rule_for_one_sender_leaves_the_same_content_from_another() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        hearts_from(&mut db);

        let n = db
            .add_suppression(
                "content+sender",
                "\u{2764}",
                "harassment-bullying",
                Some("gran"),
                None,
                200,
            )
            .unwrap();
        assert_eq!(n, 1, "only grandmother's heart matched");
        assert_eq!(
            live_fingerprints(&db),
            vec!["str-fp".to_string()],
            "the stranger's identical heart is still flagged"
        );
    }

    #[test]
    fn a_content_rule_for_anyone_covers_both() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        hearts_from(&mut db);
        let n = db
            .add_suppression(
                "content+any",
                "\u{2764}",
                "harassment-bullying",
                None,
                None,
                200,
            )
            .unwrap();
        assert_eq!(n, 2);
        assert!(live_fingerprints(&db).is_empty());
    }

    /// A finding whose sender was never recorded — every finding written before
    /// #402 — must not inherit an exemption somebody made for a different
    /// person. NULL means unknown, never "matches anyone".
    #[test]
    fn an_unknown_sender_never_inherits_someone_elses_rule() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(
            scan,
            &[NewFinding {
                thread_identifier: Some("gran".into()),
                sender: None, // pre-v11 row
                content_key: Some("\u{2764}".into()),
                ..finding("legacy-fp", Category::HarassmentBullying)
            }],
            100,
        )
        .unwrap();

        let n = db
            .add_suppression(
                "content+sender",
                "\u{2764}",
                "harassment-bullying",
                Some("gran"),
                None,
                200,
            )
            .unwrap();
        assert_eq!(n, 0, "an unknown sender is not grandmother");
        assert_eq!(live_fingerprints(&db), vec!["legacy-fp".to_string()]);
    }

    /// A finding with no content key — too long to generalize, or written
    /// before v12 — is unreachable by a content rule by construction.
    #[test]
    fn a_finding_without_a_content_key_is_unreachable_by_a_content_rule() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(
            scan,
            &[NewFinding {
                sender: Some("gran".into()),
                content_key: None,
                ..finding("long-fp", Category::HarassmentBullying)
            }],
            100,
        )
        .unwrap();
        let n = db
            .add_suppression(
                "content+any",
                "\u{2764}",
                "harassment-bullying",
                None,
                None,
                200,
            )
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(live_fingerprints(&db), vec!["long-fp".to_string()]);
    }

    /// The same conversation, the same content, two different people — two
    /// rules. The pre-v12 UNIQUE could not hold both.
    #[test]
    fn two_senders_can_carry_their_own_rule_for_the_same_content() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        hearts_from(&mut db);
        for who in ["gran", "stranger"] {
            db.add_suppression(
                "content+sender",
                "\u{2764}",
                "harassment-bullying",
                Some(who),
                None,
                200,
            )
            .unwrap();
        }
        assert_eq!(db.list_suppressions().unwrap().len(), 2);
        assert!(live_fingerprints(&db).is_empty());
    }

    /// #394. The bug: dismissing one heart emoji "for this conversation"
    /// pre-dismissed every future finding of every category from that number,
    /// including a threat months later.
    #[test]
    fn a_conversation_rule_covers_only_the_category_it_was_made_for() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(
            scan,
            &[
                NewFinding {
                    thread_identifier: Some("gran".into()),
                    ..finding("hearts", Category::HarassmentBullying)
                },
                NewFinding {
                    thread_identifier: Some("gran".into()),
                    ..finding("threat", Category::ThreatViolence)
                },
            ],
            100,
        )
        .unwrap();

        let n = db
            .add_suppression("thread", "gran", "harassment-bullying", None, None, 200)
            .unwrap();
        assert_eq!(n, 1, "only the harassment finding matched");

        let dismissed: Vec<String> = db
            .conn()
            .prepare("SELECT fingerprint FROM finding_verdicts WHERE verdict = 'dismissed'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(dismissed, vec!["hearts".to_string()]);
        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(c.live, 1, "the threat is still in the default view");
    }

    /// A standing rule is a blanket judgement made before the finding existed.
    /// The most serious tier is where that is least defensible, so a rule never
    /// reaches it — the reviewer can still dismiss one by hand, having seen it.
    #[test]
    fn a_rule_never_dismisses_the_most_serious_findings() {
        let mut db = AnalysisDb::open_in_memory().unwrap();
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(
            scan,
            &[
                NewFinding {
                    thread_identifier: Some("gran".into()),
                    severity: 3,
                    ..finding("grave", Category::ThreatViolence)
                },
                NewFinding {
                    thread_identifier: Some("gran".into()),
                    severity: 2,
                    ..finding("mild", Category::ThreatViolence)
                },
            ],
            100,
        )
        .unwrap();

        let n = db
            .add_suppression("thread", "gran", "threat-violence", None, None, 200)
            .unwrap();
        assert_eq!(n, 1, "the severity-3 finding was left alone");
        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(c.live, 1);
        let live: String = db
            .conn()
            .query_row(
                "SELECT fingerprint FROM content_findings f
                 WHERE NOT EXISTS (SELECT 1 FROM finding_verdicts v
                                   WHERE v.fingerprint = f.fingerprint
                                     AND v.verdict = 'dismissed')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, "grave");
    }

    /// The migration users will actually run. A pre-v10 rule kept the breadth
    /// it was made with: narrowing it is impossible (which category the user
    /// was looking at was never recorded) and deleting it would resurface
    /// findings they deliberately set aside.
    #[test]
    fn a_v9_rule_keeps_its_every_category_breadth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("analysis.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            // The v9 shape by hand: no category column, UNIQUE(scope, value).
            conn.execute_batch(
                "DROP TABLE IF EXISTS suppressions;
                 CREATE TABLE suppressions (
                     id         INTEGER PRIMARY KEY,
                     scope      TEXT NOT NULL,
                     value      TEXT NOT NULL,
                     reason     TEXT,
                     created_at INTEGER NOT NULL,
                     UNIQUE(scope, value)
                 );
                 INSERT INTO suppressions (scope, value, reason, created_at)
                 VALUES ('thread', 'gran', 'Family group', 4242);",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 9).unwrap();
        }

        let mut db = AnalysisDb::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);

        let rules = db.list_suppressions().unwrap();
        assert_eq!(rules.len(), 1, "the rule survived the rebuild");
        assert_eq!(rules[0].category, None, "it covers every category");
        assert_eq!(rules[0].sender, "", "and no particular sender");
        assert_eq!(rules[0].reason.as_deref(), Some("Family group"));

        // Its old breadth still applies: two different categories, both covered.
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(
            scan,
            &[
                NewFinding {
                    thread_identifier: Some("gran".into()),
                    ..finding("a", Category::HarassmentBullying)
                },
                NewFinding {
                    thread_identifier: Some("gran".into()),
                    ..finding("b", Category::ScamFraud)
                },
            ],
            100,
        )
        .unwrap();
        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(c.dismissed, 2, "a grandfathered rule still covers both");

        // And the new shape is in place: the same conversation can now carry a
        // second, narrow rule, which the old UNIQUE(scope, value) forbade.
        db.add_suppression("thread", "gran", "self-harm", None, None, 300)
            .unwrap();
        assert_eq!(db.list_suppressions().unwrap().len(), 2);
    }

    #[test]
    fn a_v6_store_keeps_its_dismissals_when_it_gains_verdicts() {
        // The migration users will actually run. Losing a dismissal would
        // resurrect a false positive they had already judged.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("analysis.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            // The v6 shape written out by hand: SCHEMA_V1 is the CURRENT schema
            // and has already moved on, so building the "old" store from it
            // would test nothing.
            conn.execute_batch(
                "DROP TABLE IF EXISTS finding_verdicts;
                 CREATE TABLE dismissals (
                     fingerprint  TEXT NOT NULL,
                     category     TEXT NOT NULL,
                     dismissed_at INTEGER NOT NULL,
                     PRIMARY KEY (fingerprint, category)
                 );
                 INSERT INTO dismissals (fingerprint, category, dismissed_at)
                 VALUES ('old1', 'self-harm', 4242);",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 6).unwrap();
        }

        let mut db = AnalysisDb::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        let scan = db.begin_scan("m", (None, None), "all", 1).unwrap();
        db.replace_findings(scan, &[finding("old1", Category::SelfHarm)], 100)
            .unwrap();

        let c = db.count_findings_breakdown("all", None, None).unwrap();
        assert_eq!(c.dismissed, 1, "the old dismissal came across");
        assert_eq!(c.live, 0);
        assert_eq!(
            c.unread, 0,
            "and counts as read — the user decided something"
        );
        let old_table: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'dismissals'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            old_table, 0,
            "the old table is gone — nothing can write to it"
        );
    }
}
