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

use std::io::Cursor;

use plist::Value;

use crate::{Error, Result};

/// The `$objects[0]` sentinel that stands in for `nil`.
const NULL_SENTINEL: &str = "$null";
/// Guard against pathological/cyclic archives.
const MAX_DEPTH: usize = 96;

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
    let mut out = plist::Dictionary::new();
    for (k, v) in top {
        out.insert(k.clone(), resolve_value(v, objects, 0));
    }
    // A single "root" is the common case — unwrap it so callers navigate directly.
    if out.len() == 1 {
        if let Some(root) = out.remove("root") {
            return Ok(root);
        }
        // (re-insert if the sole key wasn't "root")
    }
    Ok(Value::Dictionary(out))
}

/// Resolve one value, following a UID into `$objects` if needed.
fn resolve_value(v: &Value, objects: &[Value], depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return Value::String(String::new());
    }
    match v {
        Value::Uid(uid) => {
            let idx = uid.get() as usize;
            match objects.get(idx) {
                Some(obj) => resolve_object(obj, objects, depth + 1),
                None => Value::String(String::new()),
            }
        }
        // A non-UID value stands for itself.
        other => other.clone(),
    }
}

/// Resolve an entry from `$objects` (a container, a custom object, or a scalar).
fn resolve_object(obj: &Value, objects: &[Value], depth: usize) -> Value {
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
                    let key = match resolve_value(k, objects, depth) {
                        Value::String(s) => s,
                        Value::Integer(i) => i.to_string(),
                        _ => continue,
                    };
                    map.insert(key, resolve_value(val, objects, depth));
                }
                return Value::Dictionary(map);
            }
            // NSArray / NSSet: NS.objects only.
            if let Some(items) = d.get("NS.objects").and_then(Value::as_array) {
                return Value::Array(
                    items
                        .iter()
                        .map(|it| resolve_value(it, objects, depth))
                        .collect(),
                );
            }
            // A plain NSString/NSMutableString stored as a dict.
            if let Some(s) = d.get("NS.string").and_then(Value::as_string) {
                return Value::String(s.to_string());
            }
            // NSDate: `NS.time` is seconds since 2001-01-01 — convert to a Unix-based
            // plist Date so callers get a real timestamp.
            if let Some(t) = d.get("NS.time").and_then(|v| {
                v.as_real()
                    .or_else(|| v.as_signed_integer().map(|i| i as f64))
            }) {
                let unix = t + 978_307_200.0;
                if unix >= 0.0 {
                    let st = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(unix);
                    return Value::Date(st.into());
                }
            }
            // A custom object: resolve each property (skip archiver bookkeeping).
            let mut map = plist::Dictionary::new();
            for (k, val) in d {
                if k == "$class" {
                    continue;
                }
                map.insert(k.clone(), resolve_value(val, objects, depth));
            }
            Value::Dictionary(map)
        }
        // Scalars (String/Integer/Real/Boolean/Date/Data) pass through.
        other => other.clone(),
    }
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
