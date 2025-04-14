pub mod functions;
pub mod message;
pub mod task;
pub mod workflow;

// Re-export key types for easier access
pub use functions::source::SourceFunctionHandler;
pub use functions::task::TaskFunctionHandler;
pub use message::Message;
pub use task::Task;
pub use workflow::Workflow;

use message::AuditTrail;

use chrono::Utc;
use datalogic_rs::{DataLogic, DataValue, FromJson, Logic};
use std::collections::HashMap;
// Main engine that processes messages through workflows
pub struct Engine<'a> {
    task_functions: HashMap<String, Box<dyn TaskFunctionHandler>>,
    source_functions: HashMap<String, Box<dyn SourceFunctionHandler>>,
    workflows: Vec<(Logic<'a>, Workflow)>,
    data_logic: &'a DataLogic,
}

impl<'a> Engine<'a> {
    pub fn new(data_logic: &'a DataLogic) -> Self {
        Self {
            task_functions: HashMap::new(),
            source_functions: HashMap::new(),
            workflows: Vec::new(),
            data_logic,
        }
    }

    pub fn add_workflow(&mut self, workflow: &Workflow) {
        let condition = workflow.condition.clone();
        let condition_logic = self
            .data_logic
            .parse_logic_json(&condition.unwrap(), None)
            .unwrap();
        self.workflows.push((condition_logic, workflow.clone()));
    }

    pub fn register_task_function(&mut self, id: String, handler: Box<dyn TaskFunctionHandler>) {
        self.task_functions.insert(id, handler);
    }

    pub fn register_source_function(
        &mut self,
        id: String,
        handler: Box<dyn SourceFunctionHandler>,
    ) {
        self.source_functions.insert(id, handler);
    }

    pub fn process_message(&'a self, message: &mut Message<'a>) {
        for (condition_logic, workflow) in &self.workflows {
            let result = self.data_logic.evaluate(condition_logic, &message.metadata);
            match result {
                Ok(result) => {
                    if let Some(result) = result.as_bool() {
                        if result {
                            for (condition_logic, task) in &workflow.task_logics {
                                let task_result =
                                    self.data_logic.evaluate(condition_logic, &message.metadata);
                                match task_result {
                                    Ok(result) => {
                                        if let Some(result) = result.as_bool() {
                                            if result {
                                                self.execute_task(message, task);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        println!("Error evaluating task: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("Error evaluating condition: {}", e);
                }
            }
        }
    }

    fn execute_task(&'a self, message: &mut Message<'a>, task: &Task) {
        if let Some(function) = self.task_functions.get(&task.function.name) {
            let arena = self.data_logic.arena();
            let input_value = DataValue::from_json(&task.input, arena);

            match function.execute(message, &input_value, arena) {
                Ok(changes) => {
                    let audit_trail = AuditTrail {
                        workflow_id: message.id.clone(),
                        task_id: task.id.clone(),
                        timestamp: Utc::now(),
                        changes,
                    };

                    message.audit_trail.push(audit_trail);
                }
                Err(e) => {
                    println!("Error executing task: {}", e);
                }
            }
        } else {
            println!("Function not found: {}", task.function.name);
        }
    }

    // This is a non-functional placeholder - see example for proper implementation
    pub fn start(&self) {
        println!("To start source functions, create per-thread processors as in the example");
        println!("The Engine is not thread-safe so it needs thread-local instances");
    }
}
