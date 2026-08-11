//! # Workflow Compilation Module
//!
//! Pre-compiles all JSONLogic expressions used by workflows and tasks at engine
//! initialization. Each compiled `Arc<Logic>` is stored directly on the
//! workflow/task/config struct that owns it — no central `logic_cache`, no
//! index lookup, no bounds check on the hot path. The `Engine` is wrapped in
//! `Arc` and is `Send + Sync` so the entire stack is safe to share across
//! Tokio worker threads.

use crate::engine::error::{DataflowError, Result};
use crate::engine::functions::integration::{EnrichConfig, HttpCallConfig, PublishKafkaConfig};
use crate::engine::functions::template::{Template, TemplateCompiler};
use crate::engine::functions::{FilterConfig, LogConfig, MapConfig, ValidationConfig};
use crate::engine::{FunctionConfig, Workflow};
use datalogic_rs::{Engine, Logic};
use log::debug;
use serde_json::Value;
use std::sync::Arc;

/// Compiles JSONLogic expressions and stamps them onto workflow/task/config
/// structs as `Option<Arc<Logic>>` slots.
pub struct LogicCompiler {
    /// Shared datalogic Engine used both for compilation and (later) evaluation.
    engine: Arc<Engine>,
    /// Handed to `AsyncFunctionHandler::compile_input` and used internally to
    /// compile `Template` fields on the built-in integration configs. Wraps the
    /// same `engine`, so a `Template` compiled here or by a custom handler is
    /// evaluable by the engine that will run the message.
    template_compiler: TemplateCompiler,
}

impl Default for LogicCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl LogicCompiler {
    /// Create a new LogicCompiler with a fresh datalogic `Engine` configured for
    /// templating mode (preserves object structure in JSONLogic operations).
    pub fn new() -> Self {
        let engine = Arc::new(Engine::builder().with_templating(true).build());
        let template_compiler = TemplateCompiler::new(Arc::clone(&engine));
        Self {
            engine,
            template_compiler,
        }
    }

    /// Get the Engine instance
    pub fn engine(&self) -> Arc<Engine> {
        Arc::clone(&self.engine)
    }

    /// Consume the compiler and return the shared engine.
    pub fn into_engine(self) -> Arc<Engine> {
        self.engine
    }

    /// Compile all workflows and their tasks, returning them sorted by priority.
    /// Returns `Err` on the first validation or compilation failure — engine
    /// construction is fail-loud so misconfigured workflows can't silently
    /// disappear at runtime.
    pub fn compile_workflows(&self, workflows: Vec<Workflow>) -> Result<Vec<Workflow>> {
        let mut compiled_workflows = Vec::with_capacity(workflows.len());

        for mut workflow in workflows {
            workflow.validate()?;

            // Populate the cached Arc<str> ids so audit emission can refcount-bump
            // rather than reallocate per AuditTrail entry.
            workflow.id_arc = Arc::from(workflow.id.as_str());
            for task in &mut workflow.tasks {
                task.id_arc = Arc::from(task.id.as_str());
            }

            // Pre-split `temp_data.{counter}` so a loop sweep never re-splits
            // the write path.
            if let Some(loop_config) = workflow.loop_config.as_mut() {
                loop_config.precompute_counter_path();
            }

            // Compile the workflow condition (defaults to `true`, which folds
            // to `None` so the hot path skips the eval — see `compile_condition`).
            let label = format!("workflow {} condition", workflow.id);
            workflow.compiled_condition = self.compile_condition(&workflow.condition, &label)?;
            debug!("Workflow {} condition compiled", workflow.id);

            // Compile task conditions and function-specific logic.
            self.compile_workflow_tasks(&mut workflow)?;

            // Stamp whether every task is a synchronous built-in. A fully-sync
            // workflow can be folded into a shared cross-workflow `with_arena`
            // scope (no `.await`), so the message context is deep-walked into
            // the arena once per *run* of consecutive fully-sync workflows
            // instead of once per workflow. Any async/custom task forces the
            // per-workflow `.await` path.
            workflow.fully_sync = workflow.tasks.iter().all(|t| t.function.is_sync_builtin());

            compiled_workflows.push(workflow);
        }

        // Sort by priority once at construction time
        compiled_workflows.sort_by_key(|w| w.priority);
        Ok(compiled_workflows)
    }

