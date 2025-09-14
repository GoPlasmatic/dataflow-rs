use crate::engine::error::ErrorInfo;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub id: String,
    pub payload: Arc<Value>,
    /// Unified context containing data, metadata, and temp_data
    pub context: Value,
    pub audit_trail: Vec<AuditTrail>,
    /// Errors that occurred during message processing
    pub errors: Vec<ErrorInfo>,
}

impl Message {
    pub fn new(payload: Arc<Value>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            payload,
            context: json!({
                "data": {},
                "metadata": {},
                "temp_data": {}
            }),
            audit_trail: vec![],
            errors: vec![],
        }
    }

    /// Convenience method for creating a message from a Value reference
    /// Note: This clones the entire Value. Use from_arc() to avoid cloning when possible.
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

    /// Get a reference to the data field in context
    pub fn data(&self) -> &Value {
        &self.context["data"]
    }

    /// Get a mutable reference to the data field in context
    pub fn data_mut(&mut self) -> &mut Value {
        &mut self.context["data"]
    }

    /// Get a reference to the metadata field in context
    pub fn metadata(&self) -> &Value {
        &self.context["metadata"]
    }

    /// Get a mutable reference to the metadata field in context
    pub fn metadata_mut(&mut self) -> &mut Value {
        &mut self.context["metadata"]
    }

    /// Get a reference to the temp_data field in context
    pub fn temp_data(&self) -> &Value {
        &self.context["temp_data"]
    }

    /// Get a mutable reference to the temp_data field in context
    pub fn temp_data_mut(&mut self) -> &mut Value {
        &mut self.context["temp_data"]
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
