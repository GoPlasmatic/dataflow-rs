# Workflows

A workflow is an ordered sequence of tasks that run when a condition matches. It is the core **IF → THEN** unit in `dataflow-rs`: **IF** the workflow's condition evaluates to `true`, **THEN** its tasks execute.

## Overview

Workflows provide:

- **Conditional Execution** — Run only when a JSONLogic condition matches against `{data, metadata, temp_data}`.
- **Priority Ordering** — Control the order in which workflows run across a message pipeline.
- **Task Organization** — Group related steps, nest them under shared conditions (`GroupDef`), or repeat them in a bounded loop.
- **Failure Policy** — Control whether an unhandled task failure stops the entire run (`"fail"`) or continues to the next workflow (`"continue"`).

## Workflow Structure

```jsonc
{
    "id": "premium_order",
    "name": "Premium Order Processing",
    "priority": 1,
    "channel": "orders",
    "version": 2,
    "status": "active",
    "tags": ["premium", "high-priority"],
    "condition": { ">=": [{"var": "data.order.total"}, 1000] },
    "on_failure": "fail",
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
| `id` | string | Yes | Unique workflow identifier (must be non-empty) |
| `name` | string | No | Human-readable display name |
| `description` | string | No | Free-text description |
| `priority` | number | No | Execution order (default: `0`; lower numbers run first, ties keep input order) |
| `condition` | JSONLogic | No | Condition evaluated against `{data, metadata, temp_data}`; when omitted, the workflow always runs |
| `on_failure` | string | No | What happens when a task failure reaches the workflow: `"fail"` (default) stops the run; `"continue"` moves to the next workflow |
| `tasks` | array | Yes | Non-empty list of tasks or task groups (see [Control Flow](../advanced/control-flow.md)) |
| `channel` | string | No | Routing channel name (default: `"default"`) |
| `version` | number | No | Workflow version number (default: `1`) |
| `status` | string | No | Lifecycle status: `"active"` (default), `"paused"`, or `"archived"` |
| `tags` | array | No | Arbitrary string tags for organization (default: `[]`) |
| `rollout` | object | No | Traffic split `{bucket_start, bucket_end}` over `0..100` (default: none) |
| `loop` | object | No | Bounded loop configuration; see [Loops](../advanced/loops.md) (default: none) |
| `ext` | object | No | Custom host or UI metadata preserved verbatim by the parser |
| `created_at` | datetime | No | ISO 8601 creation timestamp (metadata only) |
| `updated_at` | datetime | No | ISO 8601 update timestamp (metadata only) |

When the compatibility reader is enabled (the default), unrecognized keys are moved into the node's `ext` map rather than rejecting the workflow. They are reported as `UNKNOWN_KEY` advisories—or as `LIKELY_TYPO` defects if they are within edit distance of a known field name (for example, `conditon` instead of `condition`). If you disable compatibility mode via `ParseOptions::default().compat(false)`, unrecognized keys are rejected.

## Creating Workflows

### From JSON

Pass workflow JSON directly to `Engine::builder()`:

```rust
# fn _demo() -> Result<(), Vec<dataflow_rs::Issue>> {
use dataflow_rs::Engine;
use serde_json::json;

let engine = Engine::builder()
    .workflow_json(&json!({
        "id": "my_workflow",
        "name": "My Workflow",
        "tasks": [
            {"id": "t", "function": {"name": "map", "input": {
                "mappings": [{"path": "data.seen", "logic": true}]}}}
        ]
    }))
    .build()?;
# let _ = engine;
# Ok(()) }
```

### Parsing a Definition Manually

`def::parse` parses and lints a JSON value into a `WorkflowDef`. It never panics or short-circuits on the first error: it collects every structural problem as an `Issue` with its exact JSON path, and returns `Some(WorkflowDef)` whenever no issue has `Severity::Rejected`:

```rust
use dataflow_rs::def::{ParseOptions, parse};
use dataflow_rs::program::{IssueCode, Severity};
use serde_json::json;

