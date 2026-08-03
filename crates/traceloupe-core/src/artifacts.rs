//! Declarative artifact modules — the spine that makes the long tail affordable.
//!
//! An artifact is declared in a TOML file rather than written as Rust: which
//! backup file it needs, the SQL to run over it, and what each output column
//! means. A loader validates them, a runner executes them, and the rows land in
//! one generic table. Adding the next artifact is a data change, not a code
//! change and not a schema migration.
//!
//! The reasoning is on issue #190. In short: `docs/reference/backup-coverage-audit.md`
//! sizes the addressable set at ~360 backup-reachable artifacts, and at the cost
//! of a hand-written parser plus a designed view each, that is not a thing
//! anyone finishes. Modules that carry no logic can be reviewed in a ~20-line
//! diff and contributed by someone who does not write Rust.
//!
//! ```toml
//! id     = "tcc"
//! name   = "App permissions"
//! domain = "HomeDomain"
//! path   = "Library/TCC/TCC.db"
//! sql    = "SELECT client, service, auth_value, last_modified FROM access"
//!
//! [[columns]]
//! name = "App"
//! from = "client"
//!
//! [[columns]]
//! name  = "Granted"
//! from  = "last_modified"
//! kind  = "timestamp"
//! epoch = "unix"
//! ```
//!
//! **What is deliberately not here.** No Rust escape hatch yet (#190 decided one
//! is needed eventually — Notes' gzip-protobuf and Instagram's NSKeyedArchiver
//! cannot be declared — but declarative comes first), and no UI. This module is
//! backend-only so it is covered entirely by `cargo test`.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::crypto::BackupDecryptor;
use crate::manifest::ManifestIndex;
use crate::{Error, Result};

/// How a stored number becomes a date. iOS mixes all of these across stores,
/// which is why the epoch is declared per column rather than guessed: a Cocoa
/// timestamp read as Unix lands in 1970, and nothing about the value says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Epoch {
    /// Seconds since 1970-01-01 (classic Unix).
    Unix,
    /// Milliseconds since 1970-01-01.
    UnixMs,
    /// Seconds since 2001-01-01 — Core Data / CFAbsoluteTime. The most common
    /// one in Apple's own stores, and the one most often mistaken for Unix.
    Cocoa,
    /// Microseconds since 1601-01-01 (WebKit / Chromium).
    Webkit,
}

impl Epoch {
    /// Convert to Unix seconds. Returns None for values that cannot be a date,
    /// so a bad row degrades to "no date" instead of poisoning the artifact.
    pub fn to_unix_seconds(self, raw: f64) -> Option<i64> {
        let secs = match self {
            Epoch::Unix => raw,
            Epoch::UnixMs => raw / 1_000.0,
            Epoch::Cocoa => raw + 978_307_200.0,
            Epoch::Webkit => raw / 1_000_000.0 - 11_644_473_600.0,
        };
        // A backup cannot contain a date before iOS existed or far in the
        // future; treating those as "no date" is the rule app-data-coverage.md
        // already applies to messages whose timestamp does not decode.
        //
        // This also handles NaN and ±inf: every comparison against NaN is
        // false, so `contains` rejects them. An explicit `is_finite` guard used
        // to sit above and was dead code — mutation testing showed removing it
        // changed nothing.
        if !(0.0..=4_102_444_800.0).contains(&secs) {
            return None;
        }
        Some(secs as i64)
    }
}

/// What a column is, once read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColumnKind {
    #[default]
    Text,
    Integer,
    Real,
    Bool,
    Timestamp,
    /// Raw bytes rendered as colon-separated lowercase hex — `6a:22:32:98:f4:df`.
    ///
    /// Apple stores identifiers as `Data` constantly: MAC addresses, rotation
    /// keys, hardware ids. `text` gives null for those (they are not UTF-8, and
    /// pretending otherwise was a bug worth fixing), so without this the value is
    /// simply unreachable. Colon-separated because that is how a MAC is written
    /// everywhere else, and a MAC is overwhelmingly what a 6-byte `Data` is.
    Hex,
    /// A byte count, rendered as a human size by the UI.
    ///
    /// Declared rather than inferred for the same reason `Timestamp` is: the
    /// module knows, and nothing else can tell a byte count from any other large
    /// integer.
    Bytes,
    /// A length of time in SECONDS, rendered the way every other duration in the
    /// app is.
    ///
    /// Declared, not inferred, for the same reason as `Bytes` and `Timestamp`:
    /// nothing downstream can tell 900 seconds from any other 900. iLEAPP prints
    /// these through `datetime.timedelta`; sending a bare integer to the table
    /// would have been a second opinion about duration in one place, next to a
    /// Calls view that already formats them properly.
    Duration,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ColumnSpec {
    /// Display name.
    pub name: String,
    /// Where the value comes from: a SQL result column, or a key path into a
    /// property list.
    ///
    /// One string for the common case (`from = "SSID"`), a list to descend
    /// (`from = ["__OSSpecific__", "BSSID"]`). A list rather than a dotted string
    /// because plist keys contain dots — this store's own top-level keys look
    /// like `wifi.network.ssid.Matt_Foley` — so dotted paths would be ambiguous
    /// exactly where they are most needed.
    ///
    /// Omitted only for the column named by `plist.key_column`, whose value comes
    /// from the entry's key and so has no source to name. Required everywhere
    /// else — a column with no source would silently be all nulls.
    #[serde(default, deserialize_with = "one_or_many_path")]
    pub from: Vec<String>,
    /// A CONSTANT for this column, instead of a value read from the store.
    ///
    /// Exists for the join: `surface = "apps"` needs a bundle id in a column, and
    /// an app's own store never repeats its bundle id inside itself — it is
    /// implied by the file's location. Without this, an app-scoped artifact can
    /// be read but not attached to the app it belongs to, which is the difference
    /// between hosted and floating.
    ///
    /// Mutually exclusive with `from`: a column reads from one place.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub kind: ColumnKind,
    /// Required when `kind = "timestamp"`, meaningless otherwise.
    #[serde(default)]
    pub epoch: Option<Epoch>,
    /// Enum code → what it means, for the columns where Apple stores a number
    /// that stands for a word.
    ///
    /// `SSKeepMessages` is 0, 30 or 365, and it means Forever / 30 days /
    /// 1 year. Shipping the bare integer is the defect #287 fixed elsewhere:
    /// a table of codes is not an answer, it is homework.
    ///
    /// ```text
    /// [[columns]]
    /// name = "Keep messages for"
    /// from = "SSKeepMessages"
    /// [columns.map]
    /// 0 = "Forever"
    /// 30 = "30 days"
    /// 365 = "1 year"
    /// ```
    ///
    /// TWO RULES, both deliberate:
    ///
    /// 1. **An unmapped value passes through as itself**, never as "Unknown".
    ///    A code we have not seen is data; replacing it with a word we made up
    ///    loses it, and `tcc.toml` already establishes that unknowns travel
    ///    rather than being guessed at.
    /// 2. **A mapped column always produces a STRING**, mapped or not. A column
    ///    that is sometimes text and sometimes a number sorts and aligns
    ///    differently row to row, which looks like a rendering bug.
    ///
    /// Only for codes whose meaning is ESTABLISHED. An undocumented enum
    /// invented into words is worse than the number — see the `MTTimerState`
    /// note in `timers.toml`.
    #[serde(default)]
    pub map: Option<std::collections::BTreeMap<String, String>>,
}

/// What a host view may show about an artifact ON THE ROW, before anything is
/// expanded.
///
/// Optional, and declared by the module rather than inferred, because the Apps
/// view was doing this by hard-coding TCC: it filtered on a literal `"Decision"`
/// column for the literal values `"Allowed"`/`"Limited"` and printed "none
/// granted" when it found none. That is artifact-specific knowledge in a view
/// whose whole premise is that it knows no artifact by name — and the moment a
/// SECOND apps-surface module shipped it produced "Data usage: none granted",
/// which is not a claim data usage can make.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HighlightSpec {
    /// The column whose values are worth showing inline.
    pub column: String,
    /// Only badge a row when this column matches — omit to badge every row.
    #[serde(default)]
    pub when_column: Option<String>,
    #[serde(default)]
    pub when_any_of: Vec<String>,
    /// What to say when nothing matched. Omit and the host says nothing at all,
    /// which is the right default: silence claims less than a phrase.
    #[serde(default)]
    pub none_label: Option<String>,
}

/// How to read rows out of a property list.
///
/// The other half of the artifact tail. Roughly 44% of iLEAPP's artifacts read a
/// SQLite store, which is all `sql` can express; plists are the largest category
/// it cannot touch (#236). Everything downstream — the runner, `artifact_rows`,
/// column kinds, surfaces, `[highlight]` and every guard — is unchanged. Only how
/// a module names its source and projects rows is new.
/// Read a plain-text LOG instead of a database or a property list.
///
/// Some apps keep their most useful history only in their own logs — Life360
/// writes every location fix it uploads into `MainApplication/Logs/*.log` and
/// nowhere else in the backup. Without this a whole class of artifact is
/// unreachable no matter how many SQL modules exist.
///
/// The supported shape is one record per LINE, as a JSON object following a
/// fixed marker. That is what these logs actually look like: a human-readable
/// prefix (timestamp, subsystem, level) and then a structured payload. Lines
/// without the marker are ordinary log chatter and are skipped, not errors —
/// a log is mostly not records, unlike a table, which is only records.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogSpec {
    /// Marker that introduces a record. The rest of the line, after this text,
    /// must be a JSON object; `from` paths then descend into it.
    ///
    /// Matched literally, not as a regex: these markers are fixed strings in the
    /// app's source, and a regex here would invite a module to encode parsing
    /// rules that belong in code with tests.
    pub json_after: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlistSpec {
    /// Key path to the container holding the rows. Empty means the root itself,
    /// which is the single-record case (a settings plist IS one row).
    #[serde(default, deserialize_with = "one_or_many_path")]
    pub rows: Vec<String>,
    /// Whether the `rows` container legitimately may not exist.
    ///
    /// The default is strict, and stays strict: a missing key normally means
    /// the schema moved under us, and reporting that as "this device has none"
    /// is the class of lie this app exists to avoid.
    ///
    /// But some containers are genuinely written only once used. Verified on a
    /// real iPhone 11 / iOS 17.3: `com.apple.mobiletimerd.plist` has `MTAlarms`
    /// and `MTTimers` and **no `MTStopwatches` key at all** — the stopwatch had
    /// never been run. iLEAPP guards the same way (`if 'MTStopwatches' in pl`).
    /// Erroring there would take the whole artifact down over an absence that
    /// is the ordinary state of the device.
    ///
    /// Opt-in per module, so the strict default keeps protecting every module
    /// that has not thought about it, and declaring this is a statement that
    /// the author checked.
    #[serde(default)]
    pub optional: bool,
    /// When the container is a DICTIONARY, the key of each entry becomes this
    /// column.
    ///
    /// Not a nicety: in `com.apple.wifi.known-networks.plist` the network's name
    /// is the key (`wifi.network.ssid.Matt_Foley`) and nothing inside the entry
    /// repeats it as text. Without this the identifying field of the artifact
    /// would be unreachable.
    #[serde(default)]
    pub key_column: Option<String>,
    /// The column recording WHICH element a `*` in `rows` matched.
    ///
    /// Required when `rows` contains a `*`, for the same reason `path_column` is
    /// required for a path pattern: the wildcard collapses several containers into
    /// one table, and without the index nothing says which one a row came from.
    /// On a home screen that index is the PAGE, which is most of the artifact's
    /// value — "Maps is installed" and "Maps is on page 4" are different facts.
    #[serde(default)]
    pub index_column: Option<String>,
    /// The column holding a row whose value is a SCALAR rather than a dictionary.
    ///
    /// Two real shapes need this and neither is expressible without it: a dict of
    /// name → number (`MobileBackup.plist`'s `PreflightSizing` is domain → bytes)
    /// and an array of plain strings (Control Center's module identifiers). In
    /// both the row IS the value, so there is no key path to name — every `from`
    /// would be reaching into a string.
    #[serde(default)]
    pub value_column: Option<String>,
    /// Dropped from the front of each key before it becomes `key_column`.
    ///
    /// Apple namespaces those keys; `wifi.network.ssid.` is noise on every row.
    /// Declared rather than inferred, and if a key does not start with it the key
    /// is used whole — quietly trimming something else would hide a store whose
    /// shape has changed.
    #[serde(default)]
    pub key_strip_prefix: Option<String>,
}

/// How a host should PRESENT an artifact — as rows, or as facts.
///
/// The format made everything a table, and a meaningful share of the device tail
/// is not tabular: iLEAPP's "Identifiers" category alone is 16 artifacts that are
/// mostly single values — UDID, IMEI/IMSI, advertising identifier, AirDrop id,
/// device name. Sixteen one-row tables is an absurd way to show sixteen facts,
/// and two modules already shipped that way (`device_locale`, `siri_settings`)
/// read worse as tables than they would as more rows in the identity grid the
/// Device view already has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// Many rows: a table of its own. The default, and right for most artifacts.
    #[default]
    Table,
    /// ONE record, whose columns are label/value pairs the host folds into its
    /// own summary rather than giving a table.
    Facts,
}

/// Where an artifact is shown.
///
/// Not a free string: the set of hosts is small and known, and a typo must not
/// silently become "nowhere".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Surface {
    /// Inside the Apps view, against the app it belongs to.
    Apps,
    /// Inside the Contacts view.
    Contacts,
    /// Inside the Device view.
    Device,
    /// Its own destination — only for data that fits nowhere else.
    Standalone,
}

/// The container-domain families, which are **not** in
/// `tools/data/ios-backup-domains.json`.
///
/// That file holds the 19 domains iOS names outright (`HomeDomain`,
/// `MediaDomain`, …). Everything sandboxed instead gets a domain built at backup
/// time from a family prefix and an identifier —
/// `AppDomain-com.example.app`,
/// `AppDomainGroup-group.com.apple.notes`,
/// `SysSharedContainerDomain-systemgroup.com.apple.bluetooth`. There is no
/// enumerable list of them, because the set depends on what is installed, so a
/// module's domain is checked against the *families* instead.
///
/// This existed as a bare `starts_with("AppDomain")` in one test, which accepted
/// the three `AppDomain*` families and rejected the two `Sys*Container` ones — so
/// the first module in a system container (Bluetooth) failed a check that was
/// right to exist and wrong about the rules.
pub const CONTAINER_DOMAIN_PREFIXES: &[&str] = &[
    "AppDomain-",
    "AppDomainGroup-",
    "AppDomainPlugin-",
    "SysContainerDomain-",
    "SysSharedContainerDomain-",
];

/// Whether `domain` is a container domain, and so cannot appear in the static
/// list. The identifier after the prefix must be non-empty: `AppDomain-` alone
/// names nothing, and accepting it would let a truncated domain through.
pub fn is_container_domain(domain: &str) -> bool {
    CONTAINER_DOMAIN_PREFIXES
        .iter()
        .any(|p| domain.len() > p.len() && domain.starts_with(p))
}

impl Surface {
    /// Whether the host attaches each row to one of its own rows, and so needs a
    /// `join_column`.
    ///
    /// Apps and Contacts are lists of many things, and an artifact shown there is
    /// shown *against* one of them — a permission belongs to an app. Device is a
    /// list of one, so there is nothing to attach to and no column could identify
    /// it; the artifact is shown whole. Requiring a join column there would force
    /// every module to nominate an arbitrary one, which is how a required field
    /// becomes a field nobody means.
    pub fn attaches_to_a_row(self) -> bool {
        match self {
            Surface::Apps | Surface::Contacts => true,
            Surface::Device | Surface::Standalone => false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModuleSpec {
    /// Stable identifier; also the key rows are stored under.
    pub id: String,
    pub name: String,
    /// One plain-language sentence: what this is, for someone who has never
    /// heard of the store it comes from.
    ///
    /// Required. A module that cannot say what it is in a sentence is not
    /// finished — the Artifacts view shipped without this and was unintelligible
    /// to the person who commissioned it.
    pub description: String,
    /// How the host should present it. Defaults to a table.
    #[serde(default)]
    pub shape: Shape,
    /// Which real backup this module's OUTPUT has been checked against, if any.
    ///
    /// A module can be correct-looking, load, run, and pass its fixture while
    /// reading the wrong key — the fixture was written from the same reading of
    /// the store, so it agrees with the module by construction. Only a real
    /// device settles it.
    ///
    /// So implemented and verified are tracked apart. `None` means "written
    /// from iLEAPP's definition and proven to run, not yet seen against a real
    /// device" — a perfectly shippable state, and one that must not be
    /// mistaken for the other. Use the corpus key from
    /// `tools/data/dfir-images.json` plus what was actually observed:
    ///
    /// ```text
    /// verified = "iphone11_ios17 — 1 SIM in slot 1, ICCID + number"
    /// ```
    ///
    /// `tools/module-status.py` renders these into the audit doc, so the table
    /// is read from the modules and cannot drift from them.
    #[serde(default)]
    pub verified: Option<String>,
    /// The column that records WHICH matched store a row came from.
    ///
    /// Required when `path` contains a `*`, because a pattern can match several
    /// stores and nothing in a row says which one produced it. Two accounts'
    /// records merged into one anonymous table is worse than reading only one:
    /// it looks complete.
    #[serde(default)]
    pub path_column: Option<String>,
    /// Which existing view hosts this artifact, or `standalone`.
    ///
    /// Required, and deliberately not defaulted. The agreed rule is that data
    /// folds into the view closest in meaning, with its own destination only for
    /// genuinely outstanding data — and that rule was drifted from within one
    /// slice of shipping it. Making the author state a home is how it stops
    /// being a convention someone has to remember.
    pub surface: Surface,
    /// Which output column holds the value the host view joins on — a bundle id
    /// for `surface = "apps"`, a handle for Contacts.
    ///
    /// Required whenever the surface is not standalone: a hosted artifact that
    /// cannot be attached to a row is not hosted, it is just floating in someone
    /// else's view.
    #[serde(default)]
    pub join_column: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    /// What the host may show on the row itself, before expanding.
    #[serde(default)]
    pub highlight: Option<HighlightSpec>,
    /// Backup domain, e.g. `HomeDomain`.
    ///
    /// May contain `*`, which matches ANY RUN OF CHARACTERS — a domain has no
    /// `/`, so there is no segment rule to respect. This exists for stores that
    /// every app keeps its own copy of: `observations.db`, WebKit's
    /// tracking-prevention database, is in 42 different `AppDomain-…` containers
    /// on the validation device, and writing 42 identical modules would be a
    /// worse answer than one that says what it means.
    ///
    /// A glob here REQUIRES `app_column`, because a row from Signal and a row
    /// from Chrome are otherwise indistinguishable — the same rule, and the same
    /// reason, as `path_column`.
    pub domain: String,
    /// The column recording WHICH app's container a row came from.
    ///
    /// Required when `domain` contains `*`. Holds the bundle id — the domain
    /// with its `AppDomain-` prefix removed — because that is what the Apps view
    /// joins on and what a reader recognises. The raw domain is recoverable from
    /// it and is not what anyone wants to read.
    #[serde(default)]
    pub app_column: Option<String>,
    /// Exact relative path within the domain.
    ///
    /// Deliberately not a glob. A prefix-match version of this shipped in the
    /// first draft and was wrong in two ways: `Manifest.db` carries directory
    /// rows, and a directory sorts before its own children, so `Library/Foo/*`
    /// selected the directory rather than the store inside it; and `-wal` /
    /// `-shm` siblings could win over the real file. Since no artifact needed
    /// globbing, the fix is to not have it until one does — with tests.
    pub path: String,
    /// The query, or several to try in order — the first that *prepares*
    /// against this backup's store wins.
    ///
    /// Alternatives exist because iOS renames columns between versions and a
    /// single static SELECT cannot span that. TCC is the case that forced it:
    /// modern devices have `auth_value` (0/2/3), older ones `allowed` (0/1),
    /// and the two never co-exist, so `COALESCE` cannot help — the query simply
    /// fails to prepare against the wrong one.
    ///
    /// Every alternative must produce the SAME output column names, since the
    /// column spec is shared. Alias in SQL to make that so, and `SELECT NULL AS
    /// x` where an older schema has nothing to offer.
    #[serde(default, deserialize_with = "one_or_many")]
    pub sql: Vec<String>,
    /// Read a property list instead of running SQL. Exactly one of `sql`,
    /// `plist` and `log` must be declared.
    #[serde(default)]
    pub plist: Option<PlistSpec>,
    /// Read a plain-text log instead of a database or a property list.
    #[serde(default)]
    pub log: Option<LogSpec>,
    /// Declared precondition. Parsed and stored here; honoured by the UI in a
    /// later slice (#210). Apple's `RelativePathsToOnlyBackupEncrypted` covers
    /// 28 artifacts, so this is not an edge case.
    #[serde(default)]
    pub requires: Option<String>,
    pub columns: Vec<ColumnSpec>,
}

/// Accept either `sql = "…"` or `sql = ["…", "…"]`.
///
/// A hand-written visitor rather than `#[serde(untagged)]`, which collapses any
/// mistake into "data did not match any variant of untagged enum". `load_modules`
/// promises to say which file and *why*; "invalid type: integer, expected a
/// string" is the why, and untagged throws it away.
/// The same one-or-many shape as `one_or_many`, for a key path rather than SQL.
/// Separate so the "expecting" message names the right thing — a wrong type here
/// should say what a `from` may be, not what a query may be.
fn one_or_many_path<'de, D>(d: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a column name, or a list of keys forming a path into a property list")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Vec<String>, A::Error> {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                out.push(item);
            }
            Ok(out)
        }
    }
    d.deserialize_any(V)
}

fn one_or_many<'de, D>(d: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a SQL string, or a list of alternative SQL strings")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Vec<String>, A::Error> {
            let mut out = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                out.push(item);
            }
            Ok(out)
        }
    }
    d.deserialize_any(V)
}

impl ModuleSpec {
    /// Reject a module that cannot possibly work, naming the problem. A module
    /// that fails silently is worse than one that fails loudly: the artifact
    /// just never appears, and nothing says why.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("`id` is empty".into());
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "`id` = {:?} may only contain letters, digits, `_` and `-`",
                self.id
            ));
        }
        if self.description.trim().is_empty() {
            return Err("`description` is empty — say in one sentence what this is".into());
        }
        if self.domain.trim().is_empty() {
            return Err("`domain` is empty".into());
        }
        if self.domain.contains('*') {
            // Only app containers, for now. A glob over system domains would
            // match things with no common shape and no id to label them by, and
            // the one use case is per-app copies of one store.
            if !self.domain.starts_with("AppDomain-") {
                return Err(format!(
                    "`domain` = {:?} globs outside `AppDomain-`; only per-app containers \
                     may be matched by pattern, because only they have a bundle id to \
                     label the rows with",
                    self.domain
                ));
            }
            match &self.app_column {
                None => {
                    return Err(
                        "`domain` contains `*` but no `app_column` — rows from different \
                         apps would be indistinguishable, which looks complete and is not"
                            .into(),
                    )
                }
                Some(col) if !self.columns.iter().any(|c| &c.name == col) => {
                    return Err(format!(
                        "`app_column` = {col:?} is not one of the declared columns"
                    ))
                }
                Some(_) => {}
            }
        } else if self.app_column.is_some() {
            return Err(
                "`app_column` is set but `domain` has no `*` — there is only one app, so \
                 name it in a `value` column instead"
                    .into(),
            );
        }
        if self.path.trim().is_empty() {
            return Err("`path` is empty".into());
        }
        // `*` means "any run of characters WITHIN one path segment", never across
        // `/`. That is what makes the two failures the first attempt at globbing
        // hit impossible: a directory row cannot swallow its own children, and a
        // pattern cannot reach down a level it did not ask for.
        //
        // `**` is the deliberate exception, and ONLY as a whole segment. One app
        // can write the same store from several processes at different depths —
        // Life360 logs live under `MainApplication/Logs/`, `SidecarLPSE/` and
        // `PushNotificationServiceExtension/` — and a within-segment `*` cannot
        // span that, so the module would have to pick one directory and drop the
        // rest silently, looking complete while holding a quarter of the records.
        //
        // Mixed into a segment (`foo**bar`) it stays rejected: that reads like a
        // wider `*` rather than a level-spanning token, which is exactly the
        // quiet reach this rule exists to prevent.
        if let Some(seg) = self
            .path
            .split('/')
            .find(|seg| seg.contains("**") && *seg != "**")
        {
            return Err(format!(
                "`path` = {:?} has {seg:?}, which mixes `**` into a segment. `**` spans \
                 whole segments and must stand alone between slashes; a `*` inside a \
                 segment never crosses `/`",
                self.path
            ));
        }
        for segment in self.path.split('/') {
            if segment == "*" && self.path.split('/').filter(|s| *s == "*").count() > 2 {
                return Err(format!(
                    "`path` = {:?} is mostly wildcard. A pattern that vague will match \
                     stores this module was never written for, and their columns will \
                     silently be null",
                    self.path
                ));
            }
        }
        // A pattern without a `path_column` reads several stores into one table
        // with nothing saying which row came from where. That is worse than
        // reading only the first, because the result looks complete.
        if self.path.contains('*') {
            match &self.path_column {
                None => {
                    return Err(format!(
                        "`path` = {:?} is a pattern and can match several stores, but no \
                         `path_column` is declared — rows from different stores would be \
                         indistinguishable in one table",
                        self.path
                    ))
                }
                Some(col) if !self.columns.iter().any(|c| &c.name == col) => {
                    return Err(format!(
                        "`path_column` = {col:?} is not one of the declared columns"
                    ))
                }
                Some(_) => {}
            }
        } else if self.path_column.is_some() {
            return Err(
                "`path_column` is declared but `path` is exact — every row comes from the \
                 same store, so the column would repeat one value"
                    .into(),
            );
        }
        // EXACTLY one source. Both would be ambiguous about which one produced a
        // row; neither leaves a module that loads, validates and reads nothing.
        let has_sql = !self.sql.is_empty() && !self.sql.iter().all(|q| q.trim().is_empty());
        let sources = [has_sql, self.plist.is_some(), self.log.is_some()];
        match sources.iter().filter(|on| **on).count() {
            0 => {
                return Err("no source: declare one of `sql`, `[plist]` or `[log]`".into());
            }
            1 => {}
            _ => {
                return Err(
                    "more than one of `sql`, `[plist]` and `[log]` is declared — a module \
                     reads one store one way"
                        .into(),
                )
            }
        }
        if let Some(lg) = &self.log {
            if lg.json_after.trim().is_empty() {
                return Err(
                    "`log.json_after` is empty — every line would match and none would parse"
                        .into(),
                );
            }
            // `from` is checked per-column below, where the runner-filled and
            // constant cases are already understood.
        }
        if let Some(pl) = &self.plist {
            if pl.rows.iter().any(|k| k.trim().is_empty()) {
                return Err("`plist.rows` contains an empty key".into());
            }
            if let Some(k) = &pl.key_column {
                if k.trim().is_empty() {
                    return Err("`plist.key_column` is empty".into());
                }
                if !self.columns.iter().any(|c| &c.name == k) {
                    return Err(format!(
                        "`plist.key_column` = {k:?} is not one of the declared columns — the \
                         key would be read and then have nowhere to go"
                    ));
                }
            }
            let has_star = pl.rows.iter().any(|k| k == "*");
            match (&pl.index_column, has_star) {
                (None, true) => {
                    return Err(
                        "`plist.rows` contains a `*` but no `index_column` is declared — the \
                         wildcard collapses several containers into one table and nothing \
                         would say which one a row came from"
                            .into(),
                    )
                }
                (Some(_), false) => {
                    return Err(
                        "`plist.index_column` is declared but `rows` has no `*` — there is \
                         only one container, so the column would repeat one value"
                            .into(),
                    )
                }
                _ => {}
            }
            if let Some(k) = &pl.index_column {
                if !self.columns.iter().any(|c| &c.name == k) {
                    return Err(format!(
                        "`plist.index_column` = {k:?} is not one of the declared columns"
                    ));
                }
            }
            if let Some(k) = &pl.value_column {
                if !self.columns.iter().any(|c| &c.name == k) {
                    return Err(format!(
                        "`plist.value_column` = {k:?} is not one of the declared columns"
                    ));
                }
            }
            if pl.key_strip_prefix.is_some() && pl.key_column.is_none() {
                return Err(
                    "`plist.key_strip_prefix` is set but `key_column` is not — there is no \
                     key being read for it to trim"
                        .into(),
                );
            }
        }
        for sql in &self.sql {
            let sql = sql.trim();
            // A friendly early error only. It is NOT the thing that makes a module
            // read-only — SQLite accepts `WITH x AS (…) INSERT … RETURNING`, which
            // passes this check and writes. The store is opened read-only in
            // `run_module`; that is the enforcement.
            if !sql.to_ascii_lowercase().starts_with("select")
                && !sql.to_ascii_lowercase().starts_with("with")
            {
                return Err("`sql` must be a SELECT (or WITH … SELECT)".into());
            }
        }
        if self.columns.is_empty() {
            return Err("no `[[columns]]` declared".into());
        }
        let mut seen_names: Vec<&str> = Vec::new();
        for c in &self.columns {
            if let Some(m) = &c.map {
                if m.is_empty() {
                    return Err(format!(
                        "column {:?} declares an empty `[columns.map]` — either map \
                         something or drop it",
                        c.name
                    ));
                }
                // A date is not an enum. Mapping one would stringify it and the
                // UI would stop formatting it as a date, silently.
                if c.kind == ColumnKind::Timestamp {
                    return Err(format!(
                        "column {:?} is a timestamp with a `[columns.map]` — a date is \
                         not an enum, and mapping it turns off date formatting",
                        c.name
                    ));
                }
            }
            let is_plist_key = self
                .plist
                .as_ref()
                .and_then(|p| p.key_column.as_ref())
                .is_some_and(|k| k == &c.name);
            // Runner-filled columns: the value comes from WHICH store matched,
            // not from inside it, so there is no `from` to declare.
            let is_path_column = self.path_column.as_ref().is_some_and(|k| k == &c.name)
                || self.app_column.as_ref().is_some_and(|k| k == &c.name)
                || self
                    .plist
                    .as_ref()
                    .and_then(|p| p.value_column.as_ref())
                    .is_some_and(|k| k == &c.name)
                || self
                    .plist
                    .as_ref()
                    .and_then(|p| p.index_column.as_ref())
                    .is_some_and(|k| k == &c.name);
            // Both are filled by the runner rather than read from the store, so
            // both may omit `from` — but they are different mistakes and must not
            // share an error message, or a path column would be told about
            // `plist.key_column`.
            let is_key_column = is_plist_key || is_path_column;
            if c.name.trim().is_empty() {
                return Err(format!("column {:?} has an empty `name`", c.name));
            }
            if c.value.is_some() && !c.from.is_empty() {
                return Err(format!(
                    "column {:?} declares both `value` and `from` — a column reads from one \
                     place, and which one won would be invisible in the output",
                    c.name
                ));
            }
            if !is_key_column && c.from.is_empty() && c.value.is_none() {
                return Err(format!(
                    "column {:?} declares no `from` — it would be all nulls. Only the \
                     `plist.key_column` may omit it, because its value is the entry's key, \
                     or declare a constant `value`",
                    c.name
                ));
            }
            // The key column's value IS the key, so anything describing where else
            // to get it, or what to make of it, is dead. Accepting it silently let
            // an author write `from = ["SSID"]` there and quietly get the key
            // instead of the field they named.
            if is_path_column && !c.from.is_empty() {
                return Err(format!(
                    "column {:?} is the `path_column`, so its value is the store the row came \
                     from — a `from` would be ignored",
                    c.name
                ));
            }
            if is_plist_key {
                if !c.from.is_empty() {
                    return Err(format!(
                        "column {:?} is the `plist.key_column`, so its value is the entry's \
                         key — a `from` would be ignored",
                        c.name
                    ));
                }
                if c.kind != ColumnKind::Text {
                    return Err(format!(
                        "column {:?} is the `plist.key_column` and a plist key is always \
                         text, so `kind` cannot be {:?}",
                        c.name, c.kind
                    ));
                }
                if c.epoch.is_some() {
                    return Err(format!(
                        "column {:?} is the `plist.key_column`; an `epoch` has nothing to \
                         convert there",
                        c.name
                    ));
                }
            }
            // A SQL column reads ONE result name. Checked here rather than in the
            // runner, where it lived first: `run_module` returns early when the
            // store is not in the backup, so on any device lacking it the mistake
            // never surfaced at all.
            if self.plist.is_none() && self.log.is_none() && c.from.len() > 1 {
                return Err(format!(
                    "column {:?} declares a key path of {} segments, but this module reads \
                     SQL, where `from` names a single result column",
                    c.name,
                    c.from.len()
                ));
            }
            if c.from.iter().any(|k| k.trim().is_empty()) {
                return Err(format!(
                    "column {:?} has an empty key in its `from` path",
                    c.name
                ));
            }
            // A row is keyed on the display name, so two columns sharing one
            // collapse to whichever is written last — a column silently
            // disappearing with no error at load or run.
            if seen_names.contains(&c.name.as_str()) {
                return Err(format!(
                    "two columns are both named {:?} — a row is keyed on the display name, \
                     so one would silently overwrite the other",
                    c.name
                ));
            }
            seen_names.push(&c.name);
            // A plist `Date` is already an absolute time, so an epoch would have
            // nothing to convert; a NUMBER in a plist still needs one. Required for
            // SQL, where every timestamp arrives as a bare number.
            if c.kind == ColumnKind::Timestamp && c.epoch.is_none() && self.plist.is_none() {
                return Err(format!(
                    "column {:?} is a timestamp but declares no `epoch` — \
                     without it the date cannot be converted and would render as a raw number",
                    c.name
                ));
            }
        }
        // An artifact attached to a row must say which row.
        if self.surface.attaches_to_a_row() && self.join_column.is_none() {
            return Err(format!(
                "`surface` is {:?} but no `join_column` is declared — the host view \
                 needs to know which column identifies the row it belongs to",
                self.surface
            ));
        }
        // Checked whenever one is declared, including on a surface that does not
        // require it: a `join_column` naming a column that does not exist is a
        // typo either way, and silently ignoring it on `device` would let the
        // same mistake through unnoticed.
        if let Some(col) = &self.join_column {
            if !self.columns.iter().any(|c| &c.name == col) {
                return Err(format!(
                    "`join_column` = {col:?} is not one of the declared columns"
                ));
            }
        }
        // A highlight naming a column that does not exist would silently show
        // nothing, which is indistinguishable from an artifact that legitimately
        // has nothing to highlight.
        if let Some(h) = &self.highlight {
            let has = |name: &str| self.columns.iter().any(|c| c.name == name);
            if !has(&h.column) {
                return Err(format!(
                    "`highlight.column` = {:?} is not one of the declared columns",
                    h.column
                ));
            }
            if let Some(w) = &h.when_column {
                if !has(w) {
                    return Err(format!(
                        "`highlight.when_column` = {w:?} is not one of the declared columns"
                    ));
                }
                if h.when_any_of.is_empty() {
                    return Err(
                        "`highlight.when_column` is set but `when_any_of` is empty — the \
                         condition could never match, so nothing would ever be highlighted"
                            .into(),
                    );
                }
            } else if !h.when_any_of.is_empty() {
                return Err(
                    "`highlight.when_any_of` is set but `when_column` is not — there is \
                     nothing to compare it against"
                        .into(),
                );
            }
        }
        // A facts artifact is folded into its host's own summary, and only the
        // Device view has one to fold into. Declared on any other surface it would
        // fall through that host's table path and render as the one-row table this
        // shape exists to avoid — silently, which is the worst kind of wrong.
        // Relax this when a second host learns to show facts.
        if self.shape == Shape::Facts && self.surface != Surface::Device {
            return Err(format!(
                "`shape = \"facts\"` with `surface = {:?}`: only the Device view folds facts \
                 into its own summary, so anywhere else this would silently render as the \
                 one-row table the shape exists to avoid",
                self.surface
            ));
        }
        if let Some(r) = &self.requires {
            if r != "encrypted-backup" {
                return Err(format!(
                    "unknown `requires` value {r:?} (only \"encrypted-backup\")"
                ));
            }
        }
        Ok(())
    }

    /// True when this module needs an encrypted backup to have any data.
    pub fn needs_encrypted_backup(&self) -> bool {
        self.requires.as_deref() == Some("encrypted-backup")
    }
}

