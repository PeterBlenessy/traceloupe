//! Minimal NSKeyedArchiver deserializer.
//!
//! Apple's `NSKeyedArchiver` doesn't store a plain plist — it stores a flattened
//! object graph: a top-level `$top` map of root keys to UID references into a
//! `$objects` array, where containers (`NSDictionary`, `NSArray`, custom objects)
//! reference their members by further UIDs. The `plist` crate parses that raw
//! structure but doesn't *resolve* the graph; this module walks the UIDs back
//! into an ordinary [`plist::Value`] tree.
//!
//! Standard, app-independent format — so this is unit-testable on its own and
//! reused wherever an iOS artifact embeds a keyed archive (e.g. Instagram DMs).
//!
//! provenance: reference (own implementation) from the documented NSKeyedArchiver
//! layout; not a port.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plist::Value;

use crate::{Error, Result};

/// The `$objects[0]` sentinel that stands in for `nil`.
const NULL_SENTINEL: &str = "$null";
/// Guard against pathologically deep archives (backstop; the memo/cycle set is the
/// real defense against fan-out and cycles).
const MAX_DEPTH: usize = 256;
/// NSDate/Core-Data epoch (2001-01-01) as Unix seconds.
const MAC_EPOCH_SECS: f64 = 978_307_200.0;

/// Parse `bytes` as a plist and, if it's an `NSKeyedArchiver` archive, resolve its
/// object graph into a plain [`Value`]. A plist that isn't keyed-archived is
/// returned as parsed.
pub fn resolve(bytes: &[u8]) -> Result<Value> {
    let root =
        Value::from_reader(Cursor::new(bytes)).map_err(|e| Error::Parse(format!("plist: {e}")))?;
    let Some(dict) = root.as_dictionary() else {
        return Ok(root);
    };
    let is_nska = dict
        .get("$archiver")
        .and_then(Value::as_string)
        .map(|s| s.contains("NSKeyedArchiver"))
        .unwrap_or(false);
    if !is_nska {
        return Ok(root);
    }

    let objects = dict
        .get("$objects")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("NSKeyedArchiver: missing $objects".into()))?;
    let top = dict
        .get("$top")
        .and_then(Value::as_dictionary)
        .ok_or_else(|| Error::Parse("NSKeyedArchiver: missing $top".into()))?;
    // Resolve every root under $top into an output dictionary (usually just "root").
    let mut r = Resolver::new(objects);
    let mut out = plist::Dictionary::new();
    for (k, v) in top {
        out.insert(k.clone(), r.value(v, 0));
    }
    // A single "root" is the common case — unwrap it so callers navigate directly.
    if out.len() == 1 {
        if let Some(root) = out.remove("root") {
            return Ok(root);
        }
        // Sole key wasn't "root": `remove` was a no-op, so `out` is intact.
    }
    Ok(Value::Dictionary(out))
}

/// Walks the `$objects` graph. Memoizes each object by index so a shared subtree
/// is resolved once (not re-cloned exponentially), and tracks the in-progress set
/// so a cyclic reference resolves to empty instead of recursing forever.
struct Resolver<'a> {
    objects: &'a [Value],
    memo: HashMap<usize, Value>,
    in_progress: HashSet<usize>,
}

impl<'a> Resolver<'a> {
    fn new(objects: &'a [Value]) -> Self {
        Self {
            objects,
            memo: HashMap::new(),
            in_progress: HashSet::new(),
        }
    }

    /// Resolve one value, following a UID into `$objects`.
    fn value(&mut self, v: &Value, depth: usize) -> Value {
        match v {
            Value::Uid(uid) => match usize::try_from(uid.get()) {
                Ok(idx) => self.object_at(idx, depth),
                Err(_) => Value::String(String::new()), // UID out of addressable range
            },
            // A non-UID value stands for itself.
            other => other.clone(),
        }
    }

