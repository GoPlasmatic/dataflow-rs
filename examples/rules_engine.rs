//! # Rules Engine Example
//!
//! IFTTT-style rules on dataflow-rs. Each workflow is one rule: its
//! `condition` is the IF, its tasks are the THEN, and the rules run in
//! `priority` order, so a later rule can read what an earlier one wrote.
//!
//! 1. IF the order total is at least 1000 THEN apply the premium discount (10%)
//! 2. IF the order total is at least 500 and no tier is set yet THEN apply
//!    the standard discount (5%)
//! 3. IF the user is a VIP THEN add a priority flag and bonus points
//!
//! Three orders go through: a VIP's $1500 order, a regular $750 order and a
//! regular $100 order. For each, the rules that matched and the order's
//! fields are printed; the last matches no rule at all.
//!
//! Run with: `cargo run --example rules_engine`

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::run::Selection;
use dataflow_rs::{Engine, Message, Run};
use serde_json::{Value, json};

/// A message with `data` already filled in. In production the first task is
/// usually a `parse_json` that moves the payload into `data`.
fn message_with_data(data: &Value) -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(data),
        OwnedDataValue::Null,
    )
}

/// Rule 1: IF order total >= 1000 THEN apply the premium discount.
fn premium_discount_rule() -> Value {
    json!({
        "id": "premium_discount",
        "name": "Premium Order Discount",
        "priority": 0,
        "condition": {">=": [{"var": "data.order.total"}, 1000]},
        "tasks": [
            {
                "id": "apply_premium_discount",
                "name": "Apply 10% Premium Discount",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.order.discount_pct", "logic": 10 },
                            {
                                "path": "data.order.discount_amount",
                                "logic": {"*": [{"var": "data.order.total"}, 0.1]}
                            },
                            {
                                "path": "data.order.final_total",
                                "logic": {"-": [
                                    {"var": "data.order.total"},
                                    {"*": [{"var": "data.order.total"}, 0.1]}
                                ]}
                            },
                            { "path": "data.order.tier", "logic": "premium" }
                        ]
                    }
                }
            }
        ]
    })
}

/// Rule 2: IF order total >= 500 and no discount was applied yet THEN apply
/// the standard discount. The second clause reads what rule 1 may have
/// written, which is why the priorities matter.
fn standard_discount_rule() -> Value {
    json!({
        "id": "standard_discount",
        "name": "Standard Order Discount",
        "priority": 1,
        "condition": {"and": [
            {">=": [{"var": "data.order.total"}, 500]},
            {"!": {"var": "data.order.tier"}}
        ]},
        "tasks": [
            {
                "id": "apply_standard_discount",
                "name": "Apply 5% Standard Discount",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.order.discount_pct", "logic": 5 },
                            {
                                "path": "data.order.discount_amount",
                                "logic": {"*": [{"var": "data.order.total"}, 0.05]}
                            },
                            {
                                "path": "data.order.final_total",
                                "logic": {"-": [
                                    {"var": "data.order.total"},
                                    {"*": [{"var": "data.order.total"}, 0.05]}
                                ]}
                            },
                            { "path": "data.order.tier", "logic": "standard" }
                        ]
                    }
                }
            }
        ]
    })
}

/// Rule 3: IF the user is a VIP THEN add priority processing and bonus
/// points, and check that a discount rule computed the final total first.
fn vip_rule() -> Value {
    json!({
        "id": "vip_processing",
        "name": "VIP Customer Processing",
        "priority": 2,
        "condition": {"==": [{"var": "data.user.is_vip"}, true]},
        "tasks": [
            {
                "id": "add_priority",
                "name": "Add Priority Flag",
                "function": {
                    "name": "map",
                    "input": {
                        "mappings": [
                            { "path": "data.order.priority", "logic": "high" },
                            {
                                "path": "data.order.bonus_points",
                                "logic": {"*": [{"var": "data.order.total"}, 2]}
                            }
                        ]
                    }
                }
            },
            {
                "id": "validate_vip",
                "name": "Validate VIP Order",
                "function": {
                    "name": "validation",
                    "input": {
                        "rules": [
                            {
                                "logic": {"!!": {"var": "data.order.final_total"}},
                                "message": "VIP orders must have a calculated final total"
                            }
                        ]
                    }
                }
            }
        ]
    })
}

