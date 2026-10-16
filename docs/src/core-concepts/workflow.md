# Workflows

A workflow is a list of tasks that run in order when a condition is met. It is the core **IF → THEN** unit: **IF** the condition matches, **THEN** run the tasks.

## Overview

Workflows provide:

- **Conditional Execution** - Only run when a JSONLogic condition holds (against the full context: `data`, `metadata`, `temp_data`)
- **Priority Ordering** - Control execution order across workflows
- **Task Organization** - Group related processing steps, nest them under one shared condition, or loop over them
- **Failure Policy** - Decide whether a task's failure stops the run or lets the next workflow run

## Workflow Structure

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
| `id` | string | Yes | Workflow identifier: a non-empty string |
| `name` | string | No | Human-readable name |
| `description` | string | No | Free-text description |
| `priority` | number | No | Execution order (default: 0, lower runs first; equal priorities keep their input order) |
| `condition` | JSONLogic | No | Whether a message enters the workflow (evaluated against the full context); absent means always |
| `on_failure` | string | No | What a task's failure that reaches the workflow does: `fail` (default) stops the run, `continue` lets the next workflow run |
| `tasks` | array | Yes | Steps to execute: a task, or a group of steps sharing one condition (see [Control Flow](../advanced/control-flow.md)); never empty |
| `channel` | string | No | Channel for message routing (default: `"default"`) |
| `version` | number | No | Definition version (default: `1`) |
| `status` | string | No | Lifecycle status: `active`, `paused`, or `archived` (default: `active`) |
| `tags` | array | No | Arbitrary tags for organization (default: `[]`) |
| `rollout` | object | No | Traffic split: `{bucket_start, bucket_end}` over `0..100` (default: none) |
| `loop` | object | No | Run the task list as a bounded loop; see [Loops](../advanced/loops.md) (default: none) |
| `ext` | object | No | Host and UI data, kept verbatim and never read |
| `created_at` | datetime | No | Creation timestamp (ISO 8601); kept, never read |
| `updated_at` | datetime | No | Last update timestamp (ISO 8601); kept, never read |

With the compat reader on (the default), an unknown key does not fail the
definition: it moves into the owning node's `ext` and is reported as an
`UNKNOWN_KEY` advisory, or as a `LIKELY_TYPO` defect when it is one edit away
from a known key, so a misspelt `conditon` is caught rather than silently
ignored. With the reader off (`ParseOptions::default().compat(false)`), every
unknown key is refused.

## Creating Workflows

### From JSON

The engine builder takes the definition as a `serde_json::Value`:

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

### Parsing a definition yourself

`def::parse` reads one definition into a `WorkflowDef`, plain data with raw
expressions, and is also the linter: it never fails, every problem is an
`Issue` at the path the author typed, and the definition is `Some` exactly
when no issue is Rejected.

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
assert_eq!(def.id, "w");
assert_eq!(def.tasks.len(), 1);

// The near miss loads, kept in the task's `ext`, and is reported.
assert_eq!(parsed.issues[0].code, IssueCode::LikelyTypo);
assert_eq!(parsed.issues[0].severity, Severity::Defect);
assert_eq!(parsed.issues[0].path.to_string(), "tasks[0].conditon");
```

A parsed `WorkflowDef` goes to the builder with `.workflow(def)`, or to a
[`Compiler`](./engine.md#quarantining-a-bad-definition) for a host that
quarantines bad definitions.

### From File

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

### Loading a 3.x definition

A 3.x definition loads as it is. The compat reader, on by default, rewrites
each 3.x spelling into its 4.0 counterpart and reports the rewrite as a
`V3_SPELLING` advisory at the 3.x key, so nothing is silently reinterpreted:

```rust
use dataflow_rs::def::{FailurePolicy, ParseOptions, StepDef, parse};
use dataflow_rs::program::{IssueCode, Severity};
use serde_json::json;