    /// Compile task conditions and function logic for a workflow
    fn compile_workflow_tasks(&self, workflow: &mut Workflow) -> Result<()> {
        for task in &mut workflow.tasks {
            let label = format!("task {} condition (workflow {})", task.id, workflow.id);
            task.compiled_condition = self.compile_condition(&task.condition, &label)?;

            // Compile function-specific logic (map transformations, validation rules, …)
            self.compile_function_logic(&mut task.function, &task.id, &workflow.id)?;
        }
        Ok(())
    }

    /// Compile function-specific logic based on function type
    fn compile_function_logic(
        &self,
        function: &mut FunctionConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        match function {
            FunctionConfig::Map { input, .. } => {
                self.compile_map_logic(input, task_id, workflow_id)
            }
            FunctionConfig::Validation { input, .. } => {
                self.compile_validation_logic(input, task_id, workflow_id)
            }
            FunctionConfig::Filter { input, .. } => {
                self.compile_filter_logic(input, task_id, workflow_id)
            }
            FunctionConfig::Log { input, .. } => {
                self.compile_log_logic(input, task_id, workflow_id)
            }
            FunctionConfig::HttpCall { input, .. } => {
                self.compile_http_call_logic(input, task_id, workflow_id)
            }
            FunctionConfig::Enrich { input, .. } => {
                self.compile_enrich_logic(input, task_id, workflow_id)
            }
            FunctionConfig::PublishKafka { input, .. } => {
                self.compile_publish_kafka_logic(input, task_id, workflow_id)
            }
            // No JSONLogic to compile, but the `data.{target}` write path is
            // precomputed here (path string + pre-split parts) so the hot
            // path never re-formats or re-splits it.
            FunctionConfig::ParseJson { input, .. } | FunctionConfig::ParseXml { input, .. } => {
                input.precompute_target_path();
                Ok(())
            }
            FunctionConfig::PublishJson { input, .. }
            | FunctionConfig::PublishXml { input, .. } => {
                input.precompute_target_path();
                Ok(())
            }
            // Custom and other functions don't need pre-compilation
            _ => Ok(()),
        }
    }

    /// Compile a JSONLogic expression and return the `Arc<Logic>`. Errors are
    /// surfaced as `DataflowError::LogicEvaluation` with the supplied
    /// context label for debugging.
    fn compile(&self, logic: &Value, ctx_label: &str) -> Result<Arc<Logic>> {
        self.engine
            .compile_arc(logic)
            .map_err(|e| DataflowError::LogicEvaluation(format!("{}: {}", ctx_label, e)))
    }

    /// Compile a workflow/task *condition*, returning `None` when the source is
    /// the literal `true`. A `None` condition is treated as "always run" by
    /// `evaluate_condition` / `evaluate_condition_in_arena`, so the hot path
    /// skips the `engine.evaluate` call — and, in the sync stretch, the
    /// per-task arena context slice build — entirely for the overwhelmingly
    /// common default `condition: true`. datalogic already folds a literal
    /// `true` to a near-free literal-fast-path eval; this avoids even setting
    /// up the call. Non-literal conditions (including `false` and any real
    /// expression) compile as normal.
    fn compile_condition(&self, condition: &Value, ctx_label: &str) -> Result<Option<Arc<Logic>>> {
        if matches!(condition, Value::Bool(true)) {
            return Ok(None);
        }
        Ok(Some(self.compile(condition, ctx_label)?))
    }

