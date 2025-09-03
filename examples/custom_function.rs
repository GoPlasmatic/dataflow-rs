use dataflow_rs::{
    Engine, Workflow,
    engine::{
        FunctionConfig, FunctionHandler,
        error::{DataflowError, Result},
        message::{Change, Message},
    },
};
use datalogic_rs::DataLogic;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Custom function that calculates statistics from numeric data
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
            .unwrap_or("data.numbers");

        // Extract the output path where to store results
        let output_path = input
            .get("output_path")
            .and_then(Value::as_str)
            .unwrap_or("data.statistics");

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
        let mut current = if parts[0] == "data" {
            &message.data
        } else if parts[0] == "temp_data" {
            &message.temp_data
        } else if parts[0] == "metadata" {
            &message.metadata
        } else {
            &message.data
        };

        // Navigate to the target location
        for part in &parts[1..] {
            current = current.get(part).unwrap_or(&Value::Null);
        }

        // Extract numbers from the value
        match current {
            Value::Array(arr) => {
                let mut numbers = Vec::new();
                for item in arr {
                    if let Some(num) = item.as_f64() {
                        numbers.push(num);
                    } else if let Some(num) = item.as_i64() {
                        numbers.push(num as f64);
                    }
                }
                Ok(numbers)
            }
            Value::Number(num) => {
                if let Some(f) = num.as_f64() {
                    Ok(vec![f])
                } else {
                    Ok(vec![])
                }
            }
            _ => Ok(vec![]),
        }
    }

    fn calculate_statistics(&self, numbers: &[f64]) -> Value {
        let count = numbers.len();
        let sum: f64 = numbers.iter().sum();
        let mean = sum / count as f64;

        let mut sorted = numbers.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let median = if count % 2 == 0 {
            (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
        } else {
            sorted[count / 2]
        };

        let variance: f64 = numbers.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count as f64;
        let std_dev = variance.sqrt();

        json!({
            "count": count,
            "sum": sum,
            "mean": mean,
            "median": median,
            "min": sorted[0],
            "max": sorted[count - 1],
            "variance": variance,
            "std_dev": std_dev
        })
    }

    fn set_value_at_path(&self, message: &mut Message, path: &str, value: Value) -> Result<()> {
        let parts: Vec<&str> = path.split('.').collect();
        let target = if parts[0] == "data" {
            &mut message.data
        } else if parts[0] == "temp_data" {
            &mut message.temp_data
        } else if parts[0] == "metadata" {
            &mut message.metadata
        } else {
            &mut message.data
        };

        // Navigate and set the value
        let mut current = target;
        for (i, part) in parts[1..].iter().enumerate() {
            if i == parts.len() - 2 {
                // Last part, set the value
                if current.is_null() {
                    *current = json!({});
                }
                if let Value::Object(map) = current {
                    map.insert(part.to_string(), value.clone());
                }
                break;
            } else {
                // Navigate deeper
                if current.is_null() {
                    *current = json!({});
                }
                if let Value::Object(map) = current {
                    if !map.contains_key(*part) {
                        map.insert(part.to_string(), json!({}));
                    }
                    current = map.get_mut(*part).unwrap();
                }
            }
        }

        Ok(())
    }
}

/// Custom function that enriches data with external information
pub struct DataEnrichmentFunction {
    enrichment_data: HashMap<String, Value>,
}

impl FunctionHandler for DataEnrichmentFunction {
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
                    "Invalid configuration type for enrichment function".to_string(),
                ));
            }
        };

        // Extract lookup key and field
        let lookup_field = input
            .get("lookup_field")
            .and_then(Value::as_str)
            .ok_or_else(|| DataflowError::Validation("Missing lookup_field".to_string()))?;

        let lookup_value = input
            .get("lookup_value")
            .and_then(Value::as_str)
            .ok_or_else(|| DataflowError::Validation("Missing lookup_value".to_string()))?;

        let output_path = input
            .get("output_path")
            .and_then(Value::as_str)
            .unwrap_or("data.enrichment");

        // Simulate operation (e.g., database lookup, API call)
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Look up enrichment data
        let enrichment = if let Some(data) = self.enrichment_data.get(lookup_value) {
            data.clone()
        } else {
            json!({
                "status": "not_found",
                "message": format!("No enrichment data found for {}: {}", lookup_field, lookup_value)
            })
        };

        // Store enrichment data
        self.set_value_at_path(message, output_path, enrichment.clone())?;

        Ok((
            200,
            vec![Change {
                path: output_path.to_string(),
                old_value: Value::Null,
                new_value: enrichment,
            }],
        ))
    }
}

impl Default for DataEnrichmentFunction {
    fn default() -> Self {
        Self::new()
    }
}

impl DataEnrichmentFunction {
    pub fn new() -> Self {
        let mut enrichment_data = HashMap::new();

        // Sample enrichment data
        enrichment_data.insert(
            "user_123".to_string(),
            json!({
                "department": "Engineering",
                "location": "San Francisco",
                "manager": "Alice Johnson",
                "start_date": "2022-01-15",
                "security_clearance": "Level 2"
            }),
        );

        enrichment_data.insert(
            "user_456".to_string(),
            json!({
                "department": "Marketing",
                "location": "New York",
                "manager": "Bob Smith",
                "start_date": "2021-06-01",
                "security_clearance": "Level 1"
            }),
        );

        Self { enrichment_data }
    }

