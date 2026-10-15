//! Interpolated documents: data with `var` holes, `$` keys kept verbatim.

use super::config::{CompileCx, ConfigField, FieldKind, Visit};
use super::engine::ExprEngine;
use super::path::Path;
use crate::base::{Issue, IssueCode, Severity};
use datavalue::{DataValue, OwnedDataValue};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::Value;
use std::borrow::Cow;
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
    /// The document as authored, as a `serde_json::Value`: its objects'
    /// keys in that type's order, sorted unless the build turns on
    /// serde_json's `preserve_order`.
    pub fn raw(&self) -> &Value {
        &self.raw
    }

    /// The paths the holes read, in document order. A hole rooted at
    /// `payload` reads nothing and is not listed.
    pub fn holes(&self) -> impl Iterator<Item = &Path> {
        self.holes.iter().filter_map(|h| h.path.as_ref())
    }

    /// The document with each hole filled by `fill`, called once per hole in
    /// document order. Nothing is copied: the skeleton is this document's,
    /// and each hole holds what `fill` lent it.
    pub(crate) fn render_with<'s>(
        &'s self,
        fill: impl FnMut(&'s Hole) -> Filled<'s>,
    ) -> Rendered<'s> {
        Rendered {
            doc: self,
            filled: self.holes.iter().map(fill).collect(),
        }
    }
}

impl fmt::Debug for Interpolated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interpolated")
            .field("raw", &self.raw)
            .finish()
    }
}

/// One hole's value, borrowed where it can be.
pub(crate) enum Filled<'s> {
    /// An owned value: lent from the owned tree, a log or an overlay, or
    /// converted when only a spliced root knew it.
    Owned(Cow<'s, OwnedDataValue>),
    /// A value of the run's view, lent.
    View(DataValue<'s>),
    /// Nothing was there: the hole's default.
    Default(&'s OwnedDataValue),
}

impl<'s> Filled<'s> {
    /// An owned value, lent.
    pub(crate) fn lent(value: &'s OwnedDataValue) -> Self {
        Self::Owned(Cow::Borrowed(value))
    }

    /// The value as an owned one: lent unless it is a view, which is
    /// converted.
    pub(crate) fn into_cow(self) -> Cow<'s, OwnedDataValue> {
        match self {
            Self::Owned(value) => value,
            Self::View(view) => Cow::Owned(view.to_owned()),
            Self::Default(value) => Cow::Borrowed(value),
        }
    }

    fn to_owned_value(&self) -> OwnedDataValue {
        match self {
            Self::Owned(value) => value.as_ref().clone(),
            Self::View(view) => view.to_owned(),
            Self::Default(value) => (*value).clone(),
        }
    }
}

impl Serialize for Filled<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Owned(value) => value.serialize(s),
            Self::View(view) => view.serialize(s),
            Self::Default(value) => value.serialize(s),
        }
    }
}

/// An [`Interpolated`] document with its holes resolved, borrowed from the
/// compiled document and the context the call sees (R54).
///
/// Made by [`Ctx::render`](crate::function::Ctx::render). Each hole was
/// looked up once, when it was made; serializing writes the document's
/// skeleton with each hole's value in place, straight into the serializer
/// (BSON, SQL parameters, JWT claims, JSON), and builds no tree.
/// [`Rendered::to_owned`] builds one for the caller that needs it.
pub struct Rendered<'s> {
    doc: &'s Interpolated,
    /// Each hole's value, in the document's hole order.
    filled: Vec<Filled<'s>>,
}

impl Rendered<'_> {
    /// The document as a tree, each hole's value copied in.
    ///
    /// It serializes as the `Rendered` does, but for one value: an integer
    /// literal above `i64::MAX`, which `OwnedDataValue` holds as a float. So
    /// `18446744073709551615` is written with its digits by the `Rendered`
    /// and as `1.8446744073709552e+19` by this tree.
    pub fn to_owned(&self) -> OwnedDataValue {
        self.doc.node.to_owned_value(&self.filled)
    }

    /// Each hole's value, in hole order.
    #[cfg(test)]
    pub(crate) fn filled(&self) -> &[Filled<'_>] {
        &self.filled
    }
}

impl Serialize for Rendered<'_> {
    /// The skeleton with each hole's value in place: a literal subtree as
    /// its `serde_json::Value` holds it, objects and arrays entry by entry.
    /// An object's keys, a literal's or not, come out in `serde_json::Value`'s
    /// order: sorted, unless the build turns on serde_json's
    /// `preserve_order`, when they are as authored.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        At {
            node: &self.doc.node,
            filled: &self.filled,
        }
        .serialize(s)
    }
}

