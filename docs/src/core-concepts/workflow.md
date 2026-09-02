# Rules (Workflows)

A Rule (also called Workflow) is a collection of actions that execute sequentially when a condition is met. This is the core **IF → THEN** unit: **IF** condition matches, **THEN** execute actions.

## Overview

Rules provide:

- **Conditional Execution** - Only run when JSONLogic conditions are met (against full context: `data`, `metadata`, `temp_data`)
- **Priority Ordering** - Control execution order across rules
- **Action Organization** - Group related processing steps
- **Error Handling** - Continue or stop on errors

## Rule Structure

```json
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "priority": 1,
    "channel": "orders",
    "version": 2,
    "status": "active",
    "tags": ["premium", "high-priority"],
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "continue_on_error": false,
    "tasks": [
        {
            "id": "apply_discount",
            "name": "Apply Discount",
            "function": { ... }
        },
        {
            "id": "notify_manager",
            "name": "Notify Manager",
            "function": { ... }
        }
    ]
}
```

## Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | Yes | Unique rule identifier |
| `name` | string | Yes | Human-readable name |
| `description` | string | No | Free-text description |
| `priority` | number | No | Execution order (default: 0, lower = first) |
| `condition` | JSONLogic | No | When to execute rule (evaluated against full context) |
| `continue_on_error` | boolean | No | Continue on action failure (default: false) |
| `tasks` | array | Yes | Steps to execute — an action, or a group of actions sharing one condition (see [Control Flow](../advanced/control-flow.md)) |
| `channel` | string | No | Channel for message routing (default: `"default"`) |
| `version` | number | No | Workflow version number (default: `1`) |
| `status` | string | No | Lifecycle status: `active`, `paused`, or `archived` (default: `active`) |
| `tags` | array | No | Arbitrary tags for organization (default: `[]`) |
| `rollout` | object | No | Traffic split — `{bucket_start, bucket_end}` over `0..100` (default: none) |
| `loop` | object | No | Run the task list as a bounded loop — see [Loops](../advanced/loops.md) (default: none) |
| `created_at` | datetime | No | Creation timestamp (ISO 8601) |
| `updated_at` | datetime | No | Last update timestamp (ISO 8601) |

## Creating Rules

### From JSON String

```rust
# fn _demo() -> dataflow_rs::Result<()> {
use dataflow_rs::Workflow;

let rule = Workflow::from_json(r#"{
    "id": "my_rule",
    "name": "My Rule",
    "tasks": [...]
}"#)?;
# Ok(()) }
```

### Using the Convenience Constructor

```rust
use dataflow_rs::{Rule, Task};
use serde_json::json;

let rule = Rule::rule(
    "premium_discount",
    "Premium Discount",
    json!({">=": [{"var": "data.order.total"}, 1000]}),
    vec![/* actions */],
);
```

