//! `ExprEngine`: the v4 wrapper around one datalogic-rs engine.

use super::arena::with_arena;
use super::expr::{Evaluated, Expr, FromValue};
use super::operators::{ALL_OPERATORS, Operator, OperatorFamily};
use super::secrets::{SECRET_OPERATOR, SecretOperator, Secrets};
use crate::base::{Error, ErrorKind, Result};
use bumpalo::Bump;
use datalogic_rs::{CustomOperator, DataValue, Engine, Logic};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The key prefix that escapes a template key into data: `{"$cat": …}` is the
/// literal object, `{"cat": …}` the operator. Always on, applied at the one
/// place an engine is built, so an engine in a test agrees with one that runs.
pub(super) const TEMPLATE_KEY_ESCAPE: char = '$';

/// Adapter handing a shared operator to the datalogic builder, which takes
/// ownership of what it registers.
struct SharedOperator(Arc<dyn CustomOperator>);

impl CustomOperator for SharedOperator {
    #[inline]
    fn evaluate<'a>(
        &self,
        args: &[&'a datalogic_rs::DataValue<'a>],
        ctx: &mut datalogic_rs::operator::EvalContext<'_, 'a>,
        arena: &'a datalogic_rs::bumpalo::Bump,
    ) -> datalogic_rs::Result<&'a datalogic_rs::DataValue<'a>> {
        self.0.evaluate(args, ctx, arena)
    }
}

mod sealed {
    use bumpalo::Bump;
    use datalogic_rs::{DataValue, EvalInput};
    use datavalue::OwnedDataValue;
    use serde_json::Value;

    /// The seal, and the crate's view of a root: no datalogic type reaches
    /// [`EvalRoot`](super::EvalRoot)'s public face.
    pub trait Sealed<'r> {
        /// The root as an arena value in `arena`, through datalogic's own
        /// input adapter.
        fn view_in<'a>(self, arena: &'a Bump) -> datalogic_rs::Result<&'a DataValue<'a>>
        where
            'r: 'a;
    }

    impl<'r> Sealed<'r> for &'r OwnedDataValue {
        #[inline]
        fn view_in<'a>(self, arena: &'a Bump) -> datalogic_rs::Result<&'a DataValue<'a>>
        where
            'r: 'a,
        {
            <&'a OwnedDataValue as EvalInput<'a>>::into_arena_value(self, arena)
        }
    }

    impl<'r> Sealed<'r> for &'r Value {
        #[inline]
        fn view_in<'a>(self, arena: &'a Bump) -> datalogic_rs::Result<&'a DataValue<'a>>
        where
            'r: 'a,
        {
            <&'a Value as EvalInput<'a>>::into_arena_value(self, arena)
        }
    }
}

/// A root [`ExprEngine::eval`] evaluates against: an object holding the
/// three context roots, as an [`OwnedDataValue`] or as a
/// [`serde_json::Value`].
///
/// Either is viewed in place, its strings and keys borrowed and only its
/// arrays' and objects' spines built, so a host that holds its context as
/// JSON evaluates it without converting it first, and gets what the same
/// context as an `OwnedDataValue` gives. Sealed: the two are the
/// implementations.
///
/// ```
/// use dataflow_rs::datavalue::OwnedDataValue;
/// use dataflow_rs::expr::ExprEngine;
/// use dataflow_rs::Expr;
/// use serde_json::json;
///
/// let exprs = ExprEngine::default();
/// let limit: Expr<u64> = exprs.compile(&json!({"*": [{"var": "data.n"}, 2]})).unwrap();
/// let context = json!({"data": {"n": 21}, "metadata": {}, "temp_data": {}});
/// assert_eq!(exprs.eval(&limit, &context).unwrap().value, 42);
/// let owned = OwnedDataValue::from(&context);
/// assert_eq!(exprs.eval(&limit, &owned).unwrap().value, 42);
/// ```
pub trait EvalRoot<'r>: sealed::Sealed<'r> {}

impl<'r> EvalRoot<'r> for &'r OwnedDataValue {}

