# Expose the dispatch vocabulary (#46a)

**Issue:** [GoPlasmatic/dataflow-rs#46](https://github.com/GoPlasmatic/dataflow-rs/issues/46), first of two slices
**Target:** v3.7.0
**Date:** 2026-08-24

## Problem

The crate ships half the answer a host needs to screen a workflow definition.
`builtin_function_kind(name)` tells it *"this name needs a handler registered
under it"*. Nothing tells it **"…and is one?"** — `Engine::new` consumes the
handler map into a private field, and `EngineBuilder` never exposes
`self.handlers`.

That missing half is visible in the crate's own artefacts. The existing test
stops exactly at the line where the registry would be needed:

```
tests/public_api.rs:53   "a caller can screen for this before accepting the workflow"
```

and the integrations page can only advise rejecting *every* `RequiresHandler`
name, because it cannot see whether one is backed:

```
docs/src/built-in-functions/integrations.md:123
    "reject every RequiresHandler and None name, and accept SelfContained names"
```

So hosts keep the list by hand, and the hand-copy drifts. Orion shipped a bug
where `enrich` passed validation, activated cleanly, and returned 500 on every
request because the name list and the registry had diverged.

## Scope

This slice delivers the **registry half**: enumeration and a dispatch
predicate. The per-workflow reporting API (`check_workflow`, `CheckIssue`) is
#46b, deferred to Phase 2 so it can share an error vocabulary with #43
(`validate_authored`) rather than fix one unilaterally.

The seam is the issue's own deletion inventory, which splits cleanly:

| Slice | Deletes from the host | Unblocks |
|---|---|---|
| **#46a** (this) | `CUSTOM_HANDLER_FUNCTIONS` (18 names), its pinning test `registered_handler_names_match_the_constant`, and the `is_known_function` / `known_functions` union with its manual dedup | `/admin/functions` catalogue, `UNKNOWN_FUNCTION` did-you-mean |
| **#46b** | `check_custom_inputs` + `custom_input_parse_check` (~100 lines) | ends the host-side `TemplateCompiler` approximation |

**Out of scope**, per the issue's own fence: input-field schemas, suggestion
policy, quarantine/rollback policy, catalogue endpoints. All host concerns.

## Two corrections to the issue's sketch

The proposal as filed cannot compile as written.

**1. `name: &'static str` cannot hold a custom name.** Custom handler names are
owned `String` keys in the registry. The field must borrow from the engine:
`name: &'a str`. `aliases` stays `&'static [&'static str]` — built-in alias
tables are static, and custom entries get `&[]`.

**2. `kind: BuiltinKind` needs a `Custom` variant, but that enum is
deliberately closed.** From `src/engine/functions/config.rs:222`:

> Deliberately **not** `#[non_exhaustive]`. A caller matching on this is usually
> deciding whether to accept a workflow definition, and if a third kind is ever
> added that decision needs revisiting — a compile error at every match site is
> the correct signal, not a silent fall-through to a `_` arm.

Widening it is therefore a breaking change for every downstream `match`.
Resolved by reusing the convention `builtin_function_kind` already establishes:
`Option<BuiltinKind>`, where `None` means *not a built-in — a registered custom
handler*. Additive, and a host that understands one understands the other.

## One over-specification, narrowed

The elaborated issue asks `check_workflow` to *"walk the step tree … or a
function name inside a guard clause escapes the check"*.

That risk does not exist for a **parsed** `Workflow`. Since 3.6.0 the parser
flattens the step tree into `Workflow::tasks` and records each group's span on
the task that opens it (`src/engine/task.rs:18`). Every leaf task — group
members included — is already in that flat vector, so iterating it cannot skip
one. The lesson Orion paid for applies to walking **authored JSON**, which is
#42's walker.

What genuinely survives for #46b is narrower: whether `CheckIssue.path` should
carry an authored coordinate (`tasks[1].tasks[0]`). That needs `group_starts`,
which is `#[doc(hidden)]` and documented as *"not part of the stable API"*.
Deferred to #46b, where it will be decided alongside #43's path format.

## API

```rust
/// One function name this engine will dispatch.
pub struct DispatchableFunction<'a> {
    /// The canonical name. Aliases are listed in `aliases`, not yielded
    /// as separate entries.
    pub name: &'a str,
    /// `Some(..)` for a built-in, `None` for a registered custom handler —
    /// the same convention as `builtin_function_kind`.
    pub kind: Option<BuiltinKind>,
    /// Other accepted spellings of the same function. `validate` carries
    /// `["validation"]`; everything else is empty today.
    pub aliases: &'static [&'static str],
}

impl EngineBuilder {
    pub fn dispatchable_functions(&self) -> impl Iterator<Item = DispatchableFunction<'_>>;
    pub fn can_dispatch(&self, name: &str) -> bool;
}

impl Engine {
    pub fn dispatchable_functions(&self) -> impl Iterator<Item = DispatchableFunction<'_>>;
    pub fn can_dispatch(&self, name: &str) -> bool;
}
```

### The one sentence that must be exactly true

> A name `can_dispatch` accepts will execute. A name it rejects fails with
> `DataflowError::FunctionNotFound` on the first message that reaches it.

Everything else in this design exists to keep that sentence true by
construction rather than by maintenance.

### Membership rules

Derived, never mirrored:

- **Self-contained built-ins** — always dispatchable. Derived by filtering
  `BUILTIN_FUNCTION_NAMES` through `builtin_function_kind`.
- **`RequiresHandler` built-ins** (`http_call`, `enrich`, `publish_kafka`) —
  dispatchable **iff** a handler is registered under that name. This is the
  `enrich` trap, closed structurally.
- **Custom names** — dispatchable iff registered. Yielded with `kind: None`.
- **Shadowed registrations** — registering a handler under a `SelfContained`
  name (`map`) is inert: the deserializer routes `map` to `FunctionConfig::Map`,
  which the crate executes itself without consulting the registry
  (`src/engine/task_executor.rs:65`). The name appears **once**, as a
  self-contained built-in. Documented as a gotcha, pinned by a test.

### Aliases without a second name list

`validate` is canonical — it is what `FunctionConfig::function_name()` reports
for `FunctionConfig::Validation` (`config.rs:392`). `validation` is its alias.

A table of canonical entries would be a mirror of `BUILTIN_FUNCTION_NAMES`, and
mirrors are the thing this issue exists to delete. Instead, two small
crate-internal functions carry the only new fact — the alias relation:

```rust
fn canonical_builtin_name(name: &str) -> &str;        // "validation" => "validate"
fn builtin_aliases(canonical: &str) -> &'static [&'static str];
```

Enumeration then yields a built-in only when it is its own canonical name, which
performs the grouping with no list to keep in sync.

### `can_dispatch` accepts aliases; enumeration does not yield them

`can_dispatch("validation")` is `true` — a task named `validation` really does
execute. `dispatchable_functions()` yields `validate` once, carrying
`["validation"]`. The two sets are deliberately different and both are
documented; a test pins the asymmetry.

## Implementation

### Placement

`DispatchableFunction` and the two alias helpers live in
`src/engine/functions/config.rs`, beside `BuiltinKind`,
`builtin_function_kind` and `BUILTIN_FUNCTION_NAMES` — the classification
vocabulary stays in one module. Re-exported from `src/lib.rs`.

### Shared core, so builder and engine cannot diverge

Both call the same crate-internal free functions, generic over the map's value
type to avoid importing `BoxedFunctionHandler` into `config.rs`:

```rust
pub(crate) fn dispatchable_functions_in<'a, V>(
    registry: &'a HashMap<String, V>,
) -> impl Iterator<Item = DispatchableFunction<'a>>;

pub(crate) fn can_dispatch_in<V>(registry: &HashMap<String, V>, name: &str) -> bool;
```

`TaskExecutor::has_function` (`task_executor.rs:158`) is rewritten to delegate
to `can_dispatch_in`, so the predicate the engine dispatches on and the
predicate hosts query are one definition. This is the structural fix — the
alternative is two copies that agree today.

### Plumbing

`Engine` reaches the registry through `workflow_executor → task_executor`.
Today's accessor returns an owned `Arc` clone, which cannot be borrowed from for
`impl Iterator<Item = DispatchableFunction<'_>>`. Add a borrowing sibling
alongside it on both `TaskExecutor` and `WorkflowExecutor`. Both are
crate-internal — no public change.

### Free by construction

`Engine::with_new_workflows` reuses the same `Arc<HashMap>` registry
(`src/engine/mod.rs:268`), so the enumeration survives hot reload with no extra
code. It still gets a test, because "free today" is not "guaranteed tomorrow".

### Ordering

Unordered, matching the documented stance on `BUILTIN_FUNCTION_NAMES`
(*"Ordering is not meaningful and may change without notice; treat this as a
set"*). Callers wanting deterministic output collect and sort. Documented on
both methods.

## Testing

Unit tests in `config.rs` for the pure functions; integration tests appended to
the existing built-in-classification section of `tests/public_api.rs`, which
already covers the `enrich` trap.

| Test | Pins |
|---|---|
| `enumeration_covers_builtin_function_names_exactly_once` | The acceptance criterion. Every name in `BUILTIN_FUNCTION_NAMES` is a canonical entry or an alias of exactly one, and never both. |
| `handler_less_enrich_is_not_dispatchable` | `can_dispatch("enrich") == false`, absent from enumeration, on builder and engine alike. |
| `registering_enrich_makes_it_dispatchable` | Both flip together; `kind` stays `Some(RequiresHandler)`. |
| `self_contained_builtins_dispatch_on_an_empty_builder` | No registration needed. |
| `registering_a_self_contained_name_is_inert_and_not_duplicated` | `map` appears once, as `SelfContained`. |
| `aliases_dispatch_but_are_not_enumerated` | `can_dispatch("validation")` true; enumeration yields `validate` with `["validation"]`. |
| `builder_and_built_engine_agree` | Same set before and after `build()`. |
| `enumeration_survives_hot_reload` | Same set after `with_new_workflows`. |
| `an_unregistered_custom_name_is_absent_and_the_workflow_really_fails` | Ties the prediction to observed behaviour, matching the style of the existing trap test. |

The last one is the load-bearing test: it is what makes the API's central
sentence a checked claim rather than a docstring.

## Documentation

- Rustdoc examples on both methods and the struct — compiled, per repo
  convention.
- `docs/src/built-in-functions/integrations.md:99-123` currently walks a host
  into the trap and stops at `builtin_function_kind`. Extend that passage to
  complete it, replacing the "reject every `RequiresHandler` name" advice with
  the real check.
- CHANGELOG entry under Added.

## Compatibility

Purely additive. No existing signature changes, no behaviour changes,
`build()` stays permissive on purpose. `BuiltinKind` is untouched, so no
downstream `match` breaks.

## Verification

Per `CLAUDE.md`, before hand-back:

```
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p dataflow-rs --all-targets -- -D warnings
cargo test --workspace --all-features
cargo test -p dataflow-rs
cargo +1.85 check --workspace --all-targets --all-features --locked
```

MSRV 1.85: nested `if let`, never let-chains. Test counts in `CLAUDE.md`
(495 / 420) move with this change and are updated in the same commit.