/// One artifact, as the UI needs to describe it: what it is called and which
/// columns it has, without the UI knowing any artifact exists by name.
/// `camelCase` because the UI reads `joinColumn`, `rowCount` and
/// `requiresEncryptedBackup` (src/lib/ipc.ts). Without this the struct
/// serialised as snake_case and every one of those was `undefined` in the real
/// app, so no hosted artifact rendered at all — while the mock, which returns
/// camelCase, made the whole feature look fine under `pnpm dev` (#232).
/// `serialised_keys_match_the_ui_contract` now pins it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactSummary {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    /// One sentence describing the artifact, shown wherever it is hosted.
    pub description: String,
    /// Which view hosts it.
    pub surface: Surface,
    /// How the host should present it — rows, or facts folded into its summary.
    pub shape: Shape,
    /// The column the host view joins on, when hosted.
    pub join_column: Option<String>,
    /// What the host may show on the row itself, before expanding.
    pub highlight: Option<HighlightSpec>,
    /// Column display names, in declared order — the table's headers. A JSON
    /// row is an unordered map, so without this the UI would have to invent an
    /// order, and it would differ between artifacts and between runs.
    pub columns: Vec<String>,
    /// Which of `columns` are byte counts, from the module's own `kind`.
    pub byte_columns: Vec<String>,
    /// Which of `columns` are durations in seconds, from the module's own `kind`.
    pub duration_columns: Vec<String>,
    /// Which of `columns` are timestamps, from the module's own `kind`.
    ///
    /// The UI used to infer this by testing whether every value in a column fell
    /// inside a plausible Unix-seconds range. That was guesswork over a fact the
    /// module already states, and the Bluetooth module is where it would have
    /// bitten: its two counters are integers that must NOT be read as dates, and
    /// the only thing standing between them and a fabricated 1970s timestamp was
    /// the range heuristic happening to exclude them.
    pub timestamp_columns: Vec<String>,
    pub row_count: i64,
    /// True when this artifact needs an encrypted backup to hold anything.
    pub requires_encrypted_backup: bool,
}

/// The cache key recording which modules produced the stored rows.
const EXTRACTED_MODULES_KEY: &str = "artifacts_extracted_modules";

/// A stable fingerprint of the current module set: sorted ids, comma-joined.
///
/// A boolean "have we extracted" would not survive adding a module — someone who
/// extracted with two installed and later has five would keep seeing two, with
/// nothing to say why. Comparing the actual set catches both "never run" and
/// "run with a smaller set".
pub fn module_set_fingerprint(specs: &[ModuleSpec]) -> String {
    let mut ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    ids.join(",")
}

/// Why the Artifacts view might have nothing to show.
///
/// "The backup contained none" and "nobody has looked yet" are different facts,
/// and saying the first when the second is true is a claim the user cannot
/// check. Same class of mistake as an encryption-gated view rendering as merely
/// empty (#203).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtractionState {
    /// Rows were produced by exactly the modules installed now.
    UpToDate,
    /// No module has ever run against this backup.
    NeverRun,
    /// Rows exist, but from a different (usually smaller) module set.
    Stale,
}

/// Whether the stored rows came from the module set installed right now.
pub fn extraction_state(cache: &crate::cache::CacheDb) -> Result<ExtractionState> {
    let want = module_set_fingerprint(&builtin_modules()?);
    match cache.get_meta(EXTRACTED_MODULES_KEY)? {
        None => Ok(ExtractionState::NeverRun),
        Some(have) if have == want => Ok(ExtractionState::UpToDate),
        Some(_) => Ok(ExtractionState::Stale),
    }
}

/// Record that the current module set has been run against this backup.
pub fn mark_extracted(cache: &crate::cache::CacheDb, specs: &[ModuleSpec]) -> Result<()> {
    cache.set_meta(EXTRACTED_MODULES_KEY, &module_set_fingerprint(specs))
}

/// Every shipped artifact that has rows in this backup, with its shape.
///
/// Artifacts with no rows are omitted: the backup did not contain them, and a
/// list of empty tables is not navigation. The one exception is an artifact
/// gated on encryption, which is kept so it can explain itself rather than
/// vanish (#197) — an absent artifact and an impossible one are different
/// facts.
pub fn list_artifacts(conn: &Connection) -> Result<Vec<ArtifactSummary>> {
    let mut out = Vec::new();
    for spec in builtin_modules()? {
        let row_count = count_rows(conn, &spec.id)?;
        let gated = spec.needs_encrypted_backup();
        if row_count == 0 && !gated {
            continue;
        }
        out.push(ArtifactSummary {
            id: spec.id.clone(),
            name: spec.name.clone(),
            category: spec.category.clone(),
            description: spec.description.clone(),
            surface: spec.surface,
            shape: spec.shape,
            join_column: spec.join_column.clone(),
            highlight: spec.highlight.clone(),
            columns: spec.columns.iter().map(|c| c.name.clone()).collect(),
            timestamp_columns: spec
                .columns
                .iter()
                .filter(|c| c.kind == ColumnKind::Timestamp)
                .map(|c| c.name.clone())
                .collect(),
            byte_columns: spec
                .columns
                .iter()
                .filter(|c| c.kind == ColumnKind::Bytes)
                .map(|c| c.name.clone())
                .collect(),
            duration_columns: spec
                .columns
                .iter()
                .filter(|c| c.kind == ColumnKind::Duration)
                .map(|c| c.name.clone())
                .collect(),
            row_count,
            requires_encrypted_backup: gated,
        });
    }
    Ok(out)
}

/// One parsed row: column display name → value.
pub type ArtifactRow = HashMap<String, serde_json::Value>;

/// Load every `*.toml` module under `dir`, in a stable order.
///
/// A directory that does not exist is not an error — it means no modules are
/// installed. A file that does not parse *is*, and says which file and why.
pub fn load_modules(dir: &Path) -> Result<Vec<ModuleSpec>> {
    let mut out: Vec<ModuleSpec> = Vec::new();
    // Keep each module's source filename so a collision can name both files —
    // a display name gives the reader nothing to grep for.
    let mut sources: Vec<(String, String)> = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    entries.sort();

    for path in entries {
        let text = std::fs::read_to_string(&path).map_err(|e| Error::Io {
            path: path.clone(),
            source: e,
        })?;
        let spec: ModuleSpec = toml::from_str(&text).map_err(|e| {
            Error::Parse(format!(
                "artifact module {}: {e}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
        })?;
        spec.validate().map_err(|why| {
            Error::Parse(format!(
                "artifact module {}: {why}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ))
        })?;
        // Two modules sharing an id would silently overwrite each other's rows
        // — `store_rows` is keyed on it and replaces. That is a data-loss bug
        // whose only symptom is an artifact that inexplicably shows another
        // artifact's contents, so it is rejected at load, naming both files.
        let file = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Some((_, prev_file)) = sources.iter().find(|(id, _)| id == &spec.id) {
            return Err(Error::Parse(format!(
                "artifact module {file}: id {:?} is already used by {prev_file} — ids key \
                 stored rows, so two modules sharing one would overwrite each other",
                spec.id,
            )));
        }
        sources.push((spec.id.clone(), file));
        out.push(spec);
    }
    Ok(out)
}

/// Find the backup file a module wants, if the backup has it.
///
/// Checks the Manifest and nothing else — so an artifact the backup does not
/// contain costs one indexed lookup and never touches the filesystem. That is
/// the same property every hand-written source already has, and it is what
/// keeps import time flat as the module count grows.
/// Public so the validator can ask the same question the runner asks. It used its
/// own exact `index.find`, which reported the first pattern module as absent from
/// a backup that contains it — a check that disagrees with the thing it checks is
/// worse than no check.
/// Does `domain` match `pattern`? `*` matches any run of characters; a backup
/// domain has no `/`, so there is no segment rule here as there is for paths.
fn domain_matches(pattern: &str, domain: &str) -> bool {
    let mut rest = domain;
    let parts: Vec<&str> = pattern.split('*').collect();
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match (i, rest.find(part)) {
            // A leading literal must be a prefix, not merely present.
            (0, Some(0)) => rest = &rest[part.len()..],
            (0, _) => return false,
            (_, Some(at)) => rest = &rest[at + part.len()..],
            (_, None) => return false,
        }
    }
    // A pattern not ending in `*` must consume the whole domain.
    pattern.ends_with('*') || rest.is_empty()
}

pub fn locate(index: &ManifestIndex, spec: &ModuleSpec) -> Result<Vec<crate::manifest::FileEntry>> {
    if spec.domain.contains('*') {
        // The path is still matched exactly or by its own rules; only the domain
        // fans out. Every candidate is filtered in Rust, because the manifest
        // query cannot express the segment rule paths need.
        let like = spec.path.replace("**/", "%").replace('*', "%");
        let mut out = Vec::new();
        for entry in index.find_relative_like(&like)? {
            if !domain_matches(&spec.domain, &entry.domain) {
                continue;
            }
            if spec.path.contains('*') {
                if !path_matches(&spec.path, &entry.relative_path) {
                    continue;
                }
            } else if entry.relative_path != spec.path {
                continue;
            }
            if entry.relative_path.ends_with("-wal") || entry.relative_path.ends_with("-shm") {
                continue;
            }
            out.push(entry);
        }
        // By domain, then path: the manifest's order is not deterministic and an
        // artifact whose rows reorder between runs looks like it changed.
        out.sort_by(|a, b| {
            a.domain
                .cmp(&b.domain)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        return Ok(out);
    }
    if !spec.path.contains('*') {
        return Ok(index.find(&spec.domain, &spec.path)?.into_iter().collect());
    }

    // A `LIKE` to narrow it in SQL, then a segment-aware match in Rust, because
    // `%` crosses `/` and the whole point of `*` is that it does not.
    // `**` first: replacing `*` alone would leave `%%/`, which requires a literal
    // `/` the pattern does not actually demand — `**` can match nothing at all.
    let like = spec.path.replace("**/", "%").replace('*', "%");
    let mut out = Vec::new();
    for entry in index.find_relative_like(&like)? {
        if entry.domain != spec.domain {
            continue;
        }
        if !path_matches(&spec.path, &entry.relative_path) {
            continue;
        }
        // The other trap the first attempt hit: a `-wal`/`-shm` sibling is not the
        // store, and a pattern ending in `*` would happily take one.
        if entry.relative_path.ends_with("-wal") || entry.relative_path.ends_with("-shm") {
            continue;
        }
        out.push(entry);
    }
    // Deterministic: the manifest's order is not, and an artifact whose rows
    // reorder between runs looks like it changed when nothing did.
    out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(out)
}

/// Does `path` match `pattern`, where `*` matches within ONE segment?
fn path_matches(pattern: &str, path: &str) -> bool {
    path_captures(pattern, path).is_some()
}

/// The parts of `path` the wildcards matched, or `None` if it does not match.
///
/// Segment-by-segment rather than one regex over the whole string, so `*` cannot
/// cross `/` however the pattern is written.
///
/// The captures are what `path_column` shows. The full path would be honest and
/// nearly useless: every row would carry the same forty characters of boilerplate
/// around the handful that identify the store. In
/// `Library/DeviceRegistry/*/AppConduit/ACXRemoteAppList.plist` the `*` IS the
/// paired device, and that is the whole answer to "which one did this come from".
fn path_captures(pattern: &str, path: &str) -> Option<Vec<String>> {
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    walk_segments(&p, &s)
}

/// Match pattern segments against path segments, with `**` spanning whole
/// segments and `*` confined to one.
///
/// `**` exists because one app can write the same log from several processes at
/// DIFFERENT DEPTHS — Life360 keeps them under `MainApplication/Logs/`,
/// `SidecarLPSE/` and `PushNotificationServiceExtension/`. A single-segment `*`
/// cannot span that, so a module would have to pick one directory and silently
/// drop three quarters of the records, which is worse than not reading them:
/// the artifact would look complete.
///
/// It is deliberately a separate token from `*`. Making `*` cross `/` would have
/// been less code and would have quietly widened every pattern already shipped —
/// the exact mistake `segment_matches` was written to prevent.
fn walk_segments(p: &[&str], s: &[&str]) -> Option<Vec<String>> {
    match p.split_first() {
        // Pattern exhausted: a match only if the path is too.
        None => s.is_empty().then(Vec::new),
        Some((pat, rest)) if *pat == "**" => {
            // Try the shortest span first, so `**` stays as tight as it can and
            // the capture names the least boilerplate.
            for take in 0..=s.len() {
                if let Some(mut tail) = walk_segments(rest, &s[take..]) {
                    let mut caught = vec![s[..take].join("/")];
                    caught.append(&mut tail);
                    return Some(caught);
                }
            }
            None
        }
        Some((pat, rest)) => {
            let (seg, srest) = s.split_first()?;
            if !segment_matches(pat, seg) {
                return None;
            }
            let mut tail = walk_segments(rest, srest)?;
            let mut caught = Vec::new();
            if pat.contains('*') {
                caught.push((*seg).to_string());
            }
            caught.append(&mut tail);
            Some(caught)
        }
    }
}

/// `*` matches any run of characters inside a single segment, including none.
fn segment_matches(pattern: &str, segment: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == segment;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = segment;
    // The literal before the first `*` must be a prefix.
    if let Some(first) = parts.first() {
        match rest.strip_prefix(first) {
            Some(r) => rest = r,
            None => return false,
        }
    }
    // The literal after the last `*` must be a suffix.
    if let Some(last) = parts.last() {
        if parts.len() > 1 {
            match rest.strip_suffix(last) {
                Some(r) => rest = r,
                None => return false,
            }
        }
    }
    // Everything between must appear in order.
    for middle in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        match rest.find(middle) {
            Some(i) => rest = &rest[i + middle.len()..],
            None => return false,
        }
    }
    true
}

/// Run one module against a backup. `Ok(None)` means the backup does not
/// contain this artifact — which is a normal outcome, not a failure.
pub fn run_module(
    spec: &ModuleSpec,
    index: &ManifestIndex,
    decryptor: Option<&BackupDecryptor>,
    work_dir: &Path,
) -> Result<Option<Vec<ArtifactRow>>> {
    // `run_module` is public and `ModuleSpec` is deserializable, so a caller can
    // reach here with a spec that never went through `load_modules`. The id is
    // used to build a filename, so skipping validation would be a path-traversal
    // hole (`id = "../../x"` writing decrypted backup content outside work_dir).
    spec.validate()
        .map_err(|why| Error::Parse(format!("artifact module {:?}: {why}", spec.id)))?;

    let entries = locate(index, spec)?;
    if entries.is_empty() {
        return Ok(None);
    }

    fn enforce_shape(spec: &ModuleSpec, rows: Vec<ArtifactRow>) -> Result<Vec<ArtifactRow>> {
        if spec.shape == Shape::Facts && rows.len() > 1 {
            // Not truncated to the first: a store that grew a second record is a
            // shape change, and quietly showing one of two would report a device
            // fact that is no longer singular as though it still were.
            return Err(Error::Parse(format!(
                "artifact {}: declared `shape = \"facts\"` but produced {} records. Facts are \
                 folded into the Device view's identity summary, which can hold one of each — \
                 if this store really has several, it is a table",
                spec.id,
                rows.len()
            )));
        }
        Ok(rows)
    }

    // A pattern can match several stores — one per account, per container, per
    // mailbox. All of them are read and their rows concatenated: an app with two
    // accounts has two stores and both are the user's data, and picking one would
    // silently report half.
    //
    // Which store a row came from is not guessable from the row, so a module
    // matching several SHOULD declare `path_column` and `validate` insists on it.
    // Without that, two accounts' records would be indistinguishable in one table —
    // which is worse than not reading the second, because it looks complete.
    // A GLOBBED path may match stores that were never this module's.
    //
    // MEGA is the case that forced this: `megaclient_statecache14_*.db` matches
    // the node cache AND its `_status_`/`_transfers_` siblings, whose schemas
    // have nothing in common with it beyond a name. No SQL alternative can tell
    // them apart -- the siblings hold only `statecache`, which the real store
    // also has -- so a fallback query would prepare against the real store too
    // and silently return nothing the day its schema changed.
    //
    // So: with a glob, a store no alternative can run against is SKIPPED and
    // its reason kept. If NONE of the matched stores worked, the first reason is
    // raised as the error -- a genuine schema change still fails loudly, because
    // it breaks every store rather than one.
    let globbed = spec.path.contains('*') || spec.domain.contains('*');
    let mut skipped: Option<Error> = None;
    let mut any_ran = false;
    let mut all: Vec<ArtifactRow> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let mut rows = match run_one(spec, index, decryptor, work_dir, entry, i) {
            Ok(rows) => {
                any_ran = true;
                rows
            }
            Err(e) if globbed => {
                skipped.get_or_insert(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        if let Some(col) = &spec.app_column {
            // The bundle id, not the raw domain: it is what the Apps view joins
            // on and what a reader recognises.
            let bundle = entry
                .domain
                .strip_prefix("AppDomain-")
                .unwrap_or(&entry.domain)
                .to_string();
            for row in &mut rows {
                row.insert(col.clone(), serde_json::Value::String(bundle.clone()));
            }
        }
        if let Some(col) = &spec.path_column {
            // What the wildcards matched, not the whole path: the varying part is
            // what identifies the store, and the rest is the same on every row.
            let which = path_captures(&spec.path, &entry.relative_path)
                .map(|c| c.join("/"))
                .unwrap_or_else(|| entry.relative_path.clone());
            for row in &mut rows {
                row.insert(col.clone(), serde_json::Value::String(which.clone()));
            }
        }
        // Constants are filled here rather than in each runner, so `value` means
        // the same thing for a SQL, plist and log module.
        for c in spec.columns.iter().filter(|c| c.value.is_some()) {
            let v = serde_json::Value::String(c.value.clone().unwrap_or_default());
            for row in &mut rows {
                row.insert(c.name.clone(), v.clone());
            }
        }
        all.extend(rows);
    }
    if !any_ran {
        if let Some(e) = skipped {
            return Err(e);
        }
    }
    enforce_shape(spec, all).map(Some)
}

/// One store, one set of rows.
fn run_one(
    spec: &ModuleSpec,
    index: &ManifestIndex,
    decryptor: Option<&BackupDecryptor>,
    work_dir: &Path,
    entry: &crate::manifest::FileEntry,
    nth: usize,
) -> Result<Vec<ArtifactRow>> {
    // A property list is read straight from memory: no sidecars to checkpoint, no
    // temp store to open read-only, so none of the SQLite machinery below applies —
    // including the work dir, which a plist module never writes to.
    if let Some(pl) = &spec.plist {
        let bytes = index.read_bytes(entry, decryptor)?;
        return run_plist_module(spec, pl, &bytes);
    }
    // A log is read from memory too — text, no sidecars, no temp store.
    if let Some(lg) = &spec.log {
        let bytes = index.read_bytes(entry, decryptor)?;
        return run_log_module(spec, lg, &bytes);
    }

    std::fs::create_dir_all(work_dir).map_err(|e| Error::Io {
        path: work_dir.to_path_buf(),
        source: e,
    })?;

    // `nth` in the name: a pattern matching several stores would otherwise have
    // each extraction overwrite the last, and every one of them would read the
    // final store's rows.
    let dest = work_dir.join(format!("{}.{nth}.sqlite", spec.id));
    index.extract_db(entry, decryptor, &dest)?;

    // Opened READ-ONLY, because the `SELECT`-prefix check in `validate` is not
    // actually sufficient: SQLite accepts `WITH x AS (…) INSERT … RETURNING a`,
    // which starts with "with", returns named columns, and writes. The prefix
    // check stays as a friendly early error; this is the part that enforces it.
    let conn = Connection::open_with_flags(&dest, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    // Attribute a bad query to its module. Raw rusqlite errors say things like
    // "no such table: nosuchtable" with nothing identifying which of the
    // (eventually hundreds of) modules asked.
    // Try each alternative in order; the first that prepares against THIS
    // backup's schema wins. Failing to prepare is exactly how a version
    // mismatch announces itself ("no such column: auth_value"), so it is the
    // signal to fall through — until none is left, which names the module.
    let mut prepared = None;
    let mut errors: Vec<String> = Vec::new();
    for (i, query) in spec.sql.iter().enumerate() {
        match conn.prepare(query) {
            Ok(s) => {
                prepared = Some(s);
                break;
            }
            // Every alternative's error is kept. Reporting only the last one
            // blames the oldest-schema query for a fault in the modern query,
            // which is exactly backwards on a modern backup.
            Err(e) => errors.push(format!("  [{i}] {e}")),
        }
    }
    let mut stmt = prepared.ok_or_else(|| {
        Error::Parse(format!(
            "artifact {} ({}): no `sql` alternative could run against this backup:\n{}",
            spec.id,
            spec.name,
            errors.join("\n"),
        ))
    })?;
    let sql_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    // Fail loudly when a declared column is not in the result set: silently
    // emitting nulls would make a renamed upstream column look like an artifact
    // that simply has no data.
    for c in &spec.columns {
        // The `path_column` has no `from`: the runner fills it with the store the
        // row came from, so the SQL neither can nor should return it.
        if c.from.is_empty() {
            continue;
        }
        // Exactly one segment: `validate` rejected anything else before we got
        // here, and it runs at the top of `run_module`.
        let from = &c.from[0];
        if !sql_names.iter().any(|n| n == from) {
            return Err(Error::Parse(format!(
                "artifact {}: column {:?} reads `{}`, which the SQL does not return (returns: {})",
                spec.id,
                c.name,
                from,
                sql_names.join(", ")
            )));
        }
    }

    let mut rows_out: Vec<ArtifactRow> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut out: ArtifactRow = HashMap::new();
        for c in &spec.columns {
            if c.from.is_empty() {
                continue; // filled by the runner, not by the query
            }
            // Checked above: exactly one segment, and present in the result set.
            let idx = sql_names.iter().position(|n| n == &c.from[0]).unwrap();
            let raw: rusqlite::types::Value = row.get(idx)?;
            out.insert(c.name.clone(), convert(&raw, c));
        }
        rows_out.push(out);
    }
    Ok(rows_out)
}

/// Read rows out of a property list, per the module's `[plist]` block.
/// Rows from a plain-text log: one per line carrying `json_after`.
///
/// Read lossily on purpose. A log is an append-only file the app may be writing
/// when the backup is taken, so a truncated multi-byte character at the tail is
/// normal — and rejecting the whole file for it would lose every complete record
/// before it. Individual lines that do not parse are skipped for the same
/// reason: one malformed record is not evidence the other «redacted» are wrong.
fn run_log_module(spec: &ModuleSpec, lg: &LogSpec, bytes: &[u8]) -> Result<Vec<ArtifactRow>> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find(&lg.json_after) else {
            continue;
        };
        let payload = line[at + lg.json_after.len()..].trim();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        let mut row: ArtifactRow = HashMap::new();
        for c in &spec.columns {
            // An empty `from` means the runner fills it (the path column) or it is
            // a constant. Descending an empty path would return the whole payload
            // and stringify the entire record into that column.
            if c.from.is_empty() {
                continue;
            }
            let cell = match lookup_json(&value, &c.from) {
                Some(v) => convert_json(v, c),
                None => serde_json::Value::Null,
            };
            row.insert(c.name.clone(), cell);
        }
        out.push(row);
    }
    Ok(out)
}

/// Descend a `from` path into a JSON payload. Object keys only: these payloads
/// are records, and an array would need an index, which no log module has needed.
fn lookup_json<'a>(value: &'a serde_json::Value, path: &[String]) -> Option<&'a serde_json::Value> {
    let mut node = value;
    for key in path {
        node = node.get(key)?;
    }
    Some(node)
}

/// One JSON value as the column declares it.
///
/// Values in these logs are stringly-typed at random — Life360 writes `battery`
/// as a number and `charge` as `"0"` in the same object — so a numeric column
/// accepts a numeric string, and a bool column accepts `"1"`/`"true"`. Without
/// that, half the fields of a real record read as null.
/// Enum code → word, applied after conversion.
///
/// The single choke point: JSON, plist and SQL all funnel through here, so a
/// mapped column behaves identically whichever kind of store it came from.
fn apply_map(v: serde_json::Value, c: &ColumnSpec) -> serde_json::Value {
    let Some(map) = &c.map else { return v };
    // Null is "not recorded" and must stay that way; mapping it would invent a
    // value for a key the device never wrote.
    if v.is_null() {
        return v;
    }
    let key = match &v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    // Always a string, mapped or not: a column that is text on one row and a
    // number on the next sorts and aligns differently row to row, which reads
    // as a rendering bug. An unmapped code travels as itself -- never as
    // "Unknown", which would lose the one thing worth keeping.
    serde_json::Value::String(map.get(&key).cloned().unwrap_or(key))
}

fn convert_json(v: &serde_json::Value, c: &ColumnSpec) -> serde_json::Value {
    apply_map(convert_json_inner(v, c), c)
}

fn convert_json_inner(v: &serde_json::Value, c: &ColumnSpec) -> serde_json::Value {
    use serde_json::Value as J;
    let as_f64 = |v: &J| -> Option<f64> {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
    };
    match c.kind {
        ColumnKind::Timestamp => {
            let Some(raw) = as_f64(v) else { return J::Null };
            // No epoch declared means the value is already Unix seconds — the
            // usual case for a JSON log, unlike a Core Data store.
            let epoch = c.epoch.unwrap_or(Epoch::Unix);
            epoch.to_unix_seconds(raw).map_or(J::Null, J::from)
        }
        // Seconds on the wire, formatted at the edge -- the UI is told WHICH
        // columns are durations rather than being asked to guess.
        ColumnKind::Integer | ColumnKind::Duration => {
            as_f64(v).map_or(J::Null, |f| J::from(f as i64))
        }
        ColumnKind::Real => as_f64(v).map_or(J::Null, J::from),
        ColumnKind::Bool => match v {
            J::Bool(b) => J::Bool(*b),
            J::Number(_) => J::Bool(as_f64(v).is_some_and(|f| f != 0.0)),
            J::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" => J::Bool(true),
                "0" | "false" | "no" | "" => J::Bool(false),
                _ => J::Null,
            },
            _ => J::Null,
        },
        // Bytes/Hex have no meaning in JSON — there is no byte string to render —
        // so they read as text rather than silently producing something invented.
        ColumnKind::Text | ColumnKind::Bytes | ColumnKind::Hex => match v {
            J::String(s) => J::String(s.clone()),
            J::Null => J::Null,
            other => J::String(other.to_string()),
        },
    }
}

fn run_plist_module(spec: &ModuleSpec, pl: &PlistSpec, bytes: &[u8]) -> Result<Vec<ArtifactRow>> {
    // `nska::resolve` rather than `Value::from_reader`: Apple wraps a great deal of
    // structured data in NSKeyedArchiver, which is not a plain plist but a
    // FLATTENED OBJECT GRAPH — a `$objects` array whose members reference each
    // other by UID. Read as a plist it looks like `$version`/`$archiver`/`$top`
    // and none of the data is at a path a module could name.
    //
    // The decoder already existed for the native parsers, and it returns a
    // non-archived plist untouched, so this costs nothing for the modules that do
    // not need it and unlocks the class for the ones that do.
    //
    // ONE THING A MODULE AUTHOR MUST KNOW: `resolve` unwraps a sole `$top` root,
    // so for the common single-root archive the paths start INSIDE the root object
    // — `rows = ["root"]` finds nothing. An archive with several roots keeps them
    // as a dictionary keyed by root name.
    let root = crate::nska::resolve(bytes).map_err(|e| {
        Error::Parse(format!(
            "artifact {}: {}:{} is not a readable property list: {e}",
            spec.id, spec.domain, spec.path
        ))
    })?;

    // Walk to the container(s). A `*` in the path fans out: Apple nests records
    // under arrays of arrays (a home screen is pages of icons), and without this a
    // module can only ever name one page by index — which is a different artifact
    // on every device.
    //
    // Each resolved container remembers which indices got it there, so
    // `index_column` can say which page a row came from.
    fn resolve_rows<'a>(
        node: &'a plist::Value,
        path: &[String],
        at: &mut Vec<String>,
        out: &mut Vec<(String, &'a plist::Value)>,
    ) {
        let Some((seg, rest)) = path.split_first() else {
            out.push((at.join("/"), node));
            return;
        };
        if seg == "*" {
            match node {
                plist::Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        at.push(i.to_string());
                        resolve_rows(item, rest, at, out);
                        at.pop();
                    }
                }
                plist::Value::Dictionary(d) => {
                    for (k, v) in d.iter() {
                        at.push(k.clone());
                        resolve_rows(v, rest, at, out);
                        at.pop();
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some(next) = step(node, seg) {
            resolve_rows(next, rest, at, out);
        }
    }

    // Walk to the container. A missing key is an ERROR, not an empty artifact:
    // "this key is gone" and "this device has none of these" are different facts,
    // and the second is what an empty result would claim.
    // The wildcard form resolves to many containers; the plain form to one, and
    // keeps its precise error messages, which name the segment that stopped.
    if pl.rows.iter().any(|k| k == "*") {
        let mut found = Vec::new();
        resolve_rows(&root, &pl.rows, &mut Vec::new(), &mut found);
        if found.is_empty() {
            // Declared optional: the container is written only once the feature
            // is used, so its absence is "none recorded", not a broken path.
            if pl.optional {
                return Ok(Vec::new());
            }
            return Err(Error::Parse(format!(
                "artifact {}: `plist.rows` path {:?} matched nothing in {}:{}",
                spec.id,
                pl.rows.join(" / "),
                spec.domain,
                spec.path
            )));
        }
        let mut all = Vec::new();
        for (index, container) in found {
            let mut rows = rows_from(spec, pl, container)?;
            if let Some(col) = &pl.index_column {
                for row in &mut rows {
                    row.insert(col.clone(), serde_json::Value::String(index.clone()));
                }
            }
            all.extend(rows);
        }
        return Ok(all);
    }

    let mut node = &root;
    for (i, key) in pl.rows.iter().enumerate() {
        node = match step(node, key) {
            Some(v) => v,
            None if pl.optional => return Ok(Vec::new()),
            None => {
                return Err(Error::Parse(format!(
                    "artifact {}: `plist.rows` path {:?} stops at {:?} — {} in {}:{}",
                    spec.id,
                    pl.rows.join(" / "),
                    pl.rows[..=i].join(" / "),
                    // A wrong TYPE and a missing KEY are different mistakes, and
                    // saying "that key is not there" about a scalar sends the
                    // author looking for the wrong thing.
                    match node {
                        plist::Value::Dictionary(_) => "there is no such key".to_string(),
                        plist::Value::Array(a) => format!(
                            "that is an array of {} items and this is not an index",
                            a.len()
                        ),
                        other => format!(
                            "that is {}, which cannot be descended into",
                            kind_name(other)
                        ),
                    },
                    spec.domain,
                    spec.path
                )));
            }
        };
    }

    rows_from(spec, pl, node)
}

/// Rows from ONE resolved container. Shared by the plain and wildcard paths so
/// they cannot drift — the wildcard form is the same reading, repeated.
fn rows_from(spec: &ModuleSpec, pl: &PlistSpec, node: &plist::Value) -> Result<Vec<ArtifactRow>> {
    // (key, row) pairs, decided by what the module ASKED FOR rather than by what
    // the shape happens to be.
    //
    // The first version inferred: a dictionary became many rows whenever `rows`
    // was non-empty. That made three different mistakes look like success —
    // a `rows` path landing on a scalar produced one row of nulls; descending to a
    // nested single RECORD produced one null row per field of it; and an empty
    // root dictionary produced a phantom row. All of them loaded, validated and
    // ran green. `key_column` is the declaration that says "this container holds
    // many things, keyed", so it is what decides.
    let entries: Vec<(Option<&str>, &plist::Value)> = match (node, pl.key_column.is_some()) {
        (plist::Value::Array(items), false) => items.iter().map(|v| (None, v)).collect(),
        (plist::Value::Array(_), true) => {
            return Err(Error::Parse(format!(
                "artifact {}: `plist.key_column` is declared but {} is an ARRAY, whose \
                 entries have no keys — every row's key would be blank",
                spec.id,
                describe_at(&pl.rows)
            )))
        }
        (plist::Value::Dictionary(d), true) => {
            d.iter().map(|(k, v)| (Some(k.as_str()), v)).collect()
        }
        // A dictionary with no `key_column` is ONE record — a settings plist is a
        // row. Empty means nothing was recorded, which is zero rows, not one row
        // of nulls.
        (plist::Value::Dictionary(d), false) => {
            if d.is_empty() {
                Vec::new()
            } else {
                vec![(None, node)]
            }
        }
        // A SCALAR is one row when the module said the value is the row. This is
        // what `rows = ["buttonBar", "*"]` resolves to: the wildcard hands over
        // each string, and each string is a dock entry.
        (scalar, _) if pl.value_column.is_some() => vec![(None, scalar)],
        (other, _) => {
            return Err(Error::Parse(format!(
                "artifact {}: {} is {}, which holds no rows — `plist.rows` should point at \
                 a dictionary or an array, or declare a `value_column` if the value IS the row",
                spec.id,
                describe_at(&pl.rows),
                kind_name(other)
            )))
        }
    };

    let mut out = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        let mut row: ArtifactRow = HashMap::new();
        if let Some(col) = &pl.key_column {
            let k = key.unwrap_or_default();
            // Trim only when it really is the prefix. Trimming blindly would hide
            // a key whose shape has changed, which is the interesting case.
            let shown = match &pl.key_strip_prefix {
                Some(prefix) => k.strip_prefix(prefix.as_str()).unwrap_or(k),
                None => k,
            };
            row.insert(col.clone(), serde_json::Value::String(shown.to_string()));
        }
        // A row whose value is a scalar: the value IS the row, so it goes in the
        // declared column and no key path is walked. Declaring one and meeting a
        // CONTAINER is a mis-declaration, not a null — the module has misread the
        // store's shape, and a column of nulls would hide that.
        if let Some(col) = &pl.value_column {
            match value {
                // Still an error for a container: a value column is for rows that
                // ARE a value, and meeting a dictionary means the module misread
                // the shape. The scalar case above is the intended one.
                plist::Value::Dictionary(_) | plist::Value::Array(_) => {
                    return Err(Error::Parse(format!(
                        "artifact {}: `plist.value_column` is declared but a row is {} — a \
                         value column is for rows that ARE a value (a number, a string), not \
                         for containers with fields to name",
                        spec.id,
                        kind_name(value)
                    )))
                }
                scalar => {
                    let c = spec
                        .columns
                        .iter()
                        .find(|c| &c.name == col)
                        .expect("validated to exist");
                    row.insert(col.clone(), convert_plist(scalar, c)?);
                }
            }
        }
        for c in &spec.columns {
            if Some(&c.name) == pl.key_column.as_ref() {
                continue; // already filled from the key
            }
            if Some(&c.name) == pl.value_column.as_ref() {
                continue; // already filled from the value
            }
            let cell = match lookup_path(value, &c.from) {
                Some(v) => convert_plist(v, c)
                    .map_err(|e| Error::Parse(format!("artifact {}: {e}", spec.id)))?,
                None => serde_json::Value::Null,
            };
            row.insert(c.name.clone(), cell);
        }
        out.push(row);
    }
    Ok(out)
}

/// One step along a path: a dictionary key, or an array index.
///
/// Array indexing exists because Apple nests records under arrays routinely
/// (`Root / Items / 0 / …`), and a dictionary-only walk made every such artifact
/// unreachable. No ambiguity: a dictionary is indexed by key and an array by
/// number, and which one applies is decided by the node, not by the segment.
fn step<'a>(node: &'a plist::Value, segment: &str) -> Option<&'a plist::Value> {
    match node {
        plist::Value::Dictionary(d) => d.get(segment),
        plist::Value::Array(a) => segment.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    }
}