impl<'r> EvalRoot<'r> for &'r Value {}

struct Inner {
    logic: Engine,
    operators: Vec<Operator>,
    by_name: HashMap<Box<str>, usize>,
    secrets: Arc<Secrets>,
}

/// Compiles and evaluates the JSONLogic a definition is written in.
///
/// Cheap to clone: a clone shares the datalogic engine, the operator table and
/// the secrets.
#[derive(Clone)]
pub struct ExprEngine {
    inner: Arc<Inner>,
}

impl Default for ExprEngine {
    fn default() -> Self {
        // The builder with no operators has nothing to refuse.
        Self::builder()
            .build()
            .expect("an engine with no custom operators cannot fail to build")
    }
}

impl std::fmt::Debug for ExprEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExprEngine")
            .field("operators", &self.inner.operators.len())
            .field("secrets", &self.inner.secrets)
            .finish()
    }
}

impl ExprEngine {
    /// Starts a builder.
    pub fn builder() -> ExprEngineBuilder {
        ExprEngineBuilder::default()
    }

    /// Every operator this engine evaluates: the datalogic built-ins its
    /// selected families enable, then the host's operators in registration
    /// order, then `secret`.
    pub fn operators(&self) -> &[Operator] {
        &self.inner.operators
    }

    /// The operator named `name`, if the engine has one.
    pub(crate) fn operator(&self, name: &str) -> Option<&Operator> {
        self.inner
            .by_name
            .get(name)
            .map(|&i| &self.inner.operators[i])
    }

    /// The secrets the `secret` operator reads.
    pub fn secrets(&self) -> &Secrets {
        &self.inner.secrets
    }

    /// The template-key escape: `$`.
    pub fn escape(&self) -> char {
        TEMPLATE_KEY_ESCAPE
    }

    /// Compiles `raw`. An expression that reads nothing is evaluated once, here,
    /// and kept as its `T`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Compile`] when datalogic refuses the expression;
    /// [`ErrorKind::Eval`] when evaluating a constant at fold time fails;
    /// [`ErrorKind::Config`] when a folded constant is not a `T`.
    pub fn compile<T: FromValue>(&self, raw: &Value) -> Result<Expr<T>> {
        let logic = self
            .logic()
            .compile(raw)
            .map_err(|e| Error::new(ErrorKind::Compile, e.to_string()))?;
        // `is_constant`, never `is_static`: the latter is also true for a rule
        // that failed to fold because it errors, and that error belongs at eval.
        let constant = if logic.is_constant() {
            let folded = with_arena(|arena| {
                self.logic()
                    .evaluate(&logic, DataValue::Object(&[]), arena)
                    .map_err(|e| Error::new(ErrorKind::Eval, e.to_string()))
                    .and_then(|v| {
                        T::from_view(v).map_err(|why| {
                            Error::new(
                                ErrorKind::Config,
                                format!(
                                    "`{}` must be {}: {why}",
                                    super::expr::snippet(raw),
                                    T::type_name()
                                ),
                            )
                        })
                    })
            })?;
            Some(folded)
        } else {
            None
        };
        Ok(Expr {
            raw: raw.clone(),
            logic,
            constant,
        })
    }

