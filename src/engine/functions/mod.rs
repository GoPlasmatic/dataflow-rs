pub mod source;
pub mod task;

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, Clone, Debug)]
pub struct Function {
    pub name: String,
    pub input: Value,
}

impl Function {
    pub fn new(name: String, input: Value) -> Self {
        Self { name, input }
    }
}
