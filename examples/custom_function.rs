//! # Custom Function Example
//!
//! Demonstrates the v3 typed-handler API: each `AsyncFunctionHandler`
//! declares its `Input` shape with serde, the engine deserializes the
//! `FunctionConfig::Custom { input }` JSON exactly once at `Engine::new()`
//! time, and the handler reads/writes through `TaskContext` (which records
//! audit-trail changes automatically). No more `match FunctionConfig::Custom
//! { input, .. } => ... | _ => Err(...)` boilerplate.
//!
//! Run with: `cargo run --example custom_function`

use async_trait::async_trait;
use dataflow_rs::{
    AsyncFunctionHandler, BoxedFunctionHandler, Engine, Result, TaskContext, TaskOutcome, Workflow,
};
use datavalue::OwnedDataValue;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Typed input for the statistics function: which `data` array to summarize
/// and where to write the resulting object. Misshapen JSON config now fails
/// at engine construction, not on first message.
#[derive(Debug, Deserialize)]
pub struct StatisticsInput {
    /// Dot-path inside `data` to the array of numbers.
    #[serde(default = "default_data_path")]
    data_path: String,
    /// Dot-path inside `data` for the result object.
    #[serde(default = "default_output_path")]
    output_path: String,
}

fn default_data_path() -> String {
    "numbers".to_string()
}
fn default_output_path() -> String {
    "statistics".to_string()
}

/// Custom async function that calculates statistics from numeric data.
pub struct StatisticsFunction;

#[async_trait]
impl AsyncFunctionHandler for StatisticsFunction {
    type Input = StatisticsInput;

    async fn execute(
        &self,
        ctx: &mut TaskContext<'_>,
        input: &StatisticsInput,
    ) -> Result<TaskOutcome> {
        let numbers: Vec<f64> = match ctx.data().get(input.data_path.as_str()) {
            Some(v) => v
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
                .unwrap_or_default(),
            None => Vec::new(),
        };

        if numbers.is_empty() {
            return Ok(TaskOutcome::Status(204));
        }

        let stats = compute(&numbers);
        ctx.set(
            &format!("data.{}", input.output_path),
            OwnedDataValue::from(&stats),
        );
        Ok(TaskOutcome::Success)
    }
}

fn compute(numbers: &[f64]) -> Value {
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

    json!({
        "count": count,
        "sum": sum,
        "mean": mean,
        "median": median,
        "min": sorted[0],
        "max": sorted[sorted.len() - 1],
        "std_dev": variance.sqrt(),
        "variance": variance,
    })
}

/// Typed input for the async enrichment function.
#[derive(Debug, Deserialize)]
pub struct EnrichInput {
    /// Field on `data` whose string value identifies the user to enrich.
    #[serde(default = "default_user_id_path")]
    user_id_path: String,
}

fn default_user_id_path() -> String {
    "user_id".to_string()
}

/// Custom async function that enriches data with external information.
pub struct AsyncDataEnrichmentFunction;

#[async_trait]
impl AsyncFunctionHandler for AsyncDataEnrichmentFunction {
    type Input = EnrichInput;

    async fn execute(&self, ctx: &mut TaskContext<'_>, input: &EnrichInput) -> Result<TaskOutcome> {
        let user_id = ctx
            .data()
            .get(input.user_id_path.as_str())
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Simulate async API call
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let enriched = json!({
            "user_profile": {
                "id": user_id,
                "name": format!("User {}", user_id),
                "email": format!("{}@example.com", user_id),
                "created_at": "2024-01-15T10:30:00Z",
                "preferences": { "theme": "dark", "notifications": true }
            },
            "enrichment_timestamp": chrono::Utc::now().to_rfc3339(),
        });

        ctx.set("data.enriched", OwnedDataValue::from(&enriched));
        Ok(TaskOutcome::Success)
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("Custom Function Example");
    println!("==========================\n");

    let workflow_json = r#"
    {
        "id": "statistics_workflow",
        "name": "Data Processing Workflow",
        "tasks": [
            {
                "id": "load_payload",
                "name": "Load Payload",
                "function": {
                    "name": "parse_json",
                    "input": { "source": "payload", "target": "input" }
                }
            },
            {
                "id": "prepare_data",
                "name": "Prepare Data",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.numbers", "logic": { "var": "data.input.measurements" } },
                            { "path": "data.user_id", "logic": { "var": "data.input.user_id" } }
                        ]
                    }
                }
            },
            {
                "id": "calculate_stats",
                "name": "Calculate Statistics",
                "function": {
                    "name": "statistics",
                    "input": { "data_path": "numbers", "output_path": "statistics" }
                }
            },
            {
                "id": "enrich_user_data",
                "name": "Enrich User Data",
                "function": {
                    "name": "enrich_data",
                    "input": { "user_id_path": "user_id" }
                }
            },
            {
                "id": "validate_results",
                "name": "Validate Results",
                "function": {
                    "name": "validation",
                    "input": {
                        "rules": [
                            { "logic": { ">": [{ "var": "data.statistics.count" }, 0] },
                              "message": "Statistics must have at least one data point" },
                            { "logic": { "!!": { "var": "data.enriched.user_profile" } },
                              "message": "User profile enrichment is required" }
                        ]
                    }
                }
            }
        ]
    }
    "#;

    let workflow = Workflow::from_json(workflow_json)?;

    // Register typed handlers. The HashMap holds `BoxedFunctionHandler` (=
    // `Box<dyn DynAsyncFunctionHandler + Send + Sync>`); `Box::new(MyHandler)`
    // auto-coerces via the engine's blanket impl.
    let mut custom_functions: HashMap<String, BoxedFunctionHandler> = HashMap::new();
    custom_functions.insert("statistics".to_string(), Box::new(StatisticsFunction));
    custom_functions.insert(
        "enrich_data".to_string(),
        Box::new(AsyncDataEnrichmentFunction),
    );

    // Engine::new pre-parses each Custom task's `input` JSON into the
    // matching handler's typed `Input` — config-shape errors fail here.
    let engine = Engine::new(vec![workflow], Some(custom_functions))?;

    let sample_data = json!({
        "measurements": [10.5, 15.2, 8.7, 22.1, 18.9, 12.3, 25.6, 14.8, 19.4, 16.7],
        "user_id": "user_123",
        "timestamp": "2024-01-15T10:30:00Z"
    });

    let mut message = dataflow_rs::Message::from_value(&sample_data);

    println!("Processing message with custom functions...\n");

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
