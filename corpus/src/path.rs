//! The engine's dot paths on [`Json`], as 3.x's `set_nested_value` and
//! `remove_nested_value` applied them: an independent model that
//! `tests/it/path.rs` holds `Path` to.
//!
//! A segment is a key on an object, with one leading `#` stripped, and an index
//! on an array, parsed from the raw segment, so `#20` is never an index, and
//! at most [`MAX_INDEX`]: a larger number is a key, where 3.x read any `usize`
//! (the one rule the engine departs from on purpose). A
//! missing container is created on demand: an array when the next segment parses
//! as an index, an object otherwise. A write either applies in full or does
//! nothing, and the empty path reads the root and writes nothing.

use crate::json::Json;

/// The largest segment that is an index, `2^20 - 1`: the engine's
/// `MAX_ARRAY_INDEX`, so no path can grow an array without bound.
pub const MAX_INDEX: usize = (1 << 20) - 1;

struct Seg<'a> {
    key: &'a str,
    index: Option<usize>,
}

fn segs(path: &str) -> Vec<Seg<'_>> {
    if path.is_empty() {
        return Vec::new();
    }
    path.split('.')
        .map(|raw| Seg {
            key: raw.strip_prefix('#').unwrap_or(raw),
            index: raw.parse().ok().filter(|&i| i <= MAX_INDEX),
        })
        .collect()
}

fn empty(next: &Seg<'_>) -> Json {
    if next.index.is_some() {
        Json::Array(Vec::new())
    } else {
        Json::object()
    }
}

pub fn get<'a>(root: &'a Json, path: &str) -> Option<&'a Json> {
    let mut at = root;
    for seg in segs(path) {
        at = match at {
            Json::Object(_) => at.get(seg.key)?,
            Json::Array(items) => items.get(seg.index?)?,
            _ => return None,
        };
    }
    Some(at)
}

pub fn get_mut<'a>(root: &'a mut Json, path: &str) -> Option<&'a mut Json> {
    get_mut_segs(root, &segs(path))
}

fn get_mut_segs<'a>(root: &'a mut Json, segs: &[Seg<'_>]) -> Option<&'a mut Json> {
    let mut at = root;
    for seg in segs {
        at = match at {
            Json::Object(_) => at.get_mut(seg.key)?,
            Json::Array(items) => items.get_mut(seg.index?)?,
            _ => return None,
        };
    }
    Some(at)
}

/// Assign `value` at `path`, creating containers on the way.
pub fn set(root: &mut Json, path: &str, value: Json) {
    let segs = segs(path);
    let Some((last, parents)) = segs.split_last() else {
        return;
    };
    let mut at = root;
    for (i, seg) in parents.iter().enumerate() {
        let next = &segs[i + 1];
        at = match at {
            Json::Object(pairs) => {
                let pos = match pairs.iter().position(|(k, _)| k == seg.key) {
                    Some(pos) => pos,
                    None => {
                        pairs.push((seg.key.to_owned(), empty(next)));
                        pairs.len() - 1
                    }
                };
                &mut pairs[pos].1
            }
            Json::Array(items) => {
                let Some(pos) = seg.index else { return };
                if items.len() <= pos {
                    items.resize(pos + 1, Json::Null);
                }
                if items[pos] == Json::Null {
                    items[pos] = empty(next);
                }
                &mut items[pos]
            }
            _ => return,
        };
    }
    match at {
        Json::Object(pairs) => match pairs.iter_mut().find(|(k, _)| k == last.key) {
            Some(slot) => slot.1 = value,
            None => pairs.push((last.key.to_owned(), value)),
        },
        Json::Array(items) => {
            let Some(pos) = last.index else { return };
            if items.len() <= pos {
                items.resize(pos + 1, Json::Null);
            }
            items[pos] = value;
        }
        _ => {}
    }
}

/// Remove what `path` names: an object key leaves with the order of the rest
/// kept, an array element with the tail shifted down.
pub fn remove(root: &mut Json, path: &str) {
    let segs = segs(path);
    let Some((last, parents)) = segs.split_last() else {
        return;
    };
    match get_mut_segs(root, parents) {
        Some(Json::Object(pairs)) => {
            if let Some(pos) = pairs.iter().position(|(k, _)| k == last.key) {
                pairs.remove(pos);
            }
        }
        Some(Json::Array(items)) => {
            if let Some(pos) = last.index.filter(|&pos| pos < items.len()) {
                items.remove(pos);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(text: &str) -> Json {
        Json::from_text(text).unwrap()
    }

    fn after_set(start: &str, path: &str, value: &str) -> String {
        let mut root = j(start);
        set(&mut root, path, j(value));
        root.to_text()
    }

    #[test]
    fn set_follows_v3s_rules() {
        let cases = [
            // A nested create picks array or object from the next segment.
            ("{}", "a.b.0.c", "1", r#"{"a":{"b":[{"c":1}]}}"#),
            // One leading `#` makes a numeric key on an object.
            ("{}", "a.#0", "1", r#"{"a":{"0":1}}"#),
            // On an object a bare number is a key too.
            (r#"{"a":{}}"#, "a.0", "1", r#"{"a":{"0":1}}"#),
            // On an array it is an index, and a gap is padded with nulls.
            (r#"{"a":[]}"#, "a.2", "1", r#"{"a":[null,null,1]}"#),
            // `#0` never indexes an array: nothing is written.
            (r#"{"a":[5]}"#, "a.#0", "1", r#"{"a":[5]}"#),
            // A scalar cannot be written through.
            (r#"{"a":"s"}"#, "a.b", "1", r#"{"a":"s"}"#),
            // A null array slot becomes the container the next segment needs.
            (r#"{"a":[null]}"#, "a.0.b", "1", r#"{"a":[{"b":1}]}"#),
            // An existing key keeps its position.
            (r#"{"x":1,"y":2}"#, "x", "3", r#"{"x":3,"y":2}"#),
            // The empty path writes nothing.
            (r#"{"x":1}"#, "", "3", r#"{"x":1}"#),
        ];
        for (start, path, value, expected) in cases {
            assert_eq!(after_set(start, path, value), expected, "{start} / {path}");
        }
    }

    #[test]
    fn remove_keeps_order_and_shifts_arrays() {
        let mut root = j(r#"{"a":1,"b":2,"c":[1,2,3]}"#);
        remove(&mut root, "a");
        remove(&mut root, "c.0");
        remove(&mut root, "c.9");
        remove(&mut root, "missing.x");
        assert_eq!(root.to_text(), r#"{"b":2,"c":[2,3]}"#);
    }

    #[test]
    fn get_reads_through_both_container_kinds() {
        let root = j(r##"{"a":[{"#b":1,"b":2}]}"##);
        assert_eq!(get(&root, "a.0.b"), Some(&j("2")));
        assert_eq!(get(&root, "a.0.##b"), Some(&j("1")));
        assert_eq!(get(&root, ""), Some(&root));
        assert_eq!(get(&root, "a.x"), None);
    }
}