    /// Compile map transformation logic
    fn compile_map_logic(
        &self,
        config: &mut MapConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        for mapping in &mut config.mappings {
            // Pre-split the dot path so the hot path doesn't re-split per
            // write. The `#` prefix is preserved here — it's the explicit
            // "treat this as an object key, not an array index" hint that
            // `set_nested_value` consumes when deciding container shape; the
            // strip happens at lookup time inside `*_parts` helpers.
            let parts: Vec<Arc<str>> = mapping.path.split('.').map(Arc::from).collect();
            mapping.path_parts = Arc::from(parts.into_boxed_slice());
            mapping.path_arc = Arc::from(mapping.path.as_str());

            let label = format!(
                "map logic for task {} in workflow {} (path {})",
                task_id, workflow_id, mapping.path
            );
            mapping.compiled_logic = Some(self.compile(&mapping.logic, &label)?);
        }
        Ok(())
    }

    /// Compile validation rule logic
    fn compile_validation_logic(
        &self,
        config: &mut ValidationConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        for (idx, rule) in config.rules.iter_mut().enumerate() {
            let label = format!(
                "validation rule {} for task {} in workflow {}",
                idx, task_id, workflow_id
            );
            rule.compiled_logic = Some(self.compile(&rule.logic, &label)?);
        }
        Ok(())
    }

    /// Compile log message and field expressions
    fn compile_log_logic(
        &self,
        config: &mut LogConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        let msg_label = label("log message", task_id, workflow_id);
        config.compiled_message = Some(self.compile(&config.message, &msg_label)?);

        // Compile each field expression. Collect into a fresh Vec, then
        // assign — keeps the immutable borrow of `config.fields` from
        // overlapping with the mutable borrow of `config.compiled_fields`.
        let mut compiled_fields = Vec::with_capacity(config.fields.len());
        for (key, logic) in &config.fields {
            let label = format!(
                "log field '{}' for task {} in workflow {}",
                key, task_id, workflow_id
            );
            compiled_fields.push((key.clone(), Some(self.compile(logic, &label)?)));
        }
        config.compiled_fields = compiled_fields;
        Ok(())
    }

    /// Compile filter condition logic
    fn compile_filter_logic(
        &self,
        config: &mut FilterConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        let label = label("filter condition", task_id, workflow_id);
        config.compiled_condition = Some(self.compile(&config.condition, &label)?);
        Ok(())
    }

    /// Compile http_call JSONLogic expressions (path_logic, body_logic)
    fn compile_http_call_logic(
        &self,
        config: &mut HttpCallConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        self.compile_template_field(
            &mut config.path_logic,
            "http_call path_logic",
            task_id,
            workflow_id,
        )?;
        self.compile_template_field(
            &mut config.body_logic,
            "http_call body_logic",
            task_id,
            workflow_id,
        )?;
        Ok(())
    }

    /// Compile enrich JSONLogic expressions (path_logic)
    fn compile_enrich_logic(
        &self,
        config: &mut EnrichConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        self.compile_template_field(
            &mut config.path_logic,
            "enrich path_logic",
            task_id,
            workflow_id,
        )
    }

    /// Compile publish_kafka JSONLogic expressions (key_logic, value_logic)
    fn compile_publish_kafka_logic(
        &self,
        config: &mut PublishKafkaConfig,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        self.compile_template_field(
            &mut config.key_logic,
            "publish_kafka key_logic",
            task_id,
            workflow_id,
        )?;
        self.compile_template_field(
            &mut config.value_logic,
            "publish_kafka value_logic",
            task_id,
            workflow_id,
        )?;
        Ok(())
    }

    /// Compile an optional built-in integration `Template` field — `path_logic`,
    /// `body_logic`, `key_logic`, `value_logic` — against `self.template_compiler`.
    /// A `None` field is a no-op, matching every one of these fields being
    /// optional. `what` labels the compile-error context as `"{what} for task
    /// {task_id} in workflow {workflow_id}"`, e.g. `"http_call body_logic"`.
    fn compile_template_field(
        &self,
        field: &mut Option<Template>,
        what: &str,
        task_id: &str,
        workflow_id: &str,
    ) -> Result<()> {
        if let Some(t) = field {
            t.compile(&self.template_compiler, &label(what, task_id, workflow_id))?;
        }
        Ok(())
    }
}