let parsed = parse(
    &json!({"id": "w", "tasks": [
        {"id": "t", "conditon": true, "function": {"name": "map", "input": {"mappings": []}}}
    ]}),
    ParseOptions::default(),
);
let def = parsed.def.expect("nothing was rejected");
assert_eq!(def.id(), "w");
assert_eq!(def.tasks().len(), 1);

// The typo `conditon` is preserved in `ext` and flagged as a `LikelyTypo` defect.
assert_eq!(parsed.issues[0].code, IssueCode::LikelyTypo);
assert_eq!(parsed.issues[0].severity, Severity::Defect);
assert_eq!(parsed.issues[0].path.to_string(), "tasks[0].conditon");
```

You can pass a parsed `WorkflowDef` to `.workflow(def)` on `EngineBuilder`, or compile it individually with [`Compiler::compile_one`](./engine.md#quarantining-invalid-definitions).

### Loading from a File

```rust
# fn _demo() -> Result<(), Box<dyn std::error::Error>> {
use dataflow_rs::def::{ParseOptions, parse};

let text = std::fs::read_to_string("workflows/premium_order.json")?;
let raw: serde_json::Value = serde_json::from_str(&text)?;
let parsed = parse(&raw, ParseOptions::default());
for issue in &parsed.issues {
    eprintln!("{issue}");
}
# Ok(()) }
```

### Loading 3.x Definitions

Existing 3.x workflow definitions load automatically through the compatibility reader (enabled by default). The parser rewrites 3.x field names to their 4.0 equivalents and emits a `V3_SPELLING` advisory at each legacy key:

```rust
use dataflow_rs::def::{FailurePolicy, ParseOptions, StepDef, parse};
use dataflow_rs::program::{IssueCode, Severity};
use serde_json::json;

// In 3.x, `continue_on_error: true` on a task maps to `on_failure: "continue"` in 4.0.
let parsed = parse(
    &json!({"id": "w", "tasks": [
        {"id": "t", "continue_on_error": true,
         "function": {"name": "map", "input": {"mappings": []}}}
    ]}),
    ParseOptions::default(),
);
let def = parsed.def.expect("the 3.x definition loads");
let StepDef::Task(task) = &def.tasks()[0] else { panic!("a task") };
assert_eq!(task.on_failure(), FailurePolicy::Continue);
assert_eq!(parsed.issues[0].code, IssueCode::V3Spelling);
assert_eq!(parsed.issues[0].severity, Severity::Advisory);
assert_eq!(parsed.issues[0].path.to_string(), "tasks[0].continue_on_error");
```

To permanently convert a 3.x JSON definition into 4.0 format, call `dataflow_rs::compat::migrate_v3`. See [Migrating from 3.x](../migration/from-3.md) for the complete mapping table.

## Priority Ordering (Chaining Workflows)

Workflows execute in ascending `priority` order (lowest number runs first). This powers the **THAT** (chaining) in the IF → THEN → THAT model:

```jsonc
// Runs first (priority 1): validate input
{
    "id": "validation",
    "priority": 1,
    "tasks": [...]
}

// Runs second (priority 2): transform data
{
    "id": "transformation",
    "priority": 2,
    "tasks": [...]
}

// Runs last (priority 10): send notifications
{
    "id": "notification",
    "priority": 10,
    "tasks": [...]
}
```

Each workflow sees all context changes made by earlier workflows, including `metadata.progress`. After every executed task, the engine updates `metadata.progress` to `{"workflow_id", "task_id", "status_code"}`, allowing later workflows to condition their execution on the outcome of earlier steps.

## Conditional Execution

Use JSONLogic conditions to control when a workflow runs. Conditions can read any field in `data`, `metadata`, and `temp_data`, as well as secrets from the engine's [secret store](../advanced/secrets.md) via `{"secret": "name"}`:

```jsonc
{
    "id": "premium_user_workflow",
    "condition": {
        "and": [
            {">=": [{"var": "data.order.total"}, 500]},
            {"==": [{"var": "data.user.is_vip"}, true]}
        ]
    },
    "tasks": [...]
}
```

A workflow is selected only when its condition evaluates to boolean `true`. If evaluation fails with an error, the condition is treated as `false` and the workflow is recorded on `Run` with `Selection::ConditionFailed(error)`.

### Common Condition Patterns

```jsonc
// Numeric comparison on `data`
{">=": [{"var": "data.order.total"}, 1000]}

