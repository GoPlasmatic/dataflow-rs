//! # Utility Functions Module
//!
//! Path-based read/write helpers for the [`datavalue::OwnedDataValue`] tree
//! that backs `Message::context`. The same dot-path syntax that worked on
//! `serde_json::Value` works here unchanged — including `#`-prefix escapes
//! for numeric object keys.

use datavalue::OwnedDataValue;
use std::sync::Arc;

/// Get a reference to the value at `path`, walking the tree.
///
/// Path syntax:
/// - `"user.name"` — object property
/// - `"items.0"` — array index
/// - `"user.addresses.0.city"` — mixed
/// - `"data.#20"` — object key literally named `"20"` (strip one leading `#`)
/// - `"data.##"` — object key literally named `"#"` (strip one leading `#`)
///
/// Returns `None` for missing keys, out-of-bounds indices, invalid index
/// formats, or attempts to descend through a non-container.
pub fn get_nested_value<'b>(data: &'b OwnedDataValue, path: &str) -> Option<&'b OwnedDataValue> {
    if path.is_empty() {
        return Some(data);
    }

    let mut current = data;

    for part in path.split('.') {
        match current {
            OwnedDataValue::Object(pairs) => {
                let key = strip_hash_prefix(part);
                let slot = pairs.iter().find(|(k, _)| k == key)?;
                current = &slot.1;
            }
            OwnedDataValue::Array(items) => {
                let idx: usize = part.parse().ok()?;
                current = items.get(idx)?;
            }
            _ => return None,
        }
    }

    Some(current)
}

/// Set the value at `path`, creating intermediate containers as needed.
///
/// Mirrors the original `serde_json::Value` flavour:
/// - intermediate containers are created on demand; the next path part
///   determines whether to create an `Object` (string key) or `Array`
///   (numeric index);
/// - arrays grow with `OwnedDataValue::Null` padding when an index past
///   the current end is assigned;
/// - `#`-prefix escape applies inside object contexts only;
/// - silently no-ops when traversing through a non-container in a non-
///   terminal hop or when an array path part isn't a valid `usize`.
pub fn set_nested_value(data: &mut OwnedDataValue, path: &str, value: OwnedDataValue) {
    if path.is_empty() {
        return;
    }

    let parts: Vec<&str> = path.split('.').collect();
    let last = parts.len() - 1;
    let mut current = data;

    for (i, part) in parts.iter().enumerate() {
        if i == last {
            match current {
                OwnedDataValue::Object(pairs) => {
                    let key = strip_hash_prefix(part);
                    if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == key) {
                        slot.1 = value;
                    } else {
                        pairs.push((key.to_string(), value));
                    }
                }
                OwnedDataValue::Array(items) => {
                    if let Ok(idx) = part.parse::<usize>() {
                        while items.len() <= idx {
                            items.push(OwnedDataValue::Null);
                        }
                        items[idx] = value;
                    }
                }
                _ => {}
            }
            return;
        }

        // Non-terminal hop: locate-or-create the child and descend.
        // Use the next part to decide whether the child container is an Array
        // (next part parses as usize) or an Object (anything else).
        let next_is_array = parts[i + 1].parse::<usize>().is_ok();

        match current {
            OwnedDataValue::Object(pairs) => {
                let key = strip_hash_prefix(part);
                let idx = match pairs.iter().position(|(k, _)| k == key) {
                    Some(idx) => idx,
                    None => {
                        let child = if next_is_array {
                            OwnedDataValue::Array(Vec::new())
                        } else {
                            OwnedDataValue::Object(Vec::new())
                        };
                        pairs.push((key.to_string(), child));
                        pairs.len() - 1
                    }
                };
                current = &mut pairs[idx].1;
            }
            OwnedDataValue::Array(items) => {
                let Ok(idx) = part.parse::<usize>() else {
                    return; // can't use a non-numeric key on an Array
                };
                while items.len() <= idx {
                    items.push(OwnedDataValue::Null);
                }
                if matches!(items[idx], OwnedDataValue::Null) {
                    items[idx] = if next_is_array {
                        OwnedDataValue::Array(Vec::new())
                    } else {
                        OwnedDataValue::Object(Vec::new())
                    };
                }
                current = &mut items[idx];
            }
            _ => return,
        }
    }
}

