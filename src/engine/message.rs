use chrono::{DateTime, Utc};
use datalogic_rs::DataValue;

#[derive(Debug, Clone)]
pub struct Message<'a> {
    pub id: String,
    pub data: DataValue<'a>,
    pub payload: DataValue<'a>,
    pub metadata: DataValue<'a>,
    pub temp_data: DataValue<'a>,
    pub audit_trail: Vec<AuditTrail<'a>>,
}

#[derive(Debug, Clone)]
pub struct AuditTrail<'a> {
    pub workflow_id: String,
    pub task_id: String,
    pub timestamp: DateTime<Utc>,
    pub changes: Vec<Change<'a>>,
}

#[derive(Debug, Clone)]
pub struct Change<'a> {
    pub path: String,
    pub old_value: DataValue<'a>,
    pub new_value: DataValue<'a>,
}

impl<'a> Message<'a> {
    pub fn new(input: &'a DataValue) -> Self {
        Self {
            id: input.get("id").unwrap().to_string(),
            data: input.get("data").unwrap().clone(),
            payload: input.get("payload").unwrap().clone(),
            metadata: input.get("metadata").unwrap().clone(),
            temp_data: input.get("temp_data").unwrap().clone(),
            audit_trail: vec![],
        }
    }
}