/// Descend a key path. `None` when any segment is missing — which becomes a null
/// cell, because a key absent from ONE record is ordinary (not every network is
/// hidden, not every account has a label), unlike the `rows` path being wrong.
fn lookup_path<'a>(value: &'a plist::Value, path: &[String]) -> Option<&'a plist::Value> {
    let mut node = value;
    for key in path {
        node = step(node, key)?;
    }
    Some(node)
}

/// Names a plist value's kind for an error message.
fn kind_name(v: &plist::Value) -> &'static str {
    match v {
        plist::Value::Array(_) => "an array",
        plist::Value::Dictionary(_) => "a dictionary",
        plist::Value::Boolean(_) => "a boolean",
        plist::Value::Data(_) => "raw data",
        plist::Value::Date(_) => "a date",
        plist::Value::Real(_) => "a number",
        plist::Value::Integer(_) => "a number",
        plist::Value::String(_) => "a string",
        plist::Value::Uid(_) => "a uid",
        _ => "an unknown value",
    }
}

/// How to refer to where `plist.rows` points, in an error.
fn describe_at(rows: &[String]) -> String {
    if rows.is_empty() {
        "the root".to_string()
    } else {
        format!("`plist.rows` path {:?}", rows.join(" / "))
    }
}

/// A plist value as JSON, coerced by the column's declared kind.
///
/// `Err` for a mistake the module could not have detected at load time — a
/// timestamp column meeting a number when no `epoch` was declared. Everything
/// else that cannot be represented is a null cell, which is an honest "not this".
fn convert_plist(v: &plist::Value, c: &ColumnSpec) -> Result<serde_json::Value> {
    convert_plist_inner(v, c).map(|out| apply_map(out, c))
}

fn convert_plist_inner(v: &plist::Value, c: &ColumnSpec) -> Result<serde_json::Value> {
    use serde_json::Value as J;
    // plist Integers are i128-backed and Apple writes plenty of ids as u64 above
    // i64::MAX (persistent ids, address-shaped values). `as_signed` alone turns
    // those into nulls, so the unsigned half is tried too.
    fn as_i64(i: &plist::Integer) -> Option<i64> {
        i.as_signed()
            .or_else(|| i.as_unsigned().and_then(|u| i64::try_from(u).ok()))
    }
    Ok(match c.kind {
        ColumnKind::Timestamp => match v {
            // A plist Date is already absolute; no epoch is involved. It still
            // goes through the same plausibility clamp as every other timestamp:
            // an out-of-range value reaching the UI is not merely odd, it makes
            // `Intl.DateTimeFormat.format` throw on a non-finite Date.
            plist::Value::Date(d) => {
                let t: std::time::SystemTime = (*d).into();
                let secs = match t.duration_since(std::time::UNIX_EPOCH) {
                    Ok(dur) => dur.as_secs() as f64,
                    // Before 1970. `duration()` is the positive gap by which the
                    // time precedes the epoch, so negating it is the real value.
                    Err(e) => -(e.duration().as_secs() as f64),
                };
                Epoch::Unix.to_unix_seconds(secs).map_or(J::Null, J::from)
            }
            // A number still needs the module to say what epoch it is in — and
            // if it did not, saying so beats a column of silent nulls. This is
            // the case `validate` cannot catch, because the value's type is only
            // known here.
            plist::Value::Integer(_) | plist::Value::Real(_) if c.epoch.is_none() => {
                return Err(Error::Parse(format!(
                    "column {:?} is a timestamp and this store holds it as a NUMBER, but no \
                     `epoch` is declared. A plist `Date` needs none; a number does — say \
                     which epoch it counts from",
                    c.name
                )))
            }
            plist::Value::Integer(i) => as_i64(i)
                .and_then(|n| c.epoch.and_then(|e| e.to_unix_seconds(n as f64)))
                .map_or(J::Null, J::from),
            plist::Value::Real(f) => c
                .epoch
                .and_then(|e| e.to_unix_seconds(*f))
                .map_or(J::Null, J::from),
            _ => J::Null,
        },
        ColumnKind::Bool => match v {
            plist::Value::Boolean(b) => J::Bool(*b),
            plist::Value::Integer(i) => as_i64(i).map_or(J::Null, |n| J::Bool(n != 0)),
            _ => J::Null,
        },
        ColumnKind::Integer | ColumnKind::Bytes | ColumnKind::Duration => match v {
            plist::Value::Integer(i) => as_i64(i).map_or(J::Null, J::from),
            plist::Value::Real(f) => J::from(*f as i64),
            plist::Value::Boolean(b) => J::from(i64::from(*b)),
            _ => J::Null,
        },
        ColumnKind::Real => match v {
            plist::Value::Real(f) => J::from(*f),
            plist::Value::Integer(i) => as_i64(i).map_or(J::Null, |n| J::from(n as f64)),
            _ => J::Null,
        },
        ColumnKind::Hex => match v {
            plist::Value::Data(d) => J::String(
                d.iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(":"),
            ),
            // Already text: a store that writes the same field as a string on some
            // devices should not become null on those.
            plist::Value::String(s) => J::String(s.clone()),
            _ => J::Null,
        },
        ColumnKind::Text => match v {
            plist::Value::String(s) => J::String(s.clone()),
            // Formatted from the Integer itself rather than through i64, so a
            // value too large for i64 still prints instead of vanishing.
            plist::Value::Integer(i) => J::String(format!("{i}")),
            plist::Value::Real(f) => J::String(f.to_string()),
            plist::Value::Boolean(b) => J::String(if *b { "Yes" } else { "No" }.into()),
            // Apple stores plenty of text as Data — an SSID is raw bytes. Decoded
            // when it really is UTF-8; NULL otherwise, matching what the SQL path
            // does with a blob. The first version fabricated "<10 bytes>", which
            // is a string: it would flow into the row store, exports and search as
            // though it were content.
            //
            // Only C0 controls disqualify it, and not tab/newline/carriage return —
            // multi-line text in a plist is ordinary.
            plist::Value::Data(d) => match std::str::from_utf8(d) {
                Ok(t)
                    if !t.is_empty()
                        && !t
                            .chars()
                            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')) =>
                {
                    J::String(t.to_string())
                }
                _ => J::Null,
            },
            // A nested container has no single text form, and inventing one
            // (JSON? a count?) would be this file guessing on a module's behalf.
            _ => J::Null,
        },
    })
}

fn convert(raw: &rusqlite::types::Value, c: &ColumnSpec) -> serde_json::Value {
    apply_map(convert_inner(raw, c), c)
}

fn convert_inner(raw: &rusqlite::types::Value, c: &ColumnSpec) -> serde_json::Value {
    use rusqlite::types::Value as V;
    use serde_json::Value as J;
    match c.kind {
        ColumnKind::Timestamp => {
            let n = match raw {
                V::Integer(i) => *i as f64,
                V::Real(f) => *f,
                _ => return J::Null,
            };
            // `epoch` is guaranteed present for timestamps by validate().
            match c.epoch.and_then(|e| e.to_unix_seconds(n)) {
                Some(secs) => J::from(secs),
                None => J::Null,
            }
        }
        ColumnKind::Bool => match raw {
            V::Integer(i) => J::Bool(*i != 0),
            V::Null => J::Null,
            _ => J::Null,
        },
        // Bytes arrive as an integer count and are formatted by the UI. Core
        // Data stores these as FLOAT (DataUsage's ZWIFIIN and friends are
        // declared FLOAT), so the Real arm is the one that actually fires.
        // Durations travel the same way: seconds on the wire, formatted at the
        // edge.
        ColumnKind::Integer | ColumnKind::Bytes | ColumnKind::Duration => match raw {
            V::Integer(i) => J::from(*i),
            V::Real(f) => J::from(*f as i64),
            _ => J::Null,
        },
        ColumnKind::Real => match raw {
            V::Real(f) => J::from(*f),
            V::Integer(i) => J::from(*i as f64),
            _ => J::Null,
        },
        // A SQLite BLOB is the same idea as a plist Data.
        ColumnKind::Hex => match raw {
            V::Blob(b) => J::String(
                b.iter()
                    .map(|x| format!("{x:02x}"))
                    .collect::<Vec<_>>()
                    .join(":"),
            ),
            V::Text(s) => J::String(s.clone()),
            _ => J::Null,
        },
        ColumnKind::Text => match raw {
            V::Text(s) => J::String(s.clone()),
            V::Integer(i) => J::String(i.to_string()),
            V::Real(f) => J::String(f.to_string()),
            V::Blob(_) => J::Null,
            V::Null => J::Null,
        },
    }
}

/// Replace every stored row for `artifact_id` with `rows`.
///
/// Replace rather than append, so a re-import is idempotent — the same property
/// the hand-written importers already have (they clear and reinsert).
pub fn store_rows(conn: &Connection, artifact_id: &str, rows: &[ArtifactRow]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM artifact_rows WHERE artifact_id = ?1",
        rusqlite::params![artifact_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO artifact_rows (artifact_id, row_idx, payload) VALUES (?1, ?2, ?3)",
        )?;
        for (i, row) in rows.iter().enumerate() {
            let payload = serde_json::to_string(row).map_err(|e| {
                Error::Parse(format!("artifact {artifact_id}: encoding row {i}: {e}"))
            })?;
            stmt.execute(rusqlite::params![artifact_id, i as i64, payload])?;
        }
    }
    tx.commit()?;
    Ok(rows.len())
}

/// Rows for one artifact, in the order the module produced them.
pub fn read_rows(
    conn: &Connection,
    artifact_id: &str,
    offset: i64,
    limit: i64,
) -> Result<Vec<ArtifactRow>> {
    let mut stmt = conn.prepare(
        "SELECT payload FROM artifact_rows WHERE artifact_id = ?1
         ORDER BY row_idx LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![artifact_id, limit, offset], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    rows.iter()
        .map(|p| {
            serde_json::from_str(p)
                .map_err(|e| Error::Parse(format!("artifact {artifact_id}: decoding row: {e}")))
        })
        .collect()
}

/// How many rows are stored for an artifact.
pub fn count_rows(conn: &Connection, artifact_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM artifact_rows WHERE artifact_id = ?1",
        rusqlite::params![artifact_id],
        |r| r.get(0),
    )?)
}

/// The modules shipped with the app, compiled in.
///
/// Embedded rather than read from disk because a shipped `.app` has no
/// `modules/` directory to read — and pointing at one would mean a path that
/// only exists on the machine that built the binary.
///
/// The cost is that adding an artifact is a one-line code change here as well
/// as a TOML file, which dents #190's "adding a module is a data change". The
/// data half still holds — the SQL, paths and columns are reviewed as data —
/// and this list is the smallest possible seam. If it ever gets long enough to
/// be annoying, `include_dir` removes it.
const BUILTIN: &[(&str, &str)] = &[
    ("tcc.toml", include_str!("../modules/tcc.toml")),
    ("accounts.toml", include_str!("../modules/accounts.toml")),
    (
        "bluetooth_paired.toml",
        include_str!("../modules/bluetooth_paired.toml"),
    ),
    (
        "carplay_recent_apps.toml",
        include_str!("../modules/carplay_recent_apps.toml"),
    ),
    (
        "carplay_session.toml",
        include_str!("../modules/carplay_session.toml"),
    ),
    (
        "data_usage.toml",
        include_str!("../modules/data_usage.toml"),
    ),
    ("sim_cards.toml", include_str!("../modules/sim_cards.toml")),
    (
        "wifi_networks.toml",
        include_str!("../modules/wifi_networks.toml"),
    ),
    (
        "bluetooth_devices.toml",
        include_str!("../modules/bluetooth_devices.toml"),
    ),
    (
        "wifi_private_mac.toml",
        include_str!("../modules/wifi_private_mac.toml"),
    ),
    (
        "bluetooth_nearby.toml",
        include_str!("../modules/bluetooth_nearby.toml"),
    ),
    (
        "device_locale.toml",
        include_str!("../modules/device_locale.toml"),
    ),
    ("alarms.toml", include_str!("../modules/alarms.toml")),
    ("timers.toml", include_str!("../modules/timers.toml")),
    ("stopwatch.toml", include_str!("../modules/stopwatch.toml")),
    ("airdrop.toml", include_str!("../modules/airdrop.toml")),
    (
        "message_retention.toml",
        include_str!("../modules/message_retention.toml"),
    ),
    (
        "backup_settings.toml",
        include_str!("../modules/backup_settings.toml"),
    ),
    (
        "location_services.toml",
        include_str!("../modules/location_services.toml"),
    ),
    ("imei_imsi.toml", include_str!("../modules/imei_imsi.toml")),
    ("find_my.toml", include_str!("../modules/find_my.toml")),
    (
        "icloud_drive.toml",
        include_str!("../modules/icloud_drive.toml"),
    ),
    (
        "icloud_devices.toml",
        include_str!("../modules/icloud_devices.toml"),
    ),
    (
        "webkit_domains.toml",
        include_str!("../modules/webkit_domains.toml"),
    ),
    (
        "service_workers.toml",
        include_str!("../modules/service_workers.toml"),
    ),
    (
        "chromium_logins.toml",
        include_str!("../modules/chromium_logins.toml"),
    ),
    (
        "chromium_top_sites.toml",
        include_str!("../modules/chromium_top_sites.toml"),
    ),
    (
        "mega_files.toml",
        include_str!("../modules/mega_files.toml"),
    ),
    (
        "waze_places.toml",
        include_str!("../modules/waze_places.toml"),
    ),
    (
        "waze_recents.toml",
        include_str!("../modules/waze_recents.toml"),
    ),
    (
        "waze_favorites.toml",
        include_str!("../modules/waze_favorites.toml"),
    ),
    (
        "os_build_history.toml",
        include_str!("../modules/os_build_history.toml"),
    ),
    (
        "icloud_app_libraries.toml",
        include_str!("../modules/icloud_app_libraries.toml"),
    ),
    (
        "world_clock.toml",
        include_str!("../modules/world_clock.toml"),
    ),
    (
        "sleep_schedule.toml",
        include_str!("../modules/sleep_schedule.toml"),
    ),
    (
        "siri_settings.toml",
        include_str!("../modules/siri_settings.toml"),
    ),
    (
        "life360_locations.toml",
        include_str!("../modules/life360_locations.toml"),
    ),
    (
        "location_clients.toml",
        include_str!("../modules/location_clients.toml"),
    ),
    (
        "watch_apps.toml",
        include_str!("../modules/watch_apps.toml"),
    ),
    (
        "backup_sizing.toml",
        include_str!("../modules/backup_sizing.toml"),
    ),
    ("podcasts.toml", include_str!("../modules/podcasts.toml")),
    (
        "podcast_episodes.toml",
        include_str!("../modules/podcast_episodes.toml"),
    ),
    ("alltrails.toml", include_str!("../modules/alltrails.toml")),
    (
        "health_current_device.toml",
        include_str!("../modules/health_current_device.toml"),
    ),
    (
        "home_screen.toml",
        include_str!("../modules/home_screen.toml"),
    ),
    (
        "home_screen_widgets.toml",
        include_str!("../modules/home_screen_widgets.toml"),
    ),
    ("dock.toml", include_str!("../modules/dock.toml")),
];

/// Parse the compiled-in modules. Errors carry the module's filename, exactly
/// as `load_modules` does for on-disk ones.
pub fn builtin_modules() -> Result<Vec<ModuleSpec>> {
    let mut out: Vec<ModuleSpec> = Vec::new();
    for (file, text) in BUILTIN {
        let spec: ModuleSpec = toml::from_str(text)
            .map_err(|e| Error::Parse(format!("artifact module {file}: {e}")))?;
        spec.validate()
            .map_err(|why| Error::Parse(format!("artifact module {file}: {why}")))?;
        if let Some(prev) = out.iter().find(|m| m.id == spec.id) {
            return Err(Error::Parse(format!(
                "artifact module {file}: id {:?} is already used by {:?}",
                spec.id, prev.name
            )));
        }
        out.push(spec);
    }
    Ok(out)
}

