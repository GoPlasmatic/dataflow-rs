//! # Custom Function Example
//!
//! Two host functions on the `Function` trait: `statistics`, which
//! summarises an array of numbers, and `enrich_data`, which looks a user up
//! (an awaited call, simulated). Each declares its input as a struct with
//! `#[derive(FunctionConfig)]`: the engine compiles a task's `input` into
//! it once, at build, so a misshapen config fails `Engine::build` with an
//! issue at its path instead of failing on the first message. A function
//! reads its fields through `Ctx::eval` and writes through its `Output`,
//! which records the audit-trail change for it.
//!
//! It prints the data after the run and the audit trail, then the issues a
//! misspelt config gets at build.
//!
//! Run with: `cargo run --example custom_function`

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::Output;
use dataflow_rs::function::{FunctionConfig, FunctionSpec};
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Result};
use serde_json::{Value, json};
use std::time::Duration;

/// `statistics`' input: the numbers to summarise and where the summary
/// goes. A field's doc comment is its description, for tooling.
#[derive(FunctionConfig)]
struct StatisticsConfig {
    /// The array of numbers, usually `{"var": "data.<path>"}`.
    numbers: Expr,
    /// Where the summary object is written.
    #[output(default = "data.statistics")]
    output: Output,
}

/// Calculates statistics from numeric data.
struct Statistics;

impl Function for Statistics {
    type Config = StatisticsConfig;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default()
            .with_description("Summarises an array of numbers")
            .with_category("math")
    }

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &StatisticsConfig) -> Result<Outcome> {
        // `eval` runs the expression against the context as this call sees
        // it; a literal in the config was folded at build and costs nothing.
        let numbers = ctx.eval(&cfg.numbers)?;
        let numbers: Vec<f64> = numbers
            .as_array()
            .map(|items| items.iter().filter_map(OwnedDataValue::as_f64).collect())
            .unwrap_or_default();

        if numbers.is_empty() {
            // Nothing to summarise: the audit entry records a 204, and the
            // workflow goes on.
            return Ok(Outcome::Status(204));
        }

        // The write goes through the config's `Output`, so the destination
        // is the task's to choose, and the `Change` is recorded for it.
        ctx.emit(&cfg.output, OwnedDataValue::from(&compute(&numbers)))?;
        Ok(Outcome::Ok)
    }
}

fn compute(numbers: &[f64]) -> Value {
    let count = numbers.len() as f64;
    let sum: f64 = numbers.iter().sum();
    let mean = sum / count;

    let mut sorted = numbers.to_vec();
    sorted.sort_by(f64::total_cmp);

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

/// `enrich_data`'s input.
#[derive(FunctionConfig)]
struct EnrichConfig {
    /// The id of the user to look up.
    user_id: Expr<String>,
    /// Where the profile is written.
    #[output(default = "data.enriched")]
    output: Output,
}

/// Enriches the data with information from an external system.
struct EnrichData;

impl Function for EnrichData {
    type Config = EnrichConfig;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default()
            .with_description("Looks a user profile up")
            .with_category("io")
    }

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &EnrichConfig) -> Result<Outcome> {
        // An `Expr<String>` evaluates to text: a string's contents, or any
        // other value's JSON.
        let user_id = ctx.eval(&cfg.user_id)?;

        // A real lookup awaits its answer here; `call` is an ordinary
        // `async fn`, so it may.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let enriched = json!({
            "user_profile": {
                "id": &*user_id,
                "name": format!("User {user_id}"),
                "email": format!("{user_id}@example.com"),
                "created_at": "2024-01-15T10:30:00Z",
                "preferences": { "theme": "dark", "notifications": true }
            },
            "enrichment_timestamp": chrono::Utc::now().to_rfc3339(),
        });

        // `emit_default` writes through the config's only `Output`.
        ctx.emit_default(OwnedDataValue::from(&enriched))?;
        Ok(Outcome::Ok)
    }
}

fn workflow() -> Value {
    json!({
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
                    "input": { "numbers": { "var": "data.numbers" }, "output": "data.statistics" }
                }
            },
            {
                "id": "enrich_user_data",
                "name": "Enrich User Data",
                "function": {
                    "name": "enrich_data",
                    "input": { "user_id": { "var": "data.user_id" } }
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
    })
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("Custom Function Example");
    println!("==========================\n");

    // The builder compiles each task's `input` into its function's `Config`
    // at `build`: a config-shape error fails here, not on the first message.
    let engine = Engine::builder()
        .workflow_json(&workflow())
        .register("statistics", Statistics)
        .register("enrich_data", EnrichData)
        .build()
        .map_err(|issues| {
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })?;

    let mut message = Message::from_json(&json!({
        "measurements": [10.5, 15.2, 8.7, 22.1, 18.9, 12.3, 25.6, 14.8, 19.4, 16.7],
        "user_id": "user_123",
        "timestamp": "2024-01-15T10:30:00Z"
    }));

    println!("Processing message with custom functions...\n");

    // `into_result` is `Err` only when a task's failure stopped the run.
    engine.run(&mut message).await.into_result()?;
    println!("Message processed successfully!\n");

    println!("Final Results:");
    let data = Value::from(message.context().data());
    println!("{}\n", serde_json::to_string_pretty(&data)?);

    println!("Audit Trail:");
    for (i, entry) in message.audit().iter().enumerate() {
        println!(
            "{}. Task: {} (Status: {})",
            i + 1,
            entry.task_id,
            entry.status
        );
        println!("   Timestamp: {}", entry.timestamp);
        println!("   Changes: {} field(s) modified", entry.changes.len());
    }

    if !message.errors().is_empty() {
        println!("\nErrors encountered:");
        for error in message.errors() {
            println!(
                "   - {}: {}",
                error.task_id.as_deref().unwrap_or("unknown"),
                error.message
            );
        }
    }

    // The point of a typed config: a key the config does not know is
    // reported at its path, with the near miss it probably meant, and the
    // field it left out is reported as missing. Nothing runs.
    println!("\nA misspelt config fails at build, not on the first message:");
    let misspelt = json!({
        "id": "typo",
        "tasks": [{
            "id": "calculate_stats",
            "function": {
                "name": "statistics",
                "input": { "numbres": { "var": "data.numbers" } }
            }
        }]
    });
    match Engine::builder()
        .workflow_json(&misspelt)
        .register("statistics", Statistics)
        .build()
    {
        Ok(_) => println!("   (the definition built, which it should not have)"),
        Err(issues) => {
            for issue in &issues {
                println!("   {} {issue}", issue.severity);
            }
        }
    }

    println!("\nCustom function example completed!");

    Ok(())
}
