use crate::engine::error::ErrorInfo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

/// Evaluation context for DataLogic to avoid repeated JSON creation
pub struct EvaluationContext<'a> {
    pub data: &'a Value,
    pub payload: &'a Value,
    pub metadata: &'a Value,
    pub temp_data: &'a Value,
}

impl<'a> EvaluationContext<'a> {
    /// Create evaluation context from a message
    pub fn from_message(message: &'a Message) -> Self {
        Self {
            data: &message.data,
            payload: message.payload.as_ref(),
            metadata: &message.metadata,
            temp_data: &message.temp_data,
        }
    }

    /// Convert to JSON Value for DataLogic evaluation
    /// This is still needed for DataLogic but we avoid creating it repeatedly
    pub fn to_json(&self) -> Value {
        json!({
            "data": self.data,
            "payload": self.payload,
            "metadata": self.metadata,
            "temp_data": self.temp_data
        })
    }

    /// Convert to Arc<Value> for DataLogic evaluation
    pub fn to_arc_json(&self) -> Arc<Value> {
        Arc::new(self.to_json())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub id: String,
    pub data: Value,
    pub payload: Arc<Value>,
    pub metadata: Value,
    pub temp_data: Value,
    pub audit_trail: Vec<AuditTrail>,
    /// Errors that occurred during message processing
    pub errors: Vec<ErrorInfo>,
}

impl Message {
    pub fn new(payload: Arc<Value>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            data: json!({}),
            payload,
            metadata: json!({}),
            temp_data: json!({}),
            audit_trail: vec![],
            errors: vec![],
        }
    }

    /// Convenience method for creating a message from a Value reference
    pub fn from_value(payload: &Value) -> Self {
        Self::new(Arc::new(payload.clone()))
    }

    /// Create a message from an Arc<Value> directly, avoiding cloning
    pub fn from_arc(payload: Arc<Value>) -> Self {
        Self::new(payload)
    }

    /// Add an error to the message
    pub fn add_error(&mut self, error: ErrorInfo) {
        self.errors.push(error);
    }

    /// Check if message has errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuditTrail {
    pub workflow_id: Arc<str>,
    pub task_id: Arc<str>,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<Change>,
    pub status: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Change {
    pub path: Arc<str>,
    pub old_value: Arc<Value>,
    pub new_value: Arc<Value>,
}
