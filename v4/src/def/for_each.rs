//! A task's `for_each`: at most one issue, its first problem, in v3's order
//! (`ForEach::problem`) with v3's messages, then its unknown keys.

use super::BUILTIN_FUNCTIONS;
use super::keys::{Ext, FOR_EACH_KEYS};
use super::loops::{is_slot_path, over_literal_problem, slots_overlap};
use super::parse::Walk;
use super::types::{ForEachDef, Logic};
use crate::base::{AuthoredPath, IssueCode};
use serde_json::{Map, Value};

/// The three roots every context path starts at.
const ROOTS: [&str; 3] = ["data", "metadata", "temp_data"];

/// Whether `path` names a key below a context root, such as `temp_data.m`.
fn is_context_path(path: &str) -> bool {
    let Some((root, rest)) = path.split_once('.') else {
        return false;
    };
    ROOTS.contains(&root) && is_slot_path(rest)
}

/// The first rule a `for_each` breaks: the key it is reported at under
/// `for_each` (empty for the whole), its code and its message.
struct Problem {
    field: &'static str,
    code: IssueCode,
    message: String,
}

fn invalid(field: &'static str, message: impl Into<String>) -> Problem {
    Problem {
        field,
        code: IssueCode::InvalidForEach,
        message: message.into(),
    }
}

fn parse_failed(field: &'static str, message: impl Into<String>) -> Problem {
    Problem {
        field,
        code: IssueCode::ParseFailed,
        message: message.into(),
    }
}

/// An optional string: absent or `null` is `None`.
fn opt_str<'v>(
    obj: &'v Map<String, Value>,
    field: &'static str,
) -> Result<Option<&'v str>, Problem> {
    match obj.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(parse_failed(field, "expected a string")),
    }
}

/// Reads a `for_each` object, or its first problem. The built-in rule, which
/// needs the task's function, is the caller's and comes first.
fn read(obj: &Map<String, Value>) -> Result<ForEachDef, Problem> {
    let over = obj
        .get("over")
        .ok_or_else(|| parse_failed("over", "missing field `over`"))?;
    let item = obj
        .get("as")
        .ok_or_else(|| parse_failed("as", "missing field `as`"))?;

    if let Some(problem) = over_literal_problem(over) {
        return Err(invalid(
            "over",
            problem.replacen("loop over", "for_each over", 1),
        ));
    }
    let item = item
        .as_str()
        .ok_or_else(|| parse_failed("as", "expected a string"))?;
    if !is_slot_path(item) {
        return Err(invalid(
            "as",
            format!("for_each as must be a non-empty temp_data field path, got {item:?}"),
        ));
    }
    let max_concurrency = match obj.get("max_concurrency") {
        None => 1,
        Some(raw) => raw
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| parse_failed("max_concurrency", "expected a non-negative integer"))?,
    };
    if max_concurrency == 0 {
        return Err(invalid(
            "max_concurrency",
            "for_each max_concurrency must be at least 1",
        ));
    }

    let collect = opt_str(obj, "collect")?;
    let into = opt_str(obj, "into")?;
    let def = ForEachDef {
        over: Logic(over.clone()),
        r#as: item.to_string(),
        max_concurrency,
        collect: collect.map(str::to_string),
        into: into.map(str::to_string),
    };
    let (collect, into) = match (collect, into) {
        (None, None) => return Ok(def),
        (Some(collect), Some(into)) => (collect, into),
        (collect, _) => {
            return Err(invalid(
                if collect.is_some() { "collect" } else { "into" },
                "for_each collect and into go together: collect names what each call writes, \
                 into names the array the results land in",
            ));
        }
    };
    for (field, path) in [("collect", collect), ("into", into)] {
        if !is_context_path(path) {
            return Err(invalid(
                field,
                format!(
                    "for_each {field} must be a context path below data, metadata or \
                     temp_data, got {path:?}"
                ),
            ));
        }
    }
    let item = format!("temp_data.{item}");
    let index = format!("{item}_index");
    for binding in [&item, &index] {
        if slots_overlap(collect, binding) {
            return Err(invalid(
                "collect",
                format!(
                    "for_each collect ({collect:?}) and {binding:?} overlap — the bindings live \
                     only in each call's copy, and a result written under one would be replayed \
                     into the message and left behind"
                ),
            ));
        }
    }
    for other in [collect, &item, &index] {
        if slots_overlap(into, other) {
            return Err(invalid(
                "into",
                format!(
                    "for_each into ({into:?}) and {other:?} overlap — the results array would \
                     overwrite, or be overwritten by, a slot written per call"
                ),
            ));
        }
    }
    Ok(def)
}

impl Walk<'_> {
    /// 4.5 on a task: the `for_each` at `<node>.for_each`, then its unknown
    /// keys. Absent or `null` is no fan-out.
    pub(super) fn for_each(
        &mut self,
        raw: Option<&Value>,
        function: Option<&str>,
        node: &AuthoredPath,
        ext: &mut Ext,
    ) -> Option<ForEachDef> {
        let raw = match raw {
            None | Some(Value::Null) => return None,
            Some(raw) => raw,
        };
        let at = node.join("for_each");
        let read = match (function, raw.as_object()) {
            (Some(name), _) if BUILTIN_FUNCTIONS.contains(&name) => Err(invalid(
                "",
                format!(
                    "for_each runs a handler-backed function once per element, and '{name}' is \
                     a built-in that runs inline — use JSONLogic's map, filter or reduce over \
                     the array instead"
                ),
            )),
            (_, None) => Err(parse_failed("", "expected an object")),
            (_, Some(obj)) => read(obj),
        };
        let def = match read {
            Ok(def) => Some(def),
            Err(problem) => {
                let path = if problem.field.is_empty() {
                    at.clone()
                } else {
                    at.join(problem.field)
                };
                self.report(problem.code, path, problem.message);
                None
            }
        };
        if let Some(obj) = raw.as_object() {
            self.unknown_keys(obj, FOR_EACH_KEYS, &[], &at, "for_each.", ext);
        }
        def
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_path_is_a_slot_below_a_root() {
        for good in ["data.x", "metadata.a.b", "temp_data.m"] {
            assert!(is_context_path(good), "{good}");
        }
        for bad in ["data", "temp_data.", "payload.x", "m", "data..x", "Data.x"] {
            assert!(!is_context_path(bad), "{bad}");
        }
    }
}
