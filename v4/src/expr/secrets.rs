//! Secrets: values an expression may read but the engine never records.
//!
//! They live in the engine, not in a message's context, and are reached only
//! through the reserved `{"secret": "name"}` operator. The operator is
//! registered on every engine: in templating mode an unregistered name would
//! echo back as literal data, so `{"secret": "k"}` on a plain engine would be
//! handed on as an ordinary object.

use super::path::{Seg, get};
use crate::base::{Error, ErrorKind, Result};
use datalogic_rs::bumpalo::Bump;
use datalogic_rs::operator::EvalContext;
use datalogic_rs::{CustomOperator, DataValue, Error as LogicError};
use datavalue::OwnedDataValue;
use std::fmt;
use std::sync::Arc;

/// The reserved operator name. A host cannot register its own under it.
pub(crate) const SECRET_OPERATOR: &str = "secret";

/// An engine-scoped store of values readable through `{"secret": "name"}`.
///
/// Always an object; nested objects let a host namespace
/// (`{"secret": "partner.hmac"}`). It is neither `Serialize` nor `Clone`, and
/// its `Debug` prints key names with every value masked.
pub struct Secrets {
    root: OwnedDataValue,
}

impl Default for Secrets {
    /// A store with nothing in it: the operator still exists, and every
    /// lookup fails.
    fn default() -> Self {
        Self {
            root: OwnedDataValue::Object(Vec::new()),
        }
    }
}

impl Secrets {
    /// Wraps a host-supplied value. It must be an object, else
    /// [`ErrorKind::Config`].
    pub fn new(root: OwnedDataValue) -> Result<Self> {
        if !root.is_object() {
            return Err(Error::new(
                ErrorKind::Config,
                "secrets must be a JSON object of name -> value",
            ));
        }
        Ok(Self { root })
    }

    /// Looks up a dotted path. The empty path is `None`, never the whole store.
    pub fn get(&self, path: &str) -> Option<&OwnedDataValue> {
        if path.is_empty() {
            return None;
        }
        let segs: Vec<Seg> = path.split('.').map(Seg::parse).collect();
        get(&self.root, &segs)
    }

    /// Top-level key names. Never values.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.root
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k.as_str())
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
/// Error text names the key, never a value and never the store.
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
    use crate::expr::ExprEngine;
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
        assert_eq!(
            format!("{:?}", store()),
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
        let err = Secrets::new(OwnedDataValue::from(&json!(["a"]))).unwrap_err();
        assert_eq!(err.kind(), &ErrorKind::Config);
        assert!(Secrets::new(OwnedDataValue::from(&json!("s"))).is_err());
        assert_eq!(Secrets::default().names().count(), 0);
    }

    #[test]
    fn the_operator_reads_a_nested_secret_and_refuses_an_undeclared_one() {
        let engine = ExprEngine::builder().secrets(store()).build().unwrap();
        assert_eq!(
            engine.eval_json(&json!({"secret": "partner.hmac"})),
            json!("hmac-value-1a2b")
        );
        let err = engine
            .try_eval_json(&json!({"secret": "nope"}))
            .unwrap_err();
        assert_eq!(err.tag(), "VariableNotFound");
        assert!(!err.to_string().contains("hmac-value"));
        // With no store configured the operator is still live, and refuses.
        let bare = ExprEngine::default();
        assert!(bare.try_eval_json(&json!({"secret": "x"})).is_err());
    }
}