    /// Evaluates `expr` against `root`, an object holding the three context
    /// roots, as an [`OwnedDataValue`] or a [`serde_json::Value`]
    /// ([`EvalRoot`]). A folded constant is lent from `expr` and takes no
    /// arena.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Budget`] when the operation ceiling is exceeded;
    /// [`ErrorKind::Eval`] for any other failure, including a result that is
    /// not a `T`.
    pub fn eval<'e, 'r, T: FromValue>(
        &self,
        expr: &'e Expr<T>,
        root: impl EvalRoot<'r>,
    ) -> Result<Evaluated<T::Out<'e>>> {
        if let Some(c) = &expr.constant {
            return Ok(Evaluated {
                value: T::lend(c),
                ops: if cfg!(feature = "budget") {
                    Some(0)
                } else {
                    None
                },
            });
        }
        with_arena(|arena| {
            let root = sealed::Sealed::view_in(root, arena).map_err(eval_error)?;
            #[cfg(feature = "budget")]
            let (view, ops) = {
                let budget = self.logic().resolve_ops_budget(None);
                let m = self
                    .logic()
                    .evaluate_metered(&expr.logic, root, arena, budget)
                    .map_err(eval_error)?;
                (m.value, Some(m.ops))
            };
            #[cfg(not(feature = "budget"))]
            let (view, ops) = (
                self.logic()
                    .evaluate(&expr.logic, root, arena)
                    .map_err(eval_error)?,
                None,
            );
            Ok(Evaluated {
                value: T::give(expr.convert(view)?),
                ops,
            })
        })
    }

    /// Evaluates `expr` against a view `root`, allocating in `arena` and
    /// converting to `T`: the shared evaluation path for `SyncCtx`, `Ctx` and
    /// `map`'s destination.
    pub(crate) fn eval_view<'e, T: FromValue>(
        &self,
        expr: &'e Expr<T>,
        root: &DataValue<'_>,
        arena: &Bump,
    ) -> Result<T::Out<'e>> {
        if let Some(c) = expr.constant() {
            return Ok(T::lend(c));
        }
        let view = self.eval_in(expr.logic(), root, arena)?;
        expr.convert(view).map(T::give)
    }

    /// Evaluates `logic` against a view root, allocating in `arena`: how a
    /// built-in reads the run view, and how `map` evaluates in the run arena
    /// so its result splices without a copy.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Budget`] when the operation ceiling is exceeded;
    /// [`ErrorKind::Eval`] for any other failure (the classifier
    /// [`Self::eval`] uses).
    pub(crate) fn eval_in<'a>(
        &self,
        logic: &'a Logic,
        root: &'a DataValue<'a>,
        arena: &'a Bump,
    ) -> Result<&'a DataValue<'a>> {
        self.logic()
            .evaluate(logic, root, arena)
            .map_err(eval_error)
    }

    /// The datalogic engine underneath.
    pub(crate) fn logic(&self) -> &Engine {
        &self.inner.logic
    }
}

/// Classifies a datalogic evaluation failure: a spent budget is its own kind.
fn eval_error(e: datalogic_rs::Error) -> Error {
    let kind = if matches!(e.kind, datalogic_rs::ErrorKind::BudgetExceeded { .. }) {
        ErrorKind::Budget
    } else {
        ErrorKind::Eval
    };
    Error::new(kind, e.to_string())
}

/// Builds an [`ExprEngine`].
///
/// Defaults to [`OperatorFamily::Core`] (and [`OperatorFamily::Secret`]) only.
/// Select optional operator families at runtime with [`family`](Self::family),
/// [`families`](Self::families) or [`all_operators`](Self::all_operators).
#[derive(Default)]
pub struct ExprEngineBuilder {
    families: Vec<OperatorFamily>,
    operators: Vec<(Operator, Arc<dyn CustomOperator>)>,
    secrets: Secrets,
    #[cfg(feature = "budget")]
    ops_budget: Option<u64>,
}

impl std::fmt::Debug for ExprEngineBuilder {
    /// The selected families, the custom operators' names and the secrets, redacted.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let operators: Vec<&str> = self.operators.iter().map(|(info, _)| &*info.name).collect();
        f.debug_struct("ExprEngineBuilder")
            .field("families", &self.families)
            .field("operators", &operators)
            .field("secrets", &self.secrets)
            .finish_non_exhaustive()
    }
}

impl ExprEngineBuilder {
    /// Enables one built-in [`OperatorFamily`].
    ///
    /// Enabling a family is not a no-op for existing workflows: under
    /// templating mode an unknown operator name passes through as literal data,
    /// so `{"length": ...}` starts evaluating once
    /// [`OperatorFamily::ExtString`] is enabled (`{"$length": ...}` pins the
    /// literal). [`OperatorFamily::Datetime`] and [`OperatorFamily::Tensor`]
    /// also require their cargo features (`datetime`, `tensor`) to be enabled.
    #[must_use]
    pub fn family(mut self, family: OperatorFamily) -> Self {
        self.families.push(family);
        self
    }

