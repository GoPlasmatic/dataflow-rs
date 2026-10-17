//! `ExprEngine`: the v4 wrapper around one datalogic-rs engine.

use super::arena::with_arena;
use super::expr::{Evaluated, Expr, FromValue};
use super::operators::{Family, OperatorInfo, TABLE};
use super::secrets::{SECRET_OPERATOR, SecretOperator, Secrets};
use crate::base::{Error, ErrorKind, Result};
use bumpalo::Bump;
use datalogic_rs::{CustomOperator, DataValue, Engine, EvalInput, Logic};
use datavalue::OwnedDataValue;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

/// The key prefix that escapes a template key into data: `{"$cat": …}` is the
/// literal object, `{"cat": …}` the operator. Always on, applied at the one
/// place an engine is built, so an engine in a test agrees with one that runs.
const TEMPLATE_KEY_ESCAPE: char = '$';

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
    pub trait Sealed {}
    impl Sealed for &datavalue::OwnedDataValue {}
    impl Sealed for &serde_json::Value {}
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
pub trait EvalRoot<'r>: sealed::Sealed {
    /// The root as an arena value in `arena`, through datalogic's own input
    /// adapter.
    #[doc(hidden)]
    fn view_in<'a>(self, arena: &'a Bump) -> datalogic_rs::Result<&'a DataValue<'a>>
    where
        'r: 'a;
}

impl<'r> EvalRoot<'r> for &'r OwnedDataValue {
    #[inline]
    fn view_in<'a>(self, arena: &'a Bump) -> datalogic_rs::Result<&'a DataValue<'a>>
    where
        'r: 'a,
    {
        <&'a OwnedDataValue as EvalInput<'a>>::into_arena_value(self, arena)
    }
}

impl<'r> EvalRoot<'r> for &'r Value {
    #[inline]
    fn view_in<'a>(self, arena: &'a Bump) -> datalogic_rs::Result<&'a DataValue<'a>>
    where
        'r: 'a,
    {
        <&'a Value as EvalInput<'a>>::into_arena_value(self, arena)
    }
}

struct Inner {
    logic: Engine,
    operators: Vec<OperatorInfo>,
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
    /// features enable, then the host's operators in registration order, then
    /// `secret`.
    pub fn operators(&self) -> &[OperatorInfo] {
        &self.inner.operators
    }

    /// The operator named `name`, if the engine has one.
    pub(crate) fn operator(&self, name: &str) -> Option<&OperatorInfo> {
        self.operators().iter().find(|op| op.name == name)
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
                                format!("`{raw}` must be {}: {why}", T::type_name()),
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
            let root = root.view_in(arena).map_err(eval_error)?;
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
    let kind = if e.tag() == "BudgetExceeded" {
        ErrorKind::Budget
    } else {
        ErrorKind::Eval
    };
    Error::new(kind, e.to_string())
}

/// Builds an [`ExprEngine`].
#[derive(Default)]
pub struct ExprEngineBuilder {
    operators: Vec<(OperatorInfo, Arc<dyn CustomOperator>)>,
    secrets: Secrets,
    #[cfg(feature = "budget")]
    ops_budget: Option<u64>,
}

impl std::fmt::Debug for ExprEngineBuilder {
    /// The custom operators' names and the secrets, redacted.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let operators: Vec<&str> = self.operators.iter().map(|(info, _)| &*info.name).collect();
        f.debug_struct("ExprEngineBuilder")
            .field("operators", &operators)
            .field("secrets", &self.secrets)
            .finish_non_exhaustive()
    }
}

impl ExprEngineBuilder {
    /// Registers a custom operator, neither scoping nor deterministic.
    #[must_use]
    pub fn operator(self, name: impl Into<String>, op: impl CustomOperator + 'static) -> Self {
        self.operator_with(OperatorInfo::custom(name.into()), op)
    }