/// The modules directory on disk — tests only, so they can prove the shipped
/// TOML files themselves parse rather than only the embedded copies.
#[cfg(test)]
pub fn builtin_modules_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("modules")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_toml(extra: &str) -> String {
        format!(
            r#"
id = "demo"
name = "Demo"
description = "A demo artifact."
surface = "standalone"
domain = "HomeDomain"
path = "Library/Demo/demo.db"
sql = "SELECT who, at FROM events"

[[columns]]
name = "Who"
from = "who"

[[columns]]
name = "At"
from = "at"
kind = "timestamp"
epoch = "cocoa"
{extra}
"#
        )
    }

    fn write_module(dir: &Path, file: &str, body: &str) {
        std::fs::write(dir.join(file), body).unwrap();
    }

    #[test]
    fn loads_and_validates_a_module() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "demo.toml", &spec_toml(""));
        let mods = load_modules(tmp.path()).unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].id, "demo");
        assert_eq!(mods[0].columns.len(), 2);
        assert!(!mods[0].needs_encrypted_backup());
    }

    #[test]
    fn missing_module_dir_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = load_modules(&tmp.path().join("nope")).unwrap();
        assert!(mods.is_empty());
    }

    /// A malformed module must name the file and the problem. Every case below
    /// is a way a module could be wrong; each must be rejected at load, not
    /// discovered when the artifact mysteriously has no rows.
    #[test]
    fn malformed_modules_fail_by_name_and_reason() {
        let cases: Vec<(&str, &str, &str)> = vec![
            // The needle must be something only the TOML parser produces; "artifact
            // module" is the shared prefix of every error in this function.
            ("bad-toml", "id = \"x\"\nname =", "TOML parse error"),
            // A module with BOTH sources: which one produced a row would be
            // unanswerable.
            (
                "two-sources",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.plist"
sql = "SELECT a FROM t"
[plist]
[[columns]]
name = "A"
from = "a"
"#,
                "more than one of `sql`, `[plist]` and `[log]`",
            ),
            // A key that is read and then has nowhere to go.
            (
                "key-column-not-declared",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.plist"
[plist]
key_column = "Nope"
[[columns]]
name = "A"
from = "a"
"#,
                "`plist.key_column`",
            ),
            // Trimming a prefix off a key nobody reads.
            (
                "strip-without-key",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.plist"
[plist]
key_strip_prefix = "wifi."
[[columns]]
name = "A"
from = "a"
"#,
                "`key_column` is not",
            ),
            (
                "no-epoch",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT at FROM t"
[[columns]]
name = "At"
from = "at"
kind = "timestamp"
"#,
                "declares no `epoch`",
            ),
            (
                "not-select",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "DELETE FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "must be a SELECT",
            ),
            (
                "no-columns",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
columns = []
"#,
                "no `[[columns]]` declared",
            ),
            (
                "bad-requires",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
requires = "a-pony"
[[columns]]
name = "A"
from = "a"
"#,
                "unknown `requires`",
            ),
            (
                "empty-id",
                r#"
id = ""
description = "Case fixture."
surface = "standalone"
name = "X"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "`id` is empty",
            ),
            (
                "empty-domain",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = ""
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "`domain` is empty",
            ),
            (
                "empty-path",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = ""
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "`path` is empty",
            ),
            (
                "empty-sql",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = ""
[[columns]]
name = "A"
from = "a"
"#,
                "no source: declare one of `sql`, `[plist]` or `[log]`",
            ),
            (
                "empty-column",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = ""
"#,
                "empty key in its `from` path",
            ),
            // A column with no `from` at all. Allowed ONLY for a plist key
            // column; here it would be a column of nothing but nulls.
            (
                "no-from",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
"#,
                "declares no `from`",
            ),
            (
                "empty-column-name",
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = " "
from = "a"
"#,
                "has an empty `name`",
            ),
            (
                "bad-id",
                r#"
id = "x y"
description = "Case fixture."
surface = "standalone"
name = "X"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
                "may only contain",
            ),
        ];

        for (label, body, needle) in cases {
            let tmp = tempfile::tempdir().unwrap();
            write_module(tmp.path(), &format!("{label}.toml"), body);
            let err = load_modules(tmp.path())
                .expect_err(&format!("{label} should have been rejected"))
                .to_string();
            assert!(
                err.contains(needle),
                "{label}: error {err:?} does not mention {needle:?}"
            );
            assert!(
                err.contains(label),
                "{label}: error {err:?} does not name the offending file"
            );
        }
    }

    /// The conversions, including the one that matters most: a Cocoa timestamp
    /// read as Unix lands in 1970 and nothing about the number says so.
    #[test]
    fn epochs_convert_to_unix_seconds() {
        // 2024-01-01T00:00:00Z
        let unix = 1_704_067_200_i64;
        assert_eq!(Epoch::Unix.to_unix_seconds(unix as f64), Some(unix));
        assert_eq!(
            Epoch::UnixMs.to_unix_seconds(unix as f64 * 1000.0),
            Some(unix)
        );
        assert_eq!(
            Epoch::Cocoa.to_unix_seconds(unix as f64 - 978_307_200.0),
            Some(unix)
        );
        assert_eq!(
            Epoch::Webkit.to_unix_seconds((unix as f64 + 11_644_473_600.0) * 1_000_000.0),
            Some(unix)
        );
        // A Cocoa value misread as Unix is 31 years out — the failure this
        // per-column declaration exists to prevent.
        assert_ne!(
            Epoch::Unix.to_unix_seconds(unix as f64 - 978_307_200.0),
            Some(unix)
        );
        // Undecodable values degrade to "no date" rather than 1970 or a
        // fifty-year chart axis.
        assert_eq!(Epoch::Unix.to_unix_seconds(-1.0), None);
        assert_eq!(Epoch::Unix.to_unix_seconds(f64::NAN), None);
        assert_eq!(Epoch::Cocoa.to_unix_seconds(1e18), None);
    }

    /// Build a minimal plaintext backup containing one SQLite store, so the
    /// runner can be exercised end to end without any real device data.
    fn make_backup(dir: &Path, rel: &str, build: impl FnOnce(&Connection)) {
        make_backup_in(dir, "HomeDomain", rel, build)
    }

    fn make_backup_in(dir: &Path, domain: &str, rel: &str, build: impl FnOnce(&Connection)) {
        let file_id = "cd00000000000000000000000000000000000001";
        let blob_dir = dir.join(&file_id[..2]);
        std::fs::create_dir_all(&blob_dir).unwrap();
        let store = blob_dir.join(file_id);
        let conn = Connection::open(&store).unwrap();
        build(&conn);
        drop(conn);
        finish_manifest(dir, domain, rel, file_id);
    }

    /// The same, for a store that is NOT a database — a property list is raw
    /// bytes, and `extract_db`'s checkpointing has nothing to do with it.
    fn make_backup_bytes_in(dir: &Path, domain: &str, rel: &str, bytes: &[u8]) {
        let file_id = "cd00000000000000000000000000000000000001";
        let blob_dir = dir.join(&file_id[..2]);
        std::fs::create_dir_all(&blob_dir).unwrap();
        std::fs::write(blob_dir.join(file_id), bytes).unwrap();
        finish_manifest(dir, domain, rel, file_id);
    }

    fn finish_manifest(dir: &Path, domain: &str, rel: &str, file_id: &str) {
        let conn = Connection::open(dir.join("Manifest.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO Files VALUES (?1, ?2, ?3, 1, NULL)",
            rusqlite::params![file_id, domain, rel],
        )
        .unwrap();
    }

    /// The whole spine: a declared module, a real backup, typed rows out —
    /// with the date arriving as a date rather than the raw Cocoa float.
    #[test]
    fn runs_a_module_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        // 2024-01-01T00:00:00Z expressed in Core Data seconds.
        let cocoa = 1_704_067_200_f64 - 978_307_200.0;
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
            c.execute(
                "INSERT INTO events VALUES ('alice', ?1), ('bob', NULL)",
                rusqlite::params![cocoa],
            )
            .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(&mods_dir, "demo.toml", &spec_toml(""));
        let spec = &load_modules(&mods_dir).unwrap()[0];

        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .expect("the backup contains this artifact");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["Who"], serde_json::json!("alice"));
        assert_eq!(
            rows[0]["At"],
            serde_json::json!(1_704_067_200_i64),
            "a Cocoa timestamp must arrive as Unix seconds, not the raw stored float"
        );
        // A NULL date is absent, not 1970.
        assert_eq!(rows[1]["At"], serde_json::Value::Null);
    }

    /// An artifact the backup does not contain is a normal outcome, and it must
    /// cost nothing: the Manifest is consulted and the filesystem is not
    /// touched. Proven by pointing at a path no Manifest row mentions and
    /// asserting the work dir was never even created.
    #[test]
    fn absent_artifact_returns_none_and_opens_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Something/else.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(&mods_dir, "demo.toml", &spec_toml(""));
        let spec = &load_modules(&mods_dir).unwrap()[0];

        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let work = tmp.path().join("work");
        let out = run_module(spec, &index, None, &work).unwrap();

        assert!(
            out.is_none(),
            "an absent artifact yields None, not an error"
        );
        assert!(
            !work.exists(),
            "the runner created its work dir for an artifact the backup does not have — \
             absence must cost one indexed lookup and no filesystem work"
        );
    }

    /// A module whose SQL stops returning a declared column must fail loudly.
    /// Emitting nulls instead would make an upstream rename look exactly like
    /// an artifact that happens to be empty.
    #[test]
    fn column_missing_from_the_result_set_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        // Declares a `Missing` column the SELECT never returns.
        write_module(
            &mods_dir,
            "demo.toml",
            &format!(
                "{}\n[[columns]]\nname = \"Missing\"\nfrom = \"nope\"\n",
                spec_toml("")
            ),
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(spec, &index, None, &tmp.path().join("work"))
            .expect_err("a declared column that the SQL does not return must fail")
            .to_string();
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("which the SQL does not return"), "{err}");
    }

    /// Storage round-trips, preserves order, and a re-import replaces rather
    /// than appends — otherwise every import would double the row count.
    #[test]
    fn rows_round_trip_and_reimport_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let conn = db.conn();

        let mk = |who: &str| -> ArtifactRow {
            let mut r = HashMap::new();
            r.insert("Who".to_string(), serde_json::json!(who));
            r
        };
        store_rows(conn, "demo", &[mk("alice"), mk("bob")]).unwrap();
        assert_eq!(count_rows(conn, "demo").unwrap(), 2);

        let back = read_rows(conn, "demo", 0, 10).unwrap();
        assert_eq!(back[0]["Who"], serde_json::json!("alice"));
        assert_eq!(back[1]["Who"], serde_json::json!("bob"), "order preserved");

        // A second run of the same artifact replaces, never appends.
        store_rows(conn, "demo", &[mk("carol")]).unwrap();
        assert_eq!(count_rows(conn, "demo").unwrap(), 1);
        assert_eq!(
            read_rows(conn, "demo", 0, 10).unwrap()[0]["Who"],
            serde_json::json!("carol")
        );

        // Artifacts do not tread on each other.
        store_rows(conn, "other", &[mk("dave")]).unwrap();
        assert_eq!(count_rows(conn, "demo").unwrap(), 1);
        assert_eq!(count_rows(conn, "other").unwrap(), 1);

        // Paging.
        store_rows(conn, "demo", &[mk("a"), mk("b"), mk("c")]).unwrap();
        let page = read_rows(conn, "demo", 1, 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["Who"], serde_json::json!("b"));
    }

    /// Two modules with the same id must be rejected: `store_rows` is keyed on
    /// the id and replaces, so a collision silently destroys one artifact's
    /// rows and shows the other's in its place.
    #[test]
    fn duplicate_module_ids_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "one.toml", &spec_toml(""));
        write_module(tmp.path(), "two.toml", &spec_toml(""));
        let err = load_modules(tmp.path())
            .expect_err("two modules sharing an id must not load")
            .to_string();
        assert!(err.contains("already used"), "{err}");
        assert!(
            err.contains("two.toml"),
            "the error must name the offending file: {err}"
        );
    }

    /// Regressions for the four defects a review found in this module's first
    /// draft. Each was silent — the reason they are pinned rather than trusted.
    #[test]
    fn duplicate_column_names_are_rejected() {
        // A row is keyed on the display name, so two columns called "Date"
        // collapsed to one: a column vanishing with no error anywhere.
        let tmp = tempfile::tempdir().unwrap();
        write_module(
            tmp.path(),
            "dup.toml",
            r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT created, modified FROM t"
[[columns]]
name = "Date"
from = "created"
[[columns]]
name = "Date"
from = "modified"
"#,
        );
        let err = load_modules(tmp.path())
            .expect_err("duplicate names must fail")
            .to_string();
        assert!(err.contains("both named"), "{err}");
    }

    /// A `*` in `path` is a pattern now, and the rules around it are what stop it
    /// repeating the failures that got globbing removed the first time.
    ///
    /// Prefix matching picked up directory rows (a directory sorts before its own
    /// children) and `-wal`/`-shm` siblings. Both are impossible here because `*`
    /// never crosses `/` and the sidecars are excluded by name — see
    /// `a_pattern_reads_every_matching_store` for the behavioural half.
    #[test]
    fn path_pattern_rules_are_enforced_at_load() {
        let module = |path: &str, extra: &str| {
            format!(
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "{path}"
{extra}
sql = "SELECT a AS a FROM t"

[[columns]]
name = "A"
from = "a"

[[columns]]
name = "Store"
"#
            )
        };
        let fails = |src: String, needle: &str| {
            let tmp = tempfile::tempdir().unwrap();
            write_module(tmp.path(), "m.toml", &src);
            let err = load_modules(tmp.path())
                .expect_err("should have been rejected")
                .to_string();
            assert!(err.contains(needle), "{err}");
        };

        // A pattern with nothing saying which store a row came from.
        fails(
            module("Library/Foo/*/store.db", ""),
            "no `path_column` is declared",
        );
        // `**` would cross `/` — the exact thing that made prefix matching wrong.
        fails(
            module("Library/fo**o/store.db", "path_column = \"Store\""),
            "mixes `**` into a segment",
        );
        // A `path_column` on an exact path repeats one value on every row.
        fails(
            module("Library/Foo/store.db", "path_column = \"Store\""),
            "`path` is exact",
        );
        // Naming a column that does not exist.
        fails(
            module("Library/Foo/*/store.db", "path_column = \"Nope\""),
            "not one of the declared columns",
        );

        // And the well-formed shape loads.
        let tmp = tempfile::tempdir().unwrap();
        write_module(
            tmp.path(),
            "m.toml",
            &module("Library/Foo/*/store.db", "path_column = \"Store\""),
        );
        let mods = load_modules(tmp.path()).expect("a well-formed pattern should load");
        assert_eq!(mods.len(), 1);
    }

    /// A pattern reads EVERY matching store, and each row says which one it came
    /// from.
    ///
    /// This is the behaviour the whole feature exists for: an app with two
    /// accounts has two stores, and reading one of them would report half the
    /// user's data as all of it. It also pins the two traps that got globbing
    /// removed the first time — a `-wal` sidecar beside a matched store, and a
    /// directory row that sorts before its own children.
    #[test]
    fn a_pattern_reads_every_matching_store() {
        let spec: ModuleSpec = toml::from_str(
            r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "AppDomainGroup-group.example"
path = "accounts/*/chat.db"
path_column = "Store"
sql = "SELECT who AS who FROM t"

[[columns]]
name = "Who"
from = "who"

[[columns]]
name = "Store"
"#,
        )
        .unwrap();

        // Two accounts, each with its own store — plus the traps.
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Connection::open(tmp.path().join("Manifest.db")).unwrap();
        manifest
            .execute_batch(
                "CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT, relativePath TEXT, flags INTEGER, file BLOB);",
            )
            .unwrap();
        let add = |id: &str, rel: &str, build: Option<&dyn Fn(&Connection)>| {
            let dir = tmp.path().join(&id[..2]);
            std::fs::create_dir_all(&dir).unwrap();
            if let Some(b) = build {
                let c = Connection::open(dir.join(id)).unwrap();
                b(&c);
            } else {
                std::fs::write(dir.join(id), b"not a store").unwrap();
            }
            manifest
                .execute(
                    "INSERT INTO Files VALUES (?1, ?2, ?3, 1, NULL)",
                    rusqlite::params![id, "AppDomainGroup-group.example", rel],
                )
                .unwrap();
        };
        let seed = |name: &'static str| {
            move |c: &Connection| {
                c.execute_batch("CREATE TABLE t (who TEXT);").unwrap();
                c.execute("INSERT INTO t VALUES (?1)", rusqlite::params![name])
                    .unwrap();
            }
        };
        let alice = seed("alice");
        let bob = seed("bob");
        add(
            "aa00000000000000000000000000000000000001",
            "accounts/a1/chat.db",
            Some(&alice),
        );
        add(
            "aa00000000000000000000000000000000000002",
            "accounts/a2/chat.db",
            Some(&bob),
        );
        // TRAP 1: a sidecar beside a matched store.
        add(
            "aa00000000000000000000000000000000000003",
            "accounts/a1/chat.db-wal",
            None,
        );
        // TRAP 2: the directory row, which sorts before its own children.
        add(
            "aa00000000000000000000000000000000000004",
            "accounts/a1",
            None,
        );
        // A store one level deeper: `*` must not reach it.
        add(
            "aa00000000000000000000000000000000000005",
            "accounts/a3/sub/chat.db",
            Some(&alice),
        );
        drop(manifest);

        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(&spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();

        // BOTH accounts, and only those: not the sidecar, not the directory, not
        // the store a level deeper.
        assert_eq!(rows.len(), 2, "got {rows:#?}");
        let mut who: Vec<&str> = rows.iter().map(|r| r["Who"].as_str().unwrap()).collect();
        who.sort();
        assert_eq!(who, vec!["alice", "bob"]);

        // And each row says which store produced it — without that the two
        // accounts would be indistinguishable in one table.
        // The captured segment, not the whole path — `accounts/…/chat.db` is the
        // same on every row and says nothing.
        let mut stores: Vec<&str> = rows.iter().map(|r| r["Store"].as_str().unwrap()).collect();
        stores.sort();
        assert_eq!(stores, vec!["a1", "a2"]);
    }

    /// Segment matching, which is what keeps `*` from crossing `/`.
    #[test]
    fn a_pattern_captures_only_the_part_that_varied() {
        assert_eq!(
            path_captures("a/*/c.db", "a/b/c.db"),
            Some(vec!["b".to_string()])
        );
        // Several wildcards: all of them, in order.
        assert_eq!(
            path_captures("a/*/x/*.db", "a/one/x/two.db"),
            Some(vec!["one".to_string(), "two.db".to_string()])
        );
        assert_eq!(path_captures("a/b.db", "a/b.db"), Some(vec![]));
        assert_eq!(path_captures("a/*/c.db", "a/b/x/c.db"), None);
    }

    #[test]
    fn a_star_never_crosses_a_slash() {
        assert!(path_matches("a/*/c.db", "a/b/c.db"));
        assert!(path_matches("a/ig-*.db", "a/ig-12345.db"));
        assert!(
            path_matches("a/*/c.db", "a//c.db"),
            "an empty segment matches"
        );
        // The failure that removed globbing the first time: a pattern reaching
        // down a level it did not ask for.
        assert!(!path_matches("a/*/c.db", "a/b/x/c.db"));
        assert!(!path_matches("a/*", "a/b/c.db"));
        // And the directory row itself is a different path, so it cannot match a
        // pattern that names a file.
        assert!(!path_matches("a/*/c.db", "a/b"));
        assert!(!path_matches("a/ig-*.db", "a/ig-12345.db-wal"));
    }

    #[test]
    fn duplicate_id_error_names_both_files() {
        // It used to name the previous module's *display name*, which gives the
        // reader nothing to grep for.
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "aaa.toml", &spec_toml(""));
        write_module(tmp.path(), "zzz.toml", &spec_toml(""));
        let err = load_modules(tmp.path())
            .expect_err("duplicate ids must fail")
            .to_string();
        assert!(err.contains("aaa.toml"), "must name the first file: {err}");
        assert!(err.contains("zzz.toml"), "must name the second file: {err}");
    }

    /// The store is opened read-only, so a module cannot write even when its
    /// SQL slips past the `SELECT`-prefix check — SQLite accepts
    /// `WITH x AS (…) INSERT … RETURNING`, which starts with "with" and writes.
    #[test]
    fn a_writing_module_cannot_modify_the_extracted_store() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
            c.execute_batch("INSERT INTO events VALUES ('alice', 0);")
                .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(
            &mods_dir,
            "evil.toml",
            r#"
id = "evil"
description = "evil artifact."
surface = "standalone"
name = "Evil"
domain = "HomeDomain"
path = "Library/Demo/demo.db"
sql = "WITH q AS (SELECT 'x' AS v) INSERT INTO events(who) SELECT v FROM q RETURNING who"
[[columns]]
name = "Who"
from = "who"
"#,
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(spec, &index, None, &tmp.path().join("work"))
            .expect_err("a writing statement must be refused by the read-only connection");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("readonly")
                || msg.contains("read-only")
                || msg.contains("attempt to write"),
            "expected a read-only refusal, got: {err}"
        );
    }

    /// `run_module` is public and `ModuleSpec` deserializable, so a caller could
    /// bypass `load_modules`. The id builds a filename, so an unvalidated spec
    /// is a path-traversal hole.
    #[test]
    fn run_module_validates_a_hand_built_spec() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT);").unwrap();
        });
        let spec = ModuleSpec {
            id: "../../escape".into(),
            name: "Escape".into(),
            description: "A test artifact.".into(),
            surface: Surface::Standalone,
            join_column: None,
            highlight: None,
            category: None,
            domain: "HomeDomain".into(),
            path: "Library/Demo/demo.db".into(),
            shape: Shape::Table,
            verified: None,
            path_column: None,
            app_column: None,
            plist: None,
            log: None,
            sql: vec!["SELECT who FROM events".into()],
            requires: None,
            columns: vec![ColumnSpec {
                name: "Who".into(),
                from: vec!["who".into()],
                kind: ColumnKind::Text,
                value: None,
                epoch: None,
                map: None,
            }],
        };
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(&spec, &index, None, &tmp.path().join("work"))
            .expect_err("an id that escapes the work dir must be refused")
            .to_string();
        assert!(err.contains("may only contain"), "{err}");
    }

    /// Every `ColumnKind`, against every SQLite storage class it can meet.
    ///
    /// This is the module format's whole typing contract, and it was the least
    /// tested thing in the file: a review found `Bool`, `Integer` and `Real`
    /// could each be replaced with "always null" — or `Bool` with "always true"
    /// — without a single test noticing.
    #[test]
    fn every_column_kind_converts() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch(
                "CREATE TABLE t (s TEXT, i INTEGER, r REAL, b INTEGER, blob BLOB, n TEXT);
                 INSERT INTO t VALUES ('hi', 42, 1.5, 1, x'00ff', NULL);
                 INSERT INTO t VALUES ('bye', -7, -0.5, 0, x'01', NULL);",
            )
            .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(
            &mods_dir,
            "kinds.toml",
            r#"
id = "kinds"
description = "kinds artifact."
surface = "standalone"
name = "Kinds"
domain = "HomeDomain"
path = "Library/Demo/demo.db"
sql = "SELECT s, i, r, b, blob, n, i AS i_as_text FROM t ORDER BY rowid"
[[columns]]
name = "S"
from = "s"
[[columns]]
name = "I"
from = "i"
kind = "integer"
[[columns]]
name = "R"
from = "r"
kind = "real"
[[columns]]
name = "B"
from = "b"
kind = "bool"
[[columns]]
name = "Blob"
from = "blob"
[[columns]]
name = "N"
from = "n"
[[columns]]
name = "IAsText"
from = "i_as_text"
"#,
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 2);

        assert_eq!(rows[0]["S"], serde_json::json!("hi"));
        assert_eq!(rows[0]["I"], serde_json::json!(42));
        assert_eq!(rows[1]["I"], serde_json::json!(-7), "negatives survive");
        assert_eq!(rows[0]["R"], serde_json::json!(1.5));
        assert_eq!(rows[1]["R"], serde_json::json!(-0.5));
        // `true` and `false` must BOTH be produced — "always true" was a
        // surviving mutation.
        assert_eq!(rows[0]["B"], serde_json::json!(true));
        assert_eq!(rows[1]["B"], serde_json::json!(false));
        // A blob is not text; rendering its bytes as a string would be noise.
        assert_eq!(rows[0]["Blob"], serde_json::Value::Null);
        assert_eq!(rows[0]["N"], serde_json::Value::Null);
        // A number read as text is coerced rather than dropped, so a module
        // author who omits `kind` still gets the value.
        assert_eq!(rows[0]["IAsText"], serde_json::json!("42"));
    }

    /// `WITH … SELECT` is promised by both the code comment and the error
    /// string. Deleting that half of the check — rejecting every CTE module —
    /// used to pass.
    #[test]
    fn a_with_cte_module_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(
            tmp.path(),
            "cte.toml",
            r#"
id = "cte"
description = "cte artifact."
surface = "standalone"
name = "CTE"
domain = "HomeDomain"
path = "a/b.db"
sql = "WITH q AS (SELECT 1 AS a) SELECT a FROM q"
[[columns]]
name = "A"
from = "a"
kind = "integer"
"#,
        );
        assert_eq!(load_modules(tmp.path()).unwrap().len(), 1);
    }

    /// Only `*.toml` is a module. Without the filter a stray `README.md` or
    /// `.DS_Store` beside the modules becomes a hard load failure that takes
    /// every artifact down with it.
    #[test]
    fn non_toml_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "demo.toml", &spec_toml(""));
        std::fs::write(tmp.path().join("README.md"), "# not a module").unwrap();
        std::fs::write(tmp.path().join(".DS_Store"), [0u8, 1, 2]).unwrap();
        let mods = load_modules(tmp.path()).unwrap();
        assert_eq!(mods.len(), 1, "only the .toml file is a module");
    }

    /// Rows come back in `row_idx` order even when storage order disagrees.
    /// The previous assertion passed by accident: rows were inserted in order,
    /// so a scan happened to return them right, and deleting `ORDER BY` from
    /// `read_rows` changed nothing.
    #[test]
    fn rows_are_ordered_by_row_idx_not_insertion_order() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let conn = db.conn();
        // Drop the (artifact_id, row_idx) index first. With it in place SQLite
        // satisfies the WHERE clause from the index and hands back rows already
        // in row_idx order, so removing `ORDER BY` from `read_rows` changed
        // nothing and this test passed either way. Without the index a scan
        // returns rowid order, which is what makes the ordering contract real
        // rather than a coincidence of the current schema.
        conn.execute_batch("DROP INDEX IF EXISTS idx_artifact_rows")
            .unwrap();
        // Insert deliberately out of order, so storage order != row_idx order.
        for (idx, who) in [(2_i64, "third"), (0, "first"), (1, "second")] {
            conn.execute(
                "INSERT INTO artifact_rows (artifact_id, row_idx, payload) VALUES ('demo', ?1, ?2)",
                rusqlite::params![idx, format!(r#"{{"Who":"{who}"}}"#)],
            )
            .unwrap();
        }
        let back = read_rows(conn, "demo", 0, 10).unwrap();
        let names: Vec<&str> = back.iter().map(|r| r["Who"].as_str().unwrap()).collect();
        assert_eq!(names, ["first", "second", "third"], "row_idx decides order");
    }

    #[test]
    fn store_rows_reports_how_many_it_wrote() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let mut r: ArtifactRow = HashMap::new();
        r.insert("Who".into(), serde_json::json!("a"));
        assert_eq!(store_rows(db.conn(), "demo", &[r.clone(), r]).unwrap(), 2);
        assert_eq!(store_rows(db.conn(), "demo", &[]).unwrap(), 0);
    }

    /// A corrupt payload names the artifact rather than surfacing a bare serde
    /// error from nowhere.
    #[test]
    fn a_corrupt_stored_row_is_reported_with_its_artifact() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        db.conn()
            .execute(
                "INSERT INTO artifact_rows (artifact_id, row_idx, payload) VALUES ('demo', 0, 'not json')",
                [],
            )
            .unwrap();
        let err = read_rows(db.conn(), "demo", 0, 10)
            .expect_err("a corrupt payload must be an error")
            .to_string();
        assert!(err.contains("demo"), "{err}");
    }

    /// Invalid SQL must name the module. A raw rusqlite "no such table" says
    /// nothing about which of (eventually) hundreds of modules asked.
    #[test]
    fn invalid_sql_names_the_module() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT);").unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(
            &mods_dir,
            "broken.toml",
            r#"
