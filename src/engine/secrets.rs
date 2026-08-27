//! # Secrets
//!
//! Values a workflow may *read* but the engine must never *record*.
//!
//! `Message.context` is both what expressions evaluate against and what the
//! engine serializes, snapshots into every [`crate::ExecutionTrace`] step and
//! clones per `map` mapping. For almost every value that is right. For a
//! signing key it is exactly wrong, and there is no way to say so from inside
//! the context — `TraceOptions::redact_paths` prunes named subtrees after the
//! fact, which is the tool you need when a value should not have been there.
//!
//! So secrets do not live in the context at all. They live in a [`Secrets`]
//! store held by the [`crate::Engine`] and are reached through one door: the
//! reserved JSONLogic operator `{"secret": "name"}`, registered on the
//! engine's datalogic instance. Because the store is never part of a
//! `Message`, a secret cannot appear in `Serialize for Message`, in a trace
//! snapshot, in a `mapping_contexts` clone, or in anything a host derives from
//! a message — there is nothing to exclude.
//!
//! The operator is registered on every engine, whether or not secrets were
//! configured. In templating mode an unregistered name would echo back as
//! literal data, so `{"secret": "k"}` on a plain engine would be handed to a
//! handler as an ordinary object — say, as an `Authorization` header. Always
//! registering makes it a loud error instead.

use crate::engine::error::{DataflowError, Result};
use crate::engine::utils::get_nested_value;
use datalogic_rs::bumpalo::Bump;
use datalogic_rs::operator::EvalContext;
use datalogic_rs::{CustomOperator, DataValue, Error as LogicError};
use datavalue::OwnedDataValue;
use std::fmt;
use std::sync::Arc;

/// The reserved operator name. A host cannot register its own operator under
/// it — [`crate::EngineBuilder::build`] refuses.
pub const SECRET_OPERATOR: &str = "secret";

/// An engine-scoped store of values readable through `{"secret": "name"}`.
///
/// Always an object; nested objects are allowed so a host can namespace
/// (`{"secret": "partner.hmac"}`). Deliberately implements neither
/// `Serialize` nor `Clone`, and its `Debug` prints key names with the values
/// masked.
pub struct Secrets {
    root: OwnedDataValue,
}

impl Secrets {
    /// A store with nothing in it. What every engine carries when the host
    /// configured no secrets — the operator still exists, and every lookup
    /// fails.
    pub(crate) fn empty() -> Self {
        Self {
            root: OwnedDataValue::Object(Vec::new()),
        }
    }

    /// Wrap a host-supplied value. Must be an object.
    pub(crate) fn new(root: OwnedDataValue) -> Result<Self> {
        if !root.is_object() {
            return Err(DataflowError::Validation(
                "secrets must be a JSON object of name -> value".to_string(),
            ));
        }
        Ok(Self { root })
    }

    /// Look up a dotted path. The empty path is `None` — never the whole
    /// store.
    pub fn get(&self, path: &str) -> Option<&OwnedDataValue> {
        if path.is_empty() {
            return None;
        }
        get_nested_value(&self.root, path)
    }

    /// Top-level key names. Never values.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        match &self.root {
            OwnedDataValue::Object(pairs) => pairs.iter().map(|(k, _)| k.as_str()),
            _ => unreachable!("Secrets::new only accepts objects"),
        }
    }
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Secrets");
        for name in self.names() {
            s.field(name, &"******");
        }
        s.finish()
    }
}

/// The `secret` operator: one string argument, a dotted path into the store.
///
/// Error text names the *key* — never a value, and never the store.
pub(crate) struct SecretOperator(pub(crate) Arc<Secrets>);

impl CustomOperator for SecretOperator {
    fn evaluate<'a>(
        &self,
        args: &[&'a DataValue<'a>],
        _ctx: &mut EvalContext<'_, 'a>,
        arena: &'a Bump,
    ) -> datalogic_rs::Result<&'a DataValue<'a>> {
        let key = match args {
            [DataValue::String(s)] if !s.is_empty() => *s,
            [DataValue::String(_)] => {
                return Err(LogicError::invalid_arguments(
                    "secret: the key must not be empty",
                ));
            }
            [_] => {
                return Err(LogicError::invalid_arguments(
                    "secret: the key must be a string",
                ));
            }
            _ => {
                return Err(LogicError::invalid_arguments(format!(
                    "secret: takes exactly one argument, got {}",
                    args.len()
                )));
            }
        };
        match self.0.get(key) {
            Some(value) => Ok(arena.alloc(value.to_arena(arena))),
            None => Err(LogicError::variable_not_found(format!(
                "secret '{key}' is not declared"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> Secrets {
        Secrets::new(OwnedDataValue::from(&json!({
            "api_token": "tok-value-9f8e",
            "partner": { "hmac": "hmac-value-1a2b" }
        })))
        .unwrap()
    }

    #[test]
    fn debug_prints_names_and_masks_every_value() {
        let rendered = format!("{:?}", store());
        assert_eq!(
            rendered,
            r#"Secrets { api_token: "******", partner: "******" }"#
        );
    }

    #[test]
    fn the_empty_path_is_not_the_whole_store() {
        assert!(store().get("").is_none());
        assert_eq!(
            store().get("partner.hmac"),
            Some(&OwnedDataValue::from(&json!("hmac-value-1a2b")))
        );
        assert!(store().get("partner.nope").is_none());
    }

    #[test]
    fn only_objects_are_accepted() {
        assert!(Secrets::new(OwnedDataValue::from(&json!(["a"]))).is_err());
        assert!(Secrets::new(OwnedDataValue::from(&json!("s"))).is_err());
        assert_eq!(Secrets::empty().names().count(), 0);
    }
}
