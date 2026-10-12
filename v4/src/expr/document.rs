//! Interpolated documents: data with `var` holes, `$` keys kept verbatim.

use super::config::{CompileCx, ConfigField, FieldKind, Visit};
use super::engine::ExprEngine;
use super::path::Path;
use crate::base::{Issue, IssueCode, Severity};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::fmt;
use std::str::FromStr;

/// A document of `var` holes in otherwise literal data, `$` keys kept
/// verbatim. It is for payloads such as a MongoDB filter, where `$oid` is data
/// and `{"var": ...}` is the only live part.
///
/// Any other operator-shaped node, `val` included, stays data and is reported
/// as `OPERATOR_IN_DOCUMENT`; a `secret` node is refused as
/// `SECRET_IN_DOCUMENT`. A hole's default is data too, and is checked the same
/// way.
pub struct Interpolated {
    raw: Value,
    node: Node,
    holes: Vec<Hole>,
}

/// A hole: the path it reads and what it yields when that is missing.
pub(crate) struct Hole {
    /// `None` when rooted at `payload`, which is never in the context: the
    /// hole always yields its default.
    pub path: Option<Path>,
    pub default: OwnedDataValue,
}

enum Node {
    /// A subtree with no hole, kept as authored.
    Literal(Value),
    Hole(usize),
    Object(Vec<(String, Self)>),
    Array(Vec<Self>),
}

impl Interpolated {
    /// The document as authored.
    pub fn raw(&self) -> &Value {
        &self.raw
    }

    /// The paths the holes read, in document order. A hole rooted at
    /// `payload` reads nothing and is not listed.
    pub fn holes(&self) -> impl Iterator<Item = &Path> {
        self.holes.iter().filter_map(|h| h.path.as_ref())
    }

    /// The document with every hole filled from `context`, an object holding
    /// the three roots; a missing hole takes its default.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the renderer of phase 5 is its caller")
    )]
    pub(crate) fn resolve_owned(&self, context: &OwnedDataValue) -> OwnedDataValue {
        self.node.resolve(&self.holes, context)
    }
}

impl fmt::Debug for Interpolated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interpolated")
            .field("raw", &self.raw)
            .finish()
    }
}

impl Node {
    fn resolve(&self, holes: &[Hole], context: &OwnedDataValue) -> OwnedDataValue {
        match self {
            Self::Literal(v) => OwnedDataValue::from(v),
            Self::Hole(i) => {
                let hole = &holes[*i];
                hole.path
                    .as_ref()
                    .and_then(|p| p.get(context))
                    .cloned()
                    .unwrap_or_else(|| hole.default.clone())
            }
            Self::Object(children) => OwnedDataValue::Object(
                children
                    .iter()
                    .map(|(k, n)| (k.clone(), n.resolve(holes, context)))
                    .collect(),
            ),
            Self::Array(children) => {
                OwnedDataValue::Array(children.iter().map(|n| n.resolve(holes, context)).collect())
            }
        }
    }
}

/// Compiles a document, recording each issue at its node. `None` when any of
/// them rejects.
pub(crate) fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Interpolated> {
    let before = cx.rejected();
    let mut holes = Vec::new();
    let node = walk(raw, cx, Some(&mut holes));
    (cx.rejected() == before).then(|| Interpolated {
        raw: raw.clone(),
        node,
        holes,
    })
}

/// Walks one node. `holes` is `None` inside a hole's default, which is data
/// through and through: a `var` there opens no hole, and is an operator left
/// as data like any other.
fn walk(raw: &Value, cx: &mut CompileCx<'_>, mut holes: Option<&mut Vec<Hole>>) -> Node {
    match raw {
        Value::Object(o) => {
            if o.len() == 1 {
                let (key, arg) = o.iter().next().expect("one entry");
                if key == "var"
                    && let Some(holes) = holes.as_deref_mut()
                {
                    return hole(raw, arg, cx, holes);
                }
                match key.as_str() {
                    "secret" => {
                        cx.issue(
                            IssueCode::SecretInDocument,
                            "a document cannot read a secret",
                        );
                        return Node::Literal(raw.clone());
                    }
                    name if cx.exprs().operators().iter().any(|op| op.name == name) => {
                        cx.at(name, |cx| {
                            cx.issue(
                                IssueCode::OperatorInDocument,
                                format!(
                                    "`{name}` is not evaluated in a document, it stays data; \
                                     only `var` holes are live"
                                ),
                            );
                        });
                    }
                    _ => {}
                }
            }
            let children: Vec<(String, Node)> = o
                .iter()
                .map(|(k, v)| {
                    let node = cx.at(k.as_str(), |cx| walk(v, cx, holes.as_deref_mut()));
                    (k.clone(), node)
                })
                .collect();
            if children.iter().all(|(_, n)| matches!(n, Node::Literal(_))) {
                Node::Literal(raw.clone())
            } else {
                Node::Object(children)
            }
        }
        Value::Array(a) => {
            let children: Vec<Node> = a
                .iter()
                .enumerate()
                .map(|(i, v)| cx.at(i, |cx| walk(v, cx, holes.as_deref_mut())))
                .collect();
            if children.iter().all(|n| matches!(n, Node::Literal(_))) {
                Node::Literal(raw.clone())
            } else {
                Node::Array(children)
            }
        }
        other => Node::Literal(other.clone()),
    }
}

