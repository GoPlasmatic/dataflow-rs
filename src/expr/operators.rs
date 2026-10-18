//! The operators an engine evaluates, as datalogic-rs describes its own
//! (`Engine::operators`), plus the engine's `secret` and the host's.

use datalogic_rs::ScopedArg;
use std::borrow::Cow;

/// The family an operator belongs to: the cargo feature that turns it on.
///
/// This crate's own, not `datalogic_rs::Family`: it adds the engine's
/// [`Secret`](Self::Secret) and a host's [`Custom`](Self::Custom). A host
/// that imports both aliases one.
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
    /// The engine's own `secret` operator.
    Secret,
    /// An operator the host registered.
    Custom,
}

impl Family {
    /// The family of datalogic's family `upstream`. `None` for one this
    /// crate has no feature for, which the engine never turns on.
    fn of(upstream: datalogic_rs::Family) -> Option<Self> {
        use datalogic_rs::Family as F;
        Some(match upstream {
            F::Core => Self::Core,
            F::DateTime => Self::Datetime,
            F::ExtString => Self::ExtString,
            F::ExtArray => Self::ExtArray,
            F::ExtMath => Self::ExtMath,
            F::ExtControl => Self::ExtControl,
            F::ExtObject => Self::ExtObject,
            F::ErrorHandling => Self::ErrorHandling,
            F::Tensor => Self::Tensor,
            _ => return None,
        })
    }

    /// The family datalogic names `name` in an operator's description.
    fn named(name: &str) -> Option<Self> {
        datalogic_rs::Family::ALL
            .iter()
            .find(|f| f.name() == name)
            .and_then(|f| Self::of(*f))
    }
}

/// The datalogic families this build turns on: each by this crate's feature
/// of its name, and no other. A family another crate's features compile into
/// datalogic stays off here, so it can never change what a definition does.
pub(crate) const FAMILIES: &[datalogic_rs::Family] = &[
    #[cfg(feature = "datetime")]
    datalogic_rs::Family::DateTime,
    #[cfg(feature = "ext-string")]
    datalogic_rs::Family::ExtString,
    #[cfg(feature = "ext-array")]
    datalogic_rs::Family::ExtArray,
    #[cfg(feature = "ext-math")]
    datalogic_rs::Family::ExtMath,
    #[cfg(feature = "ext-control")]
    datalogic_rs::Family::ExtControl,
    #[cfg(feature = "ext-object")]
    datalogic_rs::Family::ExtObject,
    #[cfg(feature = "error-handling")]
    datalogic_rs::Family::ErrorHandling,
    #[cfg(feature = "tensor")]
    datalogic_rs::Family::Tensor,
];

/// One operator an engine can evaluate.
///
/// This crate's own, not `datalogic_rs::OperatorInfo`, which it is built
/// from: its [`family`](Self::family) is this crate's [`Family`].
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
    /// Which argument a datalogic operator evaluates per element: its
    /// `scoped_arg`. A host's scoping operator declares none.
    pub(crate) scoped: Option<ScopedArg>,
}