    /// Enables each built-in [`OperatorFamily`] in `families`.
    #[must_use]
    pub fn families(mut self, families: impl IntoIterator<Item = OperatorFamily>) -> Self {
        self.families.extend(families);
        self
    }

    /// Enables the standard optional operator families:
    /// [`ExtString`](OperatorFamily::ExtString),
    /// [`ExtArray`](OperatorFamily::ExtArray),
    /// [`ExtMath`](OperatorFamily::ExtMath),
    /// [`ExtControl`](OperatorFamily::ExtControl),
    /// [`ExtObject`](OperatorFamily::ExtObject),
    /// [`ErrorHandling`](OperatorFamily::ErrorHandling), and (when the
    /// `datetime` cargo feature is enabled)
    /// [`Datetime`](OperatorFamily::Datetime).
    ///
    /// [`OperatorFamily::Tensor`] is deliberately left out: names like
    /// `shape`, `full`, `cast` and `concat` are ordinary JSON keys, which a
    /// live operator would start evaluating.
    #[must_use]
    pub fn all_operators(self) -> Self {
        self.families(ALL_OPERATORS.iter().copied())
    }

    /// Registers a custom operator, neither scoping nor deterministic.
    ///
    /// # Dependency
    ///
    /// `op` is a [`datalogic_rs::CustomOperator`](crate::datalogic_rs::CustomOperator),
    /// named through this crate's re-export. It is part of the committed
    /// dependency surface, as [`OwnedDataValue`](datavalue::OwnedDataValue)
    /// is: wrapping it would still take datalogic's value and arena types, so
    /// the engine keeps the trait itself.
    #[must_use]
    pub fn operator(self, name: impl Into<String>, op: impl CustomOperator + 'static) -> Self {
        self.operator_with(Operator::custom(name.into()), op)
    }

    /// Registers a custom operator with its own flags. The family is always
    /// [`OperatorFamily::Custom`]; `op` is a
    /// [`datalogic_rs::CustomOperator`](crate::datalogic_rs::CustomOperator),
    /// the committed dependency type [`operator`](Self::operator) documents.
    #[must_use]
    pub fn operator_with(mut self, mut info: Operator, op: impl CustomOperator + 'static) -> Self {
        info.family = OperatorFamily::Custom;
        self.operators.push((info, Arc::new(op)));
        self
    }

    /// Sets the secrets the `secret` operator reads.
    #[must_use]
    pub fn secrets(mut self, secrets: Secrets) -> Self {
        self.secrets = secrets;
        self
    }

    /// Sets the ceiling on operations per evaluation.
    #[cfg(feature = "budget")]
    #[must_use]
    pub fn ops_budget(mut self, ops: u64) -> Self {
        self.ops_budget = Some(ops);
        self
    }

