//! # Custom Function Example
//!
//! This example demonstrates how to create and use custom async functions
//! with the dataflow-rs engine.
//!
//! The workflow follows the recommended pattern:
//! 1. parse_json - Load payload into data context
//! 2. Custom functions - Process the data
//! 3. validation - Validate results
//!
//! Run with: `cargo run --example custom_function`

use async_trait::async_trait;
use dataflow_rs::Result;
use dataflow_rs::{
    Engine, Workflow,
    engine::{
        AsyncFunctionHandler, FunctionConfig,
        error::DataflowError,
        message::{Change, Message},
        utils::set_nested_value,
    },
};
use datalogic_rs::Engine as DatalogicEngine;
use datavalue::OwnedDataValue;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Custom async function that calculates statistics from numeric data
pub struct StatisticsFunction;

#[async_trait]
impl AsyncFunctionHandler for StatisticsFunction {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        _engine: Arc<DatalogicEngine>,
    ) -> Result<(usize, Vec<Change>)> {
        // Extract the raw input from config
        let input = match config {
            FunctionConfig::Custom { input, .. } => input,
            _ => {
                return Err(DataflowError::Validation(
                    "Invalid configuration type for statistics function".to_string(),
                ));
            }
        };

        // Extract the data path to analyze
        let data_path = input
            .get("data_path")
            .and_then(Value::as_str)
            .unwrap_or("numbers");

        // Extract the output path where to store results
        let output_path = input
            .get("output_path")
            .and_then(Value::as_str)
            .unwrap_or("statistics");

        // Get the numbers from the specified path
        let numbers = self.extract_numbers_from_path(message, data_path)?;

        if numbers.is_empty() {
            return Err(DataflowError::Validation(
                "No numeric data found to analyze".to_string(),
            ));
        }

        // Calculate statistics
        let stats = self.calculate_statistics(&numbers);

        // Store the results in the message
        self.set_value_at_path(message, output_path, stats.clone())?;

        // Return success with changes
        let stats_owned = OwnedDataValue::from(&stats);
        Ok((
            200,
            vec![Change {
                path: Arc::from(output_path),
                old_value: Arc::new(OwnedDataValue::Null),
                new_value: Arc::new(stats_owned),
            }],
        ))
    }
}

impl Default for StatisticsFunction {
    fn default() -> Self {
        Self::new()
    }
}

impl StatisticsFunction {
    pub fn new() -> Self {
        Self
    }

    fn extract_numbers_from_path(&self, message: &Message, path: &str) -> Result<Vec<f64>> {
        use dataflow_rs::engine::utils::get_nested_value;
        let target = get_nested_value(message.data(), path);
        match target.and_then(|v| v.as_array()) {
            Some(arr) => Ok(arr.iter().filter_map(|v| v.as_f64()).collect()),
            None => Err(DataflowError::Validation(format!(
                "Expected array at path '{}', found {:?}",
                path, target
            ))),
        }
    }

    fn calculate_statistics(&self, numbers: &[f64]) -> Value {
        let count = numbers.len() as f64;
        let sum: f64 = numbers.iter().sum();
        let mean = sum / count;

        let mut sorted = numbers.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let median = if sorted.len().is_multiple_of(2) {
            let mid = sorted.len() / 2;
            (sorted[mid - 1] + sorted[mid]) / 2.0
        } else {
            sorted[sorted.len() / 2]
        };

        let variance: f64 = numbers.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count;
        let std_dev = variance.sqrt();

        json!({
            "count": count,
            "sum": sum,
            "mean": mean,
            "median": median,
            "min": sorted[0],
            "max": sorted[sorted.len() - 1],
            "std_dev": std_dev,
            "variance": variance
        })
    }

    fn set_value_at_path(&self, message: &mut Message, path: &str, value: Value) -> Result<()> {
        // Use the engine's helper; it understands the OwnedDataValue shape and
        // auto-creates intermediate objects/arrays along the path.
        set_nested_value(
            &mut message.context,
            &format!("data.{}", path),
            OwnedDataValue::from(&value),
        );
        Ok(())
    }
}

/// Custom async function that enriches data with external information
/// This demonstrates a native async handler
pub struct AsyncDataEnrichmentFunction;