/// Format a JSONLogic compile-error label as `"{what} for task {task_id} in
/// workflow {workflow_id}"` — the shape shared by every built-in whose
/// context needs no further detail (a few, like map mappings and validation
/// rules, append per-item detail and format their own label instead).
fn label(what: &str, task_id: &str, workflow_id: &str) -> String {
    format!("{what} for task {task_id} in workflow {workflow_id}")
}

#[cfg(test)]
mod tests {
    //! Pins the datalogic operator semantics this crate's own behaviour
    //! depends on. Not an attempt at a general operator-semantics table — that
    //! was investigated and refused: `datalogic-rs` keeps `mod opcode;` private
    //! and `OpCode` `pub(crate)`, so this crate could only hand-maintain the
    //! same unverified table one layer lower, and it would actively mislead —
    //! see `an_unrecognised_operator_is_not_an_error_under_templating` below,
    //! which is exactly the case a static "known operators" table would get
    //! wrong. Every value here was read from a live `datalogic_rs::Engine`
    //! built the way `LogicCompiler::new` builds one, not assumed.
    //!
    //! If a `datalogic-rs` upgrade changes any of these, that is a real
    //! behaviour change for every workflow in production — these tests exist
    //! so it fails CI instead of surfacing as a support ticket.
    //!
    //! These values are also *feature*-dependent. This crate exposes the
    //! `datalogic-rs` operator families as cargo features, all off by default.
    //! Any test whose answer changes when a family is enabled carries a
    //! `#[cfg(feature = ...)]` so **both** configurations stay pinned —
    //! otherwise the `--all-features` CI run would be the only one checking
    //! anything and the default build, which is what `cargo add dataflow-rs`
    //! delivers, would go untested.

    use super::*;
    use serde_json::json;

    /// The exact engine construction `LogicCompiler::new` uses: templating
    /// enabled, plus whichever `datalogic-rs` operator families this crate's
    /// cargo features turned on — none, by default. Which families are live is
    /// fixed at compile time, so a test whose result depends on one must be
    /// `#[cfg]`-gated rather than assuming the default build.
    fn engine() -> Engine {
        Engine::builder().with_templating(true).build()
    }

    fn eval(engine: &Engine, logic: &Value) -> Value {
        let compiled = engine.compile_arc(logic).expect("should compile");
        let ctx = datavalue::OwnedDataValue::from(&json!({}));
        serde_json::from_str(
            &engine
                .session()
                .eval_str(&compiled, &ctx)
                .expect("should evaluate"),
        )
        .expect("eval_str output should be valid JSON")
    }