impl OperatorInfo {
    /// A host operator: not scoping, not deterministic, until told otherwise.
    pub fn custom(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            name: name.into(),
            family: Family::Custom,
            scoping: false,
            deterministic: false,
            scoped: None,
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
            name: Cow::Borrowed(super::secrets::SECRET_OPERATOR),
            family: Family::Secret,
            scoping: false,
            deterministic: true,
            scoped: None,
        }
    }

    /// One row per name datalogic's operator answers to, its canonical name
    /// first, then each alias (`val`, then `var`): an expression may spell
    /// it either way. Scoping is its `scoped_arg`; only the clock makes it
    /// non-deterministic.
    pub(crate) fn from_logic(op: datalogic_rs::OperatorInfo) -> impl Iterator<Item = Self> {
        let family = Family::named(op.family);
        debug_assert!(
            family.is_some(),
            "`{}` is in family `{}`, which no feature turns on",
            op.name,
            op.family
        );
        std::iter::once(op.name)
            .chain(op.aliases.iter().copied())
            .filter(move |_| family.is_some())
            .map(move |name| Self {
                name: Cow::Borrowed(name),
                family: family.unwrap_or(Family::Core),
                scoping: op.scoped_arg.is_some(),
                deterministic: op.effect != "clock",
                scoped: op.scoped_arg,
            })
    }
}

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

    /// Every operator this build lists, as `ExprEngine::operators` does.
    fn listed() -> Vec<OperatorInfo> {
        ExprEngine::default().operators().to_vec()
    }

    #[test]
    fn the_expanded_names_are_datalogics_in_its_order_with_no_duplicate() {
        let logic = datalogic_rs::Engine::builder()
            .with_families(FAMILIES.iter().copied())
            .build();
        let ours: Vec<String> = listed()
            .into_iter()
            .filter(|o| o.family != Family::Secret)
            .map(|o| o.name.into_owned())
            .collect();
        let mut theirs: Vec<&str> = logic.builtin_operator_names().collect();
        let mut sorted = ours.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ours.len(), "a name twice");
        theirs.sort_unstable();
        assert_eq!(
            sorted, theirs,
            "every name datalogic answers to, aliases included"
        );
        // An alias follows its operator.
        let at = |name: &str| ours.iter().position(|n| n == name).unwrap();
        assert_eq!(at("var"), at("val") + 1);
        assert_eq!(at("?:"), at("if") + 1);
    }

    #[test]
    fn the_scoping_and_the_clock_are_datalogics_facts() {
        let ops = listed();
        let scoping: Vec<&str> = ops.iter().filter(|o| o.scoping).map(|o| &*o.name).collect();
        let non: Vec<&str> = ops
            .iter()
            .filter(|o| !o.deterministic)
            .map(|o| &*o.name)
            .collect();
        let mut want_scoping = vec!["filter", "map", "reduce", "all", "some", "none"];
        let mut want_non = Vec::new();
        if cfg!(feature = "ext-array") {
            want_scoping.extend(["sort", "group_by", "distinct"]);
        }
        if cfg!(feature = "error-handling") {
            want_scoping.push("try");
        }
        if cfg!(feature = "datetime") {
            want_non.push("now");
        }
        let mut scoping = scoping;
        scoping.sort_unstable();
        want_scoping.sort_unstable();
        assert_eq!(scoping, want_scoping);
        assert_eq!(non, want_non);
        let at = |name: &str| ops.iter().find(|o| o.name == name).unwrap().scoped;
        assert_eq!(at("map"), Some(ScopedArg::Index(1)));
        assert_eq!(at("reduce"), Some(ScopedArg::Index(1)));
        #[cfg(feature = "ext-array")]
        assert_eq!(at("sort"), Some(ScopedArg::Index(2)));
        #[cfg(feature = "error-handling")]
        assert_eq!(at("try"), Some(ScopedArg::LastOfMany));
    }

    #[test]
    fn every_family_datalogic_has_but_flagd_is_one_of_ours() {
        // A family datalogic adds fails here until it is mapped or named
        // unwanted; `flagd` is feature-flag specific and never turned on.
        for family in datalogic_rs::Family::ALL {
            let mapped = Family::of(*family);
            assert_eq!(
                mapped.is_none(),
                family.name() == "Flagd",
                "{}",
                family.name()
            );
        }
    }

    #[test]
    fn the_effects_are_the_ones_this_crate_reads() {
        // `deterministic` reads `effect`: a new effect fails here at the
        // bump that brings it, rather than silently counting as pure.
        let logic = datalogic_rs::Engine::builder()
            .with_families(FAMILIES.iter().copied())
            .build();
        let mut effects: Vec<&str> = logic.operators().map(|o| o.effect).collect();
        effects.sort_unstable();
        effects.dedup();
        for effect in effects {
            assert!(
                ["pure", "clock", "throws", "catches"].contains(&effect),
                "an effect this crate does not read: {effect}"
            );
        }
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
