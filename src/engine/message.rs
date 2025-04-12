use datalogic_rs::DataValue;
use chrono::{DateTime, Utc};

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