#[async_trait]
impl AsyncFunctionHandler for AsyncDataEnrichmentFunction {
    async fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        _engine: Arc<DatalogicEngine>,
    ) -> Result<(usize, Vec<Change>)> {
        // Extract the raw input from config
        let input = match config {
            FunctionConfig::Custom { input, .. } => input,
            _ => {
                return Err(DataflowError::Validation(
                    "Invalid configuration type for enrichment function".to_string(),
                ));
            }
        };

        // Get user ID to enrich
        let user_id = input
            .get("user_id_path")
            .and_then(Value::as_str)
            .unwrap_or("user_id");

        let user_id_value = message
            .data()
            .get(user_id)
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Simulate async API call
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Create enriched data (simulated)
        let enriched_data = json!({
            "user_profile": {
                "id": user_id_value,
                "name": format!("User {}", user_id_value),
                "email": format!("{}@example.com", user_id_value),
                "created_at": "2024-01-15T10:30:00Z",
                "preferences": {
                    "theme": "dark",
                    "notifications": true
                }
            },
            "enrichment_timestamp": chrono::Utc::now().to_rfc3339()
        });
        let enriched_owned = OwnedDataValue::from(&enriched_data);

        // Add enriched data to message
        set_nested_value(
            &mut message.context,
            "data.enriched",
            enriched_owned.clone(),
        );

        Ok((
            200,
            vec![Change {
                path: Arc::from("enriched"),
                old_value: Arc::new(OwnedDataValue::Null),
                new_value: Arc::new(enriched_owned),
            }],
        ))
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("Custom Function Example");
    println!("==========================\n");

    // Define workflow with parse_json as first task, followed by custom functions
    let workflow_json = r#"
    {
        "id": "statistics_workflow",
        "name": "Data Processing Workflow",
        "tasks": [
            {
                "id": "load_payload",
                "name": "Load Payload",
                "description": "Parse JSON payload into data context",
                "function": {
                    "name": "parse_json",
                    "input": {
                        "source": "payload",
                        "target": "input"
                    }
                }
            },
            {
                "id": "prepare_data",
                "name": "Prepare Data",
                "description": "Extract fields from parsed input",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.numbers",
                                "logic": { "var": "data.input.measurements" }
                            },
                            {
                                "path": "data.user_id",
                                "logic": { "var": "data.input.user_id" }
                            }
                        ]
                    }
                }
            },
            {
                "id": "calculate_stats",
                "name": "Calculate Statistics",
                "function": {
                    "name": "statistics",
                    "input": {
                        "data_path": "numbers",
                        "output_path": "statistics"
                    }
                }
            },
            {
                "id": "enrich_user_data",
                "name": "Enrich User Data",
                "function": {
                    "name": "enrich_data",
                    "input": {
                        "user_id_path": "user_id"
                    }
                }
            },
            {
                "id": "validate_results",
                "name": "Validate Results",
                "function": {
                    "name": "validation",
                    "input": {
                        "rules": [
                            {
                                "logic": { ">": [{ "var": "data.statistics.count" }, 0] },
                                "message": "Statistics must have at least one data point"
                            },
                            {
                                "logic": { "!!": { "var": "data.enriched.user_profile" } },
                                "message": "User profile enrichment is required"
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    let workflow = Workflow::from_json(workflow_json)?;

    // Prepare custom functions
    let mut custom_functions: HashMap<String, Box<dyn AsyncFunctionHandler + Send + Sync>> =
        HashMap::new();

    // Add statistics function
    custom_functions.insert(
        "statistics".to_string(),
        Box::new(StatisticsFunction::new()),
    );

    // Add native async function
    custom_functions.insert(
        "enrich_data".to_string(),
        Box::new(AsyncDataEnrichmentFunction),
    );

    // Create engine with custom functions
    let engine = Engine::new(vec![workflow], Some(custom_functions));

    // Create sample data
    let sample_data = json!({
        "measurements": [10.5, 15.2, 8.7, 22.1, 18.9, 12.3, 25.6, 14.8, 19.4, 16.7],
        "user_id": "user_123",
        "timestamp": "2024-01-15T10:30:00Z"
    });

    // Create and process message
    let mut message = Message::from_value(&sample_data);

    println!("Processing message with custom functions...\n");

    // Process the message through our custom workflow
    match engine.process_message(&mut message).await {
        Ok(_) => {
            println!("Message processed successfully!\n");

            println!("Final Results:");
            println!(
                "{}\n",
                serde_json::to_string_pretty(&message.context["data"])?
            );

            println!("Audit Trail:");
            for (i, audit) in message.audit_trail.iter().enumerate() {
                println!(
                    "{}. Task: {} (Status: {})",
                    i + 1,
                    audit.task_id,
                    audit.status
                );
                println!("   Timestamp: {}", audit.timestamp);
                println!("   Changes: {} field(s) modified", audit.changes.len());
            }

            if message.has_errors() {
                println!("\nErrors encountered:");
                for error in &message.errors {
                    println!(
                        "   - {}: {}",
                        error.task_id.as_ref().unwrap_or(&"unknown".to_string()),
                        error.message
                    );
                }
            }
        }
        Err(e) => {
            println!("Error processing message: {e:?}");
        }
    }

    println!("\nCustom function example completed!");

    Ok(())
}