/// A `var` node: a hole reading its path, its default walked as data.
fn hole(raw: &Value, arg: &Value, cx: &mut CompileCx<'_>, holes: &mut Vec<Hole>) -> Node {
    let (path, default) = match arg {
        Value::String(p) => (Some(p.as_str()), None),
        Value::Array(a) if matches!(a.len(), 1 | 2) => (a[0].as_str(), a.get(1)),
        _ => (None, None),
    };
    let parsed = match path {
        None => Err("`var` takes a path string, or [path] or [path, default]".to_owned()),
        Some(path) if path.split('.').next() == Some("payload") => {
            cx.issue(
                IssueCode::PayloadVar,
                format!("`{path}` reads the payload, which is not in the context; the hole always yields its default"),
            );
            Ok(None)
        }
        Some(path) => Path::from_str(path)
            .map(Some)
            .map_err(|e| e.message().to_owned()),
    };
    if let Err(message) = &parsed {
        cx.issue(IssueCode::InvalidPath, message.clone());
    }
    // The default is never evaluated: a secret or an operator in it would be
    // handed on as written, so it is reported as anywhere else in the
    // document.
    if let Some(default) = default {
        cx.at("var", |cx| cx.at(1, |cx| walk(default, cx, None)));
    }
    let Ok(path) = parsed else {
        return Node::Literal(raw.clone());
    };
    holes.push(Hole {
        path,
        default: default.map_or(OwnedDataValue::Null, OwnedDataValue::from),
    });
    Node::Hole(holes.len() - 1)
}

impl ConfigField for Interpolated {
    fn compile(raw: &Value, cx: &mut CompileCx<'_>) -> Option<Self> {
        compile(raw, cx)
    }
    fn visit(&self, v: &mut Visit<'_>) {
        v.interpolated(self);
    }
    fn kind() -> FieldKind {
        FieldKind::Interpolated
    }
}

impl ExprEngine {
    /// Compiles a document of `var` holes in otherwise literal data.
    ///
    /// Advisories and Defects (a payload hole, an operator left as data) are
    /// dropped on `Ok`; a config field compiled through [`ConfigField`] is the
    /// way to get them.
    ///
    /// # Errors
    ///
    /// Every issue, when any is Rejected.
    pub fn interpolated(&self, raw: &Value) -> Result<Interpolated, Vec<Issue>> {
        let mut issues = Vec::new();
        let doc = {
            let mut cx = CompileCx::new(self, &mut issues);
            compile(raw, &mut cx)
        };
        match doc {
            Some(doc) if !issues.iter().any(|i| i.severity == Severity::Rejected) => Ok(doc),
            _ => Err(issues),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(raw: &Value) -> Interpolated {
        ExprEngine::default().interpolated(raw).unwrap()
    }

    fn ctx(v: Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    #[test]
    fn mongo_write_resolves_with_dollar_keys_intact() {
        let raw = json!({
            "filter": { "_id": { "$oid": { "var": "data.payload.object.id" } } },
            "update": { "$set": {
                "payload": { "var": "data.payload" },
                "updated_at": { "$date": { "var": "metadata.timestamp" } },
                "deleted": false
            } },
            "upsert": true
        });
        let c = ctx(json!({
            "data": {"payload": {"object": {"id": "abc"}, "n": 1}},
            "metadata": {"timestamp": "2026-01-01"},
            "temp_data": {}
        }));
        let out = doc(&raw).resolve_owned(&c).to_json_string();
        assert_eq!(
            out,
            r#"{"filter":{"_id":{"$oid":"abc"}},"update":{"$set":{"deleted":false,"payload":{"n":1,"object":{"id":"abc"}},"updated_at":{"$date":"2026-01-01"}}},"upsert":true}"#
        );
    }

    #[test]
    fn a_missing_hole_takes_its_default_or_null() {
        let c = ctx(json!({"data": {}, "metadata": {}, "temp_data": {}}));
        let d = doc(&json!({"a": {"var": ["data.x", 7]}, "b": {"var": "data.y"}}));
        assert_eq!(d.resolve_owned(&c).to_json_string(), r#"{"a":7,"b":null}"#);
    }

    #[test]
    fn a_payload_hole_always_yields_its_default() {
        let c = ctx(json!({"data": {"x": 1}, "metadata": {}, "temp_data": {}}));
        let d = doc(&json!({"a": {"var": ["payload.x", "d"]}}));
        assert_eq!(d.resolve_owned(&c).to_json_string(), r#"{"a":"d"}"#);
    }

    #[test]
    fn a_hole_free_subtree_is_one_literal_node() {
        let d = doc(&json!({"a": [1, {"b": 2}], "c": {"var": "data.x"}}));
        assert_eq!(d.holes.len(), 1);
        match &d.node {
            Node::Object(children) => {
                assert!(matches!(children[0].1, Node::Literal(_)));
                assert!(matches!(children[1].1, Node::Hole(0)));
            }
            _ => panic!("expected an object node"),
        }
    }

    #[test]
    fn a_val_and_an_operator_shaped_default_resolve_as_written() {
        let c = ctx(json!({"data": {"x": 5}, "metadata": {}, "temp_data": {}}));
        let d = doc(&json!({
            "v": {"val": ["data", "x"]},
            "h": {"var": ["data.missing", {"var": "data.x"}]}
        }));
        assert_eq!(
            d.resolve_owned(&c).to_json_string(),
            r#"{"h":{"var":"data.x"},"v":{"val":["data","x"]}}"#
        );
    }

    #[test]
    fn holes_inside_arrays_are_filled() {
        let c = ctx(json!({"data": {"x": 5}, "metadata": {}, "temp_data": {}}));
        let d = doc(&json!([1, {"var": "data.x"}]));
        assert_eq!(d.resolve_owned(&c).to_json_string(), "[1,5]");
    }
}
