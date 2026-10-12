//! Comparing a fresh recording with a committed one. Everything is compared,
//! object key order included, except paths matching an ignore pattern (I15).

use crate::json::Json;
use std::fmt;

/// What the comparator never compares: diagnostics whose wording v4 owns.
/// Codes, severities and paths are compared; so is every byte of a message.
pub const IGNORED: &[&str] = &[
    "/definitions/*/authored/*/message",
    "/definitions/*/checked/*/message",
    "/definitions/*/parse/error/message",
    "/build/error/message",
    "/steps/*/reload/error/message",
];

/// One difference, at a JSON pointer. A missing side is `None`. A difference
/// in key order alone carries both key lists, as arrays.
#[derive(Debug, PartialEq)]
pub struct Diff {
    pub path: String,
    pub expected: Option<Json>,
    pub actual: Option<Json>,
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let show = |v: &Option<Json>| v.as_ref().map_or("<missing>".to_owned(), Json::to_text);
        let path = if self.path.is_empty() {
            "/"
        } else {
            &self.path
        };
        write!(
            f,
            "{path}: expected {}, got {}",
            show(&self.expected),
            show(&self.actual)
        )
    }
}

/// Every difference between `expected` and `actual`, skipping any path that
/// matches a pattern in `ignore` (JSON pointers in which `*` matches any one
/// segment).
pub fn diff(expected: &Json, actual: &Json, ignore: &[&str]) -> Vec<Diff> {
    let mut out = Vec::new();
    walk(expected, actual, &mut String::new(), ignore, &mut out);
    out
}

/// Does `path` match any pattern in `ignore`?
pub fn ignored(path: &str, ignore: &[&str]) -> bool {
    ignore.iter().any(|pattern| matches(pattern, path))
}

/// Does the pointer `path` match `pattern`, segment for segment?
pub fn matches(pattern: &str, path: &str) -> bool {
    let (mut p, mut a) = (pattern.split('/'), path.split('/'));
    loop {
        match (p.next(), a.next()) {
            (None, None) => return true,
            (Some(want), Some(got)) if want == "*" || want == got => {}
            _ => return false,
        }
    }
}

fn walk(expected: &Json, actual: &Json, path: &mut String, ignore: &[&str], out: &mut Vec<Diff>) {
    if ignored(path, ignore) {
        return;
    }
    match (expected, actual) {
        (Json::Object(e), Json::Object(a)) => {
            let order = |pairs: &[(String, Json)], other: &[(String, Json)]| -> Vec<Json> {
                pairs
                    .iter()
                    .filter(|(k, _)| other.iter().any(|(o, _)| o == k))
                    .map(|(k, _)| Json::str(k.clone()))
                    .collect()
            };
            let (e_order, a_order) = (order(e, a), order(a, e));
            if e_order != a_order {
                out.push(Diff {
                    path: path.clone(),
                    expected: Some(Json::Array(e_order)),
                    actual: Some(Json::Array(a_order)),
                });
            }
            for (key, e_value) in e {
                let len = path.len();
                path.push('/');
                path.push_str(&escape(key));
                match actual.get(key) {
                    Some(a_value) => walk(e_value, a_value, path, ignore, out),
                    None if !ignored(path, ignore) => out.push(Diff {
                        path: path.clone(),
                        expected: Some(e_value.clone()),
                        actual: None,
                    }),
                    None => {}
                }
                path.truncate(len);
            }
            for (key, a_value) in a {
                if expected.get(key).is_none() {
                    let at = format!("{path}/{}", escape(key));
                    if !ignored(&at, ignore) {
                        out.push(Diff {
                            path: at,
                            expected: None,
                            actual: Some(a_value.clone()),
                        });
                    }
                }
            }
        }
        (Json::Array(e), Json::Array(a)) => {
            for i in 0..e.len().max(a.len()) {
                let len = path.len();
                path.push('/');
                path.push_str(&i.to_string());
                match (e.get(i), a.get(i)) {
                    (Some(e_item), Some(a_item)) => walk(e_item, a_item, path, ignore, out),
                    (e_item, a_item) if !ignored(path, ignore) => out.push(Diff {
                        path: path.clone(),
                        expected: e_item.cloned(),
                        actual: a_item.cloned(),
                    }),
                    _ => {}
                }
                path.truncate(len);
            }
        }
        _ if expected == actual => {}
        _ => out.push(Diff {
            path: path.clone(),
            expected: Some(expected.clone()),
            actual: Some(actual.clone()),
        }),
    }
}

fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(text: &str) -> Json {
        Json::from_text(text).unwrap()
    }

    #[test]
    fn key_order_is_a_difference() {
        let d = diff(
            &j(r#"{"x":{"a":1,"b":2}}"#),
            &j(r#"{"x":{"b":2,"a":1}}"#),
            &[],
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "/x");
    }

    #[test]
    fn ignored_patterns_hide_only_what_they_name() {
        let a = j(r#"{"definitions":[{"authored":[{"code":"A","message":"one"}]}]}"#);
        let b = j(r#"{"definitions":[{"authored":[{"code":"A","message":"two"}]}]}"#);
        assert!(diff(&a, &b, IGNORED).is_empty());
        let c = j(r#"{"definitions":[{"authored":[{"code":"B","message":"one"}]}]}"#);
        assert_eq!(diff(&a, &c, IGNORED).len(), 1);
    }

    #[test]
    fn a_missing_key_is_reported_with_one_side_empty() {
        let d = diff(&j(r#"{"a":1}"#), &j("{}"), &[]);
        assert_eq!((d[0].path.as_str(), d[0].actual.is_none()), ("/a", true));
        let d = diff(&j("{}"), &j(r#"{"a/b":1}"#), &[]);
        assert_eq!(
            (d[0].path.as_str(), d[0].expected.is_none()),
            ("/a~1b", true)
        );
    }

    #[test]
    fn arrays_compare_by_position_and_length() {
        let d = diff(&j("[1,2]"), &j("[1,3,4]"), &[]);
        assert_eq!(
            d.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["/1", "/2"]
        );
    }

    #[test]
    fn equal_values_have_no_differences() {
        let v = j(r#"{"a":[1,{"b":null}],"c":"x"}"#);
        assert!(diff(&v, &v.clone(), &[]).is_empty());
    }
}