`Workflow` is `#[non_exhaustive]` as of 3.7.0, so struct-literal construction no
longer compiles: build with `Workflow::new()`, `Workflow::rule()` or
`Workflow::from_json()` and assign the optional fields afterwards. See
[Creating Actions Programmatically](./task.md#creating-actions-programmatically)
for the reasoning and the migration shape.

### From File

```rust
# use dataflow_rs::Workflow;
# fn _demo() -> dataflow_rs::Result<()> {
let rule = Workflow::from_file("rules/my_rule.json")?;
# Ok(()) }
```

## Priority Ordering

Rules execute in priority order (lowest first). This enables the **THAT** (chaining) in the IF → THEN → THAT model:

```json
// Executes first (priority 1) — validate input
{
    "id": "validation",
    "priority": 1,
    "tasks": [...]
}

// Executes second (priority 2) — transform data
{
    "id": "transformation",
    "priority": 2,
    "tasks": [...]
}

// Executes last (priority 10) — send notifications
{
    "id": "notification",
    "priority": 10,
    "tasks": [...]
}
```

## Conditional Execution

Use JSONLogic conditions to control when rules run. Conditions evaluate against the **full message context** — `data`, `metadata`, and `temp_data` — and may also read `{"secret": "name"}` from the engine's [secret store](../advanced/secrets.md), which is never part of the message:

```json
{
    "id": "premium_user_rule",
    "condition": {
        "and": [
            {">=": [{"var": "data.order.total"}, 500]},
            {"==": [{"var": "data.user.is_vip"}, true]}
        ]
    },
    "tasks": [...]
}
```

### Common Condition Patterns

```json
// Match on data fields
{">=": [{"var": "data.order.total"}, 1000]}

// Check data exists
{"!!": {"var": "data.email"}}

// Multiple conditions
{"and": [
    {">=": [{"var": "data.amount"}, 100]},
    {"==": [{"var": "data.currency"}, "USD"]}
]}

// Either condition
{"or": [
    {"==": [{"var": "metadata.source"}, "api"]},
    {"==": [{"var": "metadata.source"}, "webhook"]}
]}
```

## Error Handling

### Stop on Error (Default)

```json
{
    "id": "strict_rule",
    "continue_on_error": false,
    "tasks": [...]
}
```

If any action fails, the rule stops and the error is recorded.

### Continue on Error

```json
{
    "id": "resilient_rule",
    "continue_on_error": true,
    "tasks": [...]
}
```

A rule's `continue_on_error` governs what happens **after this rule fails** —
subsequent rules still run, and `process_message` returns `Ok` rather than
`Err`. It is *not* a default inherited by the rule's actions: whether the rule
keeps going past a failing action is decided by that action's own
`continue_on_error`. See
[Error Handling](./error-handling.md#rule-level-error-handling) for the full
matrix.

Errors are collected in `message.errors()` either way.

## Action Dependencies

Actions within a rule execute sequentially, allowing later actions to depend on earlier results:

```json
{
    "id": "pipeline",
    "name": "Pipeline",
    "tasks": [
        {
            "id": "fetch_data",
            "name": "Fetch data",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "temp_data.fetched", "logic": {"var": "data.source"}}
                    ]
                }
            }
        },
        {
            "id": "process_data",
            "name": "Process data",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.result", "logic": {"var": "temp_data.fetched"}}
                    ]
                }
            }
        }
    ]
}
```

## Workflow Lifecycle

Workflows support lifecycle management with status, versioning, and tagging. All lifecycle fields are optional and backward-compatible.

### Status

Control whether a workflow is active using the `status` field:

```json
{"id": "my_rule", "status": "active", "tasks": [...]}
{"id": "old_rule", "status": "paused", "tasks": [...]}
{"id": "legacy_rule", "status": "archived", "tasks": [...]}
```

- **`active`** (default) — the workflow executes normally and is included in channel routing
- **`paused`** — the workflow is excluded from channel routing but still runs via `process_message()`
- **`archived`** — same as paused; used to indicate permanently retired workflows

### Channel Routing

Group workflows by channel for efficient message routing:

```json
[
    {"id": "order_validate", "channel": "orders", "priority": 1, "tasks": [...]},
    {"id": "order_process", "channel": "orders", "priority": 2, "tasks": [...]},
    {"id": "user_notify", "channel": "notifications", "priority": 1, "tasks": [...]}
]
```

Then route messages to a specific channel:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message)
#     -> dataflow_rs::Result<()> {
// Only runs workflows on the "orders" channel
engine.process_message_for_channel("orders", &mut message).await?;
# Ok(()) }
```

### Version and Tags

Use `version` and `tags` for workflow organization:

```json
{
    "id": "discount_rule",
    "version": 3,
    "tags": ["finance", "discount", "v3"],
    "tasks": [...]
}
```

### Traffic Splits (Rollout)

Give a workflow a slice of its channel's traffic with a half-open bucket range
over `0..100`:

```json
{
    "id": "checkout_v2",
    "channel": "checkout",
    "rollout": { "bucket_start": 0, "bucket_end": 10 },
    "tasks": [...]
}
```

That workflow serves buckets `0..=9` — 10% of traffic. Pair it with a
`{"bucket_start": 10, "bucket_end": 100}` sibling to run the old version for the
rest. `bucket_start` is inclusive and `bucket_end` exclusive, so the two ranges
partition `0..=99` exactly with no overlap and no gap. An empty or inverted range
(`bucket_end <= bucket_start`) serves nothing.

The engine does **not** derive the bucket — set it on the message:

```rust
# use dataflow_rs::Message;
# fn _demo() {
let message = Message::builder().routing_bucket(7).build();
assert_eq!(message.routing_bucket(), Some(7));
# }
```

How you map a request to a bucket is entirely your policy: a sticky hash of some
request identity (so a given user always sees the same version), a per-message
random draw, round-robin. That deliberately stays outside this crate.

Two rules worth knowing:

- **A message with no bucket is admitted by every workflow**, split or not. Every
  message built without `routing_bucket` behaves exactly as it did before rollouts
  existed, and the WASM entry points — which have no way to set one — keep working
  on any workflow JSON. The trade-off is that setting `rollout` and forgetting the
  bucket runs *every* version on the same message, so set both together.
- **An excluded workflow is skipped exactly like a false condition**: no audit
  entry, `metadata.progress` untouched, and one workflow-level `Skipped` step in a
  trace. The gate runs before any arena work, so exclusion is cheap.

Values `>= 100` passed to `routing_bucket` are clamped to `99`, keeping the
builder infallible.

### Building and checking a split

A single `rollout` is only half the picture. What makes a deployment correct is
a property of the whole *set* — the versions of one logical workflow must
partition `0..100` exactly. Both ways of getting that wrong are silent in
production: a **gap** blackholes a slice of traffic, and an **overlap** makes
which version answers depend on workflow ordering rather than on the rollout.

`Rollout::partition` turns percentages into contiguous ranges, in traffic order:

```rust
use dataflow_rs::{Rollout, RolloutError};

let split = Rollout::partition(&[90, 10]).unwrap();
assert_eq!(split[0], Rollout { bucket_start: 0, bucket_end: 90 });
assert_eq!(split[1], Rollout { bucket_start: 90, bucket_end: 100 });

// The percentages must sum to exactly 100, and the error names the direction.
assert_eq!(Rollout::partition(&[90, 9]), Err(RolloutError::Under { total: 99 }));
assert_eq!(Rollout::partition(&[90, 11]), Err(RolloutError::Over { total: 101 }));
```

A `0` entry is allowed and yields an empty range, which serves nothing — the
natural way to express a version that is staged but takes no traffic yet.

`Rollout::validate_set` checks a set you already have, wherever it came from:

```rust
use dataflow_rs::{Rollout, RolloutError};

let good = Rollout::partition(&[50, 50]).unwrap();
assert!(Rollout::validate_set(&good).is_ok());

// Order does not matter — partitioning is a property of the set.
let reversed: Vec<_> = good.iter().rev().copied().collect();
assert!(Rollout::validate_set(&reversed).is_ok());

// A gap is reported at the lowest affected bucket.
let gapped = [
    Rollout { bucket_start: 0,  bucket_end: 40 },
    Rollout { bucket_start: 41, bucket_end: 100 },
];
assert_eq!(Rollout::validate_set(&gapped), Err(RolloutError::Gap { bucket: 40 }));
```

Ranges are checked individually first, so an inverted range or one reaching past
bucket 100 is reported as itself rather than as whatever downstream gap it
happens to produce.

`Engine::build()` does **not** run this check. A `Workflow` does not know which
version-set it belongs to — that grouping lives in your storage schema — so
calling `validate_set` before you activate a set of versions is the host's job,
and these helpers are what it calls.

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and rule visualization.

<div class="playground-widget" data-workflows='[{"id":"parse_workflow","name":"Parse Input","priority":1,"tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}}]},{"id":"conditional_workflow","name":"Conditional Rule","priority":2,"condition":{"==":[{"var":"data.input.role"},"admin"]},"tasks":[{"id":"greet","name":"Greet User","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Welcome, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"Alice","role":"admin"}'>
</div>

Try changing `role` to something other than "admin" to see the conditional rule skip.