    /// Registers a custom operator with its own flags. The family is always
    /// [`Family::Custom`].
    #[must_use]
    pub fn operator_with(
        mut self,
        mut info: OperatorInfo,
        op: impl CustomOperator + 'static,
    ) -> Self {
        info.family = Family::Custom;
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

    /// Builds the engine. [`ErrorKind::Config`] when a custom operator is named
    /// `secret`, is named like a datalogic built-in this build enables, or is
    /// registered twice.
    pub fn build(self) -> Result<ExprEngine> {
        let builtin: HashSet<&'static str> = Engine::new().builtin_operator_names().collect();
        let mut seen = HashSet::new();
        for (info, _) in &self.operators {
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

        let secrets = Arc::new(self.secrets);
        let mut builder = Engine::builder()
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

        // The table's rows in the table's order, restricted to the names this
        // build's datalogic reports. A name it reports that the table lacks is
        // a bug the completeness test catches.
        let live: HashSet<&'static str> = logic.builtin_operator_names().collect();
        let mut operators: Vec<OperatorInfo> = TABLE
            .iter()
            .filter(|row| live.contains(row.name))
            .map(|row| row.info())
            .collect();
        operators.extend(infos);
        operators.push(OperatorInfo::secret());

        Ok(ExprEngine {
            inner: Arc::new(Inner {
                logic,
                operators,
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
        // is `Null`, silently, never `Err`. `Template::eval` and the built-in
        // `*_logic` fields inherit this — there is no engine-level signal that
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
        // operators" table, and the reason `Template` documents itself as
        // opt-in per field rather than a blanket JSON wrapper: under
        // templating (which LogicCompiler and TemplateCompiler both enable),
        // an outright typo neither fails to compile nor fails to evaluate. It
        // echoes back as a literal structured object instead — a workflow
        // author who mistypes an operator name gets silent pass-through, not a
        // validation error. True under every feature combination, so this half
        // of the tripwire is unconditional.
        let e = ExprEngine::default();
        let logic = json!({"totally_made_up_op_xyz": ["a", "b"]});
        assert_eq!(
            e.eval_json(&logic),
            logic,
            "an unrecognised operator must echo back verbatim, not error"
        );
    }

    /// With `ext-string` off, `starts_with` is not a name the engine knows, so
    /// it is indistinguishable from the typo above: silent pass-through. This
    /// is the failure mode a workflow author hits when they reach for an
    /// operator whose family this build did not enable — no error, just a
    /// wrong value.
    #[cfg(not(feature = "ext-string"))]
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
    /// not a no-op for existing workflows: `ext-string` converts a previously
    /// inert `{"starts_with": [...]}` *literal* into a live operator call.
    /// Anyone carrying such an object as data through a `map` mapping sees
    /// their value silently replaced by the operator's result.
    #[cfg(feature = "ext-string")]
    #[test]
    fn a_gated_operator_evaluates_once_its_family_is_on() {
        let e = ExprEngine::default();
        assert_eq!(
            e.eval_json(&json!({"starts_with": ["hello", "he"]})),
            json!(true),
            "with ext-string on, starts_with must evaluate, not echo"
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

    /// Each family's cargo feature actually reaches `datalogic-rs`. One
    /// representative operator per family is enough — the feature either
    /// forwards or it does not.
    #[cfg(feature = "ext-string")]
    #[test]
    fn ext_string_feature_reaches_datalogic() {
        let e = ExprEngine::default();
        assert_eq!(e.eval_json(&json!({"upper": "ab"})), json!("AB"));
    }

    #[cfg(feature = "ext-array")]
    #[test]
    fn ext_array_feature_reaches_datalogic() {
        let e = ExprEngine::default();
        assert_eq!(e.eval_json(&json!({"sort": [[3, 1, 2]]})), json!([1, 2, 3]));
    }

    #[cfg(feature = "ext-math")]
    #[test]
    fn ext_math_feature_reaches_datalogic() {
        let e = ExprEngine::default();
        assert_eq!(e.eval_json(&json!({"abs": -5})), json!(5));
    }

    #[cfg(feature = "ext-control")]
    #[test]
    fn ext_control_feature_reaches_datalogic() {
        let e = ExprEngine::default();
        assert_eq!(
            e.eval_json(&json!({"??": [null, "fallback"]})),
            json!("fallback")
        );
    }

    #[cfg(feature = "ext-object")]
    #[test]
    fn ext_object_feature_reaches_datalogic() {
        let e = ExprEngine::default();
        assert_eq!(
            e.eval_json(&json!({"keys": [{"a": 1, "b": 2}]})),
            json!(["a", "b"])
        );
    }

    #[cfg(feature = "error-handling")]
    #[test]
    fn error_handling_feature_reaches_datalogic() {
        // `error-handling` is the JSONLogic `try`/`throw` pair — unrelated to
        // this crate's own always-on error handling.
        let e = ExprEngine::default();
        assert_eq!(
            e.eval_json(&json!({"try": [{"throw": "boom"}, "recovered"]})),
            json!("recovered")
        );
    }

    /// The `datetime` family's own operators. Their exact output depends on
    /// the ambient clock and on format details this crate does not pin, so
    /// assert only the property the feature actually buys: the operator is
    /// recognised and evaluates, rather than echoing back as a literal.
    #[cfg(feature = "datetime")]
    #[test]
    fn datetime_feature_reaches_datalogic() {
        let e = ExprEngine::default();
        let logic = json!({"now": []});
        let result = e.eval_json(&logic);
        assert_ne!(result, logic, "with datetime on, `now` must not echo back");
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
