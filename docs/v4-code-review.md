# Draft v4.0 code review

Reviewed on 2026-10-19 against the local `4.0.0-dev` working tree.

## Assessment

The main architecture is sound. Parsing, compilation and execution have
separate representations; programs are immutable; one writer owns context
changes; and host calls have transactional overlays. The best improvements
are removing unused state, keeping validation at the boundary that owns it,
and simplifying the debugger's state transitions. A broad engine rewrite
would put established ordering, compatibility and performance contracts at
risk without a demonstrated benefit.

The working tree already contained refactors in definitions, error types,
observation and wire readers, plus the deletion of `tech-debts.md`. Those
changes were preserved. This report describes the additional review and
improvements; it does not attribute the earlier work to this pass.

| Area | Assessment | Result of this pass |
| --- | --- | --- |
| Code quality | Strong behavioral coverage, but the draft did not build and had gaps in runtime-spec validation and deadline handling. | Fixed the build, validation and timer bugs; added regression coverage. |
| Maintainability | Good module boundaries; unnecessary bookkeeping remained in the debugger and condition table. | Consolidated engine ownership, made timer cleanup local to its effect, and removed unused condition metadata. |
| Unused code and overengineering | Some custom code duplicated existing library or local behavior. Other complex code enforces real contracts. | Replaced manual JSON escaping and duplicate XML traversal; retained the arena, overlays and exact wire reader. |
| Simplicity | Playback and workflow-state calculation used intermediate lists and multiple passes. | Replaced them with direct scans and simpler transitions. |

## Findings fixed

### 1. The draft failed to compile

`base/context.rs::fold_repeats` borrowed a vector mutably while its search
closure also indexed it immutably. Taking the current `(key, value)` pair
before searching the compacted prefix removes the conflicting borrow and
keeps first-key position / last-value behavior. Existing repeated-key tests
cover nested objects, arrays and wider objects.

The shared corpus driver also imported `WorkflowExit` through `observe`
after the existing refactor made that re-export private. It now imports
the type from the public `run` module.

### 2. Timer availability could be wrong after changing runtimes

`run/cancel.rs` cached timer availability once per thread. A host can enter
a runtime without a time driver and then a runtime with one on that same
thread, or leave Tokio entirely. The cached answer could disable deadline
racing or allow timer construction in an incompatible context.

A regression test reproduced the stale `false` result before the fix. The
probe now runs once per run when a deadline is present. Runs without a
deadline skip it. The test enters timed and untimed runtimes successively
and checks the result after leaving them.

The existing fallback for hosts without a Tokio time driver remains: limits
are checked around operations and calls. This change does not add timer
support to those hosts.

### 3. Runtime field specifications could be internally inconsistent

`Registry::try_register` accepted duplicate field names. `Templated::get`
uses the first matching field, while full resolution processes every field;
such a spec could therefore produce conflicting views of the config.
Registration now rejects duplicate names before replacing a registry entry.

It also rejects runtime field kinds that `Templated` cannot compile:
nested configs, lists, maps and another templated set. Previously those
specs registered successfully and failed later as an author's input error.
These checks use the existing registration validation path and error type.

Tests cover duplicate fields, preservation of an existing registration on
failure, and all four unsupported runtime kinds. Typed configs retain their
existing support for nested configs, lists and maps.

### 4. Compiled conditions retained unused authored paths

`CondSite.path` was read only by tests and required a production dead-code
exemption promising future reporting. Runtime reporting never used it.
The field and exemption are removed. Compilation still uses authored paths
when producing diagnostics; condition-table tests now check the expressions
and step identities the runtime actually consumes.

### 5. Serialization contained avoidable duplicate implementations

The derive crate's manual JSON string encoder is replaced by
`serde_json::to_string`, using an existing dependency. Default strings and
output paths continue to be encoded as JSON literals.

