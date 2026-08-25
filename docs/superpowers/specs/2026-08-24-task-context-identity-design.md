# Execution identity on TaskContext (#44)

**Issue:** [GoPlasmatic/dataflow-rs#44](https://github.com/GoPlasmatic/dataflow-rs/issues/44)
**Target:** v3.7.0
**Date:** 2026-08-24

## Problem

`TaskContext` hands a handler the message, the eval engine, and audited
get/set — but not *who is executing*. A handler that wants to label anything it
produces (a log line, a metric, a recorded call in a test harness) has no way to
learn its own `task_id` or `workflow_id`.

The engine plainly has both. `TaskEvent` already carries them
(`observer.rs:23-26`), and the dispatch site passes both `Arc<str>`s to
`handle_task_result` one line later:

```
workflow_executor.rs:1086  self.task_executor.execute(task, message).await
workflow_executor.rs:1093  self.handle_task_result(result, &workflow.id_arc, &task.id_arc, …)
```

What stops them reaching the handler is one layer: `TaskContext` is built inside
`TaskExecutor::dispatch_handler_any` (`task_executor.rs:151`), which receives
only `name`, `message` and `any_input`.

Downstream, that gap costs real correctness. Orion's offline test harness
records every stubbed handler call anonymously, then re-derives task ids after
the run by walking `ExecutionTrace.steps` and pairing `Executed` steps with
recorded calls **positionally** — and deliberately stops labelling at the first
mismatch, because a wrong label is worse than a missing one. Its per-request
profiler can only label handler samples by function name. Both are workarounds
for one missing accessor.

## API

```rust
impl<'a> TaskContext<'a> {
    pub fn workflow_id(&self) -> Option<&str>;
    pub fn task_id(&self) -> Option<&str>;
    pub fn loop_counter(&self) -> Option<i64>;
}
```

`Option` on the first two makes the test-construction path honest rather than
inventing ids. `loop_counter` is `None` outside a looping workflow, which is a
different fact from "identity unknown" and so stays independent.

## Design

### Borrowed, not `Arc<str>` cloned

The issue proposes carrying "clones of the existing `Arc<str>`s, zero-copy".
`Arc::clone` is cheap but it is two atomic read-modify-writes, **per task
dispatch**, on the hot path — and the accessors hand back `&str` either way, so
the refcount traffic buys nothing.

The identity is therefore borrowed: `&'a str`. The ids live in `workflow` and
`task`, both of which outlive the dispatch call, and `&'a mut Message` is
covariant in `'a`, so the lifetimes unify at the shorter message borrow without
friction.

### One optional struct, not two optional fields

```rust
pub(crate) struct TaskIdentity<'a> {
    pub workflow_id: &'a str,
    pub task_id: &'a str,
}
```

`TaskContext` stores `Option<TaskIdentity<'a>>`. Two independent
`Option<&'a str>` parameters would be adjacent, same-typed, and silently
swappable at the call site with no type error — and the state they would allow,
"workflow known but task unknown", never occurs. Identity is all-or-nothing:
the engine executing a task inside a workflow has both, and every other path
has neither.

### Construction

`TaskContext::new(message, datalogic)` keeps its signature exactly — it is
documented as the entry point for tests and benchmarks driving a handler
directly, and two integration test files use it. It yields `None` for all three
accessors.

A `pub(crate)` sibling carries identity, so there is no partially-initialised
state to get wrong:

```rust
pub(crate) fn with_identity(
    message: &'a mut Message,
    datalogic: &'a Arc<DatalogicEngine>,
    identity: Option<TaskIdentity<'a>>,
    loop_counter: Option<i64>,
) -> Self;
```

### Threading

`TaskExecutor` is publicly reachable — `engine/mod.rs:66` declares
`pub mod task_executor` — so changing `execute`'s signature would be a breaking
change even though only one in-crate caller exists. It keeps its signature and
delegates; an additive sibling carries the identity:

```rust
pub async fn execute(&self, task, message)                      // -> identity None
pub(crate) async fn execute_in_workflow(&self, task, message, identity, loop_counter)
```

`workflow_executor.rs:1086` calls the new one, passing `&workflow.id_arc`,
`&task.id_arc` and `pass.loop_counter` — all three already in scope, one line
above where two of them are passed to `handle_task_result`.

### Group ids never appear

Handlers run only on leaf tasks; a group is span bookkeeping recorded on the
task that opens it (`Task::group_starts`), never a dispatch target. So
`task_id()` is always a leaf task's id, never a group's — documented and pinned
by a test that runs a handler inside a group.

## Why `loop_counter` is worth including

The issue lists it as "fine to defer". Including it because the data is already
at the call site (`pass.loop_counter`) and the alternative available to a
handler today is poor: it can read the counter from `temp_data` only if the host
configured a **named** counter *and* the handler hardcodes that path. A workflow
whose `LoopConfig` has no `counter` name writes to an empty path
(`resolve_counter_parts`, `workflow_executor.rs:278`) and the sweep index is
then unreachable by any means. This closes that hole for four lines.

## Testing

| Test | Pins |
|---|---|
| `a_handler_sees_its_own_workflow_and_task_ids` | The acceptance criterion, via a recording handler under `process_message`. |
| `identity_is_none_for_a_directly_constructed_context` | `TaskContext::new` stays honest. |
| `a_handler_inside_a_group_reports_the_leaf_task_id` | Group ids never surface. |
| `loop_counter_tracks_the_sweep` | Counts up across sweeps of a looping workflow. |
| `loop_counter_is_reported_without_a_named_counter` | The case that is unreachable via `temp_data` today. |
| `loop_counter_is_none_outside_a_loop` | Distinct from "identity unknown". |
| `identity_survives_a_handler_that_returns_an_error` | Recorded before the `?` in the result path. |

## Compatibility

Additive. No existing signature changes: `TaskContext::new` and
`TaskExecutor::execute` are untouched, so the two integration test files using
`TaskContext::new` and any external direct driver keep compiling.

## Verification

```
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p dataflow-rs --all-targets -- -D warnings
cargo test --workspace --all-features
cargo test -p dataflow-rs
cargo +1.85 check --workspace --all-targets --all-features --locked
```

MSRV 1.85: nested `if let`, never let-chains. Test counts in `CLAUDE.md`
(535 / 455) move and are updated against a measured baseline.
