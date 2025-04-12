use crate::engine::message::Message;
use datalogic_rs::{arena::DataArena, DataValue};
use serde::Deserialize;
use serde_json::Value;

use super::message::Change;
// Task configuration struct - represents task definition in workflow configuration
#[derive(Deserialize, Clone, Debug)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub condition: Option<Value>,
    pub function: String,
    pub input: Value,
}

impl Task {
    pub fn builder() -> TaskBuilder {
        TaskBuilder::new()
    }
}

// Task Builder for programmatic task creation
pub struct TaskBuilder {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    condition: Option<Value>,
    function: Option<String>,
    input: Value,
}

impl Default for TaskBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            name: None,
            description: None,
            condition: None,
            function: None,
            input: serde_json::json!({}),
        }
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn condition(mut self, condition: Value) -> Self {
        self.condition = Some(condition);
        self
    }

    pub fn function(mut self, function: impl Into<String>) -> Self {
        self.function = Some(function.into());
        self
    }

    pub fn input(mut self, input: Value) -> Self {
        self.input = input;
        self
    }

    pub fn build(self) -> Task {
        Task {
            id: self.id.expect("Task ID is required"),
            name: self.name.expect("Task name is required"),
            description: self.description,
            condition: self.condition,
            function: self.function.expect("Task function is required"),
            input: self.input,
        }
    }
}

// Task trait for implementing custom task behaviors
pub trait FunctionHandler {
    fn execute<'a>(
        &self,
        message: &mut Message<'a>,
        input: &DataValue,
        arena: &'a DataArena,
    ) -> Result<Vec<Change<'a>>, String>;
}