    /// Builds the engine, with [`OperatorFamily::Core`],
    /// [`OperatorFamily::Secret`] and the families selected on this builder,
    /// and no other. [`ErrorKind::Config`] when [`OperatorFamily::Custom`] or
    /// an uncompiled cargo-feature family (`Datetime`, `Tensor`) is requested,
    /// or when a custom operator is named `secret`, is named like a built-in
    /// operator of the selected families, or is registered twice.
    pub fn build(self) -> Result<ExprEngine> {
        let mut upstream_families = Vec::with_capacity(self.families.len());
        for family in self.families {
            match family {
                OperatorFamily::Core | OperatorFamily::Secret => {}
                OperatorFamily::Custom => {
                    return Err(Error::new(
                        ErrorKind::Config,
                        "custom operators are registered with `operator`, not `family`",
                    ));
                }
                other => {
                    let upstream = other
                        .to_upstream()
                        .expect("every built-in family maps to a datalogic family");
                    if !upstream.is_compiled() {
                        return Err(Error::new(
                            ErrorKind::Config,
                            format!(
                                "operator family `{other:?}` requires its cargo feature to be enabled"
                            ),
                        ));
                    }
                    upstream_families.push(upstream);
                }
            }
        }
        let secrets = Arc::new(self.secrets);
        let mut builder = Engine::builder()
            .with_families(upstream_families)
            .with_templating(true)
            .with_template_key_escape(TEMPLATE_KEY_ESCAPE);
        // `with_config` replaces the whole `EvaluationConfig`, and `default()`
        // is what the builder already holds, so an unset ceiling is a no-op.
        #[cfg(feature = "budget")]
        {
            builder = builder.with_config(
                datalogic_rs::EvaluationConfig::default().with_ops_budget(self.ops_budget),
            );
        }
        let mut infos = Vec::with_capacity(self.operators.len());
        for (info, op) in self.operators {
            builder = builder.add_operator(info.name.to_string(), SharedOperator(op));
            infos.push(info);
        }
        builder = builder.add_operator(SECRET_OPERATOR, SecretOperator(Arc::clone(&secrets)));
        let logic = builder.build();

        // A custom name is judged against the built-ins this engine has, so
        // no throwaway engine is built for it.
        let builtin: HashSet<&'static str> = logic.builtin_operator_names().collect();
        let mut seen = HashSet::new();
        for info in &infos {
            let name = &*info.name;
            let refusal = if name == SECRET_OPERATOR {
                Some("is reserved for the engine's own operator")
            } else if builtin.contains(name) {
                Some("is the name of a built-in operator")
            } else if !seen.insert(name) {
                Some("is registered twice")
            } else {
                None
            };
            if let Some(why) = refusal {
                return Err(Error::new(
                    ErrorKind::Config,
                    format!("operator `{name}` {why}"),
                ));
            }
        }

        // datalogic's own description of its operators, in its order, one
        // row per name.
        let mut operators: Vec<Operator> =
            logic.operators().flat_map(Operator::from_logic).collect();
        operators.extend(infos);
        operators.push(Operator::secret());
        let by_name: HashMap<Box<str>, usize> = operators
            .iter()
            .enumerate()
            .map(|(i, op)| (Box::from(&*op.name), i))
            .collect();

        Ok(ExprEngine {
            inner: Arc::new(Inner {
                logic,
                operators,
                by_name,
                secrets,
            }),
        })
    }
}