The XML writer no longer repeats top-level array handling:
`value_to_xml` delegates to `content`, whose array branch already implements
the same `<item>` representation for every nesting level.

### 6. Debugger navigation did excess work and mishandled boundaries

Playback navigation constructed every visible step index, then searched
that list for each movement. A direct scan now finds the next visible step
in the requested direction. The full index list remains available for the
toolbar, where it is needed.

This also fixes two behaviors:

- After manually selecting a hidden skipped step, forward navigation moves
  to the next visible step instead of restarting from the first one.
- Playback does not start when no visible step remains, and an exhausted
  playback pauses instead of leaving an interval running indefinitely when
  every step is filtered out.

Tests cover hidden-step navigation, empty and fully filtered traces, the
last-step boundary, and filtering with a previous-step or ready fallback.

### 7. Debugger resource ownership was unnecessarily distributed

Three refs separately held the engine, serialized workflows and factory.
One cache entry now holds them together. The entry is cleared before
disposal or replacement, so a failed rebuild cannot expose a freed engine.
The lifecycle regression test checks reuse, a failed replacement, recovery
and unmount disposal.

The playback interval now belongs to its effect's closure and is cleared
by that effect's cleanup. A ref and duplicate clearing branches are removed.
The run wrapper also no longer catches an error solely to log and rethrow
it; the existing caller handles and presents execution errors.

`getWorkflowState` now reads only steps up to the current position in one
pass, without first allocating a list of every index owned by the workflow.

### 8. Vite configs used a legacy directory global

Both configs now use `import.meta.dirname` instead of `__dirname`. The
application and library build successfully without the native-config-loader
warning previously emitted by Vite.

## Architecture and cleanup decisions

| Subsystem | Review conclusion |
| --- | --- |
| Definitions and compatibility | Keep the compatibility reader separate from the structural walk. Rewrites and diagnostics have different ordering responsibilities; combining them would complicate both. |
| Expression engine | Keep typed folded constants and borrowed results. They remove per-message conversion work and are part of the host API. |
| Wire reader | Keep the custom exact-value visitor. The tests demonstrate that the upstream value deserializer collapses whole-number floats; replacing the visitor would change round trips. |
| Compiler and lowering | Keep the small instruction set, side tables and centralized checks. Remove unused table fields instead of adding more speculative metadata. |
| State and paths | Keep the arena view, write log and overlay. They provide shared reads, transactional host calls and deterministic fan-out without copying a whole message per call. Their independent model tests are valuable. |
| Execution policies | Keep status classification in `policy::decide` and recording centralized. Duplicating those rules in built-ins or fan-out would increase maintenance risk. |
| Observability | Keep trace capture separate from audit retention. A trace must retain replay changes even when audit changes are disabled. |
| WASM and UI | Keep replay in Rust and wire declarations generated from Rust. The UI should consume those rules rather than implement a second replay engine. |
| Tests and tooling | Keep the independent corpus, golden comparisons, documentation tests, feature checks and example startup assertions. They test contracts that refactors can easily disturb. |

No public API was removed in this pass, and no dependency was removed on the
basis of text-search counts. Public extension points and feature-specific
dependencies cannot be classified as unused merely because the default
engine does not call them. Large files also contain substantial docs and
tests; line count alone is not a reason to split them.

## Remaining improvement candidates

These are follow-up candidates, not measured performance regressions.
They require representative measurements or a deliberate API decision.

