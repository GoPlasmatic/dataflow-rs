use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub id: String,
    pub data: Value,
    pub payload: Value,
    pub metadata: Value,
    pub temp_data: Value,
    pub audit_trail: Vec<AuditTrail>,
}

impl Message {
    pub fn new(payload: &Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            data: Value::Null,
            payload: payload.clone(),
            metadata: Value::Null,
            temp_data: Value::Null,
            audit_trail: vec![],
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuditTrail {
    pub workflow_id: String,
    pub task_id: String,
    pub timestamp: String,
    pub changes: Vec<Change>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Change {
    pub path: String,
    pub old_value: Value,
    pub new_value: Value,
}