id = "broken"
description = "broken artifact."
surface = "standalone"
name = "Broken Module"
domain = "HomeDomain"
path = "Library/Demo/demo.db"
sql = "SELECT nope FROM nosuchtable"
[[columns]]
name = "Nope"
from = "nope"
"#,
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(spec, &index, None, &tmp.path().join("work"))
            .expect_err("invalid SQL must fail")
            .to_string();
        assert!(err.contains("broken"), "must name the module id: {err}");
        assert!(err.contains("Broken Module"), "must name the module: {err}");
    }

    /// Two modules must not extract over each other's temp store.
    #[test]
    fn modules_extract_to_separate_temp_files() {
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), "Library/Demo/demo.db", |c| {
            c.execute_batch("CREATE TABLE events (who TEXT, at REAL);")
                .unwrap();
            c.execute_batch("INSERT INTO events VALUES ('alice', 0);")
                .unwrap();
        });
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(&mods_dir, "one.toml", &spec_toml(""));
        write_module(
            &mods_dir,
            "two.toml",
            &spec_toml("").replace("id = \"demo\"", "id = \"demo2\""),
        );
        let mods = load_modules(&mods_dir).unwrap();
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let work = tmp.path().join("work");
        for m in &mods {
            run_module(m, &index, None, &work).unwrap().unwrap();
        }
        let files: Vec<String> = std::fs::read_dir(&work)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".sqlite"))
            .collect();
        assert_eq!(
            files.len(),
            2,
            "each module gets its own temp store: {files:?}"
        );
    }

    /// TCC.db as a modern device writes it, matching `seed_tcc_db` in
    /// tools/make_fixture_backup.py.
    fn seed_tcc(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE access (
                service TEXT, client TEXT, client_type INTEGER,
                auth_value INTEGER, auth_reason INTEGER, auth_version INTEGER,
                last_modified INTEGER);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO access (service, client, client_type, auth_value, auth_reason, auth_version, last_modified) VALUES
                ('kTCCServiceCamera','com.example.chatapp',0,2,2,«redacted»0000000),
                ('kTCCServiceLocation','com.example.weather',0,0,2,«redacted»0000300),
                ('kTCCServicePhotos','com.example.chatapp',0,3,2,«redacted»0000200),
                ('kTCCServiceReminders','com.example.todo',0,9,2,«redacted»0000500),
                ('kTCCServiceContacts','com.example.notprompted',0,1,2,«redacted»0000600),
                ('kTCCServiceMicrophone','com.example.norecord',0,NULL,2,«redacted»0000700);",
        )
        .unwrap();
    }

    /// Accounts3.sqlite as accountsd writes it — the Core Data column names and
    /// the two joined tables, matching what `explore_real_backup` printed for
    /// Josh Hickman's iOS 17 image.
    ///
    /// The rows deliberately include the cases the module claims to handle: an
    /// account whose type row is MISSING (so the LEFT JOIN is what keeps it —
    /// an inner join drops it and the count silently falls), a NULL username, and
    /// a NULL `ZACTIVE`.
    fn seed_accounts(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE ZACCOUNT (
                Z_PK INTEGER PRIMARY KEY, ZACTIVE INTEGER, ZAUTHENTICATED INTEGER,
                ZACCOUNTTYPE INTEGER, ZPARENTACCOUNT INTEGER, ZDATE TIMESTAMP,
                ZACCOUNTDESCRIPTION VARCHAR, ZIDENTIFIER VARCHAR,
                ZOWNINGBUNDLEID VARCHAR, ZUSERNAME VARCHAR);
             CREATE TABLE ZACCOUNTTYPE (
                Z_PK INTEGER PRIMARY KEY, ZACCOUNTTYPEDESCRIPTION VARCHAR,
                ZIDENTIFIER VARCHAR, ZOWNINGBUNDLEID VARCHAR);",
        )
        .unwrap();
        // 726000000 Cocoa seconds = 2024-01-03T…Z, comfortably inside iOS 17.
        // ZIDENTIFIER values are GUIDs, as on a real device — the point of the
        // module's three-rung COALESCE is that this column is NOT a service name,
        // and a fixture that put a friendly string here would hide that.
        c.execute_batch(
            "INSERT INTO ZACCOUNTTYPE (Z_PK, ZACCOUNTTYPEDESCRIPTION, ZIDENTIFIER) VALUES
                (1,'Gmail','com.apple.account.Google'),
                (2,'Holiday Calendar','com.apple.account.HolidayCalendar'),
                -- No description: the middle COALESCE rung must fall to the type's
                -- own reverse-DNS identifier, not to the account GUID.
                (3,NULL,'com.apple.account.undescribed');
             INSERT INTO ZACCOUNT
                (Z_PK, ZACTIVE, ZAUTHENTICATED, ZACCOUNTTYPE, ZPARENTACCOUNT, ZDATE,
                 ZACCOUNTDESCRIPTION, ZIDENTIFIER, ZOWNINGBUNDLEID, ZUSERNAME) VALUES
                (1,1,1,1,NULL,726000000,'Gmail',
                 '6D60660E-344F-4E62-97A0-0A9EA8174CDE','com.apple.mobilemail','person@example.com'),
                (2,1,1,2,NULL,725000000,'US Holidays',
                 'AD041785-D028-495F-9008-62F26C114CBA','dataaccessd',NULL),
                -- No ZACCOUNTTYPE row: only the LEFT JOIN keeps this one, and the
                -- last COALESCE rung must name it rather than print its GUID.
                (3,0,0,NULL,NULL,724000000,NULL,
                 'B61380AE-7269-4769-A39F-69D7935848EA','appstored','local'),
                (4,NULL,NULL,1,NULL,723000000,'Unrecorded',
                 'C9FA6B49-5667-4CE7-A88A-60C0543E82B5','accountsd',NULL),
                -- A CHILD of account 1: on real data these are what make one
                -- sign-in look like several duplicate rows.
                (5,1,1,3,«redacted»000000,NULL,
                 '0EE306D8-66AF-47E5-8FD1-CF2EAF5DC8C2','accountsd',NULL);",
        )
        .unwrap();
    }

    /// The paired-LE store as bluetoothd writes it, including Apple's own
    /// `Public `/`Random ` prefix inside the address strings and the two
    /// device-relative counters that are NOT dates.
    fn seed_bluetooth_paired(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE PairedDevices(Uuid TEXT, Name TEXT, NameOrigin INT,
                Address TEXT, ResolvedAddress TEXT, LastSeenTime INT,
                LastConnectionTime INT, GATTServiceChangeConfig INT, Tags TEXT,
                iCloudIdentifier TEXT);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO PairedDevices
                (Uuid, Name, NameOrigin, Address, ResolvedAddress, LastSeenTime,
                 LastConnectionTime, iCloudIdentifier) VALUES
                ('E3B37CA8-1AA5-AD44-B0FE-A617BB09B64A','Fitness Band',2,
                 'Public B4:C2:6A:7F:D3:7A','Public B4:C2:6A:7F:D3:7A',395626,2143,''),
                ('6C0C35A0-84CE-3572-2E72-4CF3D03BD1AF','Example Watch',2,
                 'Random 50:32:66:45:35:EF','Public F8:6F:C1:4E:FF:6A',4315986,9639,''),
                ('C4E4E254-6060-26CA-7C80-EE01F3C5C346','Nameless Tag',2,
                 'Random E8:F0:58:00:C0:FB',NULL,748458,3662,NULL);",
        )
        .unwrap();
    }

    /// DataUsage.sqlite as the modern lineage writes it, with both Wi-Fi and WWAN
    /// columns — matching what `explore_real_backup` printed for the validation
    /// image.
    ///
    /// The rows cover the cases the module claims to handle: several buckets for
    /// one app (so the aggregation is exercised rather than assumed), a compound
    /// `daemon/bundle` process name, a bare process name with no slash, and the
    /// ROLLUP row with a NULL bundle id whose total is the sum of everything else
    /// — the one the module must exclude, and which iLEAPP's `ZKIND != 257`
    /// constant would not catch here (this device's rollup is ZKIND 255).
    /// `CloudDocs/session/db/client.db` — the iCloud Drive index.
    ///
    /// A THREE-LEVEL folder chain, because the module builds its path with a
    /// recursive CTE and a one-level fixture would let a broken recursion pass.
    /// Parent ids are 16-byte blobs and roots are not, which is the terminating
    /// condition the query relies on — so the root here is deliberately short.
    fn seed_icloud_drive(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE client_items (
                item_id BLOB NOT NULL, item_parent_id BLOB NOT NULL,
                item_filename TEXT, item_type INTEGER NOT NULL,
                item_birthtime INTEGER NOT NULL, version_mtime INTEGER,
                item_lastusedtime INTEGER, version_size INTEGER,
                item_hidden_ext INTEGER NOT NULL DEFAULT 0,
                item_sharing_options INTEGER NOT NULL DEFAULT 0,
                item_user_visible INTEGER, item_trash_put_back_path BLOB,
                app_library_rowid INTEGER);
             CREATE TABLE app_libraries (
                rowid INTEGER PRIMARY KEY, app_library_name TEXT,
                auto_client_item_count INTEGER, auto_document_count INTEGER,
                auto_document_with_local_changes_count INTEGER,
                auto_aggregate_size INTEGER);
             CREATE TABLE boot_history (
                date INTEGER, os TEXT, br TEXT, bird_schema INTEGER,
                db_schema INTEGER, device_id INTEGER);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO app_libraries VALUES
                (1, 'com.apple.CloudDocs', 25, 20, 0, 22453826),
                (2, 'iCloud.com.apple.MobileSMS', 1, 0, 0, 0);
             -- Two boots under different builds, and a NULL device_id on the
             -- first: the daemon only learns its id later, and the module shows
             -- that rather than hiding it.
             INSERT INTO boot_history VALUES
                (1688242985, '20B110', '1177.42.1', 21004, 21004, NULL),
                (1706205831, '21D50', '2461.80.8', 30016, 30016, 4829738);
             -- root: a parent id that is NOT 16 bytes, which ends the walk.
             INSERT INTO client_items VALUES
                (X'1111111111111111', X'00', 'Documents', 1, 1684594000, NULL,
                 NULL, NULL, 0, 0, 1, NULL, 1);
             INSERT INTO client_items VALUES
                (X'2222222222222222', X'1111111111111111', 'Reports', 1,
                 1684594100, NULL, NULL, NULL, 0, 0, 1, NULL, 1);
             -- the file, two folders deep; shared (>4) and once deleted.
             INSERT INTO client_items VALUES
                (X'3333333333333333', X'2222222222222222', 'q3.pdf', 0,
                 1684594624, 1684594700, 1684594708, 1462115, 0, 5, 1,
                 X'AA', 1);
             -- a second file at the root, to prove an empty path is possible.
             INSERT INTO client_items VALUES
                (X'4444444444444444', X'00', 'loose.txt', 0,
                 1621718855, 1621718900, NULL, 1295, 0, 0, 1, NULL, 1);",
        )
        .unwrap();
    }

    /// `CloudDocs/session/db/server.db` — the account's view, not this device's.
    fn seed_icloud_server(c: &Connection) {
        c.execute_batch("CREATE TABLE devices (key INTEGER PRIMARY KEY, name TEXT);")
            .unwrap();
        // A Mac among them, because the point of the artifact is the machines
        // that are NOT the phone being examined.
        c.execute_batch(
            "INSERT INTO devices VALUES (1, 'iPhone'), (2, 'A Mac'), (3, 'Another iPhone');",
        )
        .unwrap();
    }

    /// Chromium's `Login Data`. An EXTENSIONLESS SQLite file, which is why an
    /// audit that enumerated `%.db`/`%.sqlite` could not see it.
    ///
    /// Includes a `blacklisted_by_user` row -- a site the person told the
    /// browser never to save -- because that is a different fact from an
    /// account and the module labels it rather than dropping it.
    fn seed_chromium_logins(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE logins (origin_url VARCHAR NOT NULL, signon_realm VARCHAR NOT NULL,
                username_value VARCHAR, password_value BLOB, date_created INTEGER NOT NULL,
                date_last_used INTEGER NOT NULL DEFAULT 0, times_used INTEGER,
                blacklisted_by_user INTEGER NOT NULL, keychain_identifier BLOB);",
        )
        .unwrap();
        // WebKit/Chrome epoch: microseconds since 1601.
        c.execute_batch(
            "INSERT INTO logins VALUES
                ('https://example.com/login', 'https://example.com/', 'someone@example.com',
                 x'DEADBEEF', 13350000000000000, 13360000000000000, 4, 0, x'01'),
                ('https://never.example/', 'https://never.example/', NULL,
                 NULL, 13350000000000000, 0, 0, 1, NULL);",
        )
        .unwrap();
    }

    /// Chromium's `Top Sites`, also extensionless.
    fn seed_chromium_top_sites(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE top_sites (url TEXT NOT NULL PRIMARY KEY,
                url_rank INTEGER NOT NULL, title TEXT NOT NULL);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO top_sites VALUES
                ('https://www.nhl.com/', 0, 'Official Site of the National Hockey League');",
        )
        .unwrap();
    }

    /// WebKit's service-worker registry.
    fn seed_service_workers(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE Records (key TEXT NOT NULL UNIQUE, origin TEXT NOT NULL,
                scopeURL TEXT NOT NULL, topOrigin TEXT NOT NULL,
                lastUpdateCheckTime DOUBLE NOT NULL, updateViaCache TEXT NOT NULL,
                scriptURL TEXT NOT NULL, workerType TEXT NOT NULL);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO Records VALUES
                ('k1', 'https://www.nhl.com', '/', 'https://www.nhl.com',
                 1720367276.884735, 'imports', 'https://www.nhl.com/serviceWorker.js',
                 'classic');",
        )
        .unwrap();
    }

    /// MEGA's decrypted state cache.
    ///
    /// A three-level tree under a ROOT NODE whose name is the literal
    /// `CRYPTO_ERROR` — which is what the real store holds, because MEGA's
    /// roots have no encrypted name attribute. The module names them from
    /// `type` instead, and a fixture without that literal would let it get away
    /// with passing the raw value through.
    fn seed_mega(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE nodes (nodehandle INTEGER PRIMARY KEY, parenthandle INTEGER,
                name TEXT, type INTEGER, size INTEGER, share INTEGER, fav INTEGER,
                ctime INTEGER, mtime INTEGER);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO nodes VALUES
                -- the Cloud Drive root: type 2, and no readable name.
                (1, 0, 'CRYPTO_ERROR', 2, NULL, 0, 0, 1600000000, 1600000000),
                (2, 1, 'My chat files', 1, NULL, 0, 0, 1650000000, 1650000000),
                (3, 2, 'IMG_4552.jpg', 0, 170856, 0, 0, 1714243988, 1682368329),
                -- shared and favourited, so both flags are exercised.
                (4, 2, 'notes.pdf', 0, 2048, 1, 1, 1714243000, 1714243000),
                -- the Rubbish Bin root and a file in it.
                (5, 0, 'CRYPTO_ERROR', 4, NULL, 0, 0, 1600000000, 1600000000),
                (6, 5, 'deleted.txt', 0, 12, 0, 0, 1700000000, 1700000000);",
        )
        .unwrap();
    }

    /// WebKit's ITP store, `observations.db`.
    ///
    /// `mostRecentUserInteractionTime` is -1, not NULL, when a domain was never
    /// touched — the fixture carries one of each so a module that forgets to
    /// NULLIF it reports December 1969 and is caught.
    fn seed_observations(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE ObservedDomains (
                domainID INTEGER PRIMARY KEY, registrableDomain TEXT NOT NULL,
                lastSeen REAL NOT NULL, hadUserInteraction INTEGER NOT NULL,
                mostRecentUserInteractionTime REAL NOT NULL,
                firstSeen REAL, isPrevalentResource INTEGER);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO ObservedDomains VALUES
                (1, 'digitalcorpora.org', 1705006820, 1, 1705006820, 1704998000, 0),
                (2, 'gstatic.com', 1704998430, 0, -1, 1704998430, 1);",
        )
        .unwrap();
    }

    /// Waze's `Documents/user.db`.
    ///
    /// COORDINATES ARE MICRODEGREES here, as the app writes them — `35589221`
    /// for 35.589221. A fixture holding real degrees would let the module drop
    /// the division and still look right.
    ///
    /// One place carries `created_time = 0`, which Waze writes for a place it
    /// never dated. The module NULLIFs it; without a zero in the fixture that
    /// could regress into 1 January 1970.
    fn seed_waze(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE PLACES (id INTEGER PRIMARY KEY, name TEXT, street TEXT,
                city TEXT, state TEXT, country TEXT, house TEXT,
                longitude INTEGER, latitude INTEGER, venue_id TEXT,
                created_time INTEGER, routing_context TEXT,
                is_residential INTEGER DEFAULT NULL);
             CREATE TABLE RECENTS (id INTEGER PRIMARY KEY, place_id INTEGER,
                name TEXT, created_time INTEGER, access_time INTEGER,
                type INTEGER DEFAULT 0, string_context TEXT, image_id TEXT,
                waypoint_access_time INTEGER);
             CREATE TABLE FAVORITES (id INTEGER PRIMARY KEY, place_id INTEGER,
                name TEXT, created_time INTEGER, modified_time INTEGER,
                rank INTEGER, type INTEGER DEFAULT 0, server_id INTEGER,
                access_time INTEGER, waypoint_access_time INTEGER);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO PLACES VALUES
                (1, 'Starbucks', 'N Main St', 'Fuquay-Varina', 'North Carolina',
                 'US', '110', -78775217, 35591915, 'VENUE1', 1705430233, NULL, 0),
                (2, NULL, 'Bridge St', 'Fuquay-Varina', 'NC', 'US', NULL,
                 -78808746, 35592426, NULL, 0, NULL, 1);
             INSERT INTO RECENTS VALUES
                (1, 1, 'Starbucks', 1705430233, 1721818400, 0, NULL, NULL, 0),
                (2, 2, NULL, 1705426741, 1705426741, 0, NULL, NULL, 0);
             INSERT INTO FAVORITES VALUES
                (1, 2, 'Home', 1705426741, 1705426800, 0, 1, -1, 1721818316, 0);",
        )
        .unwrap();
    }

    fn seed_data_usage(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE ZLIVEUSAGE (
                Z_PK INTEGER PRIMARY KEY, ZKIND INTEGER, ZHASPROCESS INTEGER,
                ZTIMESTAMP TIMESTAMP, ZWIFIIN FLOAT, ZWIFIOUT FLOAT,
                ZWWANIN FLOAT, ZWWANOUT FLOAT);
             CREATE TABLE ZPROCESS (
                Z_PK INTEGER PRIMARY KEY, ZFIRSTTIMESTAMP TIMESTAMP,
                ZTIMESTAMP TIMESTAMP, ZBUNDLENAME VARCHAR, ZPROCNAME VARCHAR,
                ZEXTENSIONNAME VARCHAR);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO ZPROCESS (Z_PK, ZBUNDLENAME, ZPROCNAME) VALUES
                (1,'com.example.chatapp','ChatApp/com.example.chatapp'),
                (2,'com.example.photos','nsurlsessiond/com.example.photos'),
                -- No slash: the substr must not eat the whole name.
                (3,'com.example.plain','plainproc'),
                -- The ROLLUP: no bundle id, and a total that is the sum of the
                -- rest. If it were ever included, every figure would double.
                (4,NULL,'CumulativeUsageTracker');
             INSERT INTO ZLIVEUSAGE
                (Z_PK, ZKIND, ZHASPROCESS, ZTIMESTAMP, ZWIFIIN, ZWIFIOUT, ZWWANIN, ZWWANOUT) VALUES
                (1,0,«redacted»000000,1000,2000,3000,4000),
                (2,0,«redacted»001000,«redacted»),
                (3,0,«redacted»000000,0,«redacted»000,10000),
                (4,0,«redacted»000000,10,20,30,40),
                («redacted»,«redacted»002000,1510,2620,903730,14840);",
        )
        .unwrap();
    }

    /// CellularUsage.db's `subscriber_info`, with the column names the schema
    /// really uses — `subscriber_id` for the ICCID and `subscriber_mdn` for the
    /// number, which is precisely the pair a fixture using friendly names would
    /// stop the module from getting wrong.
    ///
    /// Two SIMs, so the slot ordering is exercised, and the mostly-NULL tail of
    /// the table is present so the module cannot accidentally depend on it.
    fn seed_sim_cards(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE subscriber_info (
                ROWID INTEGER PRIMARY KEY AUTOINCREMENT, subscriber_id TEXT,
                subscriber_mdn TEXT, tag INTEGER, last_update_time INTEGER,
                slot_id INTEGER, home_budget INTEGER, roaming_budget INTEGER,
                user_entered_bill_end_dom INTEGER, low_data_mode INTEGER,
                reliable_network_fallback INTEGER, smart_data_mode INTEGER,
                interface_cost INTEGER, privacy_proxy INTEGER);
             CREATE TABLE bundle_info (
                ROWID INTEGER PRIMARY KEY AUTOINCREMENT, bundle_id TEXT, flags INTEGER);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO subscriber_info
                (subscriber_id, subscriber_mdn, tag, last_update_time, slot_id) VALUES
                ('8901260971148676693','+15550100',«redacted»000000,1),
                ('8944500000000000001','+15550199',«redacted»000000,2);
             -- Present but deliberately unread: an opaque flag with one value.
             INSERT INTO bundle_info (bundle_id, flags) VALUES ('com.example.watchapp',48);",
        )
        .unwrap();
    }

    /// `com.apple.wifi.known-networks.plist` as iOS writes it: a ROOT DICTIONARY
    /// whose keys name the networks, values holding the rest, and the access point
    /// nested under `__OSSpecific__`.
    ///
    /// The cases the module claims to handle: a key that does NOT carry Apple's
    /// `wifi.network.ssid.` prefix (so blind trimming would be caught), a network
    /// missing the whole `__OSSpecific__` subtree, and a network joined
    /// automatically rather than by the user.
    ///
    /// The `SSID` Data field is present because the real store has it, but NO
    /// column reads it — the key is the better source, and the module says why.
    /// `convert_plist`'s Data arm is covered by a unit test instead, so this
    /// docstring does not claim coverage that does not exist.
    fn seed_wifi_networks() -> Vec<u8> {
        use plist::{Date, Dictionary, Value};

        fn at(secs: i64) -> Value {
            Value::Date(Date::from(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64),
            ))
        }

        let mut root = Dictionary::new();

        let mut home = Dictionary::new();
        home.insert("SSID".into(), Value::Data(b"HomeNet".to_vec()));
        home.insert(
            "SupportedSecurityTypes".into(),
            Value::String("WPA2 Personal".into()),
        );
        home.insert("Hidden".into(), Value::Boolean(false));
        home.insert("JoinedByUserAt".into(), at(1_688_243_921));
        home.insert("JoinedBySystemAt".into(), at(1_689_450_000));
        home.insert("AddedAt".into(), at(1_688_243_920));
        home.insert("LastDiscoveredAt".into(), at(1_689_450_218));
        let mut os = Dictionary::new();
        os.insert("BSSID".into(), Value::String("6a:22:32:98:f4:df".into()));
        os.insert("CHANNEL".into(), Value::Integer(153.into()));
        home.insert("__OSSpecific__".into(), Value::Dictionary(os));
        root.insert("wifi.network.ssid.HomeNet".into(), Value::Dictionary(home));

        // No `__OSSpecific__` at all: the access point and channel must come back
        // null rather than failing the whole artifact.
        let mut cafe = Dictionary::new();
        cafe.insert("SSID".into(), Value::Data(b"Cafe Wifi".to_vec()));
        cafe.insert(
            "SupportedSecurityTypes".into(),
            Value::String("None".into()),
        );
        cafe.insert("Hidden".into(), Value::Boolean(true));
        cafe.insert("AddedAt".into(), at(1_700_000_000));
        root.insert(
            "wifi.network.ssid.Cafe Wifi".into(),
            Value::Dictionary(cafe),
        );

        // A key WITHOUT the namespace: `key_strip_prefix` must leave it alone
        // rather than trim something else off the front.
        let mut odd = Dictionary::new();
        odd.insert("SSID".into(), Value::Data(vec![0xff, 0xfe, 0x00, 0x41]));
        odd.insert("AddedAt".into(), at(1_710_000_000));
        root.insert("legacy-entry".into(), Value::Dictionary(odd));

        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `com.apple.MobileBluetooth.devices.plist`: entries keyed by MAC address,
    /// with the three different names the store keeps.
    ///
    /// One device deliberately has a `UserNameKey` naming someone OTHER than the
    /// owner, because that disagreement is the reason all three names are kept —
    /// and one has no user name at all, so the column must be null rather than
    /// falling back to the model.
    fn seed_bluetooth_devices() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();

        let mut a = Dictionary::new();
        a.insert("UserNameKey".into(), Value::String("Alex's AirPods".into()));
        a.insert("Name".into(), Value::String("AirPods 3".into()));
        a.insert("DefaultName".into(), Value::String("Headphones".into()));
        // Radio state, deliberately unread.
        a.insert("LastAVCTPVersion".into(), Value::Data(vec![0x01, 0x04]));
        root.insert("08:65:18:75:5E:75".into(), Value::Dictionary(a));

        // A second person's device on this phone.
        let mut b = Dictionary::new();
        b.insert("UserNameKey".into(), Value::String("Sam's AirPods".into()));
        b.insert("Name".into(), Value::String("AirPods".into()));
        b.insert("DefaultName".into(), Value::String("Headphones".into()));
        root.insert("7C:04:D0:89:89:A0".into(), Value::Dictionary(b));

        // Never renamed: no UserNameKey at all.
        let mut c = Dictionary::new();
        c.insert("Name".into(), Value::String("Apple Watch".into()));
        c.insert("DefaultName".into(), Value::String("Watch".into()));
        root.insert("F8:6F:C1:4E:FF:6A".into(), Value::Dictionary(c));

        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// The private-MAC store: rows in an ARRAY under a key with spaces, the
    /// address six raw bytes nested a level down.
    ///
    /// One network's address is marked INVALID, because bytes that are present but
    /// not in use are not an address this phone presented — and showing them
    /// without the flag would say it did.
    fn seed_wifi_private_mac() -> Vec<u8> {
        use plist::{Date, Dictionary, Value};
        fn at(secs: u64) -> Value {
            Value::Date(Date::from(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            ))
        }
        fn mac(valid: bool, bytes: Vec<u8>) -> Value {
            let mut d = Dictionary::new();
            d.insert("PRIVATE_MAC_ADDRESS_VALID".into(), Value::Boolean(valid));
            d.insert("PRIVATE_MAC_ADDRESS_VALUE".into(), Value::Data(bytes));
            Value::Dictionary(d)
        }

        let mut first = Dictionary::new();
        first.insert("SSID_STR".into(), Value::String("HomeNet".into()));
        first.insert("BSSID".into(), Value::String("6a:22:32:98:f4:df".into()));
        first.insert("IsOpenNetwork".into(), Value::Boolean(false));
        first.insert("PresentInKnownNetworks".into(), Value::Boolean(true));
        first.insert("lastJoined".into(), at(1_689_450_273));
        first.insert("MacGenerationTimeStamp".into(), at(1_700_312_363));
        first.insert(
            "PRIVATE_MAC_ADDRESS".into(),
            mac(true, vec![0x8a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f]),
        );

        let mut second = Dictionary::new();
        second.insert("SSID_STR".into(), Value::String("Cafe Wifi".into()));
        second.insert("IsOpenNetwork".into(), Value::Boolean(true));
        second.insert("PresentInKnownNetworks".into(), Value::Boolean(false));
        second.insert("lastJoined".into(), at(1_700_000_000));
        second.insert("MacGenerationTimeStamp".into(), at(1_699_000_000));
        // Present but NOT in use.
        second.insert(
            "PRIVATE_MAC_ADDRESS".into(),
            mac(false, vec![0x00, 0x11, 0x22, 0x33, 0x44, 0x55]),
        );

        let mut root = Dictionary::new();
        root.insert(
            "List of scanned networks with private mac".into(),
            Value::Array(vec![Value::Dictionary(first), Value::Dictionary(second)]),
        );
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `…ledevices.other.db` — devices seen but never paired. Same schema as the
    /// paired store.
    ///
    /// Two named, two anonymous, so the ordering rule (named first, nothing
    /// dropped) is exercised rather than asserted. `ResolvedAddress` is left NULL
    /// throughout because that is what the real store holds — an unpaired sighting
    /// cannot be resolved — which is why no column reads it.
    fn seed_bluetooth_nearby(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE OtherDevices(Uuid TEXT, Name TEXT, NameOrigin INT,
                Address TEXT, ResolvedAddress TEXT, LastSeenTime INT,
                LastConnectionTime INT, GATTServiceChangeConfig INT, Tags TEXT,
                iCloudIdentifier TEXT);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO OtherDevices (Uuid, Name, Address, ResolvedAddress, LastSeenTime) VALUES
                ('11111111-0000-0000-0000-000000000001',NULL,'Random AA:BB:CC:DD:EE:01',NULL,4000000),
                ('11111111-0000-0000-0000-000000000002','Garage Opener','Public CC:6A:10:54:65:FF',NULL,4352299),
                ('11111111-0000-0000-0000-000000000003','','Random AA:BB:CC:DD:EE:03',NULL,4100000),
                ('11111111-0000-0000-0000-000000000004','Fitness Band','Random ED:FD:03:AC:36:76',NULL,4337974);",
        )
        .unwrap();
    }

    /// `.GlobalPreferences.plist` — a SINGLE-RECORD plist: the root dictionary is
    /// the row. Includes an array-valued setting, so indexing into one is
    /// exercised, and internal counters the module deliberately does not read.
    fn seed_device_locale() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();
        root.insert(
            "AppleLanguages".into(),
            Value::Array(vec![
                Value::String("en-US".into()),
                // A second preference exists and is deliberately not shown.
                Value::String("sv-SE".into()),
            ]),
        );
        root.insert("AppleLocale".into(), Value::String("en_US".into()));
        root.insert("AKLastLocale".into(), Value::String("en_US".into()));
        root.insert("AppleICUForce24HourTime".into(), Value::Boolean(true));
        root.insert(
            "ApplePasscodeKeyboards".into(),
            Value::Array(vec![Value::String("en_US@sw=QWERTY;hw=Automatic".into())]),
        );
        // Internal, unread.
        root.insert("PKKeychainVersionKey".into(), Value::Integer(8.into()));
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `com.apple.mobiletimerd.plist` — BOTH collections, because the real file has
    /// both and two modules read it.
    ///
    /// Each element is wrapped in Apple's `$MTAlarm` class marker, which the
    /// modules step over; a fixture without the wrapper would let a module drop
    /// that path segment and still pass.
    fn seed_clock() -> Vec<u8> {
        use plist::{Date, Dictionary, Value};
        fn at(secs: u64) -> Value {
            Value::Date(Date::from(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            ))
        }
        fn wrap(d: Dictionary) -> Value {
            let mut outer = Dictionary::new();
            outer.insert("$MTAlarm".into(), Value::Dictionary(d));
            Value::Dictionary(outer)
        }

        let mut alarm = Dictionary::new();
        alarm.insert("MTAlarmHour".into(), Value::Integer(10.into()));
        alarm.insert("MTAlarmMinute".into(), Value::Integer(41.into()));
        alarm.insert("MTAlarmEnabled".into(), Value::Boolean(false));
        alarm.insert("MTAlarmAllowsSnooze".into(), Value::Boolean(true));
        alarm.insert("MTAlarmLastModifiedDate".into(), at(1_722_177_663));
        alarm.insert("MTAlarmDismissDate".into(), at(1_722_177_663));
        alarm.insert(
            "MTAlarmID".into(),
            Value::String("4ABC24C8-A16E-440D-A56D-0F7C2D46825E".into()),
        );
        // Undocumented and deliberately unread.
        alarm.insert("MTAlarmRepeatSchedule".into(), Value::Integer(0.into()));

        let mut sleep = Dictionary::new();
        sleep.insert("MTAlarmHour".into(), Value::Integer(6.into()));
        sleep.insert("MTAlarmMinute".into(), Value::Integer(0.into()));
        sleep.insert("MTAlarmBedtimeHour".into(), Value::Integer(22.into()));
        sleep.insert("MTAlarmBedtimeMinute".into(), Value::Integer(45.into()));
        sleep.insert("MTAlarmEnabled".into(), Value::Boolean(false));
        sleep.insert("MTAlarmSleepTrackingKey".into(), Value::Boolean(true));
        sleep.insert("MTAlarmKeepOffUntilDate".into(), at(1_689_849_000));
        sleep.insert("MTAlarmLastModifiedDate".into(), at(1_722_076_501));

        // TWO timers, because the fire time is POLYMORPHIC and a real device
        // settled which shape is ordinary. `MTTimerFireTime` holds either
        // `$MTTimerDate` (running, due at a moment) or `$MTTimerTimeInterval`
        // (stored, not scheduled), with `MTTimerFireTimerClass` naming which.
        //
        // The first version of this fixture had only the DATE shape, because
        // that is what iLEAPP's code reaches for. The iPhone 11 / iOS 17.3
        // image has only the INTERVAL shape — so the fixture agreed with the
        // module about something the device does not do, which is exactly the
        // failure mode `ModuleSpec::verified` exists to name.
        fn tone(id: &str) -> Value {
            let mut t = Dictionary::new();
            t.insert("MTSoundToneID".into(), Value::String(id.into()));
            let mut sound = Dictionary::new();
            sound.insert("$MTSound".into(), Value::Dictionary(t));
            Value::Dictionary(sound)
        }
        fn timer(title: &str, secs: i64, fire: Value, class: &str, id: &str) -> Value {
            let mut t = Dictionary::new();
            t.insert("MTTimerTitle".into(), Value::String(title.into()));
            t.insert("MTTimerDuration".into(), Value::Integer(secs.into()));
            t.insert("MTTimerState".into(), Value::Integer(1.into()));
            t.insert("MTTimerFireTime".into(), fire);
            t.insert("MTTimerFireTimerClass".into(), Value::String(class.into()));
            t.insert("MTTimerSound".into(), tone("system:Radial"));
            t.insert("MTTimerID".into(), Value::String(id.into()));
            let mut w = Dictionary::new();
            w.insert("$MTTimer".into(), Value::Dictionary(t));
            Value::Dictionary(w)
        }

        // Running: due at an absolute moment.
        let mut fire_date = Dictionary::new();
        fire_date.insert("MTTimerTimeDate".into(), at(1_722_180_000));
        let mut running = Dictionary::new();
        running.insert("$MTTimerDate".into(), Value::Dictionary(fire_date));

        // Stored: an interval, no moment. What the real image actually has.
        let mut interval_inner = Dictionary::new();
        interval_inner.insert("MTTimerTimeInterval".into(), Value::Real(900.0));
        let mut stored = Dictionary::new();
        stored.insert(
            "$MTTimerTimeInterval".into(),
            Value::Dictionary(interval_inner),
        );

        let mut timers_inner = Dictionary::new();
        timers_inner.insert(
            "MTTimers".into(),
            Value::Array(vec![
                timer(
                    "Pasta",
                    600,
                    Value::Dictionary(running),
                    "MTTimerDate",
                    "1D8B30D8-DF6F-4644-B7E3-534F4E26CB86",
                ),
                timer(
                    "CURRENT_TIMER",
                    900,
                    Value::Dictionary(stored),
                    "MTTimerTimeInterval",
                    "2E9C41E9-EF70-5755-C8F4-645F5F37DC97",
                ),
            ]),
        );

        let mut inner = Dictionary::new();
        inner.insert("MTAlarms".into(), Value::Array(vec![wrap(alarm)]));
        inner.insert("MTSleepAlarms".into(), Value::Array(vec![wrap(sleep)]));
        let mut root = Dictionary::new();
        root.insert("MTAlarms".into(), Value::Dictionary(inner));
        root.insert("MTTimers".into(), Value::Dictionary(timers_inner));

        // A stopwatch. `MTStopwatchLaps` is an array of bare numbers, which is
        // exactly why `stopwatch.toml` cannot report laps -- it is in the
        // fixture so that limitation is visible rather than theoretical.
        let mut sw = Dictionary::new();
        sw.insert("MTStopwatchState".into(), Value::Integer(2.into()));
        sw.insert("MTStopwatchCurrentInterval".into(), Value::Real(93.5));
        sw.insert(
            "MTStopwatchLaps".into(),
            Value::Array(vec![Value::Real(31.2), Value::Real(28.9)]),
        );
        let mut sw_wrapped = Dictionary::new();
        sw_wrapped.insert("$MTStopwatch".into(), Value::Dictionary(sw));
        let mut sw_inner = Dictionary::new();
        sw_inner.insert(
            "MTStopwatches".into(),
            Value::Array(vec![Value::Dictionary(sw_wrapped)]),
        );
        root.insert("MTStopwatches".into(), Value::Dictionary(sw_inner));
        // Other keys in the real file, none of them read.
        root.insert("MTTimerDefaultDuration".into(), Value::Real(900.0));

        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `com.apple.commcenter.plist` — cellular identity, keyed by SIM.
    ///
    /// TWO SIMs, because the middle key is the SIM's own identifier and a
    /// single-SIM fixture would let a module hard-code one and still pass.
    fn seed_commcenter() -> Vec<u8> {
        use plist::{Dictionary, Value};
        fn sim(imei: &str, imsi: &str, number: &str, plmn: &str) -> Value {
            let mut ent = Dictionary::new();
            ent.insert("lastGoodImsi".into(), Value::String(imsi.into()));
            ent.insert(
                "kEntitlementsSelfRegistrationUpdateImei".into(),
                Value::String(imei.into()),
            );
            let mut phonebook = Dictionary::new();
            phonebook.insert("PNRPhoneNumber".into(), Value::String(number.into()));
            phonebook.insert("CopiedSIMPhoneNumber".into(), Value::String(number.into()));
            let mut deact = Dictionary::new();
            deact.insert(
                "LastRegisteredNetworkPlmn".into(),
                Value::String(plmn.into()),
            );
            let mut caps = Dictionary::new();
            caps.insert("NetworkSupportsVoPS".into(), Value::Boolean(true));

            let mut d = Dictionary::new();
            d.insert("CarrierEntitlements".into(), Value::Dictionary(ent));
            d.insert("phonebook".into(), Value::Dictionary(phonebook));
            d.insert("SimDeactivationInfo".into(), Value::Dictionary(deact));
            // Radio state the module deliberately ignores.
            d.insert("Capabilities".into(), Value::Dictionary(caps));
            Value::Dictionary(d)
        }
        let mut wallet = Dictionary::new();
        wallet.insert(
            "8901260971148676693".into(),
            sim(
                "353985100845978",
                "310260974867669",
                "+19195794674",
                "310260",
            ),
        );
        wallet.insert(
            "8901260971148676694".into(),
            sim(
                "353985100845979",
                "310260974867670",
                "+19195794675",
                "310410",
            ),
        );
        let mut root = Dictionary::new();
        root.insert("PersonalWallet".into(), Value::Dictionary(wallet));
        root.insert("LastKnownServingMnc".into(), Value::Integer(260.into()));
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `…findmydeviced.FMIPAccounts.plist` — the account Find My is bound to.
    ///
    /// `addTime` is UNIX seconds, not Cocoa. Read as Cocoa this 2023 value
    /// lands in 2054, which is exactly the sort of thing a fixture should pin.
    fn seed_find_my() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();
        root.insert("dsid".into(), Value::String("17193901029".into()));
        root.insert("addTime".into(), Value::Real(1_688_242_982.504));
        root.insert("osVersion".into(), Value::String("17.3".into()));
        root.insert("lowBatteryLocate".into(), Value::Boolean(true));
        root.insert("enableContext".into(), Value::Integer(3.into()));
        // An array of bare strings, which no column can address -- present so
        // the limitation the module documents is visible.
        root.insert(
            "versionHistory".into(),
            Value::Array(vec![Value::String("u16.1.2".into())]),
        );
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `com.apple.MobileSMS.plist` — BOTH retention keys, the iOS <=16 spelling
    /// and the iOS 17+ one, so a module reading only the modern one is caught.
    /// 30 is mapped; a deliberately unmapped 90 proves an unknown code travels
    /// as itself rather than becoming "Unknown".
    fn seed_message_retention() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();
        root.insert("SSKeepMessages".into(), Value::Integer(30.into()));
        root.insert("KeepMessageForDays".into(), Value::Integer(90.into()));
        // Other keys in the real file, none of them read.
        root.insert("ShowSubject".into(), Value::Boolean(false));
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `com.apple.mobile.ldbackup.plist` — the device's own backup history.
    fn seed_backup_settings() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();
        // Cocoa seconds, not Unix: 2024-07-28 in Core Data's epoch.
        root.insert("LastiTunesBackupDate".into(), Value::Real(743_800_000.0));
        root.insert(
            "LastiTunesBackupTZ".into(),
            Value::String("Europe/Stockholm".into()),
        );
        root.insert("LastCloudBackupDate".into(), Value::Real(743_900_000.0));
        root.insert(
            "LastCloudBackupTZ".into(),
            Value::String("Europe/Stockholm".into()),
        );
        root.insert("CloudBackupEnabled".into(), Value::Boolean(true));
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `com.apple.locationd.plist` — the master Location Services switch.
    ///
    /// The key name CONTAINS A DOT (`LocationServicesEnabledIn8.0`), which is
    /// the case that makes column paths lists rather than dotted strings. A
    /// fixture without it would let that regress unnoticed.
    fn seed_location_services() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();
        root.insert("LocationServicesEnabledIn8.0".into(), Value::Boolean(true));
        root.insert("LastSystemVersion".into(), Value::String("21D50".into()));
        // Scheduler bookkeeping the module deliberately ignores.
        root.insert("kP6MWDNextEstimateTime".into(), Value::Real(744_000_000.0));
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `com.apple.sharingd.plist` — AirDrop's identifier and discoverability.
    fn seed_airdrop() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();
        root.insert("AirDropID".into(), Value::String("6f8a2b1c9d4e".into()));
        // Already words in this plist, so no enum mapping is needed.
        root.insert(
            "DiscoverableMode".into(),
            Value::String("Contacts Only".into()),
        );
        // Other sharingd keys, none of them read.
        root.insert("HandoffEnabled".into(), Value::Boolean(true));
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// `com.apple.mobiletimer.plist` — the World Clock city list.
    ///
    /// A DIFFERENT file from the one above, one letter apart: `mobiletimer`
    /// here, `mobiletimerd` (the daemon) for alarms and timers. Each city sits
    /// behind a nested key that is itself called `city`, which is Apple's shape
    /// and not a transcription error.
    fn seed_world_clock() -> Vec<u8> {
        use plist::{Dictionary, Value};
        fn city(
            name: &str,
            country: &str,
            tz: &str,
            lat: f64,
            lon: f64,
            locale: &str,
            id: &str,
        ) -> Value {
            let mut c = Dictionary::new();
            c.insert("unlocalizedName".into(), Value::String(name.into()));
            c.insert(
                "unlocalizedCountryName".into(),
                Value::String(country.into()),
            );
            c.insert("timeZone".into(), Value::String(tz.into()));
            c.insert("latitude".into(), Value::Real(lat));
            c.insert("longitude".into(), Value::Real(lon));
            c.insert("localeCode".into(), Value::String(locale.into()));
            c.insert("identifier".into(), Value::String(id.into()));
            // Present in the real file, deliberately unread.
            c.insert("yahooCode".into(), Value::String("YAH0001".into()));
            let mut outer = Dictionary::new();
            outer.insert("city".into(), Value::Dictionary(c));
            Value::Dictionary(outer)
        }

        let mut root = Dictionary::new();
        root.insert(
            "cities".into(),
            Value::Array(vec![
                city(
                    "Stockholm",
                    "Sweden",
                    "Europe/Stockholm",
                    59.3293,
                    18.0686,
                    "sv_SE",
                    "Stockholm",
                ),
                city(
                    "Cupertino",
                    "United States",
                    "America/Los_Angeles",
                    37.323,
                    -122.0322,
                    "en_US",
                    "Cupertino",
                ),
            ]),
        );
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// Siri's backed-up preferences, with the nested `Output Voice` dictionary the
    /// module reaches into, and the undocumented keys it deliberately leaves.
    fn seed_siri() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut voice = Dictionary::new();
        voice.insert("Language".into(), Value::String("en-US".into()));
        voice.insert("Name".into(), Value::String("nora".into()));
        voice.insert("Gender".into(), Value::Integer(2.into()));
        voice.insert("Custom".into(), Value::Boolean(true));
        // Undocumented, unread.
        voice.insert("Footprint".into(), Value::Integer(2.into()));

        let mut root = Dictionary::new();
        root.insert("Output Voice".into(), Value::Dictionary(voice));
        root.insert("Cloud Sync Enabled".into(), Value::Boolean(true));
        root.insert(
            "MultiUser VoiceIdentification Enabled".into(),
            Value::Boolean(false),
        );
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// locationd's client register: entries keyed by a compound identifier, some
    /// carrying an explicit `BundleId` and some not.
    ///
    /// The cases the module claims: an app client whose bundle id is present (so
    /// the Apps join works), the SAME app with a second sub-bundle session (so the
    /// key is what tells them apart), and a system location bundle with no
    /// `BundleId` at all — which must still produce a row rather than be filtered
    /// out of the store.
    fn seed_location_clients() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();

        let mut app = Dictionary::new();
        app.insert(
            "BundleId".into(),
            Value::String("com.example.chatapp".into()),
        );
        app.insert(
            "BundlePath".into(),
            Value::String("/private/var/containers/Bundle/Application/ChatApp.app".into()),
        );
        app.insert("Registered".into(), Value::String("".into()));
        // Cocoa seconds, NOT a plist Date: the module must declare an epoch.
        app.insert(
            "ReceivingLocationInformationTimeStopped".into(),
            Value::Real(744_322_588.28),
        );
        root.insert("icom.example.chatapp:".into(), Value::Dictionary(app));

        // Same app, a different session — only the key separates them.
        let mut app2 = Dictionary::new();
        app2.insert(
            "BundleId".into(),
            Value::String("com.example.chatapp".into()),
        );
        app2.insert("LocationTimeStopped".into(), Value::Real(744_291_564.14));
        root.insert(
            "lcom.example.chatapp:p/System/Library/LocationBundles/Nav.bundle".into(),
            Value::Dictionary(app2),
        );

        // A system bundle with NO BundleId: still a row, just nothing to attach to.
        let mut sys = Dictionary::new();
        sys.insert(
            "BundlePath".into(),
            Value::String("/System/Library/LocationBundles/TraceHarvest.bundle".into()),
        );
        sys.insert("Registered".into(), Value::String("".into()));
        sys.insert(
            "ReceivingLocationInformationTimeStopped".into(),
            Value::Real(744_000_000.0),
        );
        // Undocumented bitmask, deliberately unread.
        sys.insert(
            "SupportedAuthorizationMask".into(),
            Value::Integer(7.into()),
        );
        root.insert(
            "p/System/Library/LocationBundles/TraceHarvest.bundle".into(),
            Value::Dictionary(sys),
        );

        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// The paired watch's app list. One app that IS on the watch and one that is
    /// only listed, because `isLocallyAvailable` is the difference between "this
    /// app exists for the watch" and "this app is on it".
    fn seed_watch_apps() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut app = Dictionary::new();
        app.insert(
            "companionAppBundleID".into(),
            Value::String("com.example.chatapp".into()),
        );
        app.insert("bundleShortVersion".into(), Value::String("2.4".into()));
        app.insert("bundleVersion".into(), Value::String("2401".into()));
        app.insert("isLocallyAvailable".into(), Value::Boolean(true));
        app.insert("minimumOSVersion".into(), Value::String("9.6".into()));
        // Deliberately unread.
        app.insert("sequenceNumber".into(), Value::Integer(6.into()));

        let mut absent = Dictionary::new();
        absent.insert(
            "companionAppBundleID".into(),
            Value::String("com.example.todo".into()),
        );
        absent.insert("bundleShortVersion".into(), Value::String("1.0".into()));
        absent.insert("isLocallyAvailable".into(), Value::Boolean(false));

        let mut list = Dictionary::new();
        list.insert(
            "com.example.chatapp.watchkitapp".into(),
            Value::Dictionary(app),
        );
        list.insert(
            "com.example.todo.watchkitapp".into(),
            Value::Dictionary(absent),
        );

        let mut root = Dictionary::new();
        root.insert("appList".into(), Value::Dictionary(list));
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// MobileBackup.plist — a dictionary of DOMAIN → BYTES, the shape
    /// `value_column` exists for. Includes the daemon internals the module leaves.
    fn seed_backup_sizing() -> Vec<u8> {
        use plist::{Dictionary, Value};
        let mut sizing = Dictionary::new();
        sizing.insert("KeyboardDomain".into(), Value::Integer(2_535_424.into()));
        sizing.insert(
            "CameraRollDomain".into(),
            Value::Integer(3_221_225_472_i64.into()),
        );
        sizing.insert(
            "AppDomainGroup-group.com.example.chat".into(),
            Value::Integer(175_961.into()),
        );
        let mut root = Dictionary::new();
        root.insert("PreflightSizing".into(), Value::Dictionary(sizing));
        // Unread daemon internals.
        root.insert(
            "FetchMissingKeysAtNextUnlock".into(),
            Value::Integer(0.into()),
        );
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// MTLibrary.sqlite's show table. One subscription has been listened to and
    /// one has not, because `ZLASTDATEPLAYED` being null is the difference between
    /// "followed" and "actually played" — the single most useful thing this
    /// artifact says.
    fn seed_podcasts(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE ZMTPODCAST (Z_PK INTEGER PRIMARY KEY, ZSUBSCRIBED INTEGER,
                ZADDEDDATE TIMESTAMP, ZLASTDATEPLAYED TIMESTAMP, ZAUTHOR VARCHAR,
                ZCATEGORY VARCHAR, ZFEEDURL VARCHAR, ZTITLE VARCHAR);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO ZMTPODCAST
                (ZSUBSCRIBED, ZADDEDDATE, ZLASTDATEPLAYED, ZAUTHOR, ZCATEGORY, ZFEEDURL, ZTITLE)
             VALUES
                («redacted»856862,632849727,'A tech journalist','Tech News',
                 'https://example.com/feed','Listened Show'),
                («redacted»856678,NULL,'Example Radio','Daily News',
                 'https://example.org/rss','Never Played Show');",
        )
        .unwrap();
        // Episodes. The subscription caches a whole FEED -- on the validation
        // device «redacted» rows for 6 shows, of which «redacted» were never touched --
        // so the fixture carries untouched rows too. A module that forgets the
        // engagement filter returns them and looks like it found more.
        c.execute_batch(
            "CREATE TABLE ZMTEPISODE (Z_PK INTEGER PRIMARY KEY, ZPODCAST INTEGER,
                ZTITLE VARCHAR, ZDURATION FLOAT, ZPLAYHEAD FLOAT, ZPLAYCOUNT INTEGER,
                ZHASBEENPLAYED INTEGER, ZISBOOKMARKED INTEGER,
                ZDOWNLOADDATE TIMESTAMP, ZLASTDATEPLAYED TIMESTAMP,
                ZPUBDATE TIMESTAMP);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO ZMTEPISODE
                (ZPODCAST, ZTITLE, ZDURATION, ZPLAYHEAD, ZPLAYCOUNT, ZHASBEENPLAYED,
                 ZISBOOKMARKED, ZDOWNLOADDATE, ZLASTDATEPLAYED, ZPUBDATE)
             VALUES
                -- Downloaded and part-listened: the playhead beside the duration
                -- is what says 'started, did not finish'.
                (1,'Half heard',3600,900,1,NULL,«redacted»628215,711630000,711626308),
                -- Downloaded, never opened.
                (1,'Queued up',764,0,0,NULL,«redacted»543114,NULL,711540859),
                -- Bookmarked without a download.
                (2,'Saved for later',1200,0,0,NULL,1,NULL,NULL,711000000),
                -- Feed cache: no download, no play, no bookmark. MUST NOT show.
                (2,'Never touched',900,0,0,NULL,0,NULL,NULL,710000000);",
        )
        .unwrap();
    }

    /// AllTrails: the three tables an activity is spread across.
    ///
    /// One recording has TWO timed segments — a pause and resume — because the
    /// aggregation is the whole reason the join is written the way it is: joined
    /// row-for-row it would report one hike as two.
    fn seed_alltrails(c: &Connection) {
        c.execute_batch(
            "CREATE TABLE ZTRACK (Z_PK INTEGER PRIMARY KEY, ZELEVATIONGAIN INTEGER,
                ZTIMEMOVING INTEGER, ZTIMETOTAL INTEGER, ZMAP INTEGER,
                ZCALORIES FLOAT, ZDISTANCETOTAL FLOAT, ZNAME VARCHAR);
             CREATE TABLE ZMAP (Z_PK INTEGER PRIMARY KEY, ZISPRIVATE INTEGER,
                ZBOTTOMRIGHTLATITUDE FLOAT, ZBOTTOMRIGHTLONGITUDE FLOAT,
                ZTOPLEFTLATITUDE FLOAT, ZTOPLEFTLONGITUDE FLOAT, ZNAME VARCHAR);
             CREATE TABLE ZLINETIMEDSEGMENT (Z_PK INTEGER PRIMARY KEY, ZTRACK INTEGER,
                ZDATETIMESTART TIMESTAMP, ZDATETIMESTOP TIMESTAMP);",
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO ZMAP (Z_PK, ZISPRIVATE, ZTOPLEFTLATITUDE, ZTOPLEFTLONGITUDE,
                ZBOTTOMRIGHTLATITUDE, ZBOTTOMRIGHTLONGITUDE, ZNAME) VALUES
                (1,0,35.60,-78.80,35.70,-78.90,'Bass Lake Trail'),
                (2,1,38.70,-77.20,38.90,-77.40,'Morning hike');
             INSERT INTO ZTRACK (Z_PK, ZELEVATIONGAIN, ZTIMEMOVING, ZTIMETOTAL, ZMAP,
                ZCALORIES, ZDISTANCETOTAL) VALUES
                (1,«redacted»6,3025,«redacted».«redacted»9.33),
                (2,«redacted»8,4498,«redacted».«redacted»6.12);
             INSERT INTO ZLINETIMEDSEGMENT (Z_PK, ZTRACK, ZDATETIMESTART, ZDATETIMESTOP) VALUES
                -- One hike, paused and resumed: TWO segments, one activity.
                (1,«redacted»000000,660001000),
                (2,«redacted»002000,660003025),
                (3,«redacted»000000,726004498);",
        )
        .unwrap();
    }

    /// IconState.plist: pages of icons, and the dock.
    ///
    /// TWO pages, because one page could not tell a working wildcard from a path
    /// that happened to find the only container there was.
    fn seed_icon_state() -> Vec<u8> {
        use plist::{Dictionary, Value};
        fn icon(id: &str, kind: &str, size: &str) -> Value {
            let mut d = Dictionary::new();
            d.insert("displayIdentifier".into(), Value::String(id.into()));
            d.insert("iconType".into(), Value::String(kind.into()));
            d.insert("gridSize".into(), Value::String(size.into()));
            Value::Dictionary(d)
        }
        /// A widget STACK: the icon itself is an anonymous UUID and the names
        /// are one level down, which is the whole reason
        /// `home_screen_widgets.toml` exists.
        fn stack(id: &str, widgets: &[(&str, &str)]) -> Value {
            let mut d = Dictionary::new();
            d.insert("displayIdentifier".into(), Value::String(id.into()));
            d.insert("iconType".into(), Value::String("custom".into()));
            d.insert("gridSize".into(), Value::String("medium".into()));
            d.insert(
                "elements".into(),
                Value::Array(
                    widgets
                        .iter()
                        .map(|(w, container)| {
                            let mut e = Dictionary::new();
                            e.insert("elementType".into(), Value::String("widget".into()));
                            e.insert("widgetIdentifier".into(), Value::String((*w).into()));
                            e.insert(
                                "containerBundleIdentifier".into(),
                                Value::String((*container).into()),
                            );
                            Value::Dictionary(e)
                        })
                        .collect(),
                ),
            );
            Value::Dictionary(d)
        }
        let page0 = Value::Array(vec![
            icon("com.example.chatapp", "app", "small"),
            // A widget stack: the identifier is a UUID, not a bundle id, and
            // the two widgets inside it are the only readable thing about it.
            stack(
                "A5E1414E-FD2B-486D-BAC2-B0DEED262F03",
                &[
                    ("com.apple.weather", "com.apple.weather"),
                    (
                        "com.apple.mobileslideshow.PhotosReliveWidget",
                        "com.apple.mobileslideshow",
                    ),
                ],
            ),
        ]);
        let page1 = Value::Array(vec![icon("com.example.todo", "app", "small")]);

        let mut root = Dictionary::new();
        root.insert("iconLists".into(), Value::Array(vec![page0, page1]));
        root.insert(
            "buttonBar".into(),
            Value::Array(vec![
                Value::String("com.apple.mobilephone".into()),
                Value::String("com.example.chatapp".into()),
            ]),
        );
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &Value::Dictionary(root)).unwrap();
        out
    }

    /// Where each shipped module's store lives, stated HERE rather than read
    /// from the module.
    ///
    /// This is the whole point. The first version of this test built its
    /// fixture from `spec.path`, which made the assertion a tautology: a review
    /// proved that setting the module's path to `Library/TOTALLY/WRONG/Nope.db`
    /// still passed. Path and domain are the single most likely thing to be
    /// wrong in a new declarative module, so the expected values live in the
    /// test — derived from docs/reference/backup-coverage-audit.md — and the
    /// module has to match them.
    const FIXTURES: &[(&str, &str, &str)] = &[
        // id, domain, relative path
        ("tcc", "HomeDomain", "Library/TCC/TCC.db"),
        (
            "health_current_device",
            "HealthDomain",
            "Health/healthdb.sqlite",
        ),
        (
            "carplay_recent_apps",
            "HomeDomain",
            "Library/Preferences/com.apple.CarPlayApp.plist",
        ),
        (
            "carplay_session",
            "HomeDomain",
            "Library/Preferences/com.apple.CarPlayApp.plist",
        ),
        (
            "life360_locations",
            "AppDomainGroup-group.com.life360.safetymap",
            "**/com.life360.safetymap*.log",
        ),
        (
            "accounts",
            "HomeDomain",
            "Library/Accounts/Accounts3.sqlite",
        ),
        (
            "bluetooth_paired",
            "SysSharedContainerDomain-systemgroup.com.apple.bluetooth",
            "Library/Database/com.apple.MobileBluetooth.ledevices.paired.db",
        ),
        (
            "data_usage",
            "WirelessDomain",
            "Library/Databases/DataUsage.sqlite",
        ),
        (
            "sim_cards",
            "WirelessDomain",
            "Library/Databases/CellularUsage.db",
        ),
        (
            "wifi_networks",
            "SystemPreferencesDomain",
            "com.apple.wifi.known-networks.plist",
        ),
        (
            "bluetooth_devices",
            "SysSharedContainerDomain-systemgroup.com.apple.bluetooth",
            "Library/Preferences/com.apple.MobileBluetooth.devices.plist",
        ),
        (
            "wifi_private_mac",
            "SystemPreferencesDomain",
            "SystemConfiguration/com.apple.wifi-private-mac-networks.plist",
        ),
        (
            "bluetooth_nearby",
            "SysSharedContainerDomain-systemgroup.com.apple.bluetooth",
            "Library/Database/com.apple.MobileBluetooth.ledevices.other.db",
        ),
        (
            "device_locale",
            "HomeDomain",
            "Library/Preferences/.GlobalPreferences.plist",
        ),
        (
            "timers",
            "HomeDomain",
            "Library/Preferences/com.apple.mobiletimerd.plist",
        ),
        (
            "stopwatch",
            "HomeDomain",
            "Library/Preferences/com.apple.mobiletimerd.plist",
        ),
        (
            "airdrop",
            "HomeDomain",
            "Library/Preferences/com.apple.sharingd.plist",
        ),
        (
            "message_retention",
            "HomeDomain",
            "Library/Preferences/com.apple.MobileSMS.plist",
        ),
        (
            "backup_settings",
            "HomeDomain",
            "Library/Preferences/com.apple.mobile.ldbackup.plist",
        ),
        (
            "location_services",
            "HomeDomain",
            "Library/Preferences/com.apple.locationd.plist",
        ),
        (
            "imei_imsi",
            "WirelessDomain",
            "Library/Preferences/com.apple.commcenter.plist",
        ),
        (
            "find_my",
            "HomeDomain",
            "Library/Preferences/com.apple.icloud.findmydeviced.FMIPAccounts.plist",
        ),
        (
            "icloud_drive",
            "HomeDomain",
            "Library/Application Support/CloudDocs/session/db/client.db",
        ),
        (
            "os_build_history",
            "HomeDomain",
            "Library/Application Support/CloudDocs/session/db/client.db",
        ),
        (
            "icloud_app_libraries",
            "HomeDomain",
            "Library/Application Support/CloudDocs/session/db/client.db",
        ),
        (
            "icloud_devices",
            "HomeDomain",
            "Library/Application Support/CloudDocs/session/db/server.db",
        ),
        (
            "mega_files",
            "AppDomainGroup-group.mega.ios",
            "GroupSupport/megaclient_statecache14_*.db",
        ),
        (
            "chromium_logins",
            "AppDomain-*",
            "Library/Application Support/**/Default/Login Data",
        ),
        (
            "chromium_top_sites",
            "AppDomain-*",
            "Library/Application Support/**/Default/Top Sites",
        ),
        (
            "service_workers",
            "AppDomain-*",
            "Library/WebKit/WebsiteData/Default/*/*/ServiceWorkers/ServiceWorkerRegistrations-*.sqlite3",
        ),
        (
            "webkit_domains",
            "AppDomain-*",
            "Library/WebKit/WebsiteData/ResourceLoadStatistics/observations.db",
        ),
        (
            "waze_places",
            "AppDomain-com.waze.iphone",
            "Documents/user.db",
        ),
        (
            "waze_recents",
            "AppDomain-com.waze.iphone",
            "Documents/user.db",
        ),
        (
            "waze_favorites",
            "AppDomain-com.waze.iphone",
            "Documents/user.db",
        ),
        (
            "world_clock",
            "HomeDomain",
            "Library/Preferences/com.apple.mobiletimer.plist",
        ),
        (
            "alarms",
            "HomeDomain",
            "Library/Preferences/com.apple.mobiletimerd.plist",
        ),
        (
            "sleep_schedule",
            "HomeDomain",
            "Library/Preferences/com.apple.mobiletimerd.plist",
        ),
        (
            "siri_settings",
            "HomeDomain",
            "Library/Preferences/com.apple.assistant.backedup.plist",
        ),
        (
            "location_clients",
            "RootDomain",
            "Library/Caches/locationd/clients.plist",
        ),
        (
            "home_screen_widgets",
            "HomeDomain",
            "Library/SpringBoard/IconState.plist",
        ),
        (
            "home_screen",
            "HomeDomain",
            "Library/SpringBoard/IconState.plist",
        ),
        ("dock", "HomeDomain", "Library/SpringBoard/IconState.plist"),
        (
            "alltrails",
            "AppDomain-com.alltrails.AllTrails",
            "Documents/AllTrails.sqlite",
        ),
        (
            "podcast_episodes",
            "AppDomainGroup-243LU875E5.groups.com.apple.podcasts",
            "Documents/MTLibrary.sqlite",
        ),
        (
            "podcasts",
            "AppDomainGroup-243LU875E5.groups.com.apple.podcasts",
            "Documents/MTLibrary.sqlite",
        ),
        (
            "backup_sizing",
            "HomeDomain",
            "Library/Preferences/com.apple.MobileBackup.plist",
        ),
        (
            "watch_apps",
            "HomeDomain",
            // The pattern itself: there is no exact path, because the segment is
            // the paired device's UUID.
            "Library/DeviceRegistry/*/AppConduit/ACXRemoteAppList.plist",
        ),
    ];

    /// How a module's fixture store is built. Two kinds, because a module now has
    /// two kinds of source.
    enum Seed {
        Sql(fn(&Connection)),
        Bytes(fn() -> Vec<u8>),
    }

    /// `healthdb.sqlite`'s single-row device context — a different file from the
    /// provenance store above, which is the mistake this fixture pins.
    fn seed_health_device_context(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE device_context (date_modified REAL, product_type_name TEXT,
                 currentOS_name TEXT, currentOS_version TEXT);
             INSERT INTO device_context VALUES (744322680.0, 'iPhone12,1', 'iOS', '17.3');",
        )
        .unwrap();
    }

    /// CarPlay's preferences: recent apps keyed by bundle id, plus the
    /// session-end facts. One fixture for both modules, because they read one
    /// store — writing two would let them drift apart while both stayed green.
    fn seed_carplay() -> Vec<u8> {
        let mut recents = plist::Dictionary::new();
        // UNIX seconds, not Cocoa — this store's own encoding. 1706104059 is
        // 2024-01-24. Seeding Cocoa here would agree with a module that declared
        // the wrong epoch and prove nothing.
        recents.insert(
            "com.waze.iphone".into(),
            plist::Value::Real(1_722_097_299.0),
        );
        recents.insert(
            "com.spotify.client".into(),
            plist::Value::Real(1_706_104_059.0),
        );

        let mut interrupt = plist::Dictionary::new();
        interrupt.insert("batteryPercentage".into(), 84.into());
        interrupt.insert("thermalLevel".into(), "None".into());

        let mut root = plist::Dictionary::new();
        root.insert("CARRecentAppHistory".into(), recents.into());
        root.insert("CARAnalyticsSessionInterruptKey".into(), interrupt.into());
        root.insert(
            "CARAnalyticsPreviousSessionEnd".into(),
            plist::Value::Date(
                (std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_722_097_299)).into(),
            ),
        );
        // Internal counters the modules deliberately ignore; present so the
        // fixture is the real shape rather than only what we read.
        root.insert("CARStartPageIndex".into(), 103.into());
        root.insert("CARWallpaperCacheVersionKey".into(), 8.into());

        let mut out = Vec::new();
        plist::Value::Dictionary(root)
            .to_writer_binary(&mut out)
            .unwrap();
        out
    }

    /// A Life360 log: two `X-UserContext` records among ordinary chatter.
    ///
    /// The chatter is the point as much as the records. A log is mostly NOT
    /// records — including a line that merely mentions the marker's neighbours,
    /// and one whose payload is truncated mid-write, which is what an
    /// append-only file looks like when the backup catches it. Both must be
    /// skipped without costing the good records around them.
    fn seed_life360_log() -> Vec<u8> {
        concat!(
            "2024-07-18 08:36:25.721-0400 Life360[616:160346] I | NGL | Filter out: stale age\n",
            "2024-07-18 08:36:26.001-0400 Life360[616:160346] I | NET | X-UserContext header set: ",
            r#"{"flags":{"preciseLocation":"fullAccuracy"},"#,
            r#""device":{"userActivity":"os_vehicle","charge":"1","battery":45},"#,
            r#""geolocation":{"lat":35.615977,"lon":-78.812429,"alt":130.3,"speed":9.42,"#,
            r#""heading":131.1,"accuracy":4.6,"timestamp":1721306184},"#,
            r#""geolocation_meta":{"lmode":"drive"}}"#,
            "\n",
            "2024-07-18 08:37:02.114-0400 Life360[616:160346] I | NET | X-UserContext header set: ",
            r#"{"flags":{"preciseLocation":"reducedAccuracy"},"#,
            r#""device":{"userActivity":"os_walking","charge":"0","battery":44},"#,
            r#""geolocation":{"lat":35.616,"lon":-78.8125,"alt":131.0,"speed":1.2,"#,
            r#""heading":95.0,"accuracy":65.0,"timestamp":1721306222},"#,
            r#""geolocation_meta":{"lmode":"foreground"}}"#,
            "\n",
            // Truncated mid-write: skipped, not fatal.
            "2024-07-18 08:37:40.000-0400 Life360[616:160346] I | NET | X-UserContext header set: ",
            r#"{"geolocation":{"lat":35.61"#,
            "\n",
        )
        .as_bytes()
        .to_vec()
    }

    fn seed_for(id: &str) -> Seed {
        Seed::Sql(match id {
            "tcc" => seed_tcc,
            "accounts" => seed_accounts,
            "bluetooth_paired" => seed_bluetooth_paired,
            "data_usage" => seed_data_usage,
            "sim_cards" => seed_sim_cards,
            "bluetooth_nearby" => seed_bluetooth_nearby,
            // One store, two modules: the shows and the episodes.
            "podcasts" | "podcast_episodes" => seed_podcasts,
            "alltrails" => seed_alltrails,
            // Not SQL at all: return early rather than pretend.
            "carplay_recent_apps" | "carplay_session" => return Seed::Bytes(seed_carplay),
            "health_current_device" => seed_health_device_context,
            "life360_locations" => return Seed::Bytes(seed_life360_log),
            "wifi_networks" => return Seed::Bytes(seed_wifi_networks),
            "bluetooth_devices" => return Seed::Bytes(seed_bluetooth_devices),
            "wifi_private_mac" => return Seed::Bytes(seed_wifi_private_mac),
            "device_locale" => return Seed::Bytes(seed_device_locale),
            // One fixture, two modules: the store really does hold both.
            // One store, three modules: alarms, the sleep schedule and timers.
            "alarms" | "sleep_schedule" | "timers" | "stopwatch" => return Seed::Bytes(seed_clock),
            "airdrop" => return Seed::Bytes(seed_airdrop),
            "message_retention" => return Seed::Bytes(seed_message_retention),
            "backup_settings" => return Seed::Bytes(seed_backup_settings),
            "location_services" => return Seed::Bytes(seed_location_services),
            "imei_imsi" => return Seed::Bytes(seed_commcenter),
            "find_my" => return Seed::Bytes(seed_find_my),
            // One store, three modules: files, the boot log and the containers.
            "icloud_drive" | "os_build_history" | "icloud_app_libraries" => seed_icloud_drive,
            "icloud_devices" => seed_icloud_server,
            // One store, three modules: places, recents and favourites.
            "waze_places" | "waze_recents" | "waze_favorites" => seed_waze,
            "webkit_domains" => seed_observations,
            "service_workers" => seed_service_workers,
            "chromium_logins" => seed_chromium_logins,
            "chromium_top_sites" => seed_chromium_top_sites,
            "mega_files" => seed_mega,
            "world_clock" => return Seed::Bytes(seed_world_clock),
            "siri_settings" => return Seed::Bytes(seed_siri),
            "location_clients" => return Seed::Bytes(seed_location_clients),
            "watch_apps" => return Seed::Bytes(seed_watch_apps),
            "backup_sizing" => return Seed::Bytes(seed_backup_sizing),
            // One store, two modules: IconState holds pages AND the dock.
            // One store, three modules: the pages, the dock, and the widgets
            // hiding inside the pages' anonymous "custom" icons.
            "home_screen" | "dock" | "home_screen_widgets" => return Seed::Bytes(seed_icon_state),
            other => panic!(
                "module {other:?} has no fixture — add one to FIXTURES and to \
                 tools/make_fixture_backup.py, so shipping a module always means \
                 shipping data that proves it runs"
            ),
        })
    }

    /// The JSON the UI receives must have the field names the UI reads.
    ///
    /// `ArtifactSummary` shipped without `rename_all = "camelCase"` while
    /// `src/lib/ipc.ts` declared `joinColumn` / `rowCount` /
    /// `requiresEncryptedBackup`. In the real app all three were `undefined`, so
    /// `useHostedArtifacts` matched nothing and the Apps view showed no
    /// permissions — but the **mock** returns camelCase, so every browser check
    /// and every screenshot looked correct, and the Rust side was validated
    /// separately against a real backup without ever crossing IPC. Both halves
    /// were tested; the seam between them was not (#232).
    ///
    /// So this reads the TypeScript and compares. It is deliberately a Rust test
    /// rather than a lint: only serde can say what the wire format actually is.
    #[test]
    fn serialised_keys_match_the_ui_contract() {
        let summary = ArtifactSummary {
            id: "x".into(),
            name: "X".into(),
            category: None,
            description: "d".into(),
            surface: Surface::Device,
            shape: Shape::Table,
            join_column: None,
            highlight: None,
            columns: vec![],
            timestamp_columns: vec![],
            byte_columns: vec![],
            duration_columns: vec![],
            row_count: 0,
            requires_encrypted_backup: false,
        };
        let json = serde_json::to_value(&summary).unwrap();
        let mut actual: Vec<String> = json.as_object().unwrap().keys().cloned().collect();

        let ipc = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/ipc.ts"),
        )
        .expect("read src/lib/ipc.ts");
        let start = ipc
            .find("export type ArtifactSummary = {")
            .expect("no ArtifactSummary type in ipc.ts — did it move?");
        let body = &ipc[start..start + ipc[start..].find("\n};").expect("unterminated type")];
        // Top-level fields ONLY, which means the indent has to be checked rather
        // than trimmed away. Trimming first read the fields of a NESTED object
        // (`highlight`'s `column`, `whenColumn`, …) as though they were fields of
        // ArtifactSummary itself. The comment claimed "at one indent level" while
        // the code did not enforce it; a review flagged it as a latent false
        // failure, and adding `highlight` made it a real one.
        const INDENT: &str = "  ";
        let mut declared: Vec<String> = body
            .lines()
            .filter_map(|l| {
                // Exactly one indent level: two spaces, then something that is not
                // a space.
                let rest = l.strip_prefix(INDENT)?;
                if rest.starts_with(' ') {
                    return None;
                }
                if rest.starts_with('*') || rest.starts_with("/*") || rest.starts_with("//") {
                    return None;
                }
                let (name, _) = rest.split_once(':')?;
                let name = name.trim().trim_end_matches('?');
                if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    return None;
                }
                Some(name.to_string())
            })
            .collect();
        assert!(
            declared.len() > 5,
            "parsed only {} fields from ipc.ts — the parse is wrong, and a comparison \
             against nothing passes for the wrong reason: {declared:?}",
            declared.len()
        );

        actual.sort();
        declared.sort();
        assert_eq!(
            actual, declared,
            "ArtifactSummary's JSON keys and src/lib/ipc.ts have drifted. The UI reads the \
             names on the right; serde sends the names on the left. A field the UI cannot \
             find is `undefined`, which reads as a legitimately absent value rather than as \
             an error."
        );
    }

    /// Every `Surface` variant must be in the UI's union type, and vice versa.
    ///
    /// `serialised_keys_match_the_ui_contract` pins the field NAMES but not this:
    /// `src/lib/ipc.ts` declares `surface` as a string union, and
    /// `useHostedArtifacts(host: string)` compares against a plain `string`, so a
    /// new Rust variant could ship, be declared by a module, and never appear in
    /// the union — with TypeScript raising nothing, because nothing narrows it.
    /// `check-artifact-surfaces.mjs` reads the Rust enum rather than the union, so
    /// it does not cover this either.
    #[test]
    fn the_ui_surface_union_matches_the_rust_enum() {
        // The match is what makes this exhaustive: adding a variant to `Surface`
        // without adding it here is a COMPILE error, not a passing test. A plain
        // array would silently omit the new one.
        fn name(s: Surface) -> &'static str {
            match s {
                Surface::Apps => "apps",
                Surface::Contacts => "contacts",
                Surface::Device => "device",
                Surface::Standalone => "standalone",
            }
        }
        let all = [
            Surface::Apps,
            Surface::Contacts,
            Surface::Device,
            Surface::Standalone,
        ];
        // Taken from serde, not from `name` — so this also catches a
        // `rename_all` change, which would alter the wire format silently.
        let mut from_rust: Vec<String> = all
            .iter()
            .map(|s| {
                let json = serde_json::to_value(s).unwrap();
                let wire = json
                    .as_str()
                    .expect("a Surface serialises to a string")
                    .to_string();
                assert_eq!(wire, name(*s), "serde renamed a Surface variant");
                wire
            })
            .collect();

        let ipc = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/ipc.ts"),
        )
        .expect("read src/lib/ipc.ts");
        let line = ipc
            .lines()
            .find(|l| l.trim_start().starts_with("surface:"))
            .expect("no `surface:` field in ipc.ts — did ArtifactSummary change shape?");
        let mut from_ts: Vec<String> = line
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|s| s.to_string())
            .collect();
        assert!(
            from_ts.len() > 1,
            "parsed {} members from {line:?} — the parse is wrong, so this compared \
             against nothing",
            from_ts.len()
        );

        from_rust.sort();
        from_ts.sort();
        assert_eq!(
            from_rust, from_ts,
            "the Surface enum and ipc.ts's `surface` union have drifted — a module \
             could declare a surface the UI's type does not know about"
        );
    }

    /// The compiled-in modules must be exactly the files on disk.
    ///
    /// They are two copies of the same thing — `include_str!` bakes the file in
    /// at build time — so the only way they diverge is someone adding a .toml
    /// and forgetting the BUILTIN entry. Then the module works in every test
    /// and is simply absent from the app, which is the worst kind of quiet.
    #[test]
    fn embedded_modules_match_the_directory() {
        let on_disk = load_modules(&builtin_modules_dir()).unwrap();
        let embedded = builtin_modules().unwrap();
        let mut a: Vec<&str> = on_disk.iter().map(|m| m.id.as_str()).collect();
        let mut b: Vec<&str> = embedded.iter().map(|m| m.id.as_str()).collect();
        a.sort();
        b.sort();
        assert_eq!(
            a, b,
            "a module exists on disk but is not compiled in (or vice versa) — \
             add it to BUILTIN"
        );
    }

    /// EVERY shipped module must declare a real domain, sit where the audit
    /// says it does, load, run, and produce every column it declares.
    ///
    /// This is the guard that scales: it costs nothing per new artifact and
    /// fails the moment one rots — a renamed column, a typo in the SQL or the
    /// path, a column spec that stops matching.
    #[test]
    fn an_optional_container_that_is_absent_is_empty_not_an_error() {
        // Verified on the iPhone 11 / iOS 17.3 image: com.apple.mobiletimerd
        // .plist has MTAlarms and MTTimers and NO MTStopwatches -- the
        // stopwatch had never been run. Strict behaviour took the whole
        // artifact down over the ordinary state of a device.
        use plist::{Dictionary, Value};
        let mut root = Dictionary::new();
        root.insert("MTAlarms".into(), Value::Dictionary(Dictionary::new()));
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, &Value::Dictionary(root)).unwrap();

        let mut spec = load_modules(&builtin_modules_dir())
            .unwrap()
            .into_iter()
            .find(|m| m.id == "stopwatch")
            .unwrap();
        assert!(
            spec.plist.as_ref().unwrap().optional,
            "stopwatch must stay optional -- the real device has no such key"
        );
        let rows = run_plist_module(&spec, spec.plist.as_ref().unwrap(), &bytes).unwrap();
        assert!(
            rows.is_empty(),
            "absent optional container must yield no rows"
        );

        // ...and the default is still strict, or every typo'd path becomes a
        // silently empty artifact.
        spec.plist.as_mut().unwrap().optional = false;
        let err = run_plist_module(&spec, spec.plist.as_ref().unwrap(), &bytes)
            .expect_err("a missing container is an error unless declared optional");
        assert!(err.to_string().contains("MTStopwatches"), "{err}");
    }

    #[test]
    fn a_domain_glob_matches_only_what_it_should() {
        // `*` matches any run of characters -- a domain has no `/`, so there is
        // no segment rule. The cases that matter are the ones a naive
        // `contains` would get wrong.
        assert!(domain_matches(
            "AppDomain-*",
            "AppDomain-com.brave.ios.browser"
        ));
        assert!(domain_matches("AppDomain-*", "AppDomain-x"));
        // A leading literal must be a PREFIX, not merely present: an
        // AppDomainGroup is a different container and must not be swept in by a
        // pattern written for AppDomain.
        assert!(!domain_matches("AppDomain-*", "AppDomainGroup-group.x"));
        assert!(!domain_matches("AppDomain-*", "HomeDomain"));
        assert!(!domain_matches("AppDomain-*", "XAppDomain-y"));
        // Without a trailing `*` the whole domain must be consumed.
        assert!(domain_matches("AppDomain-com.a", "AppDomain-com.a"));
        assert!(!domain_matches("AppDomain-com.a", "AppDomain-com.ab"));
    }

    #[test]
    fn a_globbed_domain_demands_a_way_to_tell_the_apps_apart() {
        let base = |extra: &str| {
            format!(
                "id = \"x\"\nname = \"X\"\ndescription = \"A store every app keeps.\"\n\
                 surface = \"apps\"\ncategory = \"C\"\npath = \"a.db\"\n\
                 sql = \"SELECT 1 AS a\"\n{extra}\n[[columns]]\nname = \"A\"\nfrom = \"a\"\n"
            )
        };
        // Rows from Signal and rows from Chrome would be indistinguishable.
        let tmp = tempfile::tempdir().unwrap();
        write_module(tmp.path(), "m.toml", &base("domain = \"AppDomain-*\""));
        let err = load_modules(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("app_column"), "{err}");

        // Only app containers may be globbed: nothing else has a bundle id to
        // label the rows with.
        let tmp2 = tempfile::tempdir().unwrap();
        write_module(
            tmp2.path(),
            "m.toml",
            &base("domain = \"Home*\"\napp_column = \"A\""),
        );
        let err = load_modules(tmp2.path()).unwrap_err().to_string();
        assert!(err.contains("AppDomain-"), "{err}");

        // And an app_column with no glob is a mistake the author should hear
        // about, not a silent no-op.
        let tmp3 = tempfile::tempdir().unwrap();
        write_module(
            tmp3.path(),
            "m.toml",
            &base("domain = \"AppDomain-com.one\"\napp_column = \"A\""),
        );
        let err = load_modules(tmp3.path()).unwrap_err().to_string();
        assert!(err.contains("only one app"), "{err}");
    }

    #[test]
    fn a_glob_skips_a_foreign_store_but_not_a_broken_one() {
        // MEGA's `megaclient_statecache14_*.db` matches the node cache AND its
        // `_status_`/`_transfers_` siblings, which share no schema with it. One
        // unreadable match must not take the artifact down -- but EVERY match
        // being unreadable is a schema change, and must.
        let tmp = tempfile::tempdir().unwrap();
        let two = |rel: &str, id: &str, ddl: &str| {
            let blob_dir = tmp.path().join(&id[..2]);
            std::fs::create_dir_all(&blob_dir).unwrap();
            let c = Connection::open(blob_dir.join(id)).unwrap();
            c.execute_batch(ddl).unwrap();
            drop(c);
            let m = Connection::open(tmp.path().join("Manifest.db")).unwrap();
            m.execute_batch(
                "CREATE TABLE IF NOT EXISTS Files (fileID TEXT PRIMARY KEY, domain TEXT, \
                 relativePath TEXT, flags INTEGER, file BLOB);",
            )
            .unwrap();
            m.execute(
                "INSERT INTO Files VALUES (?1, 'HomeDomain', ?2, 1, NULL)",
                rusqlite::params![id, rel],
            )
            .unwrap();
        };
        two(
            "Group/store_real.db",
            "cd00000000000000000000000000000000000001",
            "CREATE TABLE nodes (name TEXT); INSERT INTO nodes VALUES ('a');",
        );
        two(
            "Group/store_other.db",
            "cd00000000000000000000000000000000000002",
            "CREATE TABLE unrelated (x TEXT);",
        );
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let work = tempfile::tempdir().unwrap();

        let module = |sql: &str| -> ModuleSpec {
            toml::from_str(&format!(
                "id = \"g\"\nname = \"G\"\ndescription = \"Two stores, one shape.\"\n\
                 surface = \"device\"\ncategory = \"C\"\ndomain = \"HomeDomain\"\n\
                 path = \"Group/store_*.db\"\npath_column = \"Which\"\n\
                 sql = \"{sql}\"\n\
                 [[columns]]\nname = \"Which\"\n[[columns]]\nname = \"N\"\nfrom = \"n\"\n"
            ))
            .unwrap()
        };

        let rows = run_module(
            &module("SELECT name AS n FROM nodes"),
            &index,
            None,
            work.path(),
        )
        .unwrap()
        .expect("the readable store still reports");
        assert_eq!(
            rows.len(),
            1,
            "the foreign store contributed nothing, quietly"
        );

        // Break the one that worked: with nothing left that runs, the error must
        // surface rather than an empty artifact.
        let err = run_module(
            &module("SELECT gone AS n FROM absent_table"),
            &index,
            None,
            work.path(),
        )
        .expect_err("every store failing is a schema change, not a skip");
        assert!(err.to_string().contains("sql"), "{err}");
    }

    fn mapped(pairs: &[(&str, &str)]) -> ColumnSpec {
        ColumnSpec {
            name: "Keep".into(),
            from: vec!["k".into()],
            value: None,
            kind: ColumnKind::Text,
            epoch: None,
            map: Some(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
        }
    }

    #[test]
    fn a_value_map_turns_codes_into_words() {
        let c = mapped(&[("0", "Forever"), ("30", "30 days")]);
        assert_eq!(
            apply_map(serde_json::json!(30), &c),
            serde_json::json!("30 days")
        );
        assert_eq!(
            apply_map(serde_json::json!("0"), &c),
            serde_json::json!("Forever")
        );
    }

    #[test]
    fn an_unmapped_code_travels_as_itself() {
        // The rule that matters. A code we have not seen is DATA -- replacing
        // it with "Unknown" would lose the one thing worth keeping, and a
        // fourth retention value is exactly what someone would want to see.
        let c = mapped(&[("0", "Forever")]);
        assert_eq!(
            apply_map(serde_json::json!(90), &c),
            serde_json::json!("90")
        );
    }

    #[test]
    fn a_mapped_column_is_always_a_string() {
        // Mapped or not, one type. A column that is text on one row and a
        // number on the next sorts and aligns differently row to row, which
        // reads as a rendering bug rather than as data.
        let c = mapped(&[("0", "Forever")]);
        assert!(apply_map(serde_json::json!(0), &c).is_string());
        assert!(apply_map(serde_json::json!(7), &c).is_string());
    }

    #[test]
    fn a_map_never_invents_a_value_for_null() {
        // Null is "the device never wrote this key". Mapping it would answer a
        // question the store did not.
        let c = mapped(&[("0", "Forever")]);
        assert!(apply_map(serde_json::Value::Null, &c).is_null());
    }

    #[test]
    fn a_timestamp_may_not_be_mapped() {
        // Stringifying a date turns off date formatting, silently.
        let mut spec = load_modules(&builtin_modules_dir())
            .unwrap()
            .into_iter()
            .find(|m| m.id == "backup_settings")
            .unwrap();
        spec.columns[0].map = Some([("1".into(), "x".into())].into_iter().collect());
        let err = spec.validate().unwrap_err();
        assert!(err.contains("not an enum"), "{err}");
    }

    #[test]
    fn every_shipped_module_loads_and_runs() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        assert!(
            !mods.is_empty(),
            "no modules shipped — did the directory move?"
        );

        // Apple's own domain list, so a typo'd domain ("HomeDomian") cannot
        // pass just because our fixture obligingly used it too.
        let domains_json =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/data/ios-backup-domains.json");
        let domains: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&domains_json).unwrap()).unwrap();

        for spec in &mods {
            let (_, domain, path) = FIXTURES
                .iter()
                .find(|(id, _, _)| *id == spec.id)
                .unwrap_or_else(|| panic!("module {:?} has no FIXTURES entry", spec.id));

            assert_eq!(
                &spec.domain, domain,
                "module {} declares a different domain than the audit records",
                spec.id
            );
            assert_eq!(
                &spec.path, path,
                "module {} declares a different path than the audit records",
                spec.id
            );
            assert!(
                domains.get(&spec.domain).is_some() || is_container_domain(&spec.domain),
                "module {} declares domain {:?}, which is neither one of the 19 domains iOS \
                 names outright nor a container domain ({})",
                spec.id,
                spec.domain,
                CONTAINER_DOMAIN_PREFIXES.join(", ")
            );

            let tmp = tempfile::tempdir().unwrap();
            // A PATTERN is seeded at a concrete path, so the module has to match
            // it for real. Writing the fixture at the literal pattern would let
            // `*` match itself and prove nothing — the store would be found by a
            // module whose pattern was wrong in every other respect.
            // `**` becomes SEVERAL real segments, so a module claiming to span
            // depths has to actually do it; a single replacement would let `**`
            // match one segment and prove nothing.
            let concrete = path
                .replace("**", "Outer/Inner")
                .replace('*', "48BEB26F-3064-4BEF-A616-AB96D8C5BD15");
            assert!(
                !concrete.contains('*'),
                "module {}: a `*` survived into the fixture path",
                spec.id
            );
            match seed_for(&spec.id) {
                Seed::Sql(f) => make_backup_in(tmp.path(), domain, &concrete, f),
                Seed::Bytes(f) => make_backup_bytes_in(tmp.path(), domain, &concrete, &f()),
            }
            let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
            let rows = run_module(spec, &index, None, &tmp.path().join("work"))
                .unwrap_or_else(|e| panic!("module {} failed to run: {e}", spec.id))
                .unwrap_or_else(|| panic!("module {} found nothing in its own fixture", spec.id));
            assert!(!rows.is_empty(), "module {} produced no rows", spec.id);
            for c in &spec.columns {
                assert!(
                    rows[0].contains_key(&c.name),
                    "module {} produced no {:?} column",
                    spec.id,
                    c.name
                );
                // And it must actually READ something somewhere. `contains_key`
                // is satisfied by a null, so a mistyped source — `__OSSPecific__`,
                // a renamed SQL alias — produced a full set of columns full of
                // nothing and shipped green. The SQL path has a hard guard for
                // this (a declared column absent from the result set is an
                // error); the plist path cannot have one, because a key missing
                // from a single record is legitimate. The fixture is where it can
                // be pinned: every declared column is there to carry a value, so
                // at least one fixture row must give it one.
                assert!(
                    rows.iter().any(|r| !r[&c.name].is_null()),
                    "module {}: column {:?} is null in EVERY fixture row — either its \
                     source is wrong, or the fixture does not exercise it. Both are worth \
                     failing on: a column that never carries a value is not a column.",
                    spec.id,
                    c.name
                );
                // A timestamp must land in the iPhone era. A wrong `epoch` does
                // not fail, parse badly or look odd in a table — it produces a
                // date decades out that still sorts and still renders. CarPlay's
                // `CARRecentAppHistory` holds UNIX seconds where almost every
                // other bare Apple real is Cocoa; read as Cocoa it gives 2055,
                // and only a range check or a human noticing the year catches it.
                //
                // 2007 is the first iPhone; 2035 is far enough out to never
                // bracket real data, close enough to catch a 31-year shift in
                // either direction.
                if c.kind == ColumnKind::Timestamp {
                    const FIRST_IPHONE: i64 = 1_167_609_600; // 2007-01-01
                    const FAR_FUTURE: i64 = 2_051_222_400; // 2035-01-01
                    for r in &rows {
                        let Some(t) = r[&c.name].as_i64() else {
                            continue;
                        };
                        assert!(
                            (FIRST_IPHONE..FAR_FUTURE).contains(&t),
                            "module {}: column {:?} produced {t}, which is outside the \
                             iPhone era — almost always a wrong `epoch` on a column whose \
                             store does not use the encoding it looks like it uses",
                            spec.id,
                            c.name
                        );
                    }
                }
            }
        }
    }

    /// The log runner must leave runner-owned columns alone.
    ///
    /// `run_module` fills the path column and any constants AFTER the runner, so
    /// a runner that also wrote them would be invisible end-to-end — the later
    /// write covers it. It is not harmless, though: descending an EMPTY `from`
    /// returns the payload root, so the column would briefly hold the entire JSON
    /// record, and any future caller of the runner that does not overwrite would
    /// ship that. Asserted at the runner, which is the only place it shows.
    #[test]
    fn the_log_runner_leaves_runner_owned_columns_unset() {
        let spec: ModuleSpec = toml::from_str(
            r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "**/thing *.log"
path_column = "Log"

[log]
json_after = "REC: "

[[columns]]
name = "Log"

[[columns]]
name = "App"
value = "com.example.app"

[[columns]]
name = "Lat"
from = ["geo", "lat"]
kind = "real"
"#,
        )
        .unwrap();
        let lg = spec.log.as_ref().unwrap();
        let rows = run_log_module(
            &spec,
            lg,
            br#"noise
REC: {"geo":{"lat":1.5}}
more noise"#,
        )
        .unwrap();

        assert_eq!(rows.len(), 1, "one record among the chatter: {rows:#?}");
        assert_eq!(rows[0]["Lat"], serde_json::json!(1.5));
        assert!(
            !rows[0].contains_key("Log"),
            "the path column is the runner's to fill, not this one's — it held {:?}",
            rows[0].get("Log")
        );
        assert!(
            !rows[0].contains_key("App"),
            "a constant column is filled by run_module, not from the payload"
        );
    }

    /// A `*` in `plist.rows` fans out across containers, and `index_column` says
    /// which one each row came from.
    ///
    /// Without the index the wildcard collapses five home-screen pages into one
    /// undifferentiated list — "Maps is installed" instead of "Maps is on page 4",
    /// which is the fact worth having. Two pages in the fixture, because one page
    /// cannot tell a working wildcard from a path that found the only container
    /// there was.
    #[test]
    fn a_rows_wildcard_fans_out_and_records_which() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let spec = mods.iter().find(|m| m.id == "home_screen").unwrap();
        make_backup_bytes_in(tmp.path(), &spec.domain, &spec.path, &seed_icon_state());
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        // Three icons across TWO pages, each knowing its page.
        assert_eq!(rows.len(), 3, "{rows:#?}");
        let pages: Vec<&str> = rows.iter().map(|r| r["Page"].as_str().unwrap()).collect();
        assert_eq!(pages, vec!["0", "0", "1"]);
        assert_eq!(
            rows[2]["Identifier"],
            serde_json::json!("com.example.todo"),
            "the second page's icon"
        );
        // A widget's identifier is a UUID, not a bundle id, and `iconType` is what
        // says so — both ship as stored rather than one being mapped to the other.
        assert_eq!(rows[1]["Kind"], serde_json::json!("custom"));

        // The DOCK is the same file, a different collection, and its rows are
        // plain strings — the wildcard hands each one over as a row.
        let dock = mods.iter().find(|m| m.id == "dock").unwrap();
        let rows = run_module(dock, &index, None, &tmp.path().join("work2"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["Position"], serde_json::json!("0"));
        assert_eq!(rows[0]["App"], serde_json::json!("com.apple.mobilephone"));
    }

    /// A wildcard with nothing to say which element matched is rejected: five
    /// pages in one list is a different artifact from a home screen.
    #[test]
    fn a_rows_wildcard_without_an_index_is_rejected() {
        let src = |extra: &str| {
            format!(
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.plist"

[plist]
rows = ["pages", "*"]
{extra}

[[columns]]
name = "A"
from = "a"

[[columns]]
name = "Which"
"#
            )
        };
        let spec: ModuleSpec = toml::from_str(&src("")).unwrap();
        let err = spec.validate().expect_err("no index_column");
        assert!(err.contains("no `index_column`"), "{err}");

        // And the reverse: an index column with no wildcard repeats one value.
        let spec: ModuleSpec = toml::from_str(
            &src("index_column = \"Which\"")
                .replace("rows = [\"pages\", \"*\"]", "rows = [\"pages\"]"),
        )
        .unwrap();
        let err = spec.validate().expect_err("no wildcard");
        assert!(err.contains("no `*`"), "{err}");
    }

    /// AllTrails: a paused-and-resumed recording is ONE activity, and the
    /// coordinates are the midpoint of a box rather than a corner of it.
    #[test]
    fn alltrails_aggregates_segments_and_centres_the_box() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods.iter().find(|m| m.id == "alltrails").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        make_backup_in(tmp.path(), &spec.domain, &spec.path, seed_alltrails);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();

        // TWO activities, not three: the first has two timed segments, and joining
        // row-for-row would report one hike as two.
        assert_eq!(rows.len(), 2, "{rows:#?}");

        let lake = rows
            .iter()
            .find(|r| r["Activity"] == serde_json::json!("Bass Lake Trail"))
            .unwrap();
        assert_eq!(lake["Segments"], serde_json::json!(2));
        // The span covers BOTH segments: first start to last stop.
        assert_eq!(
            lake["Started"],
            serde_json::json!(660_000_000_i64 + 978_307_200)
        );
        assert_eq!(
            lake["Ended"],
            serde_json::json!(660_003_025_i64 + 978_307_200)
        );

        // The MIDPOINT of the box (35.60..35.70), not either corner. A corner can
        // be a long way from anything actually walked.
        let lat = lake["Roughly where (lat)"].as_f64().unwrap();
        assert!((lat - 35.65).abs() < 1e-9, "got {lat}");
        let lon = lake["Roughly where (lon)"].as_f64().unwrap();
        assert!((lon + 78.85).abs() < 1e-9, "got {lon}");

        assert_eq!(lake["Distance (m)"], serde_json::json!(3049));
        assert_eq!(lake["Private"], serde_json::json!("No"));

        let other = rows
            .iter()
            .find(|r| r["Activity"] == serde_json::json!("Morning hike"))
            .unwrap();
        assert_eq!(other["Segments"], serde_json::json!(1));
        assert_eq!(other["Private"], serde_json::json!("Yes"));
    }

    /// The accounts module: a service is always NAMED, a sub-account says what it
    /// is part of, and a GUID never reaches the Service column.
    ///
    /// The first draft fell back to `ZACCOUNT.ZIDENTIFIER` believing it held
    /// "com.apple.account.Google". It holds a per-account GUID — measured on the
    /// validation image — so the fallback would have printed a UUID in a column
    /// headed "Service". The three rungs and this test exist because of that.
    #[test]
    fn podcast_episodes_lists_choices_not_the_cached_feed() {
        // Subscribing caches a whole back catalogue: «redacted» rows for 6 shows on
        // the validation device, «redacted» of them never touched. Listing those
        // would bury the real events in a table that LOOKS like thousands of
        // them, which is the failure this module's filter exists to prevent.
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "podcast_episodes")
            .expect("podcast_episodes ships");
        let tmp = tempfile::tempdir().unwrap();
        make_backup_in(tmp.path(), &spec.domain, &spec.path, seed_podcasts);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();

        let titles: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.get("Episode").and_then(|v| v.as_str()))
            .collect();
        assert!(
            !titles.contains(&"Never touched"),
            "a feed entry nobody opened is not something someone did: {titles:?}"
        );
        assert_eq!(
            rows.len(),
            3,
            "downloaded, queued and bookmarked: {titles:?}"
        );

        // The playhead beside the duration is what separates "listened" from
        // "started", so both have to survive to the row.
        let half = rows
            .iter()
            .find(|r| r.get("Episode").and_then(|v| v.as_str()) == Some("Half heard"))
            .expect("the part-listened episode is present");
        assert_eq!(half.get("Got to").and_then(|v| v.as_i64()), Some(900));
        assert_eq!(half.get("Length").and_then(|v| v.as_i64()), Some(3600));
    }

    #[test]
    fn accounts_module_names_every_service_and_never_shows_a_guid() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "accounts")
            .expect("accounts module ships");
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), &spec.path, seed_accounts);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        // 5 accounts, and the parent joins must not multiply them.
        assert_eq!(rows.len(), 5, "the parent LEFT JOINs changed the row count");

        let services: Vec<String> = rows
            .iter()
            .map(|r| r["Service"].as_str().unwrap_or_default().to_string())
            .collect();
        // Rung 1: a described type gives a readable name.
        assert!(services.contains(&"Gmail".to_string()));
        // Rung 2: a type with no description falls to its reverse-DNS identifier.
        assert!(
            services.contains(&"com.apple.account.undescribed".to_string()),
            "a type row with a NULL description should fall back to its own \
             identifier, got {services:?}"
        );
        // Rung 3: no type row at all still gets a name, not a GUID.
        assert!(
            services.contains(&"Unrecorded service type".to_string()),
            "an account with no type row should be named, got {services:?}"
        );
        // The whole point: nothing that looks like a GUID reaches this column.
        for svc in &services {
            assert!(
                !(svc.len() == 36 && svc.matches('-').count() == 4),
                "a GUID reached the Service column: {svc:?}"
            );
        }

        // The sub-account explains itself rather than looking like a duplicate.
        let child = rows
            .iter()
            .find(|r| r["Service"] == serde_json::json!("com.apple.account.undescribed"))
            .expect("the child account is present");
        assert_eq!(
            child["Part of"],
            serde_json::json!("Gmail"),
            "a sub-account should name the account it belongs to"
        );
        // A top-level account is part of nothing, and must say so as null rather
        // than as an empty string.
        let parent = rows
            .iter()
            .find(|r| r["Service"] == serde_json::json!("Gmail"))
            .expect("the parent account is present");
        assert_eq!(parent["Part of"], serde_json::Value::Null);

        // Cocoa epoch, not Unix: 726000000 Cocoa = 1704307200 Unix (2024-01-03).
        assert_eq!(parent["Added"], serde_json::json!(1_704_307_200_i64));

        // NULL flags stay distinguishable from "no".
        let unrecorded = rows
            .iter()
            .find(|r| r["Label"] == serde_json::json!("Unrecorded"))
            .expect("the NULL-flag account is present");
        assert_eq!(unrecorded["Status"], serde_json::json!("Not recorded"));
        assert_eq!(unrecorded["Signed in"], serde_json::json!("Not recorded"));
    }

    /// The Wi-Fi module — the first that reads a property list rather than SQL.
    ///
    /// Covers what the plist source has to get right: the row identity coming from
    /// the entry's KEY, a namespace prefix trimmed only when it is really there, a
    /// nested path into a subtree, a subtree that is absent on some rows, and a
    /// plist Date arriving as an absolute time with no epoch declared.
    #[test]
    fn wifi_module_reads_a_property_list() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "wifi_networks")
            .expect("wifi_networks module ships");
        assert!(spec.plist.is_some(), "this module reads a plist, not SQL");
        assert!(spec.sql.is_empty());

        let tmp = tempfile::tempdir().unwrap();
        make_backup_bytes_in(tmp.path(), &spec.domain, &spec.path, &seed_wifi_networks());
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 3);

        let by = |name: &str| {
            rows.iter()
                .find(|r| r["Network"] == serde_json::json!(name))
                .unwrap_or_else(|| panic!("no row for {name:?}, got {rows:#?}"))
        };

        // The name came from the KEY, with Apple's namespace trimmed off.
        let home = by("HomeNet");
        assert_eq!(home["Security"], serde_json::json!("WPA2 Personal"));
        // A nested path reached into __OSSpecific__.
        assert_eq!(home["Access point"], serde_json::json!("6a:22:32:98:f4:df"));
        assert_eq!(home["Channel"], serde_json::json!(153));
        assert_eq!(home["Hidden"], serde_json::json!(false));
        // A plist Date is absolute: no epoch is declared, and none is needed.
        assert_eq!(home["Joined by user"], serde_json::json!(1_688_243_921_i64));
        // Each date column asserted, so swapping AddedAt for LastDiscoveredAt
        // cannot pass — they are adjacent keys of the same type in the same dict,
        // which is exactly the mix-up nothing else would catch.
        assert_eq!(home["Added"], serde_json::json!(1_688_243_920_i64));
        assert_eq!(home["Last seen"], serde_json::json!(1_689_450_218_i64));
        assert_eq!(
            home["Joined automatically"],
            serde_json::json!(1_689_450_000_i64)
        );

        // A key with a SPACE survives, and its missing __OSSpecific__ subtree
        // yields nulls rather than failing the artifact.
        let cafe = by("Cafe Wifi");
        assert_eq!(cafe["Hidden"], serde_json::json!(true));
        assert_eq!(cafe["Access point"], serde_json::Value::Null);
        assert_eq!(cafe["Channel"], serde_json::Value::Null);
        // Absent key, not a false date.
        assert_eq!(cafe["Joined by user"], serde_json::Value::Null);
        // Never auto-joined either — and that is a null, not a zero date.
        assert_eq!(cafe["Joined automatically"], serde_json::Value::Null);
        assert_eq!(cafe["Added"], serde_json::json!(1_700_000_000_i64));

        // A key WITHOUT the namespace is shown whole. Trimming blindly would have
        // silently mangled it, and a store whose shape changed is exactly what
        // this must not hide.
        let legacy = by("legacy-entry");
        assert_eq!(legacy["Security"], serde_json::Value::Null);
    }

    /// A module can read an NSKEYEDARCHIVER archive, not only a plain plist.
    ///
    /// Apple wraps a great deal of structured data this way, and an archive is not
    /// a plist with the data at nameable paths — it is a FLATTENED OBJECT GRAPH.
    /// Read raw it presents `$version` / `$archiver` / `$top` / `$objects`, and
    /// every real key sits behind a UID reference. A module could not name a path
    /// into it at all.
    ///
    /// This builds a genuine archive by hand — the same shape the device writes,
    /// with UID references into `$objects` — and runs a module over it. Building
    /// one rather than resolving a captured blob is the point: it fails if the
    /// runner ever stops resolving, which a fixture of already-plain values could
    /// not detect.
    #[test]
    fn a_module_can_read_a_keyed_archive() {
        use plist::{Dictionary, Uid, Value};

        // $objects[0] is the "$null" sentinel; the rest are the real objects,
        // referenced by index.
        let mut record = Dictionary::new();
        record.insert(
            "NS.keys".into(),
            Value::Array(vec![Value::Uid(Uid::new(3))]),
        );
        record.insert(
            "NS.objects".into(),
            Value::Array(vec![Value::Uid(Uid::new(4))]),
        );
        let mut class = Dictionary::new();
        class.insert("$classname".into(), Value::String("NSDictionary".into()));
        class.insert(
            "$classes".into(),
            Value::Array(vec![
                Value::String("NSDictionary".into()),
                Value::String("NSObject".into()),
            ]),
        );
        record.insert("$class".into(), Value::Uid(Uid::new(5)));

        let objects = Value::Array(vec![
            Value::String("$null".into()),          // 0
            Value::Dictionary(record),              // 1 — the root object
            Value::String("unused".into()),         // 2
            Value::String("deviceName".into()),     // 3 — a key
            Value::String("Example iPhone".into()), // 4 — its value
            Value::Dictionary(class),               // 5
        ]);
        let mut top = Dictionary::new();
        top.insert("root".into(), Value::Uid(Uid::new(1)));

        let mut archive = Dictionary::new();
        archive.insert("$version".into(), Value::Integer(100_000.into()));
        archive.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        archive.insert("$top".into(), Value::Dictionary(top));
        archive.insert("$objects".into(), objects);
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, &Value::Dictionary(archive)).unwrap();

        // Read as a raw plist this would have no `deviceName` anywhere — only
        // `$objects`, `$top` and a UID.
        let raw = plist::Value::from_reader(std::io::Cursor::new(&bytes)).unwrap();
        assert!(
            raw.as_dictionary().unwrap().contains_key("$objects"),
            "the fixture is not actually an archive"
        );

        let spec: ModuleSpec = toml::from_str(
            r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.plist"

[plist]
# NOT rows = ["root"]: a sole `$top` root is UNWRAPPED by the resolver, so paths
# start inside the root object. An archive with several roots keeps them.

[[columns]]
name = "Device"
from = "deviceName"
"#,
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        make_backup_bytes_in(tmp.path(), "HomeDomain", "a/b.plist", &bytes);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(&spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["Device"], serde_json::json!("Example iPhone"));
    }

    /// The shapes a `[plist]` module can point at, and what each must do.
    ///
    /// Every one of these passed as "success" in the first version: a scalar
    /// became one row of nulls, a nested single record became one null row per
    /// field, an empty dictionary became a phantom row, and a `key_column` over an
    /// array gave every row a blank key. They load and validate — only running
    /// them tells them apart, which is why they are pinned here.
    #[test]
    fn plist_row_containers_are_read_as_declared() {
        use plist::{Dictionary, Value};

        fn store(v: Value) -> Vec<u8> {
            let mut out = Vec::new();
            plist::to_writer_binary(&mut out, &v).unwrap();
            out
        }
        fn run(toml_src: &str, bytes: &[u8]) -> Result<Option<Vec<ArtifactRow>>> {
            let spec: ModuleSpec = toml::from_str(toml_src).unwrap();
            let tmp = tempfile::tempdir().unwrap();
            make_backup_bytes_in(tmp.path(), "HomeDomain", "a/b.plist", bytes);
            let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
            let work = tmp.path().join("work");
            run_module(&spec, &index, None, &work)
        }
        let base = |extra: &str| {
            format!(
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.plist"

[plist]
{extra}

[[columns]]
name = "A"
from = "a"
"#
            )
        };
        let keyed = |extra: &str| {
            format!(
                r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.plist"

[plist]
key_column = "K"
{extra}

[[columns]]
name = "K"

[[columns]]
name = "A"
from = "a"
"#
            )
        };

        // A `rows` path onto a SCALAR is a mistake, not one empty row.
        let mut root = Dictionary::new();
        root.insert("scalar".into(), Value::String("nope".into()));
        let err = run(
            &base("rows = [\"scalar\"]"),
            &store(Value::Dictionary(root)),
        )
        .expect_err("a scalar holds no rows");
        assert!(err.to_string().contains("a string"), "{err}");

        // A dictionary with no `key_column` is ONE record — not one row per field.
        let mut inner = Dictionary::new();
        inner.insert("a".into(), Value::String("value".into()));
        inner.insert("b".into(), Value::String("other".into()));
        let mut root = Dictionary::new();
        root.insert("Settings".into(), Value::Dictionary(inner));
        let rows = run(
            &base("rows = [\"Settings\"]"),
            &store(Value::Dictionary(root)),
        )
        .unwrap()
        .unwrap();
        assert_eq!(rows.len(), 1, "a nested record is one row, got {rows:#?}");
        assert_eq!(rows[0]["A"], serde_json::json!("value"));

        // An EMPTY dictionary recorded nothing. One row of nulls would claim a
        // record exists.
        let rows = run(&base(""), &store(Value::Dictionary(Dictionary::new())))
            .unwrap()
            .unwrap();
        assert!(rows.is_empty(), "an empty store is no rows, got {rows:#?}");

        // A `key_column` over an ARRAY would give every row a blank key.
        let arr = Value::Array(vec![Value::Dictionary(Dictionary::new())]);
        let mut root = Dictionary::new();
        root.insert("items".into(), arr);
        let err = run(
            &keyed("rows = [\"items\"]"),
            &store(Value::Dictionary(root)),
        )
        .expect_err("an array has no keys");
        assert!(err.to_string().contains("ARRAY"), "{err}");

        // Descending THROUGH an array by index, which a dictionary-only walk made
        // impossible and which Apple's stores need routinely.
        let mut rec = Dictionary::new();
        rec.insert("a".into(), Value::String("deep".into()));
        let mut root = Dictionary::new();
        root.insert("items".into(), Value::Array(vec![Value::Dictionary(rec)]));
        let rows = run(
            &base("rows = [\"items\", \"0\"]"),
            &store(Value::Dictionary(root)),
        )
        .unwrap()
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["A"], serde_json::json!("deep"));
    }

    /// `Data` is text only when it really is text, and a timestamp column that
    /// meets a bare number with no declared epoch says so rather than nulling.
    #[test]
    fn plist_values_convert_or_say_why_not() {
        let text = ColumnSpec {
            name: "T".into(),
            from: vec!["t".into()],
            kind: ColumnKind::Text,
            value: None,
            epoch: None,
            map: None,
        };
        let utf8 = plist::Value::Data(b"HomeNet".to_vec());
        assert_eq!(
            convert_plist(&utf8, &text).unwrap(),
            serde_json::json!("HomeNet")
        );
        // Multi-line text is ordinary and must survive.
        let lines = plist::Value::Data(b"one\ntwo".to_vec());
        assert_eq!(
            convert_plist(&lines, &text).unwrap(),
            serde_json::json!("one\ntwo")
        );
        // Not UTF-8: NULL, matching what the SQL path does with a blob. A
        // fabricated "<4 bytes>" would flow into the store and search as content.
        let binary = plist::Value::Data(vec![0xff, 0xfe, 0x00, 0x41]);
        assert_eq!(
            convert_plist(&binary, &text).unwrap(),
            serde_json::Value::Null
        );

        // A u64 above i64::MAX — Apple writes persistent ids this way — must not
        // vanish.
        let big = plist::Value::Integer(plist::Integer::from(u64::MAX));
        assert_eq!(
            convert_plist(&big, &text).unwrap(),
            serde_json::json!(u64::MAX.to_string())
        );

        // `hex` is what makes an identifier stored as bytes reachable at all —
        // `text` correctly gives null for it.
        let hex = ColumnSpec {
            name: "H".into(),
            from: vec!["h".into()],
            kind: ColumnKind::Hex,
            value: None,
            epoch: None,
            map: None,
        };
        let mac = plist::Value::Data(vec![0x8a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f]);
        assert_eq!(
            convert_plist(&mac, &hex).unwrap(),
            serde_json::json!("8a:1b:2c:3d:4e:5f")
        );
        assert_eq!(convert_plist(&mac, &text).unwrap(), serde_json::Value::Null);
        // The same idea from SQLite: a BLOB.
        assert_eq!(
            convert(&rusqlite::types::Value::Blob(vec![0x00, 0xff]), &hex),
            serde_json::json!("00:ff")
        );

        // A timestamp column meeting a number with no epoch: an error naming the
        // column, not a column of silent nulls.
        let ts = ColumnSpec {
            name: "When".into(),
            from: vec!["w".into()],
            kind: ColumnKind::Timestamp,
            value: None,
            epoch: None,
            map: None,
        };
        let err = convert_plist(&plist::Value::Integer(1_700_000_000.into()), &ts)
            .expect_err("a number timestamp with no epoch is unreadable");
        assert!(err.to_string().contains("When"), "{err}");
        assert!(err.to_string().contains("epoch"), "{err}");

        // A Date needs no epoch, and goes through the same plausibility clamp as
        // every other timestamp — an absurd value is null, not a number the UI
        // will throw on.
        let d = plist::Value::Date(plist::Date::from(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
        ));
        assert_eq!(
            convert_plist(&d, &ts).unwrap(),
            serde_json::json!(1_700_000_000_i64)
        );
    }

    /// The two plist modules added alongside `wifi_networks`: one keyed by MAC
    /// with three different names, one whose rows are an ARRAY and whose headline
    /// value is six raw bytes.
    #[test]
    fn the_bluetooth_and_private_mac_modules_read_their_stores() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let run = |id: &str, bytes: Vec<u8>| {
            let spec = mods.iter().find(|m| m.id == id).expect("module ships");
            let tmp = tempfile::tempdir().unwrap();
            make_backup_bytes_in(tmp.path(), &spec.domain, &spec.path, &bytes);
            let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
            run_module(spec, &index, None, &tmp.path().join("work"))
                .unwrap()
                .unwrap()
        };

        let bt = run("bluetooth_devices", seed_bluetooth_devices());
        assert_eq!(bt.len(), 3);
        let alex = bt
            .iter()
            .find(|r| r["Address"] == serde_json::json!("08:65:18:75:5E:75"))
            .expect("keyed by address");
        // The three names are three different facts and must not collapse.
        assert_eq!(alex["Named by owner"], serde_json::json!("Alex's AirPods"));
        assert_eq!(alex["Device name"], serde_json::json!("AirPods 3"));
        assert_eq!(alex["Kind"], serde_json::json!("Headphones"));
        // A device the owner never renamed: null, NOT the model name. Falling back
        // would invent a name the store does not have.
        let watch = bt
            .iter()
            .find(|r| r["Device name"] == serde_json::json!("Apple Watch"))
            .unwrap();
        assert_eq!(watch["Named by owner"], serde_json::Value::Null);

        let wifi = run("wifi_private_mac", seed_wifi_private_mac());
        assert_eq!(wifi.len(), 2, "rows come from the array");
        let home = wifi
            .iter()
            .find(|r| r["Network"] == serde_json::json!("HomeNet"))
            .unwrap();
        // Six raw bytes, nested a level down, rendered the way a MAC is written.
        assert_eq!(
            home["Private address"],
            serde_json::json!("8a:1b:2c:3d:4e:5f")
        );
        assert_eq!(home["Address valid"], serde_json::json!(true));
        assert_eq!(home["Last joined"], serde_json::json!(1_689_450_273_i64));

        // Bytes present but marked invalid: both shown, so the flag can qualify
        // the address rather than the address implying it was used.
        let cafe = wifi
            .iter()
            .find(|r| r["Network"] == serde_json::json!("Cafe Wifi"))
            .unwrap();
        assert_eq!(cafe["Address valid"], serde_json::json!(false));
        assert_eq!(
            cafe["Private address"],
            serde_json::json!("00:11:22:33:44:55")
        );
        assert_eq!(cafe["Open network"], serde_json::json!(true));
        assert_eq!(cafe["Still known"], serde_json::json!(false));
    }

    /// A `rows` path that does not exist must FAIL, not report an empty artifact.
    /// "this key is gone" and "this device has none of these" are different facts,
    /// and only the first is a bug worth chasing.
    #[test]
    fn a_missing_plist_rows_path_is_an_error_not_an_empty_result() {
        let spec: ModuleSpec = toml::from_str(
            r#"
id = "x"
name = "X"
description = "X."
surface = "standalone"
domain = "HomeDomain"
path = "a/b.plist"

[plist]
rows = ["NoSuchKey"]

[[columns]]
name = "A"
from = "a"
"#,
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        make_backup_bytes_in(tmp.path(), "HomeDomain", "a/b.plist", &seed_wifi_networks());
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let err = run_module(&spec, &index, None, &tmp.path().join("work"))
            .expect_err("a missing rows path should be an error");
        let msg = err.to_string();
        assert!(msg.contains("NoSuchKey"), "unhelpful error: {msg}");
        assert!(msg.contains("plist.rows"), "unhelpful error: {msg}");
    }

    /// The SIM module: the ICCID and the number come from the columns that really
    /// hold them, and the Cocoa timestamp converts.
    ///
    /// The names are the trap here — `subscriber_id` sounds like a subscriber and
    /// is a card serial; `subscriber_mdn` is the phone number. Reading them the
    /// other way round would produce two plausible-looking strings in the wrong
    /// columns, which no schema check would catch.
    #[test]
    fn sim_cards_module_reads_the_iccid_and_the_number() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "sim_cards")
            .expect("sim_cards module ships");
        let tmp = tempfile::tempdir().unwrap();
        make_backup_in(tmp.path(), &spec.domain, &spec.path, seed_sim_cards);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 2, "both SIMs, ordered by slot");
        assert_eq!(rows[0]["Slot"], serde_json::json!(1));
        assert_eq!(rows[1]["Slot"], serde_json::json!(2));
        // An ICCID is 19-20 digits; a phone number starts with '+'. Asserting the
        // shape of each catches the two columns being swapped, which is the whole
        // risk in this store.
        let iccid = rows[0]["SIM serial (ICCID)"].as_str().unwrap();
        let number = rows[0]["Phone number"].as_str().unwrap();
        assert_eq!(iccid, "8901260971148676693");
        assert!(
            iccid.chars().all(|c| c.is_ascii_digit()) && iccid.len() >= 18,
            "the ICCID column does not hold an ICCID: {iccid:?}"
        );
        assert!(
            number.starts_with('+'),
            "the phone-number column does not hold a number: {number:?}"
        );
        // Cocoa, not Unix: 726000000 -> 1704307200.
        assert_eq!(
            rows[0]["Last updated"],
            serde_json::json!(1_704_307_200_i64)
        );
    }

    /// A `highlight` that could never fire must be rejected at load time, not
    /// silently show nothing — "this artifact has nothing to highlight" and "this
    /// module's highlight is broken" look identical on screen.
    #[test]
    fn a_highlight_that_cannot_match_is_rejected() {
        let base = r#"
id          = "h"
name        = "H"
description = "d"
surface     = "standalone"
domain      = "HomeDomain"
path        = "Library/X/x.db"
sql         = ["SELECT a AS a, b AS b FROM t"]

[[columns]]
name = "A"
from = "a"

[[columns]]
name = "B"
from = "b"
"#;
        let cases: &[(&str, &str)] = &[
            // A column that does not exist.
            (
                "[highlight]
column = \"Nope\"
",
                "highlight.column",
            ),
            // A condition column that does not exist.
            (
                "[highlight]
column = \"A\"
when_column = \"Nope\"
when_any_of = [\"x\"]
",
                "highlight.when_column",
            ),
            // A condition with nothing to match against: never fires.
            (
                "[highlight]
column = \"A\"
when_column = \"B\"
when_any_of = []
",
                "when_any_of",
            ),
            // Values with no column to compare them to.
            (
                "[highlight]
column = \"A\"
when_any_of = [\"x\"]
",
                "when_column",
            ),
        ];
        for (extra, expect) in cases {
            let spec: ModuleSpec = toml::from_str(&format!(
                "{base}
{extra}"
            ))
            .unwrap();
            let err = spec
                .validate()
                .expect_err(&format!("should have been rejected: {extra}"));
            assert!(
                err.contains(expect),
                "error {err:?} does not mention {expect:?}"
            );
        }

        // And the valid shape loads.
        let ok: ModuleSpec = toml::from_str(&format!(
            "{base}
[highlight]
column = \"A\"
when_column = \"B\"
when_any_of = [\"yes\"]
"
        ))
        .unwrap();
        ok.validate().expect("a well-formed highlight should load");
    }

    /// The data-usage module: buckets are summed per app, the rollup row is
    /// excluded, and a process name without a slash survives intact.
    #[test]
    fn data_usage_module_aggregates_and_excludes_the_rollup() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "data_usage")
            .expect("data_usage module ships");
        let tmp = tempfile::tempdir().unwrap();
        make_backup_in(tmp.path(), &spec.domain, &spec.path, seed_data_usage);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();

        // Three apps, not four: the rollup has no bundle id and must not appear.
        assert_eq!(rows.len(), 3, "got {rows:#?}");
        assert!(
            !rows
                .iter()
                .any(|r| r["Carried by"] == serde_json::json!("CumulativeUsageTracker")),
            "the device-wide rollup was included — every figure would double"
        );

        let chat = rows
            .iter()
            .find(|r| r["App"] == serde_json::json!("com.example.chatapp"))
            .expect("the chat app is present");
        // Two buckets summed: 3000+700 down, 4000+800 up, 1000+500 / 2000+600 wifi.
        assert_eq!(chat["Cellular down"], serde_json::json!(3700));
        assert_eq!(chat["Cellular up"], serde_json::json!(4800));
        assert_eq!(chat["Wi-Fi down"], serde_json::json!(1500));
        assert_eq!(chat["Wi-Fi up"], serde_json::json!(2600));
        assert_eq!(chat["Records"], serde_json::json!(2));
        // The daemon, split off the compound ZPROCNAME.
        assert_eq!(chat["Carried by"], serde_json::json!("ChatApp"));
        // Cocoa epoch on both ends of the range: 726000000 -> 1704307200.
        assert_eq!(chat["First"], serde_json::json!(1_704_307_200_i64));
        assert_eq!(chat["Last"], serde_json::json!(1_704_308_200_i64));

        // A daemon carrying traffic for an app it is not: the interesting case.
        let photos = rows
            .iter()
            .find(|r| r["App"] == serde_json::json!("com.example.photos"))
            .expect("the photos app is present");
        assert_eq!(photos["Carried by"], serde_json::json!("nsurlsessiond"));

        // No slash in ZPROCNAME: the substr must not eat the whole name.
        let plain = rows
            .iter()
            .find(|r| r["App"] == serde_json::json!("com.example.plain"))
            .expect("the plain-named process is present");
        assert_eq!(plain["Carried by"], serde_json::json!("plainproc"));
    }

    /// The older DataUsage lineage has no Wi-Fi columns at all. The fallback query
    /// must run, and must report Wi-Fi as UNRECORDED rather than as zero traffic —
    /// "we cannot know" and "none" are different claims.
    #[test]
    fn data_usage_falls_back_when_the_wifi_columns_are_absent() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods.iter().find(|m| m.id == "data_usage").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        make_backup_in(tmp.path(), &spec.domain, &spec.path, |c| {
            c.execute_batch(
                "CREATE TABLE ZLIVEUSAGE (
                    Z_PK INTEGER PRIMARY KEY, ZKIND INTEGER, ZHASPROCESS INTEGER,
                    ZTIMESTAMP TIMESTAMP, ZWWANIN FLOAT, ZWWANOUT FLOAT);
                 CREATE TABLE ZPROCESS (
                    Z_PK INTEGER PRIMARY KEY, ZBUNDLENAME VARCHAR, ZPROCNAME VARCHAR);
                 INSERT INTO ZPROCESS (Z_PK, ZBUNDLENAME, ZPROCNAME)
                    VALUES (1,'com.example.old','OldApp/com.example.old');
                 INSERT INTO ZLIVEUSAGE (Z_PK, ZKIND, ZHASPROCESS, ZTIMESTAMP, ZWWANIN, ZWWANOUT)
                    VALUES (1,0,«redacted»000000,1234,5678);",
            )
            .unwrap();
        });
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .expect("the fallback query should have run");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["Cellular down"], serde_json::json!(1234));
        assert_eq!(rows[0]["Cellular up"], serde_json::json!(5678));
        // NOT 0: this device never recorded Wi-Fi, which is not the same as
        // recording that no Wi-Fi was used.
        assert_eq!(rows[0]["Wi-Fi down"], serde_json::Value::Null);
        assert_eq!(rows[0]["Wi-Fi up"], serde_json::Value::Null);
    }

    /// The Bluetooth module: the two counters stay integers, and the resolved
    /// address is kept apart from the advertised one.
    #[test]
    fn bluetooth_module_keeps_counters_as_numbers_and_addresses_apart() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "bluetooth_paired")
            .expect("bluetooth module ships");
        let tmp = tempfile::tempdir().unwrap();
        // `make_backup` defaults to HomeDomain; this store lives in a system
        // container, and a domain mismatch shows up as the module simply finding
        // nothing.
        make_backup_in(tmp.path(), &spec.domain, &spec.path, seed_bluetooth_paired);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 3);

        let watch = rows
            .iter()
            .find(|r| r["Device"] == serde_json::json!("Example Watch"))
            .expect("the watch is present");
        // The counters must arrive as NUMBERS and be left exactly as stored. If
        // either were ever declared a timestamp these would come back converted,
        // and the module would be printing invented dates.
        assert_eq!(watch["Connection counter"], serde_json::json!(9639));
        assert_eq!(watch["Seen counter"], serde_json::json!(4_315_986));
        // A rotating Random address resolving to a different Public one is the
        // pair worth showing; collapsing the columns would lose it.
        assert_eq!(
            watch["Address"],
            serde_json::json!("Random 50:32:66:45:35:EF")
        );
        assert_eq!(
            watch["Resolves to"],
            serde_json::json!("Public F8:6F:C1:4E:FF:6A")
        );
        // An unresolved address stays null rather than repeating the advertised one.
        let tag = rows
            .iter()
            .find(|r| r["Device"] == serde_json::json!("Nameless Tag"))
            .expect("the tag is present");
        assert_eq!(tag["Resolves to"], serde_json::Value::Null);
    }

    /// The TCC module against the fixture: the decisions it maps, the date it
    /// converts, and the value it refuses to guess at.
    #[test]
    fn tcc_module_reads_permissions() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods
            .iter()
            .find(|m| m.id == "tcc")
            .expect("tcc module ships");
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), &spec.path, seed_tcc);
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 6);

        let find = |app: &str, svc: &str| {
            rows.iter()
                .find(|r| {
                    r["App"] == serde_json::json!(app) && r["Permission"] == serde_json::json!(svc)
                })
                .unwrap_or_else(|| panic!("no row for {app}/{svc}"))
        };
        assert_eq!(
            find("com.example.chatapp", "Camera")["Decision"],
            serde_json::json!("Allowed")
        );
        assert_eq!(
            find("com.example.weather", "kTCCServiceLocation")["Decision"],
            serde_json::json!("Denied")
        );
        assert_eq!(
            find("com.example.chatapp", "Photos")["Decision"],
            serde_json::json!("Limited")
        );
        // `kTCCServiceLocation` and `kTCCServiceContacts` stay RAW on purpose:
        // they are not in the module's mapping, and an unmapped code must show
        // as itself rather than be given an invented label. Ugly and true beats
        // tidy and possibly wrong.
        //
        // 1 is a documented state (kTCCAuthValueUnknown — never prompted), not a
        // decoding failure, and must not read as one.
        assert_eq!(
            find("com.example.notprompted", "kTCCServiceContacts")["Decision"],
            serde_json::json!("Not decided")
        );
        // NULL passes through as "not recorded". A `CASE auth_value WHEN` form
        // could not do this — `'x' || NULL` is NULL in SQLite, so the cell would
        // come back empty and look like a parsing bug.
        assert_eq!(
            find("com.example.norecord", "Microphone")["Decision"],
            serde_json::json!("Not recorded")
        );
        // A value outside Apple's enum is surfaced as-is rather than guessed at.
        assert_eq!(
            find("com.example.todo", "Reminders")["Decision"],
            serde_json::json!("Unrecognised (9)")
        );
        // The date arrives as a date.
        assert_eq!(
            find("com.example.chatapp", "Camera")["Decided"],
            serde_json::json!(1_700_000_000_i64)
        );
    }

    /// iOS 12/13: `allowed` instead of `auth_value`, but `last_modified` is
    /// still there. The two schema changes happened independently, and
    /// collapsing them into one branch silently dropped the date from every
    /// permission on those devices — a review caught it. "When was this
    /// granted" is the more interesting half of the artifact.
    #[test]
    fn tcc_keeps_the_date_on_the_middle_schema() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods.iter().find(|m| m.id == "tcc").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        make_backup(tmp.path(), &spec.path, |c| {
            c.execute_batch(
                "CREATE TABLE access (service TEXT, client TEXT, allowed INTEGER,
                                      prompt_count INTEGER, last_modified INTEGER);",
            )
            .unwrap();
            c.execute_batch(
                "INSERT INTO access VALUES ('kTCCServiceCamera','com.mid.app',1,«redacted»0000000);",
            )
            .unwrap();
        });
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows[0]["Decision"], serde_json::json!("Allowed"));
        assert_eq!(
            rows[0]["Decided"],
            serde_json::json!(1_700_000_000_i64),
            "the date exists on this schema and must not be thrown away"
        );
    }

    /// The older-iOS alternative runs when the modern one cannot prepare.
    /// This is the whole reason `sql` accepts a list.
    #[test]
    fn tcc_falls_back_to_the_older_schema() {
        let mods = load_modules(&builtin_modules_dir()).unwrap();
        let spec = mods.iter().find(|m| m.id == "tcc").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // A pre-auth_value device: `allowed`, and no `last_modified` at all.
        make_backup(tmp.path(), &spec.path, |c| {
            c.execute_batch("CREATE TABLE access (service TEXT, client TEXT, allowed INTEGER);")
                .unwrap();
            c.execute_batch(
                "INSERT INTO access VALUES ('kTCCServiceCamera','com.old.app',1),
                                           ('kTCCServicePhotos','com.old.app',0);",
            )
            .unwrap();
        });
        let index = ManifestIndex::open(tmp.path(), None, tmp.path()).unwrap();
        let rows = run_module(spec, &index, None, &tmp.path().join("work"))
            .unwrap()
            .unwrap();
        assert_eq!(rows.len(), 2, "the older-schema query must have run");
        assert_eq!(rows[0]["Decision"], serde_json::json!("Allowed"));
        assert_eq!(rows[1]["Decision"], serde_json::json!("Denied"));
        // No date exists on this schema, and that reads as absent, not as 1970.
        assert_eq!(rows[0]["Decided"], serde_json::Value::Null);
    }

    /// The listing distinguishes "not in this backup" from "cannot be in this
    /// backup".
    ///
    /// An artifact with no rows is normally omitted — a list of empty tables is
    /// not navigation. An artifact gated on encryption is kept even when empty,
    /// so it can explain itself instead of vanishing (#197). Dropping it would
    /// make an unencrypted backup look like a device that never had the data.
    #[test]
    fn a_gated_artifact_is_listed_even_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let conn = db.conn();

        // Every shipped module, with no rows stored for any of them.
        let listed = list_artifacts(conn).unwrap();
        let specs = builtin_modules().unwrap();
        for spec in &specs {
            let present = listed.iter().any(|a| a.id == spec.id);
            if spec.needs_encrypted_backup() {
                assert!(
                    present,
                    "gated artifact {} must stay listed so it can explain itself",
                    spec.id
                );
                let a = listed.iter().find(|a| a.id == spec.id).unwrap();
                assert!(a.requires_encrypted_backup, "and must say it is gated");
                assert_eq!(a.row_count, 0);
            } else {
                assert!(
                    !present,
                    "empty non-gated artifact {} must not clutter the list",
                    spec.id
                );
            }
        }

        // Once it has rows it is listed for the ordinary reason.
        let mut row: ArtifactRow = HashMap::new();
        row.insert("App".into(), serde_json::json!("com.example"));
        store_rows(conn, &specs[0].id, &[row]).unwrap();
        let listed = list_artifacts(conn).unwrap();
        let a = listed.iter().find(|a| a.id == specs[0].id).unwrap();
        assert_eq!(a.row_count, 1);
        assert_eq!(a.columns, vec!["App", "Permission", "Decision", "Decided"]);
    }

    /// A module declaring an unknown precondition is rejected, so a typo in
    /// `requires` cannot silently mean "no precondition at all".
    #[test]
    fn a_gated_module_round_trips_through_the_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let mods_dir = tmp.path().join("modules");
        std::fs::create_dir_all(&mods_dir).unwrap();
        write_module(
            &mods_dir,
            "gated.toml",
            r#"
id = "gated"
description = "gated artifact."
surface = "standalone"
name = "Gated thing"
domain = "HomeDomain"
path = "Library/Nowhere/x.db"
requires = "encrypted-backup"
sql = "SELECT a FROM t"
[[columns]]
name = "A"
from = "a"
"#,
        );
        let spec = &load_modules(&mods_dir).unwrap()[0];
        assert!(spec.needs_encrypted_backup());
    }

    /// The three states the Artifacts view has to tell apart.
    ///
    /// A bare "have we extracted" boolean would pass the first two and fail the
    /// third — which is the case that bites a user who updates the app.
    #[test]
    fn extraction_state_distinguishes_never_run_stale_and_current() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::cache::CacheDb::open(&tmp.path().join("cache.db")).unwrap();
        let specs = builtin_modules().unwrap();

        // A cache imported before any module existed.
        assert_eq!(extraction_state(&db).unwrap(), ExtractionState::NeverRun);

        // Extracted with a set that is not the current one — e.g. the app has
        // gained a module since.
        db.set_meta(EXTRACTED_MODULES_KEY, "something,else")
            .unwrap();
        assert_eq!(extraction_state(&db).unwrap(), ExtractionState::Stale);

        // Extracted with exactly what is installed now.
        mark_extracted(&db, &specs).unwrap();
        assert_eq!(extraction_state(&db).unwrap(), ExtractionState::UpToDate);

        // Adding a module makes an up-to-date cache stale again — the case a
        // boolean cannot see.
        let mut more = specs.clone();
        more.push(ModuleSpec {
            id: "later_addition".into(),
            name: "Later".into(),
            description: "A test artifact.".into(),
            surface: Surface::Standalone,
            join_column: None,
            highlight: None,
            category: None,
            domain: "HomeDomain".into(),
            path: "a/b.db".into(),
            shape: Shape::Table,
            verified: None,
            path_column: None,
            app_column: None,
            plist: None,
            log: None,
            sql: vec!["SELECT a FROM t".into()],
            requires: None,
            columns: vec![ColumnSpec {
                name: "A".into(),
                from: vec!["a".into()],
                kind: ColumnKind::Text,
                value: None,
                epoch: None,
                map: None,
            }],
        });
        assert_ne!(
            module_set_fingerprint(&specs),
            module_set_fingerprint(&more),
            "the fingerprint must change when a module is added"
        );
    }

    /// The fingerprint is order-independent, so a reshuffled BUILTIN list does
    /// not make every cache look stale.
    #[test]
    fn module_set_fingerprint_ignores_order() {
        let mk = |id: &str| ModuleSpec {
            id: id.into(),
            name: id.into(),
            description: "A test artifact.".into(),
            surface: Surface::Standalone,
            join_column: None,
            highlight: None,
            category: None,
            domain: "HomeDomain".into(),
            path: "a/b.db".into(),
            shape: Shape::Table,
            verified: None,
            path_column: None,
            app_column: None,
            plist: None,
            log: None,
            sql: vec!["SELECT a FROM t".into()],
            requires: None,
            columns: vec![ColumnSpec {
                name: "A".into(),
                from: vec!["a".into()],
                kind: ColumnKind::Text,
                value: None,
                epoch: None,
                map: None,
            }],
        };
        assert_eq!(
            module_set_fingerprint(&[mk("a"), mk("b")]),
            module_set_fingerprint(&[mk("b"), mk("a")])
        );
    }

    /// A module cannot skip declaring where it belongs, or explaining itself.
    ///
    /// Both are required rather than defaulted, because the agreed rule — fold
    /// into the view closest in meaning, own destination only for genuinely
    /// outstanding data — was drifted from within one slice of shipping it. A
    /// default would let the next author drift the same way silently.
    #[test]
    fn a_module_must_declare_its_surface_and_describe_itself() {
        let base = |extra: &str| {
            format!(
                r#"
id = "x"
name = "X"
domain = "HomeDomain"
path = "a/b.db"
sql = "SELECT a FROM t"
{extra}
[[columns]]
name = "A"
from = "a"
"#
            )
        };
        let tmp = tempfile::tempdir().unwrap();

        // No surface at all.
        write_module(tmp.path(), "nosurface.toml", &base("description = \"X.\""));
        let err = load_modules(tmp.path())
            .expect_err("missing surface must fail")
            .to_string();
        assert!(err.contains("surface"), "{err}");

        // No description.
        let tmp2 = tempfile::tempdir().unwrap();
        write_module(tmp2.path(), "nodesc.toml", &base("surface = \"apps\""));
        let err = load_modules(tmp2.path())
            .expect_err("missing description must fail")
            .to_string();
        assert!(err.contains("description"), "{err}");

        // An empty description is as useless as none.
        let tmp3 = tempfile::tempdir().unwrap();
        write_module(
            tmp3.path(),
            "blankdesc.toml",
            &base("surface = \"apps\"\ndescription = \"   \""),
        );
        let err = load_modules(tmp3.path())
            .expect_err("blank description must fail")
            .to_string();
        assert!(err.contains("say in one sentence"), "{err}");

        // A typo'd surface must not silently mean "nowhere".
        let tmp4 = tempfile::tempdir().unwrap();
        write_module(
            tmp4.path(),
            "badsurface.toml",
            &base("surface = \"aps\"\ndescription = \"X.\""),
        );
        let err = load_modules(tmp4.path())
            .expect_err("unknown surface must fail")
            .to_string();
        assert!(err.contains("surface"), "{err}");
    }

    /// Every shipped module folds into an existing view. The moment one declares
    /// `standalone`, that is a deliberate claim it fits nowhere — and this test
    /// failing is the prompt to check that claim rather than to update the list.
    #[test]
    fn shipped_modules_fold_into_existing_views() {
        for spec in builtin_modules().unwrap() {
            assert_ne!(
                spec.surface,
                Surface::Standalone,
                "module {} claims it fits nowhere — is that true, or is there a \
                 view closer in meaning?",
                spec.id
            );
            assert!(
                spec.description.len() > 20,
                "module {}'s description is too short to explain anything",
                spec.id
            );
        }
    }

    #[test]
    fn requires_is_parsed() {
        let tmp = tempfile::tempdir().unwrap();
        write_module(
            tmp.path(),
            "enc.toml",
            &spec_toml("\nrequires = \"encrypted-backup\"\n"),
        );
        // `requires` must sit at the top level, not inside the last [[columns]].
        let body = std::fs::read_to_string(tmp.path().join("enc.toml")).unwrap();
        let fixed = body.replace("requires = \"encrypted-backup\"", "");
        std::fs::write(
            tmp.path().join("enc.toml"),
            format!("requires = \"encrypted-backup\"\n{fixed}"),
        )
        .unwrap();
        let mods = load_modules(tmp.path()).unwrap();
        assert!(mods[0].needs_encrypted_backup());
    }
}