    /// Resolve `$objects[idx]`, using the memo and breaking cycles.
    fn object_at(&mut self, idx: usize, depth: usize) -> Value {
        if depth > MAX_DEPTH {
            return Value::String(String::new());
        }
        if let Some(v) = self.memo.get(&idx) {
            return v.clone();
        }
        if !self.in_progress.insert(idx) {
            // Already resolving `idx` further up the stack → a cycle. Break it.
            return Value::String(String::new());
        }
        // Clone the object out so we can borrow `self` mutably while resolving it.
        let resolved = match self.objects.get(idx).cloned() {
            Some(obj) => self.resolve_object(&obj, depth),
            None => Value::String(String::new()), // dangling reference
        };
        self.in_progress.remove(&idx);
        self.memo.insert(idx, resolved.clone());
        resolved
    }

    /// Resolve an entry from `$objects` (a container, a custom object, or a scalar).
    fn resolve_object(&mut self, obj: &Value, depth: usize) -> Value {
        let next = depth + 1;
        match obj {
            Value::String(s) if s == NULL_SENTINEL => Value::String(String::new()),
            Value::Dictionary(d) => {
                // NSDictionary: parallel NS.keys / NS.objects UID arrays.
                if let (Some(keys), Some(vals)) = (
                    d.get("NS.keys").and_then(Value::as_array),
                    d.get("NS.objects").and_then(Value::as_array),
                ) {
                    let mut map = plist::Dictionary::new();
                    for (k, val) in keys.iter().zip(vals.iter()) {
                        let key = match self.value(k, next) {
                            Value::String(s) => s,
                            Value::Integer(i) => i.to_string(),
                            _ => continue,
                        };
                        map.insert(key, self.value(val, next));
                    }
                    return Value::Dictionary(map);
                }
                // NSArray / NSSet: NS.objects only.
                if let Some(items) = d.get("NS.objects").and_then(Value::as_array) {
                    let items: Vec<Value> = items.clone();
                    return Value::Array(items.iter().map(|it| self.value(it, next)).collect());
                }
                // A plain NSString/NSMutableString stored as a dict.
                if let Some(s) = d.get("NS.string").and_then(Value::as_string) {
                    return Value::String(s.to_string());
                }
                // NSDate: `NS.time` is seconds since 2001-01-01 — convert to a
                // Unix-based plist Date. Reject non-finite / out-of-range values so a
                // crafted `NS.time` (inf, 1e300) can't panic `Duration::from_secs_f64`.
                if let Some(t) = d.get("NS.time").and_then(|v| {
                    v.as_real()
                        .or_else(|| v.as_signed_integer().map(|i| i as f64))
                }) {
                    if let Some(date) = mac_time_to_date(t) {
                        return Value::Date(date);
                    }
                }
                // A custom object: resolve each property (skip archiver bookkeeping).
                let entries: Vec<(String, Value)> =
                    d.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let mut map = plist::Dictionary::new();
                for (k, val) in entries {
                    if k == "$class" {
                        continue;
                    }
                    map.insert(k, self.value(&val, next));
                }
                Value::Dictionary(map)
            }
            // Scalars (String/Integer/Real/Boolean/Date/Data) pass through.
            other => other.clone(),
        }
    }
}

