//! # Custom Function Example
//!
//! This example demonstrates how to create and use custom async functions
//! with the dataflow-rs engine.
//!
//! Run with: `cargo run --example custom_function`

use async_trait::async_trait;
use dataflow_rs::Result;
use dataflow_rs::{
    Engine, Workflow,
    engine::{
        AsyncFunctionHandler, FunctionConfig, SyncFunctionWrapper,
        error::DataflowError,
        functions::FunctionHandler,
        message::{Change, Message},
    },
};
use datalogic_rs::DataLogic;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Custom synchronous function that calculates statistics from numeric data
/// This demonstrates using legacy sync handlers with the new async engine
pub struct StatisticsFunction;

impl FunctionHandler for StatisticsFunction {
    fn execute(
        &self,
        message: &mut Message,
        config: &FunctionConfig,
        _datalogic: &DataLogic,
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
        Ok((
            200,
            vec![Change {
                path: output_path.to_string(),
                old_value: Value::Null,
                new_value: stats,
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
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = &message.data;

        // Navigate to the target location
        for part in parts {
            current = current.get(part).unwrap_or(&Value::Null);
        }

        // Extract numbers from the value
        match current {
            Value::Array(arr) => {
                let mut numbers = Vec::new();
                for val in arr {
                    if let Some(num) = val.as_f64() {
                        numbers.push(num);
                    }
                }
                Ok(numbers)
            }
            _ => Err(DataflowError::Validation(format!(
                "Expected array at path '{}', found {:?}",
                path, current
            ))),
        }
    }

    fn calculate_statistics(&self, numbers: &[f64]) -> Value {
        let count = numbers.len() as f64;
        let sum: f64 = numbers.iter().sum();
        let mean = sum / count;

        let mut sorted = numbers.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let median = if sorted.len() % 2 == 0 {
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
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = &mut message.data;

        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Last part - set the value
                if let Value::Object(map) = current {
                    map.insert(part.to_string(), value);
                    return Ok(());
                }
            } else {
                // Navigate or create intermediate objects
                if !current.is_object() {
                    *current = json!({});
                }
                if let Value::Object(map) = current {
                    current = map.entry(part.to_string()).or_insert_with(|| json!({}));
                }
            }
        }

        Err(DataflowError::Validation(format!(
            "Failed to set value at path '{}'",
            path
        )))
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
        _datalogic: Arc<DataLogic>,
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
            .data
            .get(user_id)
            .and_then(Value::as_str)
            .unwrap_or("unknown");

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

        // Add enriched data to message
        if let Value::Object(ref mut map) = message.data {
            map.insert("enriched".to_string(), enriched_data.clone());
        }

        Ok((
            200,
            vec![Change {
                path: "enriched".to_string(),
                old_value: Value::Null,
                new_value: enriched_data,
            }],
        ))
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("🚀 Custom Function Example");
    println!("==========================\n");

    // Define workflow with custom function
    let workflow_json = r#"
    {
        "id": "statistics_workflow",
        "name": "Data Processing Workflow",
        "tasks": [
            {
                "id": "prepare_data",
                "name": "Prepare Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "numbers",
                                "logic": { "var": "measurements" }
                            },
                            {
                                "path": "user_id",
                                "logic": { "var": "user_id" }
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
                                "path": "statistics.count",
                                "logic": { ">": [{ "var": "statistics.count" }, 0] },
                                "message": "Statistics must have at least one data point"
                            },
                            {
                                "path": "enriched.user_profile",
                                "logic": { "!!": { "var": "enriched.user_profile" } },
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

    // Add sync function wrapped for async compatibility
    custom_functions.insert(
        "statistics".to_string(),
        Box::new(SyncFunctionWrapper::new(
            Box::new(StatisticsFunction::new()) as Box<dyn FunctionHandler + Send + Sync>,
        )),
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
    let mut message = Message::new(&sample_data);

    println!("Processing message with custom functions...\n");

    // Process the message through our custom workflow
    match engine.process_message(&mut message).await {
        Ok(_) => {
            println!("✅ Message processed successfully!\n");

            println!("📊 Final Results:");
            println!("{}\n", serde_json::to_string_pretty(&message.data)?);

            println!("📋 Audit Trail:");
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
                println!("\n⚠️  Errors encountered:");
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
            println!("❌ Error processing message: {e:?}");
        }
    }

    println!("\n🎉 Custom function example completed!");

    Ok(())
}
