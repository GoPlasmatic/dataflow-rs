//! # Rules Engine Example
//!
//! This example demonstrates IFTTT-style rules engine patterns using dataflow-rs.
//!
//! It shows three rules with priority ordering:
//! 1. IF order total >= 1000 THEN apply premium discount (10%)
//! 2. IF order total >= 500 THEN apply standard discount (5%)
//! 3. IF user is VIP THEN add priority flag and bonus points
//!
//! Run with: `cargo run --example rules_engine`

use dataflow_rs::engine::message::Message;
use dataflow_rs::{Rule, RulesEngine};
use serde_json::json;
use std::sync::Arc;

/// Helper to create a message with data already in the data context.
/// In production, you'd typically use a `parse_json` task as the first action
/// to move payload into the data context.
fn message_with_data(data: serde_json::Value) -> Message {
    let mut message = Message::new(Arc::new(json!({})));
    message.context["data"] = data;
    message.invalidate_context_cache();
    message
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    println!("Rules Engine Example (IFTTT-style)");
    println!("===================================\n");

    // Rule 1: IF order total >= 1000 THEN apply premium discount
    let premium_discount_rule = Rule::from_json(
        r#"
    {
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
                            {
                                "path": "data.order.discount_pct",
                                "logic": 10
                            },
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
                            {
                                "path": "data.order.tier",
                                "logic": "premium"
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#,
    )?;

    // Rule 2: IF order total >= 500 (and no premium discount applied) THEN apply standard discount
    let standard_discount_rule = Rule::from_json(
        r#"
    {
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
                            {
                                "path": "data.order.discount_pct",
                                "logic": 5
                            },
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
                            {
                                "path": "data.order.tier",
                                "logic": "standard"
                            }
                        ]
                    }
                }
            }
        ]
    }
    "#,
    )?;

    // Rule 3: IF user is VIP THEN add priority processing and bonus points
    let vip_rule = Rule::from_json(
        r#"
    {
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
                            {
                                "path": "data.order.priority",
                                "logic": "high"
                            },
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
    }
    "#,
    )?;

    // Create the rules engine with all rules
    let engine = RulesEngine::new(
        vec![premium_discount_rule, standard_discount_rule, vip_rule],
        None,
    );

    // --- Scenario 1: VIP customer with a large order ---
    println!("Scenario 1: VIP customer, order total = $1500");
    println!("------------------------------------------------");

    let mut message = message_with_data(json!({
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

    engine.process_message(&mut message).await?;

    let order = &message.context["data"]["order"];
    println!("  Tier:            {}", order["tier"]);
    println!("  Discount:        {}%", order["discount_pct"]);
    println!("  Discount Amount: ${}", order["discount_amount"]);
    println!("  Final Total:     ${}", order["final_total"]);
    println!("  Priority:        {}", order["priority"]);
    println!("  Bonus Points:    {}", order["bonus_points"]);

    // --- Scenario 2: Regular customer with a mid-range order ---
    println!("\nScenario 2: Regular customer, order total = $750");
    println!("------------------------------------------------");

    let mut message = message_with_data(json!({
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

    engine.process_message(&mut message).await?;

    let order = &message.context["data"]["order"];
    println!("  Tier:            {}", order["tier"]);
    println!("  Discount:        {}%", order["discount_pct"]);
    println!("  Discount Amount: ${}", order["discount_amount"]);
    println!("  Final Total:     ${}", order["final_total"]);

    // --- Scenario 3: Small order, no discount rules match ---
    println!("\nScenario 3: Regular customer, order total = $100");
    println!("------------------------------------------------");

    let mut message = message_with_data(json!({
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

    engine.process_message(&mut message).await?;

    let order = &message.context["data"]["order"];
    let has_discount = order.get("tier").is_some_and(|v| !v.is_null());
    if has_discount {
        println!("  Tier:        {}", order["tier"]);
    } else {
        println!("  No rules matched — order processed as-is");
        println!("  Order total: ${}", order["total"]);
    }

    println!("\nRules engine example completed!");

    Ok(())
}
