//! A task's `for_each`: at most one issue, its first problem, in v3's order
//! (`ForEach::problem`) with v3's messages, then its unknown keys.

use super::BUILTIN_FUNCTIONS;
use super::keys::{Ext, FOR_EACH};
use super::loops::{is_slot_path, over_literal_problem, slots_overlap};
use super::parse::Walk;
use super::types::{ForEachDef, Logic, defaults};
use crate::base::{AuthoredPath, IssueCode, ROOTS};
use serde_json::{Map, Value};

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

/// `collect` and `into`, beside the binding `item`: given together, each a
/// context path below a root, and overlapping neither each other nor a
/// binding.
fn slots_problem(collect: Option<&str>, into: Option<&str>, item: &str) -> Option<Problem> {
    let (collect, into) = match (collect, into) {
        (None, None) => return None,
        (Some(collect), Some(into)) => (collect, into),
        (collect, _) => {
            return Some(invalid(
                if collect.is_some() { "collect" } else { "into" },
                "for_each collect and into go together: collect names what each call writes, \
                 into names the array the results land in",
            ));
        }
    };
    for (field, path) in [("collect", collect), ("into", into)] {
        if !is_context_path(path) {
            return Some(invalid(
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
            return Some(invalid(
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
            return Some(invalid(
                "into",
                format!(
                    "for_each into ({into:?}) and {other:?} overlap — the results array would \
                     overwrite, or be overwritten by, a slot written per call"
                ),
            ));
        }
    }
    None
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

    if let Some(problem) = over_literal_problem("for_each", over) {
        return Err(invalid("over", problem));
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
        None => defaults::MAX_CONCURRENCY,
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
    if let Some(problem) = slots_problem(collect, into, item) {
        return Err(problem);
    }
    Ok(ForEachDef {
        over: Logic(over.clone()),
        r#as: item.to_string(),
        max_concurrency,
        collect: collect.map(str::to_string),
        into: into.map(str::to_string),
    })
}

impl Problem {
    /// The problem as an issue of the `for_each` at `at`: at its key, or at
    /// `at` itself for the whole.
    fn at(self, at: &AuthoredPath) -> (IssueCode, AuthoredPath, String) {
        let path = if self.field.is_empty() {
            at.clone()
        } else {
            at.join(self.field)
        };
        (self.code, path, self.message)
    }
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
                let (code, path, message) = problem.at(&at);
                self.report(code, path, message);
                None
            }
        };
        if let Some(obj) = raw.as_object() {
            self.unknown_keys(obj, &FOR_EACH, &[], &at, "for_each.", ext);
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
