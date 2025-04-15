use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Clone, Debug)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub condition: Option<Value>,
    pub function: Function,
}

impl Task {
    pub fn new(
        id: String,
        name: String,
        description: Option<String>,
        condition: Option<Value>,
        function: Function,
    ) -> Self {
        Self {
            id,
            name,
            description,
            condition,
            function,
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct Function {
    pub name: String,
    pub input: Value,
}
