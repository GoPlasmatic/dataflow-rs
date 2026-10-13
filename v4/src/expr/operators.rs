//! The operator table: what datalogic-rs ships, and what each operator does
//! to the way an expression reads its data.

use std::borrow::Cow;

/// The family an operator belongs to: the cargo feature that turns it on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Family {
    /// Always present.
    Core,
    /// The `datetime` feature.
    Datetime,
    /// The `ext-string` feature.
    ExtString,
    /// The `ext-array` feature.
    ExtArray,
    /// The `ext-math` feature.
    ExtMath,
    /// The `ext-control` feature.
    ExtControl,
    /// The `ext-object` feature.
    ExtObject,
    /// The `error-handling` feature.
    ErrorHandling,
    /// The `tensor` feature.
    Tensor,
    /// The `flagd` feature of datalogic-rs.
    Flagd,
    /// The engine's own `secret` operator.
    Secret,
    /// An operator the host registered.
    Custom,
}

/// One operator an engine can evaluate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OperatorInfo {
    /// The name an expression spells it with.
    pub name: Cow<'static, str>,
    /// The family it belongs to.
    pub family: Family,
    /// It evaluates an argument once per element, with `var` reading the element.
    pub scoping: bool,
    /// The same arguments always give the same result.
    pub deterministic: bool,
}

impl OperatorInfo {
    /// A host operator: not scoping, not deterministic, until told otherwise.
    pub fn custom(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            name: name.into(),
            family: Family::Custom,
            scoping: false,
            deterministic: false,
        }
    }

    /// Sets whether the operator evaluates an argument once per element.
    #[must_use]
    pub fn scoping(mut self, yes: bool) -> Self {
        self.scoping = yes;
        self
    }

    /// Sets whether the same arguments always give the same result.
    #[must_use]
    pub fn deterministic(mut self, yes: bool) -> Self {
        self.deterministic = yes;
        self
    }

    pub(crate) fn secret() -> Self {
        Self {
            name: Cow::Borrowed("secret"),
            family: Family::Secret,
            scoping: false,
            deterministic: true,
        }
    }
}

/// A row of [`TABLE`].
pub(crate) struct Row {
    pub(crate) name: &'static str,
    family: Family,
    scoping: bool,
    deterministic: bool,
}

impl Row {
    pub(crate) fn info(&self) -> OperatorInfo {
        OperatorInfo {
            name: Cow::Borrowed(self.name),
            family: self.family,
            scoping: self.scoping,
            deterministic: self.deterministic,
        }
    }
}

const fn r(name: &'static str, family: Family) -> Row {
    let scoping = matches!(
        name.as_bytes(),
        b"filter"
            | b"map"
            | b"reduce"
            | b"all"
            | b"some"
            | b"none"
            | b"sort"
            | b"group_by"
            | b"distinct"
            | b"try"
    );
    Row {
        name,
        family,
        scoping,
        deterministic: !matches!(name.as_bytes(), b"now"),
    }
}