/// Which rules the run selected: the `Run` keeps one record per workflow it
/// considered, with why it did or did not run.
fn print_selection(run: &Run) {
    for record in run.workflows() {
        let how = match record.selection {
            Selection::Ran => "ran".to_owned(),
            Selection::SkippedByCondition => "skipped, its condition was false".to_owned(),
            Selection::ConditionFailed(e) => format!("skipped, its condition failed: {e}"),
            Selection::SkippedByRollout => "skipped, outside its rollout".to_owned(),
            _ => "not selected".to_owned(),
        };
        println!("  rule {:<18} {how}", record.id);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("Rules Engine Example (IFTTT-style)");
    println!("===================================\n");

    // One engine holds every rule; `build` compiles each rule's condition
    // and mappings once, and fails with the issues if any rule is malformed.
    let engine = Engine::builder()
        .workflow_json(&premium_discount_rule())
        .workflow_json(&standard_discount_rule())
        .workflow_json(&vip_rule())
        .build()
        .map_err(|issues| {
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })?;

    // --- Scenario 1: VIP customer with a large order ---
    println!("Scenario 1: VIP customer, order total = $1500");
    println!("------------------------------------------------");

    let mut message = message_with_data(&json!({
        "order": {
            "id": "ORD-001",
            "total": 1500,
            "items": ["laptop", "mouse", "keyboard"]
        },
        "user": {
            "id": "USR-100",
            "name": "Alice",
            "is_vip": true
        }
    }));

    let run = engine.run(&mut message).await.into_result()?;
    print_selection(&run);

    let data = Value::from(message.context().data());
    let order = &data["order"];
    println!("  Tier:            {}", order["tier"]);
    println!("  Discount:        {}%", order["discount_pct"]);
    println!("  Discount Amount: ${}", order["discount_amount"]);
    println!("  Final Total:     ${}", order["final_total"]);
    println!("  Priority:        {}", order["priority"]);
    println!("  Bonus Points:    {}", order["bonus_points"]);

    // --- Scenario 2: Regular customer with a mid-range order ---
    println!("\nScenario 2: Regular customer, order total = $750");
    println!("------------------------------------------------");

    let mut message = message_with_data(&json!({
        "order": {
            "id": "ORD-002",
            "total": 750,
            "items": ["headphones", "charger"]
        },
        "user": {
            "id": "USR-200",
            "name": "Bob",
            "is_vip": false
        }
    }));

    let run = engine.run(&mut message).await.into_result()?;
    print_selection(&run);

    let data = Value::from(message.context().data());
    let order = &data["order"];
    println!("  Tier:            {}", order["tier"]);
    println!("  Discount:        {}%", order["discount_pct"]);
    println!("  Discount Amount: ${}", order["discount_amount"]);
    println!("  Final Total:     ${}", order["final_total"]);

    // --- Scenario 3: Small order, no discount rules match ---
    println!("\nScenario 3: Regular customer, order total = $100");
    println!("------------------------------------------------");

    let mut message = message_with_data(&json!({
        "order": {
            "id": "ORD-003",
            "total": 100,
            "items": ["cable"]
        },
        "user": {
            "id": "USR-300",
            "name": "Charlie",
            "is_vip": false
        }
    }));

    let run = engine.run(&mut message).await.into_result()?;
    print_selection(&run);

    let data = Value::from(message.context().data());
    let order = &data["order"];
    if run.matched() > 0 {
        println!("  Tier:        {}", order["tier"]);
    } else {
        println!("  No rules matched, the order is processed as-is");
        println!("  Order total: ${}", order["total"]);
    }

    println!("\nRules engine example completed!");

    Ok(())
}