#[cfg(test)]
impl ExprEngine {
    /// Compiles `logic` and evaluates it against an empty object.
    pub(crate) fn try_eval_json(
        &self,
        logic: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, datalogic_rs::Error> {
        let compiled = self.logic().compile_arc(logic).expect("should compile");
        let ctx = datavalue::OwnedDataValue::from(&serde_json::json!({}));
        let text = self.logic().session().eval_str(&compiled, &ctx)?;
        Ok(serde_json::from_str(&text).expect("eval_str output should be valid JSON"))
    }

    /// As [`Self::try_eval_json`], panicking on an evaluation error.
    pub(crate) fn eval_json(&self, logic: &serde_json::Value) -> serde_json::Value {
        self.try_eval_json(logic).expect("should evaluate")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_operand_results_this_crate_would_silently_break_on() {
        // A workflow author can write any of these — a map mapping folding an
        // empty list, a filter condition over an empty selector — and the
        // crate never validates operand count. If a datalogic upgrade changed
        // any of these defaults, every workflow relying on the vacuous case
        // would silently start producing a different value.
        let e = ExprEngine::default();
        for (logic, expected) in [
            (json!({"and": []}), json!(null)),
            (json!({"or": []}), json!(null)),
            (json!({"+": []}), json!(0)),
            (json!({"*": []}), json!(1)),
            (json!({"cat": []}), json!("")),
            (json!({"merge": []}), json!([])),
            (json!({"missing": []}), json!([])),
        ] {
            assert_eq!(e.eval_json(&logic), expected, "for {logic}");
        }
    }

    #[test]
    fn a_missing_var_path_resolves_to_null_not_an_error() {
        // The exact mechanism behind the pitfall CLAUDE.md documents for
        // `payload.*` expressions: a `var` over a path that does not resolve
        // is `Null`, silently, never `Err`. `Expr` and the built-in
        // expression fields inherit this — there is no engine-level signal that
        // distinguishes "field absent" from "field is null".
        let e = ExprEngine::default();
        assert_eq!(
            e.eval_json(&json!({"var": "data.does_not_exist"})),
            json!(null)
        );
    }

    #[test]
    fn truthy_falsy_matches_the_documented_semantics() {
        // Verifies the claim in docs/src/advanced/jsonlogic.md's Truthy/Falsy
        // section, which is a `json` fence and therefore NOT compiled by
        // dataflow-docs-tests — this is the only check on that claim.
        // Notable and easy to get wrong: an empty object `{}` is falsy here,
        // unlike some JSONLogic implementations that treat any object as truthy.
        let e = ExprEngine::default();
        for (v, truthy) in [
            (json!(0), false),
            (json!(""), false),
            (json!(false), false),
            (json!(null), false),
            (json!([]), false),
            (json!({}), false),
            (json!("x"), true),
            (json!(1), true),
        ] {
            assert_eq!(
                e.eval_json(&json!({"!!": v})),
                json!(truthy),
                "truthiness of {v}"
            );
        }
    }

    #[test]
    fn an_unrecognised_operator_is_not_an_error_under_templating() {
        // The load-bearing fact behind #26's refusal of a static "known
        // operators" table: under JSONLogic templating (which `ExprEngine`
        // enables), an outright typo neither fails to compile nor fails to
        // evaluate. It echoes back as a literal structured object instead — a
        // workflow author who mistypes an operator name gets silent
        // pass-through, not a validation error. True under every feature
        // combination, so this half of the tripwire is unconditional.
        let e = ExprEngine::default();
        let logic = json!({"totally_made_up_op_xyz": ["a", "b"]});
        assert_eq!(
            e.eval_json(&logic),
            logic,
            "an unrecognised operator must echo back verbatim, not error"
        );
    }

    /// With `ExtString` off (the default), `starts_with` is not a name the
    /// engine knows, so it is indistinguishable from the typo above: silent
    /// pass-through. This is the failure mode a workflow author hits when they
    /// reach for an operator whose family the engine did not enable — no error,
    /// just a wrong value.
    #[test]
    fn a_gated_operator_echoes_back_while_its_family_is_off() {
        let e = ExprEngine::default();
        let logic = json!({"starts_with": ["hello", "he"]});
        assert_eq!(
            e.eval_json(&logic),
            logic,
            "an operator behind an unenabled family must echo back, not error"
        );
    }

    /// The other side of the same coin, and the reason enabling a family is
    /// not a no-op for existing workflows: `ExtString` converts a previously
    /// inert `{"starts_with": [...]}` *literal* into a live operator call.
    /// Anyone carrying such an object as data through a `map` mapping sees
    /// their value silently replaced by the operator's result.
    #[test]
    fn a_gated_operator_evaluates_once_its_family_is_on() {
        let e = ExprEngine::builder()
            .family(OperatorFamily::ExtString)
            .build()
            .unwrap();
        assert_eq!(
            e.eval_json(&json!({"starts_with": ["hello", "he"]})),
            json!(true),
            "with ExtString on, starts_with must evaluate, not echo"
        );
    }

    /// `datetime` is the one family that is not confined to new operator
    /// names. `datalogic-rs`'s comparison path probes *plain strings* for a
    /// datetime/duration shape before falling back to byte comparison, so
    /// `==` and the ordering operators change answers on date-shaped
    /// operands. These two strings are different byte sequences naming the
    /// same instant.
    #[test]
    fn datetime_feature_changes_plain_string_comparison() {
        let e = ExprEngine::default();
        let logic = json!({"==": ["2024-01-15T00:00:00Z", "2024-01-15T01:00:00+01:00"]});
        #[cfg(feature = "datetime")]
        assert_eq!(e.eval_json(&logic), json!(true));
        #[cfg(not(feature = "datetime"))]
        assert_eq!(e.eval_json(&logic), json!(false));
    }

    /// Each family selected on `ExprEngineBuilder` reaches `datalogic-rs`. One
    /// representative operator per family is enough.
    #[test]
    fn ext_string_family_reaches_datalogic() {
        let e = ExprEngine::builder()
            .family(OperatorFamily::ExtString)
            .build()
            .unwrap();
        assert_eq!(e.eval_json(&json!({"upper": "ab"})), json!("AB"));
    }

    #[test]
    fn ext_array_family_reaches_datalogic() {
        let e = ExprEngine::builder()
            .family(OperatorFamily::ExtArray)
            .build()
            .unwrap();
        assert_eq!(e.eval_json(&json!({"sort": [[3, 1, 2]]})), json!([1, 2, 3]));
    }

    #[test]
    fn ext_math_family_reaches_datalogic() {
        let e = ExprEngine::builder()
            .family(OperatorFamily::ExtMath)
            .build()
            .unwrap();
        assert_eq!(e.eval_json(&json!({"abs": -5})), json!(5));
    }

    #[test]
    fn ext_control_family_reaches_datalogic() {
        let e = ExprEngine::builder()
            .family(OperatorFamily::ExtControl)
            .build()
            .unwrap();
        assert_eq!(
            e.eval_json(&json!({"??": [null, "fallback"]})),
            json!("fallback")
        );
    }

    #[test]
    fn ext_object_family_reaches_datalogic() {
        let e = ExprEngine::builder()
            .family(OperatorFamily::ExtObject)
            .build()
            .unwrap();
        assert_eq!(
            e.eval_json(&json!({"keys": [{"a": 1, "b": 2}]})),
            json!(["a", "b"])
        );
    }

    #[test]
    fn error_handling_family_reaches_datalogic() {
        // `ErrorHandling` is the JSONLogic `try`/`throw` pair — unrelated to
        // this crate's own always-on error handling.
        let e = ExprEngine::builder()
            .family(OperatorFamily::ErrorHandling)
            .build()
            .unwrap();
        assert_eq!(
            e.eval_json(&json!({"try": [{"throw": "boom"}, "recovered"]})),
            json!("recovered")
        );
    }

    /// The `datetime` family's own operators. Their exact output depends on
    /// the ambient clock and on format details this crate does not pin, so
    /// assert only the property the family actually buys: the operator is
    /// recognised and evaluates once `OperatorFamily::Datetime` is selected,
    /// and echoes back as a literal on `ExprEngine::default()`.
    #[cfg(feature = "datetime")]
    #[test]
    fn datetime_family_reaches_datalogic() {
        let logic = json!({"now": []});
        assert_eq!(
            ExprEngine::default().eval_json(&logic),
            logic,
            "without OperatorFamily::Datetime selected, `now` echoes back"
        );
        let e = ExprEngine::builder()
            .family(OperatorFamily::Datetime)
            .build()
            .unwrap();
        let result = e.eval_json(&logic);
        assert_ne!(result, logic, "with Datetime on, `now` must not echo back");
        assert!(!result.is_null(), "`now` should produce a value, got null");
    }
    #[cfg(feature = "budget")]
    #[test]
    fn the_ops_budget_stops_a_map_over_three_items() {
        let e = ExprEngine::builder().ops_budget(2).build().unwrap();
        let logic = json!({"map": [[1, 2, 3], {"+": [{"var": ""}, 1]}]});
        let err = e.try_eval_json(&logic).unwrap_err();
        assert_eq!(err.tag(), "BudgetExceeded");
        // Without a ceiling the same expression runs.
        assert_eq!(ExprEngine::default().eval_json(&logic), json!([2, 3, 4]));
    }

    #[test]
    fn escape_is_dollar_and_the_escaped_key_is_literal() {
        let e = ExprEngine::default();
        assert_eq!(e.escape(), '$');
        assert_eq!(e.eval_json(&json!({"$cat": [1]})), json!({"cat": [1]}));
    }
}
