//! # Validation Function Module
//!
//! This module provides rule-based validation capabilities using JSONLogic expressions.
//! The validation function evaluates a set of rules against message data and collects
//! any validation errors that occur.
//!
//! ## Features
//!
//! - Define validation rules using JSONLogic expressions
//! - Custom error messages for each rule
//! - Non-destructive: validation is read-only and doesn't modify message data
//! - Errors are collected in the message's error list
//!
//! ## Example Usage
//!
//! ```json
//! {
//!     "name": "validation",
//!     "input": {
//!         "rules": [
//!             {
//!                 "logic": {"!!": [{"var": "data.email"}]},
//!                 "message": "Email is required"
//!             },
//!             {
//!                 "logic": {">": [{"var": "data.age"}, 0]},
//!                 "message": "Age must be positive"
//!             }
//!         ]
//!     }
//! }
//! ```

use crate::engine::error::{DataflowError, ErrorInfo, Result};
use crate::engine::executor::{ArenaContext, with_arena};
use crate::engine::functions::path_template::ParamCtx;
use crate::engine::functions::template::Template;
use crate::engine::message::{Change, Message};
use crate::engine::task_outcome::TaskOutcome;
use datalogic_rs::{Engine, Logic};
use datavalue::DataValue;
use log::{debug, error};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Configuration for the validation function containing a list of rules.
///
/// Each rule specifies a JSONLogic condition that must evaluate to `true`
/// for the validation to pass. If a rule evaluates to anything other than
/// `true`, its error message is added to the message's error list.
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationConfig {
    /// List of validation rules to evaluate.
    pub rules: Vec<ValidationRule>,
}

/// A single validation rule with a condition and error message.
///
/// The rule's logic is evaluated against the message context. If it does not
/// return exactly `true`, the validation fails and the error message is recorded.
#[derive(Debug, Clone, Deserialize)]
pub struct ValidationRule {
    /// JSONLogic expression that must evaluate to `true` for validation to pass.
    /// Any other result (false, null, etc.) is considered a validation failure.
    pub logic: Value,

    /// Error message to display if validation fails.
    ///
    /// Required when a rule is deserialized as part of a workflow definition —
    /// which is the path `Engine::build` takes, so a rule without it is
    /// rejected at build time. [`ValidationConfig::from_json`], the standalone
    /// parser, is the one path that substitutes `"Validation failed"`.
    ///
    /// JSONLogic, so a message can name the value that failed:
    /// `{"cat": ["age must be positive, got ", {"var": "data.age"}]}`. A plain
    /// string is a literal, so the static spelling is unchanged.
    ///
    /// Evaluated **only when the rule fails**, so a passing validation — the
    /// common case — costs nothing for a computed message.
    ///
    /// This message is recorded in [`Message::errors`], which is serialized, so
    /// it may not read a secret. `Engine::build` refuses one that does; see
    /// [`crate::IssueCode::SecretInMessageWrite`].
    pub message: Template,

    /// Pre-compiled JSONLogic, populated by `LogicCompiler`. `None` is
    /// recorded as a `COMPILATION_ERROR` at execute time.
    #[serde(skip)]
    pub compiled_logic: Option<Arc<Logic>>,
}

impl ValidationConfig {
    /// Parses a `ValidationConfig` from a JSON value.
    ///
    /// # Arguments
    /// * `input` - JSON object containing a "rules" array
    ///
    /// # Errors
    /// Returns `DataflowError::Validation` if:
    /// - The "rules" field is missing
    /// - The "rules" field is not an array
    /// - Any rule is missing the "logic" field
    pub fn from_json(input: &Value) -> Result<Self> {
        let rules = input.get("rules").ok_or_else(|| {
            DataflowError::Validation("Missing 'rules' array in input".to_string())
        })?;

        let rules_arr = rules
            .as_array()
            .ok_or_else(|| DataflowError::Validation("'rules' must be an array".to_string()))?;

        let mut parsed_rules = Vec::new();

        for rule in rules_arr {
            let logic = rule
                .get("logic")
                .ok_or_else(|| DataflowError::Validation("Missing 'logic' in rule".to_string()))?
                .clone();

            let message = Template::from(
                rule.get("message")
                    .cloned()
                    .unwrap_or_else(|| Value::from("Validation failed")),
            );

            parsed_rules.push(ValidationRule {
                logic,
                message,
                compiled_logic: None,
            });
        }

        Ok(Self {
            rules: parsed_rules,
        })
    }