// Check that a field is present and truthy
{"!!": {"var": "data.email"}}

// All conditions must hold
{"and": [
    {">=": [{"var": "data.amount"}, 100]},
    {"==": [{"var": "data.currency"}, "USD"]}
]}

// At least one condition must hold
{"or": [
    {"==": [{"var": "metadata.source"}, "api"]},
    {"==": [{"var": "metadata.source"}, "webhook"]}
]}
```

## Failure Policy

### Stop the Run (`"fail"`, Default)

```jsonc
{
    "id": "strict_workflow",
    "on_failure": "fail",
    "tasks": [...]
}
```

When a task fails (returns `Err` or a `5xx` status) and its own `on_failure` policy is `"fail"` (the default), the failure escalates to the workflow. Under the workflow's default `on_failure: "fail"`, the entire run stops immediately: `Run::status()` becomes `Stopped(error)`, subsequent workflows are skipped, and the error is recorded in `message.errors()`.

### Continue to the Next Workflow (`"continue"`)

```jsonc
{
    "id": "resilient_workflow",
    "on_failure": "continue",
    "tasks": [...]
}
```

A workflow's `on_failure` policy controls what happens **after the workflow itself fails**:
- With `"continue"`, the current workflow stops at the failing task (recording `WorkflowExit::Failed`), but the engine continues to the next workflow in the program and finishes with `RunStatus::CompletedWithErrors`.
- Note that a workflow's `on_failure` is **not** inherited by its tasks. Whether remaining tasks *inside the same workflow* continue after a task failure is controlled by each task's own `on_failure` field. See [Error Handling](./error-handling.md#the-policy-matrix) for the full policy matrix.

In all cases, the error is recorded in `message.errors()`.

## Passing Data Between Tasks

Tasks within a workflow run sequentially, so later tasks can read fields written by earlier tasks:

```json
{
    "id": "pipeline",
    "name": "Pipeline",
    "tasks": [
        {
            "id": "fetch_data",
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

Workflows include optional fields for lifecycle management, routing, versioning, and traffic splitting.

### Status

Control whether a workflow is active using the `status` field:

```jsonc
{"id": "my_workflow", "status": "active", "tasks": [...]}
{"id": "old_workflow", "status": "paused", "tasks": [...]}
{"id": "legacy_workflow", "status": "archived", "tasks": [...]}
```

- **`active`** (default) — Considered on every matching run.
- **`paused`** — Compiled and linked into `Program`, but skipped during execution.
- **`archived`** — Same runtime behavior as `paused`; marks a permanently retired workflow.

`Program::workflows()` includes `active`, `paused`, and `archived` workflows so admin UIs can list them, while `Run::workflows()` includes only `active` workflows that were considered during the run.

### Channel Routing

Assign workflows to named channels to partition execution by event or message type:

```jsonc
[
    {"id": "order_validation", "channel": "orders", "priority": 1, "tasks": [...]},
    {"id": "order_process", "channel": "orders", "priority": 2, "tasks": [...]},
    {"id": "user_notify", "channel": "notifications", "priority": 1, "tasks": [...]}
]
```

Then target a specific channel on `RunOptions`:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::RunOptions;

// Runs only active workflows in the "orders" channel.
let run = engine
    .run_with(&mut message, &RunOptions::default().channel("orders"))
    .await;
# let _ = run;
# }
```

When a channel is specified on `RunOptions`, the engine writes its name to `metadata.channel`. If no channel is specified on `RunOptions`, all `active` workflows across all channels are considered.

### Version and Tags

Use `version` and `tags` to organize and track workflow revisions:

```jsonc
{
    "id": "discount_workflow",
    "version": 3,
    "tags": ["finance", "discount", "v3"],
    "tasks": [...]
}
```

### Traffic Splits (Canary Rollouts)

Route a percentage of a channel's traffic to a workflow using a half-open bucket range over `0..100`:

```jsonc
{
    "id": "checkout_v2",
    "channel": "checkout",
    "rollout": { "bucket_start": 0, "bucket_end": 10 },
    "tasks": [...]
}
```

This workflow matches buckets `0..=9` (10% of traffic). Pair it with a sibling workflow configured with `{"bucket_start": 10, "bucket_end": 100}` to route the remaining 90% of traffic to the existing version. Because `bucket_start` is inclusive and `bucket_end` is exclusive, the two ranges partition `0..=99` cleanly without gaps or overlap. An empty or inverted range (`bucket_end <= bucket_start`) matches no traffic.

You supply the bucket for each run via `RunOptions::routing_bucket`:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::RunOptions;

let opts = RunOptions::default().routing_bucket(7);
assert_eq!(opts.routing_bucket, Some(7));
let run = engine.run_with(&mut message, &opts).await;
# let _ = run;
# }
```

Your application decides how to compute the bucket (`0..=99`, e.g., `hash(user_id) % 100`, random sampling, or round-robin). In Rust, values above `99` are clamped to `99` (`Rollout::MAX_BUCKET`); in `@goplasmatic/dataflow-wasm`, out-of-range bucket numbers throw an error.

Key rollout rules:

- **Runs without a `routing_bucket` match all workflows**, whether or not they have a `rollout`. Make sure to set `routing_bucket` on `RunOptions` whenever you have multiple rollout versions active, or both versions will run on the same message.
- **Excluded workflows are skipped before condition evaluation**: they leave no audit entry, do not update `metadata.progress`, record `Selection::SkippedByRollout` on `Run`, and appear as a `Skipped` step in traces.
- **Custom host functions can inspect the current bucket** via `ctx.info().routing_bucket` and forward it to child runs.

### Partitioning and Validating Rollout Ranges

When splitting traffic across multiple versions of a workflow, their bucket ranges should partition `0..100` with no gaps (which would drop traffic) and no overlaps (which would run multiple versions on the same message).

Use `Rollout::partition` to convert percentage weights into contiguous `Rollout` ranges:

```rust
use dataflow_rs::def::{Rollout, RolloutError};

let split = Rollout::partition(&[90, 10]).unwrap();
assert_eq!(split[0], Rollout::new(0, 90));
assert_eq!(split[1], Rollout::new(90, 100));

// Percentages must sum to 100; otherwise `Under` or `Over` is returned.
assert_eq!(Rollout::partition(&[90, 9]), Err(RolloutError::Under { total: 99 }));
assert_eq!(Rollout::partition(&[90, 11]), Err(RolloutError::Over { total: 101 }));
```

A `0%` weight produces an empty range that matches no traffic, which is useful for staging a new version before routing traffic to it.

Use `Rollout::validate_set` to verify that a set of `Rollout` ranges forms a valid partition of `0..100`:

```rust
use dataflow_rs::def::{Rollout, RolloutError};

let good = Rollout::partition(&[50, 50]).unwrap();
assert!(Rollout::validate_set(&good).is_ok());

// Range order does not matter.
let reversed: Vec<_> = good.iter().rev().copied().collect();
assert!(Rollout::validate_set(&reversed).is_ok());

// Missing buckets are reported at the lowest gap.
let gapped = [Rollout::new(0, 40), Rollout::new(41, 100)];
assert_eq!(Rollout::validate_set(&gapped), Err(RolloutError::Gap { bucket: 40 }));
```

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"parse_workflow","name":"Parse Input","priority":1,"tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}}]},{"id":"conditional_workflow","name":"Conditional Workflow","priority":2,"condition":{"==":[{"var":"data.input.role"},"admin"]},"tasks":[{"id":"greet","name":"Greet User","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Welcome, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"Alice","role":"admin"}'>
</div>

Try changing `"role"` from `"admin"` to `"user"` in the payload to see `conditional_workflow` get skipped.
