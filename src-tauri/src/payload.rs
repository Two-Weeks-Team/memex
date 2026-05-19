//! Shared payload-extraction helpers for Qdrant point payloads.
//!
//! Before this module, `payload_str` / `payload_i64` / `payload_bool` and a
//! "with default" variant `payload_string` were duplicated across
//! `indexer.rs`, `retrieval.rs`, and `lens.rs` — every call site picked one
//! of two return-shape conventions (`Option<T>` vs. default-on-missing).
//! Consolidating them here makes the shape explicit at the call site
//! (one of `payload_str` / `payload_string` / `payload_i64` / `payload_bool`)
//! and ensures changes to the payload schema (e.g. new field, renamed
//! field) live in one place. (Gemini PR #4 review on `retrieval.rs:470`.)
//!
//! Variants on offer:
//!   - `payload_str(p, k)   -> Option<String>`  — caller chooses default
//!   - `payload_string(p, k) -> String`         — empty when missing/null
//!   - `payload_i64(p, k)   -> Option<i64>`
//!   - `payload_bool(p, k)  -> Option<bool>`
//!
//! All four are pure, allocation-light (one `String::clone` only when the
//! value exists), and panic-free.

use std::collections::HashMap;

use qdrant_client::qdrant::value::Kind as ValueKind;
use qdrant_client::qdrant::Value;

/// Extract a string field from a Qdrant point payload, returning `None`
/// when the key is missing OR the value isn't a string.
pub fn payload_str(p: &HashMap<String, Value>, key: &str) -> Option<String> {
    p.get(key)
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            ValueKind::StringValue(s) => Some(s.clone()),
            _ => None,
        })
}

/// Like [`payload_str`] but collapses missing/non-string into an empty
/// `String`. Convenient when the caller serializes to a tab-table or HTTP
/// response where empty-string is the natural rendering of "missing".
pub fn payload_string(p: &HashMap<String, Value>, key: &str) -> String {
    payload_str(p, key).unwrap_or_default()
}

/// Extract an integer field. Returns `None` for missing/non-integer values.
pub fn payload_i64(p: &HashMap<String, Value>, key: &str) -> Option<i64> {
    p.get(key)
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            ValueKind::IntegerValue(i) => Some(*i),
            _ => None,
        })
}

/// Extract a boolean field. Returns `None` for missing/non-bool values.
pub fn payload_bool(p: &HashMap<String, Value>, key: &str) -> Option<bool> {
    p.get(key)
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            ValueKind::BoolValue(b) => Some(*b),
            _ => None,
        })
}

/// Best-effort conversion of a Qdrant payload map into a `serde_json::Value`
/// so callers can ship arbitrary payload fields to the frontend.
///
/// **Recursive**: nested `ListValue` / `StructValue` round-trip as JSON
/// arrays / objects (fixes the Gemini PR #6 finding on `lens.rs:719` where
/// `payload_to_json` documented recursion but actually returned `Null` for
/// any nested case). Empty/unknown kinds collapse to `Null`.
pub fn payload_to_json(p: HashMap<String, Value>) -> serde_json::Value {
    let mut map = serde_json::Map::with_capacity(p.len());
    for (k, v) in p.into_iter() {
        map.insert(k, value_to_json(v));
    }
    serde_json::Value::Object(map)
}

/// Recursive single-value converter. Pulled out so list elements + struct
/// fields share the exact same scalar handling as the top-level map.
fn value_to_json(v: Value) -> serde_json::Value {
    use serde_json::Value as J;
    let Some(kind) = v.kind else {
        return J::Null;
    };
    match kind {
        ValueKind::NullValue(_) => J::Null,
        ValueKind::BoolValue(b) => J::Bool(b),
        ValueKind::IntegerValue(i) => J::Number(i.into()),
        ValueKind::DoubleValue(d) => serde_json::Number::from_f64(d)
            .map(J::Number)
            .unwrap_or(J::Null),
        ValueKind::StringValue(s) => J::String(s),
        ValueKind::ListValue(list) => {
            let items: Vec<J> = list.values.into_iter().map(value_to_json).collect();
            J::Array(items)
        }
        ValueKind::StructValue(strct) => {
            let mut m = serde_json::Map::with_capacity(strct.fields.len());
            for (k, vv) in strct.fields.into_iter() {
                m.insert(k, value_to_json(vv));
            }
            J::Object(m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdrant_client::qdrant::{ListValue, Struct, Value};
    use std::collections::HashMap;

    fn s(s: &str) -> Value {
        Value {
            kind: Some(ValueKind::StringValue(s.to_string())),
        }
    }
    fn i(i: i64) -> Value {
        Value {
            kind: Some(ValueKind::IntegerValue(i)),
        }
    }
    fn b(b: bool) -> Value {
        Value {
            kind: Some(ValueKind::BoolValue(b)),
        }
    }

    #[test]
    fn t_payload_str_present_returns_some() {
        let mut p = HashMap::new();
        p.insert("k".into(), s("hello"));
        assert_eq!(payload_str(&p, "k").as_deref(), Some("hello"));
    }

    #[test]
    fn t_payload_str_missing_returns_none() {
        let p: HashMap<String, Value> = HashMap::new();
        assert!(payload_str(&p, "absent").is_none());
    }

    #[test]
    fn t_payload_str_wrong_kind_returns_none() {
        let mut p = HashMap::new();
        p.insert("k".into(), i(7)); // int, not string
        assert!(payload_str(&p, "k").is_none());
    }

    #[test]
    fn t_payload_string_missing_returns_empty() {
        let p: HashMap<String, Value> = HashMap::new();
        assert_eq!(payload_string(&p, "k"), "");
    }

    #[test]
    fn t_payload_i64_and_bool_round_trip() {
        let mut p = HashMap::new();
        p.insert("n".into(), i(42));
        p.insert("flag".into(), b(true));
        assert_eq!(payload_i64(&p, "n"), Some(42));
        assert_eq!(payload_bool(&p, "flag"), Some(true));
    }

    #[test]
    fn t_payload_to_json_scalar_round_trip() {
        let mut p = HashMap::new();
        p.insert("name".into(), s("memex"));
        p.insert("count".into(), i(3));
        p.insert("active".into(), b(true));
        let j = payload_to_json(p);
        assert_eq!(j["name"], serde_json::json!("memex"));
        assert_eq!(j["count"], serde_json::json!(3));
        assert_eq!(j["active"], serde_json::json!(true));
    }

    #[test]
    fn t_payload_to_json_nested_list_recursion() {
        // List of strings — was the entities-style payload field that
        // collapsed to Null in the buggy version.
        let mut p = HashMap::new();
        let list = ListValue {
            values: vec![s("auth"), s("db")],
        };
        p.insert(
            "entities".into(),
            Value {
                kind: Some(ValueKind::ListValue(list)),
            },
        );
        let j = payload_to_json(p);
        assert_eq!(j["entities"], serde_json::json!(["auth", "db"]));
    }

    #[test]
    fn t_payload_to_json_nested_struct_recursion() {
        let mut inner = HashMap::new();
        inner.insert("intent".into(), s("debug"));
        inner.insert("count".into(), i(3));
        let strct = Struct { fields: inner };
        let mut p = HashMap::new();
        p.insert(
            "enrich".into(),
            Value {
                kind: Some(ValueKind::StructValue(strct)),
            },
        );
        let j = payload_to_json(p);
        assert_eq!(j["enrich"]["intent"], serde_json::json!("debug"));
        assert_eq!(j["enrich"]["count"], serde_json::json!(3));
    }
}