    /// Executes all validation rules using pre-compiled logic.
    ///
    /// Evaluates each rule sequentially against the message context.
    /// This is a read-only operation that does not modify message data.
    ///
    /// # Arguments
    /// * `message` - The message to validate (errors are added to its error list)
    /// * `engine` - Datalogic v5 engine for evaluation
    ///
    /// # Returns
    /// * `Ok((TaskOutcome::Success, []))` — all rules passed
    /// * `Ok((TaskOutcome::Status(400), []))` — one or more rules failed,
    ///   `ErrorInfo` entries pushed onto `message.errors`
    pub fn execute(
        &self,
        message: &mut Message,
        engine: &Arc<Engine>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        // Default path: open the arena and convert context once for this
        // task call. When called from the workflow-level sync-stretch
        // executor (`execute_in_arena`), the conversion is reused across
        // multiple tasks in the same stretch.
        with_arena(|arena| {
            let ctx_av: DataValue<'_> = message.context.to_arena(arena);
            self.run_rules(message, ctx_av, arena, engine)
        })
    }

    /// Run validation rules against an externally-provided `ArenaContext`.
    /// Reuses the cached arena form built by an earlier task in the same
    /// workflow sync stretch — the heavy `data.input` subtree stays cached
    /// across the parse_json → map → validation pipeline.
    pub(crate) fn execute_in_arena(
        &self,
        message: &mut Message,
        arena_ctx: &mut ArenaContext<'_>,
        engine: &Arc<Engine>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        let arena = arena_ctx.arena();
        let ctx_av = arena_ctx.as_data_value();
        self.run_rules(message, ctx_av, arena, engine)
    }