/// Every name in datalogic-rs 5.7's `OPCODE_NAMES`, gated or not, in its order.
/// A build lists only the rows its engine reports in `builtin_operator_names()`.
pub(crate) static TABLE: &[Row] = &[
    r("val", Family::Core),
    r("var", Family::Core),
    r("==", Family::Core),
    r("===", Family::Core),
    r("!=", Family::Core),
    r("!==", Family::Core),
    r(">", Family::Core),
    r(">=", Family::Core),
    r("<", Family::Core),
    r("<=", Family::Core),
    r("!", Family::Core),
    r("!!", Family::Core),
    r("and", Family::Core),
    r("or", Family::Core),
    r("if", Family::Core),
    r("?:", Family::Core),
    r("+", Family::Core),
    r("-", Family::Core),
    r("*", Family::Core),
    r("/", Family::Core),
    r("%", Family::Core),
    r("max", Family::Core),
    r("min", Family::Core),
    r("cat", Family::Core),
    r("substr", Family::Core),
    r("in", Family::Core),
    r("merge", Family::Core),
    r("filter", Family::Core),
    r("map", Family::Core),
    r("reduce", Family::Core),
    r("all", Family::Core),
    r("some", Family::Core),
    r("none", Family::Core),
    r("missing", Family::Core),
    r("missing_some", Family::Core),
    r("datetime", Family::Datetime),
    r("timestamp", Family::Datetime),
    r("parse_date", Family::Datetime),
    r("format_date", Family::Datetime),
    r("date_diff", Family::Datetime),
    r("now", Family::Datetime),
    r("length", Family::ExtString),
    r("starts_with", Family::ExtString),
    r("ends_with", Family::ExtString),
    r("upper", Family::ExtString),
    r("lower", Family::ExtString),
    r("trim", Family::ExtString),
    r("split", Family::ExtString),
    r("sort", Family::ExtArray),
    r("slice", Family::ExtArray),
    r("group_by", Family::ExtArray),
    r("distinct", Family::ExtArray),
    r("keys", Family::ExtObject),
    r("values", Family::ExtObject),
    r("entries", Family::ExtObject),
    r("exists", Family::ExtControl),
    r("??", Family::ExtControl),
    r("switch", Family::ExtControl),
    r("match", Family::ExtControl),
    r("type", Family::ExtControl),
    r("try", Family::ErrorHandling),
    r("throw", Family::ErrorHandling),
    r("abs", Family::ExtMath),
    r("ceil", Family::ExtMath),
    r("floor", Family::ExtMath),
    r("tensor", Family::Tensor),
    r("zeros", Family::Tensor),
    r("full", Family::Tensor),
    r("scatter", Family::Tensor),
    r("rle_expand", Family::Tensor),
    r("one_hot", Family::Tensor),
    r("stack", Family::Tensor),
    r("concat", Family::Tensor),
    r("unstack", Family::Tensor),
    r("reshape", Family::Tensor),
    r("transpose", Family::Tensor),
    r("pad", Family::Tensor),
    r("crop", Family::Tensor),
    r("cast", Family::Tensor),
    r("normalize", Family::Tensor),
    r("argmax", Family::Tensor),
    r("gather", Family::Tensor),
    r("to_list", Family::Tensor),
    r("shape", Family::Tensor),
    r("dtype", Family::Tensor),
    r("fractional", Family::Flagd),
    r("sem_ver", Family::Flagd),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprEngine;
    use serde_json::{Value, json};

    fn probe(engine: &ExprEngine, logic: Value) -> Value {
        engine.eval_json(&logic)
    }

    fn rows() -> Value {
        json!([{"a": 1}, {"a": 2}])
    }

    #[test]
    fn the_table_has_unique_names_and_the_documented_flags() {
        let mut names: Vec<&str> = TABLE.iter().map(|r| r.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate row");
        assert_eq!(total, 87, "one row per OPCODE_NAMES entry");
        let scoping: Vec<&str> = TABLE.iter().filter(|r| r.scoping).map(|r| r.name).collect();
        assert_eq!(
            scoping,
            [
                "filter", "map", "reduce", "all", "some", "none", "sort", "group_by", "distinct",
                "try"
            ]
        );
        let non: Vec<&str> = TABLE
            .iter()
            .filter(|r| !r.deterministic)
            .map(|r| r.name)
            .collect();
        assert_eq!(non, ["now"]);
    }

    #[test]
    fn core_scoping_operators_read_the_element() {
        let e = ExprEngine::default();
        let a = rows();
        assert_eq!(probe(&e, json!({"map": [a, {"var": "a"}]})), json!([1, 2]));
        assert_eq!(
            probe(&e, json!({"filter": [a, {">": [{"var": "a"}, 1]}]})),
            json!([{"a": 2}])
        );
        assert_eq!(
            probe(&e, json!({"all": [a, {">": [{"var": "a"}, 0]}]})),
            json!(true)
        );
        assert_eq!(
            probe(&e, json!({"some": [a, {">": [{"var": "a"}, 1]}]})),
            json!(true)
        );
        assert_eq!(
            probe(&e, json!({"none": [a, {">": [{"var": "a"}, 5]}]})),
            json!(true)
        );
        assert_eq!(
            probe(
                &e,
                json!({"reduce": [a, {"+": [{"var": "current.a"}, {"var": "accumulator"}]}, 0]})
            ),
            json!(3)
        );
    }

    #[cfg(feature = "ext-array")]
    #[test]
    fn ext_array_scoping_operators_read_the_element() {
        let e = ExprEngine::default();
        let a = rows();
        assert_eq!(
            probe(&e, json!({"sort": [a, false, {"var": "a"}]})),
            json!([{"a": 2}, {"a": 1}])
        );
        assert_eq!(
            probe(&e, json!({"group_by": [a, {"var": "a"}]})),
            json!([
                {"key": 1, "items": [{"a": 1}]},
                {"key": 2, "items": [{"a": 2}]}
            ])
        );
    }
}
