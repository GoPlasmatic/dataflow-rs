use datalogic_rs::{DataLogic, Logic};
use serde::Deserialize;
use serde_json::Value;
use crate::engine::task::Task;
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

    // pub fn build(self, data_logic: &'static DataLogic) -> Workflow {
    //     let mut task_logics = Vec::new();
    //     for task in &self.tasks {
    //         let condition = task.condition.clone().unwrap();
    //         let logic = data_logic.parse_logic_json(&condition, None).unwrap();
    //         task_logics.push((logic, task.clone()));
    //     }
        
    //     Workflow {
    //         id: self.id.expect("Workflow ID is required"),
    //         name: self.name.expect("Workflow name is required"),
    //         description: self.description,
    //         condition: self.condition,
    //         tasks: self.tasks,
    //         task_logics,
    //     }
    // }
