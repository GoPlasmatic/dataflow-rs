use serde::Deserialize;
use serde_json::Value;

use super::functions::Function;

// Task configuration struct - represents task definition in workflow configuration
#[derive(Deserialize, Clone, Debug)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub condition: Option<Value>,
    pub function: Function,
    pub input: Value,
}

impl Task {
    pub fn new(
        id: String,
        name: String,
        description: Option<String>,
        condition: Option<Value>,
        function: Function,
        input: Value,
    ) -> Self {
        Self {
            id,
            name,
            description,
            condition,
            function,
            input,
        }
    }
}
