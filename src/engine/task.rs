use crate::engine::functions::FunctionConfig;
use serde::Deserialize;
use serde_json::Value;

/// Task represents a single processing unit within a workflow
#[derive(Clone, Debug, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default = "default_condition")]
    pub condition: Value,
    #[serde(skip)]
    pub condition_index: Option<usize>,
    pub function: FunctionConfig,
    #[serde(default)]
    pub continue_on_error: bool,
}

fn default_condition() -> Value {
    Value::Bool(true)
}