    /// Shared inner loop: evaluate each rule against `ctx_av` and record
    /// `ErrorInfo` entries for any failures.
    fn run_rules(
        &self,
        message: &mut Message,
        ctx_av: DataValue<'_>,
        arena: &bumpalo::Bump,
        engine: &Arc<Engine>,
    ) -> Result<(TaskOutcome, Vec<Change>)> {
        let changes = Vec::new();
        let mut validation_errors = Vec::new();

        for (idx, rule) in self.rules.iter().enumerate() {
            debug!("Processing validation rule {idx}");

            let compiled_logic = match &rule.compiled_logic {
                Some(logic) => logic,
                None => {
                    error!("Validation: Logic not compiled for rule at index {}", idx);
                    validation_errors.push(ErrorInfo::simple_ref(
                        "COMPILATION_ERROR",
                        &format!("Logic not compiled for rule at index: {}", idx),
                        None,
                    ));
                    continue;
                }
            };

            // Reuse the pre-converted `ctx_av` (DataValue is Copy). The
            // result is `&DataValue<'_>` borrowed from the arena — we
            // only need to peek at the discriminant so we skip the
            // `to_owned()` deep-clone too.
            match engine.evaluate(compiled_logic, ctx_av, arena) {
                Ok(value) => {
                    if !matches!(value, DataValue::Bool(true)) {
                        // Resolved here, not up front: a computed message costs
                        // nothing on the passing path. A message that fails to
                        // evaluate still reports the validation failure — the
                        // rule did fail, and losing that to a broken message
                        // expression would be the worse outcome.
                        let text = rule
                            .message
                            .resolve_string_in_arena(ParamCtx::new(engine, ctx_av, arena))
                            .unwrap_or_else(|e| {
                                error!("Validation: rule {idx} message failed to render: {e:?}");
                                "Validation failed".to_string()
                            });
                        debug!("Validation failed for rule {idx}: {text}");
                        validation_errors.push(ErrorInfo::simple_ref(
                            "VALIDATION_ERROR",
                            &text,
                            None,
                        ));
                    } else {
                        debug!("Validation passed for rule {}", idx);
                    }
                }
                Err(e) => {
                    error!("Validation: Error evaluating rule {}: {:?}", idx, e);
                    validation_errors.push(ErrorInfo::simple_ref(
                        "EVALUATION_ERROR",
                        &format!("Failed to evaluate rule {}: {}", idx, e),
                        None,
                    ));
                }
            }
        }

        if !validation_errors.is_empty() {
            message.errors.extend(validation_errors);
            Ok((TaskOutcome::Status(400), changes))
        } else {
            Ok((TaskOutcome::Success, changes))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datavalue::OwnedDataValue;
    use serde_json::json;

    #[test]
    fn test_validation_config_from_json() {
        let input = json!({
            "rules": [
                {
                    "logic": {"!!": [{"var": "data.required_field"}]},
                    "path": "data",
                    "message": "Required field is missing"
                },
                {
                    "logic": {">": [{"var": "data.age"}, 18]},
                    "message": "Must be over 18"
                }
            ]
        });

        let config = ValidationConfig::from_json(&input).unwrap();
        assert_eq!(config.rules.len(), 2);
        assert_eq!(
            config.rules[0].message.as_json(),
            &json!("Required field is missing")
        );
        assert_eq!(config.rules[1].message.as_json(), &json!("Must be over 18"));
    }

    #[test]
    fn test_validation_config_missing_rules() {
        let input = json!({});
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_invalid_rules() {
        let input = json!({
            "rules": "not_an_array"
        });
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_missing_logic() {
        let input = json!({
            "rules": [
                {
                    "path": "data",
                    "message": "Some error"
                }
            ]
        });
        let result = ValidationConfig::from_json(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_validation_config_defaults() {
        let input = json!({
            "rules": [
                {
                    "logic": {"var": "data.field"}
                }
            ]
        });

        let config = ValidationConfig::from_json(&input).unwrap();
        assert_eq!(
            config.rules[0].message.as_json(),
            &json!("Validation failed")
        );
    }

    fn dv(v: serde_json::Value) -> OwnedDataValue {
        OwnedDataValue::from(&v)
    }

    fn message_with_data(initial: serde_json::Value) -> crate::engine::message::Message {
        use crate::engine::message::Message;
        Message::builder().data(dv(initial)).build()
    }

    /// Compile each rule's `logic` and stamp the resulting `Arc<Logic>` into
    /// the `compiled_logic` slot — mirroring `LogicCompiler`.
    fn compile_rules(engine: &Arc<Engine>, config: &mut ValidationConfig) {
        for rule in &mut config.rules {
            rule.compiled_logic = Some(engine.compile_arc(&rule.logic).unwrap());
        }
    }

    #[test]
    fn test_validation_execute_passes() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());

        let mut message = message_with_data(json!({
            "email": "test@example.com",
            "age": 25
        }));

        let mut config = ValidationConfig {
            rules: vec![
                ValidationRule {
                    logic: json!({"!!": [{"var": "data.email"}]}),
                    message: Template::from(json!("Email is required")),
                    compiled_logic: None,
                },
                ValidationRule {
                    logic: json!({">": [{"var": "data.age"}, 18]}),
                    message: Template::from(json!("Must be over 18")),
                    compiled_logic: None,
                },
            ],
        };
        compile_rules(&engine, &mut config);

        let result = config.execute(&mut message, &engine);
        assert!(result.is_ok());

        let (outcome, changes) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Success);
        assert!(changes.is_empty());
        assert!(message.errors.is_empty());
    }

    #[test]
    fn test_validation_execute_fails() {
        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());

        let mut message = message_with_data(json!({ "age": 15 }));

        let mut config = ValidationConfig {
            rules: vec![
                ValidationRule {
                    logic: json!({"!!": [{"var": "data.email"}]}),
                    message: Template::from(json!("Email is required")),
                    compiled_logic: None,
                },
                ValidationRule {
                    logic: json!({">": [{"var": "data.age"}, 18]}),
                    message: Template::from(json!("Must be over 18")),
                    compiled_logic: None,
                },
            ],
        };
        compile_rules(&engine, &mut config);

        let result = config.execute(&mut message, &engine);
        assert!(result.is_ok());

        let (outcome, _changes) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Status(400));
        assert_eq!(message.errors.len(), 2);

        let error_messages: Vec<&str> = message.errors.iter().map(|e| e.message.as_str()).collect();
        assert!(error_messages.contains(&"Email is required"));
        assert!(error_messages.contains(&"Must be over 18"));
    }

    #[test]
    fn test_validation_uncompiled_logic() {
        use crate::engine::message::Message;

        let engine = Arc::new(crate::engine::compiler::datalogic_engine_builder().build());

        let mut message = Message::new(Arc::new(dv(json!({}))));

        let config = ValidationConfig {
            rules: vec![ValidationRule {
                logic: json!(true),
                message: Template::from(json!("Test")),
                compiled_logic: None,
            }],
        };

        let result = config.execute(&mut message, &engine);
        assert!(result.is_ok());

        let (outcome, _) = result.unwrap();
        assert_eq!(outcome, TaskOutcome::Status(400));
        assert!(!message.errors.is_empty());
        assert!(message.errors[0].code == "COMPILATION_ERROR");
    }
}
