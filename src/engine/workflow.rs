use crate::engine::task::Task;
use datalogic_rs::{DataLogic, Logic};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Deserialize, Clone, Debug)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub condition: Option<Value>,
    pub tasks: Vec<Task>,

    #[serde(skip)]
    pub task_logics: Vec<(Logic<'static>, Task)>,
}

impl Default for Workflow {
    fn default() -> Self {
        Self::new()
    }
}

impl Workflow {
    pub fn new() -> Self {
        Workflow {
            id: String::new(),
            name: String::new(),
            description: None,
            condition: None,
            tasks: Vec::new(),
            task_logics: Vec::new(),
        }
    }

    // Load workflow from JSON string
    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    // Load workflow from JSON file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let json_str = fs::read_to_string(path)?;
        let workflow = Self::from_json(&json_str)?;
        Ok(workflow)
    }

    pub fn prepare(&mut self, data_logic: &'static DataLogic) {
        for task in &self.tasks {
            let condition = task.condition.clone().unwrap();
            let logic = data_logic.parse_logic_json(&condition, None).unwrap();
            self.task_logics.push((logic, task.clone()));
        }
    }
}
