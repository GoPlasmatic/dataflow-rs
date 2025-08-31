use crate::engine::functions::FunctionConfig;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub condition: Option<Value>,
    pub function_name: String,
    pub function_config: FunctionConfig,
}

impl Task {
    pub fn new(
        id: String,
        name: String,
        description: Option<String>,
        condition: Option<Value>,
        function_name: String,
        function_config: FunctionConfig,
    ) -> Self {
        Self {
            id,
            name,
            description,
            condition,
            function_name,
            function_config,
        }
    }
}

/// Temporary structure for deserializing raw task data
#[derive(Deserialize, Clone, Debug)]
pub struct RawTask {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub condition: Option<Value>,
    pub function: RawFunction,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RawFunction {
    pub name: String,
    pub input: Value,
}
