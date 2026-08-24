# A public walker over the authored step tree (#42)

**Issue:** [GoPlasmatic/dataflow-rs#42](https://github.com/GoPlasmatic/dataflow-rs/issues/42)
**Target:** v3.7.0
**Date:** 2026-08-24

## Problem

Since 3.6.0 a workflow's `tasks` array holds *steps* — tasks and task groups —
and the parser flattens that tree at deserialization. By the time a host holds a
`Workflow`, the authored shape is gone: `tasks` is flat, `group_starts` is
`#[doc(hidden)]` and documented as *"not part of the stable API"*, and nothing
is `Serialize`.

But a host that stores and validates workflow definitions must analyse the
**authored** JSON, because validation errors, lint findings and dependency
extraction all need to point at the coordinates the author typed. With no public
walker, every such host re-implements the grammar — the group test and the depth
cap. Orion carries a 242-line `engine/steps.rs` doing exactly that, with its own
`MAX_STEP_DEPTH: usize = 8` mirroring a private upstream const.

### The mirror has already diverged, in this repository

The issue argues a mirrored rule will drift. It already has, between this
crate and the UI package it ships alongside:

```
src/engine/task.rs:240       let is_group = step.get("tasks").is_some();
ui/src/types/workflow.ts:78  return Array.isArray((step as TaskGroup).tasks);
```

Given `{"id": "x", "tasks": "oops"}`, the Rust parser classifies it as a
malformed **group** (`invalid task group in workflow tasks`) while the
TypeScript classifies it as a **task**. Two implementations of one rule,
disagreeing today. That is the strongest available argument for this issue, and
it is fixed here rather than merely reported.

## Scope

Export the grammar. Validation — rule checking — is #43 and builds on this. No
`Task` construction, no `Serialize`, per the issue's own non-goals.

## API

```rust
/// Maximum group nesting the parser accepts.
pub const MAX_GROUP_DEPTH: usize = 8;

/// True when this authored step element parses as a task group.
pub fn is_group(step: &serde_json::Value) -> bool;

/// Walk an authored `tasks` array without constructing `Task`s.
pub fn walk_authored_steps(tasks: &serde_json::Value) -> AuthoredSteps<'_>;

pub struct AuthoredStep<'a> {
    /// The coordinate the author typed, e.g. `tasks[1].tasks[0]`.
    pub path: String,
    pub node: &'a serde_json::Value,
    pub kind: StepKind,
    /// Enclosing groups. 0 for a top-level element.
    pub depth: usize,
}

pub enum StepKind { Leaf, Group, TooDeep }
```

`depth` is not in the issue's sketch but falls out of the walk for free, and a
linter reporting "this group is nested too deeply" needs it to say how deeply.

### Semantics, matching the parser exactly

- **Group test is presence of a `tasks` key, nothing else.** An element with
  neither `tasks` nor `function` is yielded as `Leaf`, so a caller reports a
  broken *task* rather than a broken group — matching the parser, which hands
  such an element to `Task`'s deserializer and gets `missing field 'function'`.
- **Traversal is document order, pre-order.** A group is yielded before its
  members. Filtering to `Leaf` therefore reproduces `Workflow::tasks` exactly,
  in order.
- **`TooDeep` applies only to groups.** The parser checks depth solely in its
  group branch (`task.rs:249`), with top-level groups at depth 0 and the test
  `depth >= MAX_GROUP_DEPTH`. So groups are legal at depths 0–7; a group at
  depth 8 is `TooDeep` and is not descended into. A leaf is never `TooDeep`, at
  any depth.
- **Not an array yields nothing.** `walk_authored_steps(&Value::Null)` is an
  empty iterator. Whether `tasks` is a non-empty array is #43's rule to report,
  not this walker's to fail on.

### The walker is total; the parser fails fast

This is the one place the two deliberately differ, and it is not drift.
`flatten` returns `Err` on the first malformed element, an empty group, or an
over-deep group — correct for a parser. The walker never fails: it yields those
nodes so a validator can collect *every* violation in one pass, which is #43's
whole premise. Documented on both.

### Path format

Root-prefixed, as in the issue: top-level element `i` is `tasks[i]`; child `j`
of a group at `tasks[i]` is `tasks[i].tasks[j]`.

This is the decision #43 and #46b inherit, so the crate owns the formatting
rather than yielding index vectors for each host to format its own way — which
would recreate the mirror problem one layer up. `#43` appends its own field
segment (`format!("{}.id", step.path)`).

## What can honestly be shared, and what cannot

The issue asks for the walker to be "implemented on the same private walk so
the two cannot drift". Taken literally that is the wrong trade: the two
recursions have different shapes — one fallible and `Task`-constructing, the
other total and borrowing — and forcing them through one generic walk would
make both harder to read than the drift it prevents.

What *is* genuinely one definition is the drift surface the issue actually
names:

| Fact | Single definition |
|---|---|
| Group test | `is_group()`, called by both |
| Depth cap | `MAX_GROUP_DEPTH`, read by both |

The recursion itself is pinned by **equivalence testing** instead: for every
fixture, the walker's `Leaf` set equals the parser's flattened `Workflow::tasks`,
by id and by order. That is acceptance criterion 1, and it catches a divergence
in the recursion that sharing a helper would not.

## Placement

New module `src/engine/steps.rs`, owning the whole grammar:

- `MAX_GROUP_DEPTH`, `is_group`
- `walk_authored_steps`, `AuthoredSteps`, `AuthoredStep`, `StepKind`
- `pub(crate) fn flatten` — moved from `task.rs`
- `fn walk` — the parser recursion, moved

`task.rs` returns to ~200 lines about `Task` and `TaskGroup`. Anti-drift by
construction: someone changing the group rule sees both users on screen.

Re-exported from `src/lib.rs` and `src/engine/mod.rs`.

## Iterator

`AuthoredSteps` is a lazy iterator over an explicit stack of frames — a borrowed
slice, a cursor, the path prefix, the depth — so nothing is allocated beyond the
per-node `path` string and the frame stack, which is bounded by
`MAX_GROUP_DEPTH`.

## UI parity

The issue notes the step helpers are not re-exported from `ui/src/lib.ts`.
Confirmed: `Task` is exported (`lib.ts:54`), but `TaskGroup`, `Step`,
`isTaskGroup` and `flattenSteps` are not, despite the 3.6.0 changelog
describing `flattenSteps` as consumer-facing.

Two changes:

1. Add the four missing exports.
2. Fix `isTaskGroup` to test key presence rather than array-ness, so it agrees
   with the engine. A TS test enumerates the same malformed fixtures the Rust
   equivalence test uses, pinning the two grammars to each other across
   languages rather than by inspection.

## Testing

| Test | Pins |
|---|---|
| `walker_leaves_match_the_parsers_flattened_tasks` | The equivalence property, over nested fixtures — ids and order. Acceptance criterion 1. |
| `paths_are_the_coordinates_the_author_typed` | `tasks[1].tasks[0]` for a nested leaf. |
| `groups_are_yielded_before_their_members` | Pre-order, so the `Leaf` filter reproduces parse order. |
| `a_group_at_the_depth_cap_is_too_deep_and_is_not_descended` | The exact boundary the parser rejects, and that its children do not appear. |
| `a_leaf_is_never_too_deep` | Depth applies to groups only. |
| `an_element_with_neither_tasks_nor_function_is_a_leaf` | The malformed-task reading, not malformed-group. |
| `a_tasks_key_that_is_not_an_array_is_still_a_group` | The exact divergence the TS had. |
| `an_empty_group_is_yielded_not_an_error` | Walker totality where the parser fails. |
| `a_non_array_input_yields_nothing` | `Value::Null`, an object, a string. |
| `max_group_depth_is_the_value_the_parser_enforces` | The const and the parser cannot drift. |

## Compatibility

Purely additive. `flatten` moves module but keeps its `pub(crate)` visibility
and behaviour; `Workflow`'s `deserialize_with` target is repointed. The UI
`isTaskGroup` change alters classification only for input the engine rejects
anyway.

## Verification

```
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p dataflow-rs --all-targets -- -D warnings
cargo test --workspace --all-features
cargo test -p dataflow-rs
cargo +1.85 check --workspace --all-targets --all-features --locked
```

Plus the UI package's own test run. MSRV 1.85: nested `if let`, never
let-chains. Test counts in `CLAUDE.md` (520 / 442) move and are updated against
a measured baseline.
