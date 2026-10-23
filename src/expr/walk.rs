//! Two pure walks over authored JSON, for the checks that need them.

use super::engine::TEMPLATE_KEY_ESCAPE;
use super::secrets::SECRET_OPERATOR as SECRET;
use crate::base::AuthoredPath;
use serde_json::Value;

/// Every object key that starts with exactly one `$`, at any depth.
///
/// Each object's own keys come before the keys of the values below it. The
/// engine strips one `$` from every template key, so these are the keys that
/// emit something other than what was written.
pub fn escaped_keys(raw: &Value) -> impl Iterator<Item = AuthoredPath> + '_ {
    let mut out = Vec::new();
    let mut at = AuthoredPath::root();
    walk_escaped(raw, &mut at, &mut out);
    out.into_iter()
}

fn walk_escaped(value: &Value, at: &mut AuthoredPath, out: &mut Vec<AuthoredPath>) {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                at.push(i);
                walk_escaped(item, at, out);
                at.pop();
            }
        }
        Value::Object(map) => {
            for key in map.keys() {
                let mut chars = key.chars();
                if chars.next() == Some(TEMPLATE_KEY_ESCAPE)
                    && chars.next() != Some(TEMPLATE_KEY_ESCAPE)
                {
                    out.push(at.join(key.as_str()));
                }
            }
            for (key, child) in map {
                at.push(key.as_str());
                walk_escaped(child, at, out);
                at.pop();
            }
        }
        _ => {}
    }
}

/// Every `{"secret": …}` reference, as the literal name (`None` when the name
/// is computed) and where the reference sits. Lazy, so a caller that only
/// asks whether one exists (via `next()`) stops at the first.
///
/// The walk goes into the argument too, since a computed name may itself read
/// a secret.
pub fn secret_refs(raw: &Value) -> impl Iterator<Item = (Option<&str>, AuthoredPath)> + '_ {
    // An explicit stack, not recursion: `next()` must stop the walk where the
    // caller stops asking, so a caller that only needs the first reference
    // does not walk the whole expression. Children are pushed in reverse, so
    // they pop in v3's order.
    let mut stack = if reads_secret(raw) {
        vec![(raw, AuthoredPath::root())]
    } else {
        Vec::new()
    };
    std::iter::from_fn(move || {
        while let Some((value, at)) = stack.pop() {
            match value {
                Value::Object(map) => {
                    if map.len() == 1
                        && let Some(arg) = map.get(SECRET)
                    {
                        let literal = match arg {
                            Value::String(s) => Some(s.as_str()),
                            Value::Array(items) if items.len() == 1 => items[0].as_str(),
                            _ => None,
                        };
                        stack.push((arg, at.join(SECRET)));
                        return Some((literal, at));
                    }
                    for (key, child) in map.iter().rev() {
                        stack.push((child, at.join(key.as_str())));
                    }
                }
                Value::Array(items) => {
                    for (i, child) in items.iter().enumerate().rev() {
                        stack.push((child, at.join(i)));
                    }
                }
                _ => {}
            }
        }
        None
    })
}

/// Whether `raw` reads a secret anywhere, a computed name included: whether
/// [`secret_refs`] finds any, checked without allocating paths.
pub(crate) fn reads_secret(raw: &Value) -> bool {
    match raw {
        Value::Object(map) => {
            (map.len() == 1 && map.contains_key(SECRET)) || map.values().any(reads_secret)
        }
        Value::Array(items) => items.iter().any(reads_secret),
        _ => false,
    }
}