// 3.x's `continue_on_error: true` on a task is 4.0's `on_failure: continue`.
let parsed = parse(
    &json!({"id": "w", "tasks": [
        {"id": "t", "continue_on_error": true,
         "function": {"name": "map", "input": {"mappings": []}}}
    ]}),
    ParseOptions::default(),
);
let def = parsed.def.expect("the 3.x definition loads");
let StepDef::Task(task) = &def.tasks[0] else { panic!("a task") };
assert_eq!(task.on_failure, FailurePolicy::Continue);
assert_eq!(parsed.issues[0].code, IssueCode::V3Spelling);
assert_eq!(parsed.issues[0].severity, Severity::Advisory);
assert_eq!(parsed.issues[0].path.to_string(), "tasks[0].continue_on_error");
```

`compat::migrate_v3` writes a definition back in the 4.0 spelling. See
[Migrating from 3.x](../migration/from-3.md) for the full table of rewrites.

## Priority Ordering

Workflows execute in priority order (lowest first). This enables the **THAT** (chaining) in the IF → THEN → THAT model:

```json
// Executes first (priority 1): validate input
{
    "id": "validation",
    "priority": 1,
    "tasks": [...]
}

// Executes second (priority 2): transform data
{
    "id": "transformation",
    "priority": 2,
    "tasks": [...]
}

// Executes last (priority 10): send notifications
{
    "id": "notification",
    "priority": 10,
    "tasks": [...]
}
```

A later workflow sees everything an earlier one wrote, `metadata.progress`
included: the engine rewrites it after every task as
`{"workflow_id", "task_id", "status_code"}`, so a workflow can gate on what
the one before it did.

## Conditional Execution

Use JSONLogic conditions to control when workflows run. Conditions evaluate against the **full message context** (`data`, `metadata`, and `temp_data`) and may also read `{"secret": "name"}` from the engine's [secret store](../advanced/secrets.md), which is never part of the message:

```json
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

Only a literal `true` selects the workflow. A condition that fails to
evaluate counts as false: the `Run` reports the workflow as
`Selection::ConditionFailed(error)` and the message records nothing.

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

## Failure Policy

### Stop the Run (Default)

```json
{
    "id": "strict_workflow",
    "on_failure": "fail",
    "tasks": [...]
}
```

A task's failure (an `Err` or a `5xx` status) under the task's default
`on_failure: fail` passes to the workflow. With the workflow's default,
`fail`, the run stops: `Run::status()` is `Stopped(error)`, later workflows
are not considered, and the failure is recorded in `message.errors()`.

### Continue to the Next Workflow

```json
{
    "id": "resilient_workflow",
    "on_failure": "continue",
    "tasks": [...]
}
```

