use crate::engine::task::FunctionHandler;
use crate::engine::workflow::Workflow;
use datalogic_rs::{DataLogic, DataValue, FromJson, Logic};
use std::collections::HashMap;
use chrono::Utc;
use super::Task;
use super::message::{AuditTrail, Message};
// Main engine that processes messages through workflows
pub struct Engine<'a> {
    functions: HashMap<String, Box<dyn FunctionHandler>>,
    workflows: Vec<(Logic<'a>, Workflow)>,
    data_logic: &'a DataLogic,
}

impl<'a> Engine<'a> {
    pub fn new(data_logic: &'a DataLogic) -> Self {
        Self {
            functions: HashMap::new(),
            workflows: Vec::new(),
            data_logic,
        }
    }

    pub fn add_workflow(&mut self, workflow: &Workflow) {
        let condition = workflow.condition.clone();
        let condition_logic = self.data_logic.parse_logic_json(&condition.unwrap(), None).unwrap();
        self.workflows.push((condition_logic, workflow.clone()));
    }

    pub fn register_function(&mut self, id: String, handler: Box<dyn FunctionHandler>) {
        self.functions.insert(id, handler);
    }

    pub fn process_message<'m>(&'m self, message: &mut Message<'m>) 
    where 'a: 'm {
        for (condition_logic, workflow) in &self.workflows {
            let result = self.data_logic.evaluate(condition_logic, &message.metadata);
            match result {
                Ok(result) => {
                    if let Some(result) = result.as_bool() {
                        if result {
                            for (condition_logic, task) in &workflow.task_logics {
                                let task_result = self.data_logic.evaluate(condition_logic, &message.metadata);
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

    fn execute_task<'m>(&'m self, message: &mut Message<'m>, task: &Task) 
    where 'a: 'm {
        if let Some(function) = self.functions.get(&task.function) {
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
            println!("Function not found: {}", task.function);
        }
    }
} 