/// Clone the value at `path`, returning `None` if the path is unresolvable.
#[inline]
pub fn get_nested_value_cloned(data: &OwnedDataValue, path: &str) -> Option<OwnedDataValue> {
    get_nested_value(data, path).cloned()
}

/// Same as `get_nested_value` but consumes a pre-split slice of path parts.
/// Parts retain the original `#` prefix; `strip_hash_prefix` is applied at
/// lookup time so the `#20` → "force object key 20" semantics still hold.
pub fn get_nested_value_parts<'b>(
    data: &'b OwnedDataValue,
    parts: &[Arc<str>],
) -> Option<&'b OwnedDataValue> {
    if parts.is_empty() {
        return Some(data);
    }
    let mut current = data;
    for part in parts {
        match current {
            OwnedDataValue::Object(pairs) => {
                let key = strip_hash_prefix(part);
                let slot = pairs.iter().find(|(k, _)| k == key)?;
                current = &slot.1;
            }
            OwnedDataValue::Array(items) => {
                let idx: usize = part.parse().ok()?;
                current = items.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Same as `set_nested_value` but consumes a pre-split slice of path parts.
/// Parts retain the original `#` prefix; `strip_hash_prefix` is applied at
/// use time. Crucially, the "is the NEXT segment an array index?" decision
/// looks at the raw (unstripped) `parts[i+1]` — `#20` parses as non-numeric,
/// so the child container is an Object (key "20"), not an Array.
pub fn set_nested_value_parts(
    data: &mut OwnedDataValue,
    parts: &[Arc<str>],
    value: OwnedDataValue,
) {
    if parts.is_empty() {
        return;
    }
    let last = parts.len() - 1;
    let mut current = data;

    for (i, part) in parts.iter().enumerate() {
        if i == last {
            match current {
                OwnedDataValue::Object(pairs) => {
                    let key = strip_hash_prefix(part);
                    if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == key) {
                        slot.1 = value;
                    } else {
                        pairs.push((key.to_string(), value));
                    }
                }
                OwnedDataValue::Array(items) => {
                    if let Ok(idx) = part.parse::<usize>() {
                        while items.len() <= idx {
                            items.push(OwnedDataValue::Null);
                        }
                        items[idx] = value;
                    }
                }
                _ => {}
            }
            return;
        }

        let next_part: &str = &parts[i + 1];
        let next_is_array = next_part.parse::<usize>().is_ok();

        match current {
            OwnedDataValue::Object(pairs) => {
                let key = strip_hash_prefix(part);
                let idx = match pairs.iter().position(|(k, _)| k == key) {
                    Some(idx) => idx,
                    None => {
                        let child = if next_is_array {
                            OwnedDataValue::Array(Vec::new())
                        } else {
                            OwnedDataValue::Object(Vec::new())
                        };
                        pairs.push((key.to_string(), child));
                        pairs.len() - 1
                    }
                };
                current = &mut pairs[idx].1;
            }
            OwnedDataValue::Array(items) => {
                let Ok(idx) = part.parse::<usize>() else {
                    return;
                };
                while items.len() <= idx {
                    items.push(OwnedDataValue::Null);
                }
                if matches!(items[idx], OwnedDataValue::Null) {
                    items[idx] = if next_is_array {
                        OwnedDataValue::Array(Vec::new())
                    } else {
                        OwnedDataValue::Object(Vec::new())
                    };
                }
                current = &mut items[idx];
            }
            _ => return,
        }
    }
}

/// Strip exactly one leading `#` from an object-key path component.
/// `"#20"` → `"20"`, `"##"` → `"#"`, `"foo"` → `"foo"`.
#[inline]
fn strip_hash_prefix(part: &str) -> &str {
    part.strip_prefix('#').unwrap_or(part)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test-only helper: build OwnedDataValue from a `json!` literal.
    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    #[test]
    fn test_get_nested_value() {
        let data = dv(json!({
            "user": {
                "name": "John",
                "age": 30,
                "addresses": [
                    {"city": "New York", "zip": "10001"},
                    {"city": "San Francisco", "zip": "94102"}
                ],
                "preferences": {
                    "theme": "dark",
                    "notifications": true
                }
            },
            "items": [1, 2, 3]
        }));

        assert_eq!(
            get_nested_value(&data, "user.name"),
            Some(&dv(json!("John")))
        );
        assert_eq!(get_nested_value(&data, "user.age"), Some(&dv(json!(30))));

        assert_eq!(
            get_nested_value(&data, "user.preferences.theme"),
            Some(&dv(json!("dark")))
        );
        assert_eq!(
            get_nested_value(&data, "user.preferences.notifications"),
            Some(&dv(json!(true)))
        );

        assert_eq!(get_nested_value(&data, "items.0"), Some(&dv(json!(1))));
        assert_eq!(get_nested_value(&data, "items.2"), Some(&dv(json!(3))));

        assert_eq!(
            get_nested_value(&data, "user.addresses.0.city"),
            Some(&dv(json!("New York")))
        );
        assert_eq!(
            get_nested_value(&data, "user.addresses.1.zip"),
            Some(&dv(json!("94102")))
        );

        assert_eq!(get_nested_value(&data, "user.missing"), None);
        assert_eq!(get_nested_value(&data, "items.10"), None);
        assert_eq!(get_nested_value(&data, "user.addresses.2.city"), None);
        assert_eq!(get_nested_value(&data, "nonexistent.path"), None);
    }

    #[test]
    fn test_set_nested_value() {
        let mut data = dv(json!({}));

        set_nested_value(&mut data, "name", dv(json!("Alice")));
        assert_eq!(data, dv(json!({"name": "Alice"})));

        set_nested_value(&mut data, "user.email", dv(json!("alice@example.com")));
        assert_eq!(
            data,
            dv(json!({
                "name": "Alice",
                "user": {"email": "alice@example.com"}
            }))
        );

        set_nested_value(&mut data, "name", dv(json!("Bob")));
        assert_eq!(
            data,
            dv(json!({
                "name": "Bob",
                "user": {"email": "alice@example.com"}
            }))
        );

        set_nested_value(&mut data, "settings.theme.mode", dv(json!("dark")));
        assert_eq!(data["settings"]["theme"]["mode"], dv(json!("dark")));

        set_nested_value(&mut data, "user.age", dv(json!(25)));
        assert_eq!(data["user"]["age"], dv(json!(25)));
        assert_eq!(data["user"]["email"], dv(json!("alice@example.com")));
    }

    #[test]
    fn test_set_nested_value_with_arrays() {
        let mut data = dv(json!({ "items": [1, 2, 3] }));

        set_nested_value(&mut data, "items.0", dv(json!(10)));
        assert_eq!(data["items"], dv(json!([10, 2, 3])));

        set_nested_value(&mut data, "items.5", dv(json!(50)));
        assert_eq!(data["items"], dv(json!([10, 2, 3, null, null, 50])));

        let mut data2 = dv(json!({}));
        set_nested_value(&mut data2, "matrix.0.0", dv(json!(1)));
        set_nested_value(&mut data2, "matrix.0.1", dv(json!(2)));
        set_nested_value(&mut data2, "matrix.1.0", dv(json!(3)));
        assert_eq!(data2, dv(json!({ "matrix": [[1, 2], [3]] })));
    }

    #[test]
    fn test_set_nested_value_array_expansion() {
        let mut data = dv(json!({}));

        set_nested_value(&mut data, "array.2", dv(json!("value")));
        assert_eq!(data, dv(json!({ "array": [null, null, "value"] })));

        let mut data2 = dv(json!({}));
        set_nested_value(&mut data2, "deep.nested.0.field", dv(json!("test")));
        assert_eq!(
            data2,
            dv(json!({ "deep": { "nested": [{ "field": "test" }] } }))
        );
    }

    #[test]
    fn test_get_nested_value_cloned() {
        let data = dv(json!({
            "user": {
                "profile": {
                    "name": "Alice",
                    "settings": {"theme": "dark"}
                }
            }
        }));

        assert_eq!(
            get_nested_value_cloned(&data, "user.profile.name"),
            Some(dv(json!("Alice")))
        );
        assert_eq!(
            get_nested_value_cloned(&data, "user.profile.settings"),
            Some(dv(json!({ "theme": "dark" })))
        );
        assert_eq!(get_nested_value_cloned(&data, "user.missing"), None);
    }

    #[test]
    fn test_get_nested_value_bounds_checking() {
        let data = dv(json!({
            "items": [1, 2, 3],
            "nested": {
                "array": [
                    {"id": 1},
                    {"id": 2}
                ]
            }
        }));

        assert_eq!(get_nested_value(&data, "items.0"), Some(&dv(json!(1))));
        assert_eq!(get_nested_value(&data, "items.2"), Some(&dv(json!(3))));

        assert_eq!(get_nested_value(&data, "items.10"), None);
        assert_eq!(get_nested_value(&data, "items.999999"), None);

        assert_eq!(get_nested_value(&data, "items.abc"), None);
        assert_eq!(get_nested_value(&data, "items.-1"), None);
        assert_eq!(get_nested_value(&data, "items.2.5"), None);

        assert_eq!(
            get_nested_value(&data, "nested.array.0.id"),
            Some(&dv(json!(1)))
        );
        assert_eq!(get_nested_value(&data, "nested.array.5.id"), None);

        assert_eq!(get_nested_value(&data, ""), Some(&data));
    }

    #[test]
    fn test_set_nested_value_bounds_safety() {
        let mut data = dv(json!({}));

        set_nested_value(&mut data, "large.10", dv(json!("value")));
        assert_eq!(data["large"].as_array().unwrap().len(), 11);
        assert_eq!(data["large"][10], dv(json!("value")));
        for i in 0..10usize {
            assert_eq!(data["large"][i], dv(json!(null)));
        }

        let mut data2 = dv(json!({ "matrix": [] }));
        set_nested_value(&mut data2, "matrix.2.1", dv(json!(5)));
        assert_eq!(data2["matrix"][0], dv(json!(null)));
        assert_eq!(data2["matrix"][1], dv(json!(null)));
        assert_eq!(data2["matrix"][2][0], dv(json!(null)));
        assert_eq!(data2["matrix"][2][1], dv(json!(5)));

        let mut data3 = dv(json!({ "arr": [1, 2, 3] }));
        set_nested_value(&mut data3, "arr.1", dv(json!("replaced")));
        assert_eq!(data3["arr"], dv(json!([1, "replaced", 3])));
    }

    #[test]
    fn test_hash_prefix_in_paths() {
        let data = dv(json!({
            "fields": {
                "20": "numeric field name",
                "#": "hash field",
                "##": "double hash field",
                "normal": "normal field"
            }
        }));

        assert_eq!(
            get_nested_value(&data, "fields.#20"),
            Some(&dv(json!("numeric field name")))
        );
        assert_eq!(
            get_nested_value(&data, "fields.##"),
            Some(&dv(json!("hash field")))
        );
        assert_eq!(
            get_nested_value(&data, "fields.###"),
            Some(&dv(json!("double hash field")))
        );
        assert_eq!(
            get_nested_value(&data, "fields.normal"),
            Some(&dv(json!("normal field")))
        );
        assert_eq!(get_nested_value(&data, "fields.#999"), None);
    }

    #[test]
    fn test_set_hash_prefix_in_paths() {
        let mut data = dv(json!({}));

        set_nested_value(&mut data, "fields.#20", dv(json!("value for 20")));
        assert_eq!(data["fields"]["20"], dv(json!("value for 20")));

        set_nested_value(&mut data, "fields.##", dv(json!("hash value")));
        assert_eq!(data["fields"]["#"], dv(json!("hash value")));

        set_nested_value(&mut data, "fields.###", dv(json!("double hash value")));
        assert_eq!(data["fields"]["##"], dv(json!("double hash value")));

        set_nested_value(&mut data, "fields.normal", dv(json!("normal value")));
        assert_eq!(data["fields"]["normal"], dv(json!("normal value")));

        assert_eq!(
            data,
            dv(json!({
                "fields": {
                    "20": "value for 20",
                    "#": "hash value",
                    "##": "double hash value",
                    "normal": "normal value"
                }
            }))
        );
    }

    #[test]
    fn test_hash_prefix_with_arrays() {
        let mut data = dv(json!({
            "items": [
                {"0": "field named zero", "id": 1},
                {"1": "field named one", "id": 2}
            ]
        }));

        assert_eq!(
            get_nested_value(&data, "items.0.#0"),
            Some(&dv(json!("field named zero")))
        );
        assert_eq!(
            get_nested_value(&data, "items.1.#1"),
            Some(&dv(json!("field named one")))
        );

        set_nested_value(&mut data, "items.0.#2", dv(json!("field named two")));
        assert_eq!(data["items"][0]["2"], dv(json!("field named two")));

        assert_eq!(get_nested_value(&data, "items.0.id"), Some(&dv(json!(1))));
        assert_eq!(get_nested_value(&data, "items.1.id"), Some(&dv(json!(2))));
    }

    #[test]
    fn test_hash_prefix_field_with_array_value() {
        let data = dv(json!({
            "data": {
                "fields": {
                    "72": ["first", "second", "third"],
                    "100": ["alpha", "beta", "gamma"],
                    "normal": ["one", "two", "three"]
                }
            }
        }));

        assert_eq!(
            get_nested_value(&data, "data.fields.#72.0"),
            Some(&dv(json!("first")))
        );
        assert_eq!(
            get_nested_value(&data, "data.fields.#72.1"),
            Some(&dv(json!("second")))
        );
        assert_eq!(
            get_nested_value(&data, "data.fields.#72.2"),
            Some(&dv(json!("third")))
        );

        assert_eq!(
            get_nested_value(&data, "data.fields.#100.0"),
            Some(&dv(json!("alpha")))
        );
        assert_eq!(
            get_nested_value(&data, "data.fields.#100.1"),
            Some(&dv(json!("beta")))
        );

        assert_eq!(
            get_nested_value(&data, "data.fields.normal.0"),
            Some(&dv(json!("one")))
        );

        let mut data_mut = data.clone();
        set_nested_value(&mut data_mut, "data.fields.#72.0", dv(json!("modified")));
        assert_eq!(data_mut["data"]["fields"]["72"][0], dv(json!("modified")));

        set_nested_value(&mut data_mut, "data.fields.#999.0", dv(json!("new value")));
        assert_eq!(data_mut["data"]["fields"]["999"][0], dv(json!("new value")));

        let complex_data = dv(json!({
            "fields": {
                "42": [
                    {"name": "item1", "value": 100},
                    {"name": "item2", "value": 200}
                ]
            }
        }));

        assert_eq!(
            get_nested_value(&complex_data, "fields.#42.0.name"),
            Some(&dv(json!("item1")))
        );
        assert_eq!(
            get_nested_value(&complex_data, "fields.#42.1.value"),
            Some(&dv(json!(200)))
        );

        let multi_hash_data = dv(json!({
            "data": {
                "#fields": {
                    "##": ["hash array"],
                    "10": ["numeric array"]
                }
            }
        }));

        assert_eq!(
            get_nested_value(&multi_hash_data, "data.##fields.###.0"),
            Some(&dv(json!("hash array")))
        );
        assert_eq!(
            get_nested_value(&multi_hash_data, "data.##fields.#10.0"),
            Some(&dv(json!("numeric array")))
        );
    }
}