A workflow's `on_failure` governs what happens **after this workflow fails**:
with `continue`, later workflows still run, the workflow's exit is `Failed`,
and the run ends `CompletedWithErrors`. It is *not* a default inherited by the
workflow's tasks: whether the workflow keeps going past a failing task is
decided by that task's own `on_failure`. See
[Error Handling](./error-handling.md#the-policy-matrix) for the full matrix.

Errors are recorded in `message.errors()` either way.

## Task Dependencies

Tasks within a workflow execute sequentially, so later tasks can depend on earlier results:

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

Workflows support lifecycle management with status, versioning, and tagging. All lifecycle fields are optional.

### Status

Control whether a workflow runs with the `status` field:

```json
{"id": "my_workflow", "status": "active", "tasks": [...]}
{"id": "old_workflow", "status": "paused", "tasks": [...]}
{"id": "legacy_workflow", "status": "archived", "tasks": [...]}
```

- **`active`** (default): the workflow is considered on every run
- **`paused`**: the workflow is linked into the program but never selected
- **`archived`**: the same as paused; marks a permanently retired workflow

`Program::workflows()` lists all three, so a host can still show a paused
workflow; `Run::workflows()` never does, since a run never considers it.

### Channel Routing

Group workflows by channel for message routing:

```json
[
    {"id": "order_validation", "channel": "orders", "priority": 1, "tasks": [...]},
    {"id": "order_process", "channel": "orders", "priority": 2, "tasks": [...]},
    {"id": "user_notify", "channel": "notifications", "priority": 1, "tasks": [...]}
]
```

Then route a run to one channel:

```rust
# use dataflow_rs::{Engine, Message};
# async fn _demo(engine: Engine, mut message: Message) {
use dataflow_rs::RunOptions;

// Only the active workflows on the "orders" channel are considered.
let run = engine
    .run_with(&mut message, &RunOptions::default().channel("orders"))
    .await;
# let _ = run;
# }
```

The run stamps `metadata.channel` with the channel name. A run without a
channel considers every active workflow, whatever its channel.

### Version and Tags

Use `version` and `tags` for workflow organization:

```json
{
    "id": "discount_workflow",
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

That workflow serves buckets `0..=9`, 10% of traffic. Pair it with a
`{"bucket_start": 10, "bucket_end": 100}` sibling to run the old version for the
rest. `bucket_start` is inclusive and `bucket_end` exclusive, so the two ranges
partition `0..=99` exactly with no overlap and no gap. An empty or inverted range
(`bucket_end <= bucket_start`) serves nothing.

The engine does **not** derive the bucket. Pass it with the run:

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

How you map a request to a bucket is your policy: a sticky hash of some request
identity (so a given user always sees the same version), a per-message random
draw, round-robin. That deliberately stays outside this crate.

Two rules apply:

- **A run with no bucket is admitted by every workflow**, split or not. The
  trade-off is that setting `rollout` and forgetting the bucket runs *every*
  version on the same message, so set both together.
- **An excluded workflow is skipped exactly like a false condition**: no audit
  entry, `metadata.progress` untouched, `Selection::SkippedByRollout` in the
  `Run`, and one `Skipped` step in a trace. The gate runs before any work on
  the context, so exclusion is cheap.

### Building and checking a split

A single `rollout` is only half the picture. Correctness is a property of the
whole *set*: the versions of one logical workflow must partition `0..100`
exactly. Both ways of getting that wrong are silent in
production: a **gap** blackholes a slice of traffic, and an **overlap** makes
which version answers depend on workflow ordering rather than on the rollout.

`Rollout::partition` turns percentages into contiguous ranges, in traffic order:

```rust
use dataflow_rs::def::{Rollout, RolloutError};

let split = Rollout::partition(&[90, 10]).unwrap();
assert_eq!(split[0], Rollout::new(0, 90));
assert_eq!(split[1], Rollout::new(90, 100));

// The percentages must sum to exactly 100, and the error names the direction.
assert_eq!(Rollout::partition(&[90, 9]), Err(RolloutError::Under { total: 99 }));
assert_eq!(Rollout::partition(&[90, 11]), Err(RolloutError::Over { total: 101 }));
```

A `0` entry is allowed and yields an empty range, which serves nothing: the
natural way to express a version that is staged but takes no traffic yet.

`Rollout::validate_set` checks a set you already have, wherever it came from:

```rust
use dataflow_rs::def::{Rollout, RolloutError};

let good = Rollout::partition(&[50, 50]).unwrap();
assert!(Rollout::validate_set(&good).is_ok());

// Order does not matter: partitioning is a property of the set.
let reversed: Vec<_> = good.iter().rev().copied().collect();
assert!(Rollout::validate_set(&reversed).is_ok());

// A gap is reported at the lowest affected bucket.
let gapped = [Rollout::new(0, 40), Rollout::new(41, 100)];
assert_eq!(Rollout::validate_set(&gapped), Err(RolloutError::Gap { bucket: 40 }));
```

Ranges are checked individually first, so an inverted range or one reaching past
bucket 100 is reported as itself rather than as whatever downstream gap it
happens to produce.

Neither `parse` nor `build()` runs this check. A definition does not know
which version-set it belongs to (that grouping lives in your storage schema),
so calling `validate_set` before you activate a set of versions is the host's
job, and these helpers are what it calls.

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"parse_workflow","name":"Parse Input","priority":1,"tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}}]},{"id":"conditional_workflow","name":"Conditional Workflow","priority":2,"condition":{"==":[{"var":"data.input.role"},"admin"]},"tasks":[{"id":"greet","name":"Greet User","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Welcome, ",{"var":"data.input.name"},"!"]}}]}}}]}]' data-payload='{"name":"Alice","role":"admin"}'>
</div>

Try changing `role` to something other than "admin" to see the conditional workflow skip.