/// Convert an `NS.time` (Core-Data seconds since 2001) to a plist `Date`, or
/// `None` if the value is non-finite or outside the representable range. Handles
/// pre-1970 instants (negative Unix seconds) too.
fn mac_time_to_date(ns_time: f64) -> Option<plist::Date> {
    let unix = ns_time + MAC_EPOCH_SECS;
    if !unix.is_finite() || unix.abs() >= 8.0e18 {
        return None; // beyond Duration's ~1.8e19-second range (or NaN/inf)
    }
    let secs = unix.abs();
    let dur = Duration::try_from_secs_f64(secs).ok()?;
    let st: SystemTime = if unix >= 0.0 {
        UNIX_EPOCH.checked_add(dur)?
    } else {
        UNIX_EPOCH.checked_sub(dur)?
    };
    Some(st.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a keyed archive whose root is a dict {greeting: "hi", n: 7} and check
    /// the graph resolves back to that dict. `$objects`: [null, rootdict, "greeting",
    /// "hi", "n"]; integers are inlined (NSKeyedArchiver stores small ints directly).
    #[test]
    fn resolves_nsdictionary_graph() {
        use plist::{Uid, Value};
        let mut rootdict = plist::Dictionary::new();
        rootdict.insert(
            "NS.keys".into(),
            Value::Array(vec![Value::Uid(Uid::new(2)), Value::Uid(Uid::new(4))]),
        );
        rootdict.insert(
            "NS.objects".into(),
            Value::Array(vec![Value::Uid(Uid::new(3)), Value::Integer(7.into())]),
        );
        rootdict.insert("$class".into(), Value::Uid(Uid::new(5)));
        let objects = Value::Array(vec![
            Value::String("$null".into()),
            Value::Dictionary(rootdict),
            Value::String("greeting".into()),
            Value::String("hi".into()),
            Value::String("n".into()),
        ]);
        let mut top = plist::Dictionary::new();
        top.insert("root".into(), Value::Uid(Uid::new(1)));
        let mut archive = plist::Dictionary::new();
        archive.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        archive.insert("$top".into(), Value::Dictionary(top));
        archive.insert("$objects".into(), objects);

        let mut buf = Vec::new();
        Value::Dictionary(archive)
            .to_writer_binary(&mut buf)
            .unwrap();

        let resolved = resolve(&buf).unwrap();
        let d = resolved.as_dictionary().expect("root is a dict");
        assert_eq!(d.get("greeting").and_then(Value::as_string), Some("hi"));
        assert_eq!(d.get("n").and_then(Value::as_signed_integer), Some(7));
    }

    /// A cyclic / self-referential archive must resolve (to empty at the cycle),
    /// not hang or blow up. `$objects[1]` is an array `[Uid(1), Uid(1)]` → the
    /// former exponential-fan-out DoS.
    #[test]
    fn cyclic_and_shared_refs_terminate() {
        use plist::{Uid, Value};
        let mut arr = plist::Dictionary::new();
        arr.insert(
            "NS.objects".into(),
            Value::Array(vec![Value::Uid(Uid::new(1)), Value::Uid(Uid::new(1))]),
        );
        let objects = Value::Array(vec![Value::String("$null".into()), Value::Dictionary(arr)]);
        let mut top = plist::Dictionary::new();
        top.insert("root".into(), Value::Uid(Uid::new(1)));
        let mut archive = plist::Dictionary::new();
        archive.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        archive.insert("$top".into(), Value::Dictionary(top));
        archive.insert("$objects".into(), objects);
        let mut buf = Vec::new();
        Value::Dictionary(archive)
            .to_writer_binary(&mut buf)
            .unwrap();
        // Must return promptly without panicking or hanging.
        let resolved = resolve(&buf).unwrap();
        assert!(resolved.as_array().is_some());
    }

    /// A non-finite `NS.time` must not panic `Duration::from_secs_f64`.
    #[test]
    fn oversized_nsdate_does_not_panic() {
        assert!(mac_time_to_date(f64::INFINITY).is_none());
        assert!(mac_time_to_date(1e300).is_none());
        assert!(mac_time_to_date(f64::NAN).is_none());
        // A normal date still converts.
        assert!(mac_time_to_date(721_692_800.0).is_some());
    }

    #[test]
    fn passes_through_plain_plist() {
        use plist::Value;
        let mut d = plist::Dictionary::new();
        d.insert("k".into(), Value::String("v".into()));
        let mut buf = Vec::new();
        Value::Dictionary(d).to_writer_binary(&mut buf).unwrap();
        let resolved = resolve(&buf).unwrap();
        assert_eq!(
            resolved
                .as_dictionary()
                .unwrap()
                .get("k")
                .unwrap()
                .as_string(),
            Some("v")
        );
    }
}