| Priority | Evidence | Suggested next step |
| --- | --- | --- |
| Medium | `Trace::contexts` clones a whole context per step; the WASM `contexts` export collects them all. Mapping replay also starts from the initial context. | Measure long traces over large messages. If material, design an on-demand replay handle or checkpointed access before changing the wire API. |
| Medium | Default compatibility parsing clones the authored object before the walk copies retained fields. | Measure large definition loads. Consider an internal owned parsing path while keeping rewrite order and borrowed API behavior intact. |
| Medium | `Ctx::input` redaction and conversion failures inspect raw expression JSON for secret references; `constant_value` converts typed constants to owned values. | Profile real host calls before adding cached flags or borrowed erased values. Preserve every secret-isolation test if changed. |
| Low | `Templated::get` scans field names; full resolution owns each result key. | Measure plugin configs with many fields. Introduce field handles only if repeated lookup justifies a new API. |
| Medium | The demo build reports large bundles: approximately 4.1 MB of datalogic JavaScript and 3.6 MB of dataflow WASM before compression. The library externalizes its dependencies. | Investigate lazy loading editor/debugger views in the application. Keep application bundle work separate from the library's packaging contract. |

Use the existing interleaved benchmark gate for performance changes. This
pass makes no throughput or memory-improvement claim from source inspection.

## Sequential follow-up work

### 1. Large-trace replay: completed

Added `observe::Replay` and the WASM `TraceReplay` class. They retain a
trace and one working context, applying only intervening steps for forward
seeks and restarting from the initial context for backward seeks. Mapping
access copies only the requested mapping context. The snapshot APIs remain
available, and the wire trace shape is unchanged.

The UI uses a cursor instead of retaining a context array. It releases the
cursor when clearing or replacing the run and on unmount. The draft UI
state field changes from `contexts` to `replay`; detail views use the
provider's `contextAt` and `mappingContextAt` readers.

`wasm/scripts/bench-replay.mjs` compares both APIs on the same WASM build in
separate Node processes. For 256 steps over a 256 KiB context, three
interleaved runs measured median retained JS heap growth of 63.02 MiB for
snapshots versus 0.27 MiB for the cursor, and RSS growth of 333.16 MiB versus
8.56 MiB. Timings varied substantially, so these measurements establish the
memory reduction, not a stable speedup. Backward seeks deliberately trade
replay work for bounded retained memory.

The cursor is compared with every corpus trace in both directions and at
every recorded mapping boundary. Native tests, Clippy, rustdoc, WASM tests,
UI tests and both UI builds pass. The WASM artifact verifier also requires
the new class declaration.

### 2. Definition parsing copies: completed

Added `def::parse_owned` and `EngineBuilder::workflow_json_owned`. The owned
and borrowed entry points share the same compatibility reader and walk
through a `Cow` over the input object. Owned JSON is rewritten in place,
removing one full-document clone. The WASM constructor, check and parse
entry points now consume the JSON they already own.

Existing parsing and compatibility tests compare both entry points, including
refused inputs and exact issue ordering. A builder test compares errors after
parsing and compilation. Native, WASM and UI suites pass.

`wasm/scripts/bench-definition-loading.mjs` loads a 1 MiB definition with 64
tasks 100 times. Five interleaved baseline/candidate runs had medians of
464.64 ms and 478.46 ms; the median paired ratio was approximately 1.03,
within the observed noise. This confirms no reliable end-to-end speed
claim; the improvement is removal of the redundant owned-input copy. The
borrowed API still copies when compatibility rewriting requires ownership.

## Validation

The review uses the existing behavioral suites, plus regressions for the
timer cache, runtime specs, debugger navigation and engine lifecycle.

- Workspace tests with all features and root tests with default features.
- Separate `datetime`, `tensor` and `budget` library/integration tests.
- Workspace and default-feature Clippy with warnings denied.
- WASM-target Clippy and `wasm-pack test --node` (25 tests).
- Rust documentation build with warnings denied and default-feature book/README doctests.
- UI lint, tests (25 tests), library declarations/build and application build.
- A freshly built and verified local WASM package overlaid for UI validation.
- Artifact version agreement, benchmark-harness tests and example smoke checks.
- Formatting and whitespace checks.

The UI application still emits its large-chunk advisory. No A/B throughput
benchmark, deep state-model run, published-package installation test or
downstream Orion integration was performed. The state implementation was
not changed. This review is not a release certification.
