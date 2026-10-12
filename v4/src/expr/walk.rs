//! Two pure walks over authored JSON, for the checks that need them.

use crate::base::AuthoredPath;
use serde_json::Value;

/// The operator that reads a secret.
const SECRET: &str = "secret";

/// Every object key that starts with exactly one `$`, at any depth.
///
/// Each object's own keys come before the keys of the values below it. The
/// engine strips one `$` from every template key, so these are the keys that
/// emit something other than what was written.
pub fn escaped_keys(raw: &Value) -> impl Iterator<Item = AuthoredPath> + '_ {
    let mut out = Vec::new();
    walk_escaped(raw, &AuthoredPath::root(), &mut out);
    out.into_iter()
}

fn walk_escaped(value: &Value, at: &AuthoredPath, out: &mut Vec<AuthoredPath>) {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_escaped(item, &at.join(i), out);
            }
        }
        Value::Object(map) => {
            for key in map.keys() {
                if key.starts_with('$') && !key.starts_with("$$") {
                    out.push(at.join(key.as_str()));
                }
            }
            for (key, child) in map {
                walk_escaped(child, &at.join(key.as_str()), out);
            }
        }
        _ => {}
    }
}

/// Every `{"secret": …}` reference, as the literal name (`None` when the name
/// is computed) and where the reference sits.
///
/// The walk goes into the argument too, since a computed name may itself read
/// a secret.
pub fn secret_refs(raw: &Value) -> impl Iterator<Item = (Option<&str>, AuthoredPath)> + '_ {
    let mut out = Vec::new();
    walk_secrets(raw, &AuthoredPath::root(), &mut out);
    out.into_iter()
}

fn walk_secrets<'v>(
    value: &'v Value,
    at: &AuthoredPath,
    out: &mut Vec<(Option<&'v str>, AuthoredPath)>,
) {
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
                out.push((literal, at.clone()));
                walk_secrets(arg, &at.join(SECRET), out);
                return;
            }
            for (key, child) in map {
                walk_secrets(child, &at.join(key.as_str()), out);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                walk_secrets(child, &at.join(i), out);
            }
        }
        _ => {}
    }
}