    /// A one-task workflow carrying `extra` as additional top-level JSON keys.
    fn workflow_json(extra: &str) -> String {
        format!(
            r#"{{ "id": "w", "name": "w", {extra}
                 "tasks": [{{"id": "t", "name": "t",
                             "function": {{"name": "map", "input": {{"mappings": []}}}}}}] }}"#
        )
    }

    #[test]
    fn compile_workflows_precomputes_the_loop_counter_path() {
        let workflow =
            Workflow::from_json(&workflow_json(r#""loop": {"counter": "i", "max": 3},"#))
                .expect("should parse");

        let compiled = LogicCompiler::new()
            .compile_workflows(vec![workflow])
            .expect("should compile");

        let cfg = compiled[0].loop_config.as_ref().expect("loop config");
        let parts: Vec<&str> = cfg.counter_parts.iter().map(Arc::as_ref).collect();
        assert_eq!(parts, ["temp_data", "i"]);
    }

    #[test]
    fn compile_workflows_rejects_an_invalid_loop_config() {
        // `Workflow::validate` runs inside `compile_workflows`, so a bound that
        // could never advance fails engine construction rather than the first
        // message.
        let workflow =
            Workflow::from_json(&workflow_json(r#""loop": {"init": 5, "max": 5},"#)).unwrap();

        assert!(
            LogicCompiler::new()
                .compile_workflows(vec![workflow])
                .is_err()
        );
    }

    #[test]
    fn compile_workflows_leaves_a_non_looping_workflow_without_a_loop() {
        let workflow = Workflow::from_json(&workflow_json("")).expect("should parse");

        let compiled = LogicCompiler::new()
            .compile_workflows(vec![workflow])
            .expect("should compile");

        assert!(compiled[0].loop_config.is_none());
    }

    #[test]
    fn empty_operand_results_this_crate_would_silently_break_on() {
        // A workflow author can write any of these — a map mapping folding an
        // empty list, a filter condition over an empty selector — and the
        // crate never validates operand count. If a datalogic upgrade changed
        // any of these defaults, every workflow relying on the vacuous case
        // would silently start producing a different value.
        let e = engine();
        for (logic, expected) in [
            (json!({"and": []}), json!(null)),
            (json!({"or": []}), json!(null)),
            (json!({"+": []}), json!(0)),
            (json!({"*": []}), json!(1)),
            (json!({"cat": []}), json!("")),
            (json!({"merge": []}), json!([])),
            (json!({"missing": []}), json!([])),
        ] {
            assert_eq!(eval(&e, &logic), expected, "for {logic}");
        }
    }

    #[test]
    fn a_missing_var_path_resolves_to_null_not_an_error() {
        // The exact mechanism behind the pitfall CLAUDE.md documents for
        // `payload.*` expressions: a `var` over a path that does not resolve
        // is `Null`, silently, never `Err`. `Template::eval` and the built-in
        // `*_logic` fields inherit this — there is no engine-level signal that
        // distinguishes "field absent" from "field is null".
        let e = engine();
        assert_eq!(
            eval(&e, &json!({"var": "data.does_not_exist"})),
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
        let e = engine();
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
                eval(&e, &json!({"!!": v})),
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
        let e = engine();
        let logic = json!({"totally_made_up_op_xyz": ["a", "b"]});
        assert_eq!(
            eval(&e, &logic),
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
        let e = engine();
        let logic = json!({"starts_with": ["hello", "he"]});
        assert_eq!(
            eval(&e, &logic),
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
        let e = engine();
        assert_eq!(
            eval(&e, &json!({"starts_with": ["hello", "he"]})),
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
        let e = engine();
        let logic = json!({"==": ["2024-01-15T00:00:00Z", "2024-01-15T01:00:00+01:00"]});
        #[cfg(feature = "datetime")]
        assert_eq!(eval(&e, &logic), json!(true));
        #[cfg(not(feature = "datetime"))]
        assert_eq!(eval(&e, &logic), json!(false));
    }

    /// Each family's cargo feature actually reaches `datalogic-rs`. One
    /// representative operator per family is enough — the feature either
    /// forwards or it does not.
    #[cfg(feature = "ext-string")]
    #[test]
    fn ext_string_feature_reaches_datalogic() {
        let e = engine();
        assert_eq!(eval(&e, &json!({"upper": "ab"})), json!("AB"));
    }

    #[cfg(feature = "ext-array")]
    #[test]
    fn ext_array_feature_reaches_datalogic() {
        let e = engine();
        assert_eq!(eval(&e, &json!({"sort": [[3, 1, 2]]})), json!([1, 2, 3]));
    }

    #[cfg(feature = "ext-math")]
    #[test]
    fn ext_math_feature_reaches_datalogic() {
        let e = engine();
        assert_eq!(eval(&e, &json!({"abs": -5})), json!(5));
    }

    #[cfg(feature = "ext-control")]
    #[test]
    fn ext_control_feature_reaches_datalogic() {
        let e = engine();
        assert_eq!(
            eval(&e, &json!({"??": [null, "fallback"]})),
            json!("fallback")
        );
    }

    #[cfg(feature = "error-handling")]
    #[test]
    fn error_handling_feature_reaches_datalogic() {
        // `error-handling` is the JSONLogic `try`/`throw` pair — unrelated to
        // this crate's own always-on error handling.
        let e = engine();
        assert_eq!(
            eval(&e, &json!({"try": [{"throw": "boom"}, "recovered"]})),
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
        let e = engine();
        let logic = json!({"now": []});
        let result = eval(&e, &logic);
        assert_ne!(result, logic, "with datetime on, `now` must not echo back");
        assert!(!result.is_null(), "`now` should produce a value, got null");
    }
}