    fn set_value_at_path(&self, message: &mut Message, path: &str, value: Value) -> Result<()> {
        let parts: Vec<&str> = path.split('.').collect();
        let target = if parts[0] == "data" {
            &mut message.data
        } else if parts[0] == "temp_data" {
            &mut message.temp_data
        } else if parts[0] == "metadata" {
            &mut message.metadata
        } else {
            &mut message.data
        };

        let mut current = target;
        for (i, part) in parts[1..].iter().enumerate() {
            if i == parts.len() - 2 {
                if current.is_null() {
                    *current = json!({});
                }
                if let Value::Object(map) = current {
                    map.insert(part.to_string(), value.clone());
                }
                break;
            } else {
                if current.is_null() {
                    *current = json!({});
                }
                if let Value::Object(map) = current {
                    if !map.contains_key(*part) {
                        map.insert(part.to_string(), json!({}));
                    }
                    current = map.get_mut(*part).unwrap();
                }
            }
        }
        Ok(())
    }
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("=== Custom Function Example ===\n");

    // Define a workflow that uses our custom functions
    let workflow_json = r#"
    {
        "id": "custom_function_demo",
        "name": "Custom Function Demo",
        "description": "Demonstrates custom functions in workflow",
        "tasks": [
            {
                "id": "prepare_data",
                "name": "Prepare Data",
                "description": "Extract and prepare data for analysis",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.numbers",
                                "logic": { "var": "temp_data.measurements" }
                            },
                            {
                                "path": "data.user_id",
                                "logic": { "var": "temp_data.user_id" }
                            }
                        ]
                    }
                }
            },
            {
                "id": "calculate_stats",
                "name": "Calculate Statistics",
                "description": "Calculate statistical measures from numeric data",
                "function": {
                    "name": "statistics",
                    "input": {
                        "data_path": "data.numbers",
                        "output_path": "data.stats"
                    }
                }
            },
            {
                "id": "enrich_user_data",
                "name": "Enrich User Data",
                "description": "Add additional user information",
                "function": {
                    "name": "enrich_data",
                    "input": {
                        "lookup_field": "user_id",
                        "lookup_value": "user_123",
                        "output_path": "data.user_info"
                    }
                }
            }
        ]
    }
    "#;

    // Parse the first workflow
    let workflow = Workflow::from_json(workflow_json)?;

    // Demonstrate another example with different data
    let separator = "=".repeat(50);
    println!("\n{separator}");
    println!("=== Second Example with Different User ===\n");

    let mut message2 = dataflow_rs::engine::message::Message::new(&json!({}));
    message2.temp_data = json!({
        "measurements": [5.1, 7.3, 9.8, 6.2, 8.5],
        "user_id": "user_456",
        "timestamp": "2024-01-15T11:00:00Z"
    });
    message2.data = json!({});

    // Create a workflow for the second user
    let workflow2_json = r#"
    {
        "id": "custom_function_demo_2",
        "name": "Custom Function Demo 2",
        "description": "Second demo with different user",
        "tasks": [
            {
                "id": "prepare_data",
                "name": "Prepare Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            {
                                "path": "data.numbers",
                                "logic": { "var": "temp_data.measurements" }
                            },
                            {
                                "path": "data.user_id",
                                "logic": { "var": "temp_data.user_id" }
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
                        "data_path": "data.numbers",
                        "output_path": "data.analysis"
                    }
                }
            },
            {
                "id": "enrich_user_data",
                "name": "Enrich User Data",
                "function": {
                    "name": "enrich_data",
                    "input": {
                        "lookup_field": "user_id",
                        "lookup_value": "user_456",
                        "output_path": "data.employee_details"
                    }
                }
            }
        ]
    }
    "#;

    let workflow2 = Workflow::from_json(workflow2_json)?;

    // Prepare custom functions
    let mut custom_functions = HashMap::new();
    custom_functions.insert(
        "statistics".to_string(),
        Box::new(StatisticsFunction::new()) as Box<dyn FunctionHandler + Send + Sync>,
    );
    custom_functions.insert(
        "enrich_data".to_string(),
        Box::new(DataEnrichmentFunction::new()) as Box<dyn FunctionHandler + Send + Sync>,
    );
    // Note: map and validate are now built-in to the Engine and will be used automatically

    // Create engine with custom functions and built-ins (map/validate are always included internally)
    let engine = Engine::new(
        vec![workflow, workflow2],
        Some(custom_functions),
        None, // Use default (includes built-ins)
    );

    // Create sample data for first message
    let sample_data = json!({
        "measurements": [10.5, 15.2, 8.7, 22.1, 18.9, 12.3, 25.6, 14.8, 19.4, 16.7],
        "user_id": "user_123",
        "timestamp": "2024-01-15T10:30:00Z"
    });

    // Create and process first message
    let mut message = dataflow_rs::engine::message::Message::new(&json!({}));
    message.temp_data = sample_data;
    message.data = json!({});

    println!("Processing message with custom functions...\n");

    // Process the message through our custom workflow
    match engine.process_message(&mut message) {
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
                    audit.status_code
                );
                println!("   Timestamp: {}", audit.timestamp);
                println!("   Changes: {} field(s) modified", audit.changes.len());
            }

            if message.has_errors() {
                println!("\n⚠️  Errors encountered:");
                for error in &message.errors {
                    println!(
                        "   - {}: {:?}",
                        error.task_id.as_ref().unwrap_or(&"unknown".to_string()),
                        error.error_message
                    );
                }
            }
        }
        Err(e) => {
            println!("❌ Error processing message: {e:?}");
        }
    }

    // Process second message
    match engine.process_message(&mut message2) {
        Ok(_) => {
            println!("✅ Second message processed successfully!\n");
            println!("📊 Results for user_456:");
            println!("{}", serde_json::to_string_pretty(&message2.data)?);
        }
        Err(e) => {
            println!("❌ Error processing second message: {e:?}");
        }
    }

    println!("\n🎉 Custom function examples completed!");

    Ok(())
}
