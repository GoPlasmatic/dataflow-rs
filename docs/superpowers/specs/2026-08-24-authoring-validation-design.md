# Authoring-time validation (#43, with the vocabulary #46b shares)

**Issues:** [#43](https://github.com/GoPlasmatic/dataflow-rs/issues/43) (this spec),
[#46b](https://github.com/GoPlasmatic/dataflow-rs/issues/46) (reuses the type)
**Target:** v3.7.0
**Date:** 2026-08-24
**Depends on:** #42 (authored-step walker)

## Problem

The only enforcement of the workflow grammar is parse + `Workflow::validate()` +
`LoopConfig::validate()`, all reached at `Engine::build()`. For a host that
stores definitions and builds one engine over many workflows that is wrong three
ways: it fires when the *engine* is built rather than when a *definition* is
submitted, so one bad row aborts the whole build; `validate()` returns one
stringly error, so an authoring API cannot point a 400 at
`tasks[1].tasks[0].id`; and it is fail-fast, so the author fixes one violation
per round trip.

The docs already name the gap without offering an API for it
(`functions/mod.rs:122-125`). Orion hand-mirrors every rule over authored JSON
(`validation/workflows.rs:243-378`, `:85-204`), its docstrings saying outright
*"these track dataflow-rs's parsing rules rather than tightening them"*, and
keeps a serde round-trip catch-all as a drift net because a mirror of private
rules will drift. When it does, a definition is accepted at authoring time and
detonates at engine build — at reload, affecting every workflow in the process.

## The acceptance criterion is not achievable as written

The issue asks for a biconditional:

> `validate_authored(json).is_empty()` ⇔ `from_value::<Workflow>` + `validate()`
> succeeds

Its rule list covers roughly fourteen *semantic* rules. `from_value` enforces
the entire serde schema on top of them. Six workflows that break **none** of the
listed rules and still fail to parse, verified against the current build:

| Input | Parser says |
|---|---|
| `map` task with no `mappings` | `config for function 'map': missing field 'mappings'` |
| `"priority": "high"` | `invalid type: string "high", expected u32` |
| `"continue_on_error": 3` | `invalid type: integer 3, expected a boolean` |
| `"status": "enabled"` | `unknown variant 'enabled', expected active/paused/archived` |
| `http_call` with no `connector` | `config for function 'http_call': missing field 'connector'` |
| `"loop": {"max": "3"}` | `invalid type: string "3", expected i64` |

Satisfying the ⇔ by enumeration would mean mirroring every field type of
`Workflow`, `Task`, `LoopConfig` and all eleven typed built-in configs inside
this crate — building the exact mirror the issue exists to delete, and one that
drifts whenever a config gains a field.

## Resolution: three stages, guarantee by construction

```text
1. semantic walk   -> collect ALL violations, authored paths.  Non-empty? return.
2. parse           -> Err? return one PARSE_FAILED.
3. Workflow::validate() -> Err? return one VALIDATE_FAILED.
4. return empty
```

Stages 2 and 3 *are* the guarantee: empty ⟺ the workflow parses and validates,
true by construction rather than by diligence. Stage 1 exists to make the common
cases good — every violation at once, each with the coordinate the author typed.

This also moves the host's serde round-trip drift net inside the crate, which is
what the issue is ultimately asking for.

**The risk this design carries** is that stage 1 could quietly become a no-op
and every test would still pass, since stages 2–3 catch everything. So the
tests do not merely assert `is_empty()`; for each broken fixture they assert the
**specific** semantic code, proving stage 1 did the work rather than falling
through to the catch-all.

## Shared vocabulary

One type, used by `validate_authored` now and `check_workflow` (#46b) later.

```rust
pub struct WorkflowIssue {
    pub code: IssueCode,
    pub message: String,
    /// Authored coordinate, rooted at the workflow: `tasks[1].tasks[0].id`.
    pub path: Option<String>,
    /// The step this concerns, when it concerns one.
    pub task_id: Option<String>,
}

#[non_exhaustive]
pub enum IssueCode { /* … */ }
impl IssueCode { pub fn as_str(&self) -> &'static str }
```

Both fields are `Option` because each producer fills what it genuinely knows:
`validate_authored` always has a path and knows the step id when the violation
concerns a step; `check_workflow` always has a task id and reports a
*task-relative* path. A host writes one mapping into its 400 response instead of
two.

`IssueCode` is a `#[non_exhaustive]` enum rather than `&'static str`: a host
branching on a string literal has no protection against a typo that compiles and
silently never matches, and `#[non_exhaustive]` lets a later minor add a rule
without breaking anyone. Each new rule is forced to name itself as a variant.

### Path format

Set by #42 and unchanged: top-level element `i` is `tasks[i]`; child `j` of a
group at `tasks[i]` is `tasks[i].tasks[j]`; a field appends its own segment.
`validate_authored` produces document-rooted paths; #46b will produce
task-relative ones (`function.input.query`) which a host joins with the walker's
coordinate for that id.

## Rules and codes

Every one mirrors a rule the engine enforces today. No tightening.

| Code | Path | Mirrors |
|---|---|---|
| `EmptyWorkflowId` | `id` | `validate()` |
| `EmptyWorkflowName` | `name` | `validate()` |
| `NoTasks` | `tasks` | `validate()` — also covers missing / not-an-array |
| `MissingStepId` | `tasks[..].id` | parser: `Task` / `GroupHeader` require `id` |
| `DuplicateStepId` | `tasks[..].id` | `validate()` — one namespace for tasks and groups |
| `EmptyGroup` | `tasks[..].tasks` | parser: *"an empty group can only be a mistake"* |
| `GroupTooDeep` | `tasks[..]` | parser: `MAX_GROUP_DEPTH` |
| `MissingFunction` | `tasks[..].function` | parser: a leaf must deserialize as `Task` |
| `InvalidFunctionName` | `tasks[..].function.name` | parser: `function` object with a name |
| `InvalidTerminal` | `tasks[..].terminal` | parser: `terminal` is a bool |
| `LoopIncrementTooSmall` | `loop.increment` | `LoopConfig::validate()` |
| `LoopBoundEmpty` | `loop.max` | `LoopConfig::validate()` — `max > init` |
| `LoopCounterInvalid` | `loop.counter` | `LoopConfig::validate()` |
| `ParseFailed` | best effort | stage 2 |
| `ValidateFailed` | — | stage 3 |

`GroupTooDeep` and `EmptyGroup` come straight off the walker's `StepKind` and
node inspection, so they cannot drift from the parser's own limits.

## API

```rust
impl Workflow {
    pub fn validate_authored(json: &serde_json::Value) -> Vec<WorkflowIssue>;
}
```

Takes `&Value` because that is what a host holds after reading a stored row. A
value that is not an object yields a single `NoTasks`-style issue rather than
panicking.

## Placement

New module `src/engine/authoring.rs`, holding `WorkflowIssue`, `IssueCode` and
`validate_authored`. #46b's `check_workflow` reporting joins it later, so the
authoring-time surface lives in one place. Named `authoring` rather than
`validation` because `functions/validation.rs` is already the `validation` *task*
and the collision would be confusing.

Built on `walk_authored_steps` from #42, so the step traversal, the group test
and the depth cap have exactly one definition.

## Testing

| Test | Pins |
|---|---|
| `every_broken_fixture_reports_its_own_code` | The important one. Each fixture asserts the **specific** semantic code, so stage 1 cannot silently become a no-op behind the catch-all. |
| `empty_iff_the_workflow_loads` | The biconditional, over valid and broken fixtures. |
| `all_violations_are_reported_not_just_the_first` | A document with three distinct problems yields three issues. |
| `violations_carry_authored_coordinates` | A duplicate id inside a group reports `tasks[1].tasks[0].id`, not a flat index. |
| `duplicate_ids_span_tasks_and_groups` | One namespace — a group id colliding with a task id is reported. |
| `a_type_error_falls_through_to_parse_failed` | The six cases above each yield `ParseFailed` with the parser's own message. |
| `a_group_past_the_depth_cap_is_reported_by_the_walker` | `GroupTooDeep` at the same boundary the parser rejects. |
| `a_non_object_input_does_not_panic` | `Value::Null`, an array, a string. |
| `issue_codes_round_trip_as_str` | Every variant has a distinct, stable string. |

## Compatibility

Purely additive. No existing behaviour changes; `Engine::build()` stays exactly
as permissive as it is.

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
(562 / 479) move and are updated against a measured baseline.