impl fmt::Debug for Rendered<'_> {
    /// The document as authored and how many holes it filled; never their
    /// values.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rendered")
            .field("raw", &self.doc.raw)
            .field("holes", &self.filled.len())
            .finish_non_exhaustive()
    }
}

/// One node of a rendered document, for its serialization.
struct At<'r, 's> {
    node: &'r Node,
    filled: &'r [Filled<'s>],
}

impl Serialize for At<'_, '_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let at = |node| Self {
            node,
            filled: self.filled,
        };
        match self.node {
            Node::Literal(value) => value.serialize(s),
            Node::Hole(i) => self.filled[*i].serialize(s),
            Node::Object(children) => {
                let mut map = s.serialize_map(Some(children.len()))?;
                for (key, node) in children {
                    map.serialize_entry(key, &at(node))?;
                }
                map.end()
            }
            Node::Array(children) => {
                let mut seq = s.serialize_seq(Some(children.len()))?;
                for node in children {
                    seq.serialize_element(&at(node))?;
                }
                seq.end()
            }
        }
    }
}

impl Node {
    /// The node as a tree, the holes' values from `filled`.
    fn to_owned_value(&self, filled: &[Filled<'_>]) -> OwnedDataValue {
        match self {
            Self::Literal(value) => OwnedDataValue::from(value),
            Self::Hole(i) => filled[*i].to_owned_value(),
            Self::Object(children) => OwnedDataValue::Object(
                children
                    .iter()
                    .map(|(key, node)| (key.clone(), node.to_owned_value(filled)))
                    .collect(),
            ),
            Self::Array(children) => OwnedDataValue::Array(
                children
                    .iter()
                    .map(|node| node.to_owned_value(filled))
                    .collect(),
            ),
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
    /// `null` is a document with no hole.
    const NULL_IS_VALUE: bool = true;
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

    /// `doc` rendered over `context`, an object holding the three roots,
    /// each hole lent from it, a missing one its default; serialized, after
    /// checking that the tree `to_owned` builds serializes the same.
    fn render(doc: &Interpolated, context: &OwnedDataValue) -> String {
        let rendered = doc.render_with(|hole| {
            hole.path
                .as_ref()
                .and_then(|p| p.get(context))
                .map_or(Filled::Default(&hole.default), Filled::lent)
        });
        let wire = serde_json::to_string(&rendered).unwrap();
        assert_eq!(rendered.to_owned().to_json_string(), wire);
        wire
    }

    #[test]
    fn mongo_write_resolves_with_dollar_keys_intact() {
        let raw = json!({
            "filter": { "_id": { "$oid": { "var": "data.payload.object.id" } } },
            "update": { "$set": {
                "deleted": false,
                "payload": { "var": "data.payload" },
                "updated_at": { "$date": { "var": "metadata.timestamp" } }
            } },
            "upsert": true
        });
        let c = ctx(json!({
            "data": {"payload": {"n": 1, "object": {"id": "abc"}}},
            "metadata": {"timestamp": "2026-01-01"},
            "temp_data": {}
        }));
        let out = render(&doc(&raw), &c);
        assert_eq!(
            out,
            r#"{"filter":{"_id":{"$oid":"abc"}},"update":{"$set":{"deleted":false,"payload":{"n":1,"object":{"id":"abc"}},"updated_at":{"$date":"2026-01-01"}}},"upsert":true}"#
        );
    }

    #[test]
    fn a_missing_hole_takes_its_default_or_null() {
        let c = ctx(json!({"data": {}, "metadata": {}, "temp_data": {}}));
        let d = doc(&json!({"a": {"var": ["data.x", 7]}, "b": {"var": "data.y"}}));
        assert_eq!(render(&d, &c), r#"{"a":7,"b":null}"#);
    }

    #[test]
    fn a_payload_hole_always_yields_its_default() {
        let c = ctx(json!({"data": {"x": 1}, "metadata": {}, "temp_data": {}}));
        let d = doc(&json!({"a": {"var": ["payload.x", "d"]}}));
        assert_eq!(render(&d, &c), r#"{"a":"d"}"#);
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
            "h": {"var": ["data.missing", {"var": "data.x"}]},
            "v": {"val": ["data", "x"]}
        }));
        assert_eq!(
            render(&d, &c),
            r#"{"h":{"var":"data.x"},"v":{"val":["data","x"]}}"#
        );
    }

    #[test]
    fn holes_inside_arrays_are_filled() {
        let c = ctx(json!({"data": {"x": 5}, "metadata": {}, "temp_data": {}}));
        let d = doc(&json!([1, {"var": "data.x"}]));
        assert_eq!(render(&d, &c), "[1,5]");
    }
}
