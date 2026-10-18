# Tech debt register: dataflow-rs

- **Snapshot:** `main` @ `874827f` (4.0.0-dev), 2026-10-17; pruned at
  `38de619` and `8e1a568`, 2026-10-18, which fixed the items listed under
  Status; every remaining entry re-verified against `8e1a568` the same day
- **Scope:** the engine (`src/`), `derive/`, `wasm/`, `corpus/`, `tests/`,
  `examples/`, `bench/`, CI and release workflows, `docs/`, `ui/`
- **Method:**
  - A `/code-review` pass over `src/`, `derive/` and `wasm/src/` looked for
    correctness defects.
  - Seven read-only audits covered the subsystems: `def`+`compat`, `expr`,
    `function`+derive, `program`+`run`, `state`+`observe`+`base`, tests and
    corpus, and tooling/CI/UI. Each finding had to quote the code at a
    `file:line`.
  - Overlapping findings were merged, and the high-impact claims were
    re-checked by hand against the source.
- **Not done:** nothing was built, run or benchmarked. Performance items are
  reasoned from the code and need a gate run (`bench/gate.py`) to confirm
  them before anyone acts on them.

### How to read an entry

| Field | Meaning |
|---|---|
| **Severity** | **Critical**: wrong output or a broken release today. **High**: silent-divergence risk, or something the 4.0 API freeze makes permanent. **Medium**: a recurring cost or a likely future bug. **Low**: hygiene. |
| **Effort** | S ≤ ½ day · M ≤ 3 days · L > 3 days |
| **Verified** | ✔ re-read against the source. Every entry still listed was re-read at `8e1a568` on 2026-10-18, with its code quoted at `file:line`. (◐, "quoted by one audit agent only", no longer applies to any entry.) |
| **Freeze** | 🔒 changes public API, wire shape or semver surface, so it is free only before the 4.0.0 tag (phase 8). |

Each entry gives the reasoning (*why this is debt*) and the justification
(*why fix it, and why this way*). Designs that CLAUDE.md's "Rules that must
hold" pin on purpose are **not** listed as debt. An entry only appears when
the *implementation* of such a rule is duplicated, unenforced or leaky.

---

> **Status:** `38de619` fixed and removed CR-1 to CR-5, REL-1, KO-1, KO-2
> and API-1's dead variants. The second batch (`8e1a568`, 2026-10-18)
> fixed and removed every High item: VB-1, DX-1, TST-1, TST-2,
> CI-1, DUP-1 (with CR-6), DUP-3 (PERF-1 withdrawn), DUP-5, DUP-6 and DEP-1.
> Their entries are gone, and so is the plan; the CHANGELOG's 4.0.0
> section records the visible ones. The gate against `38de619` (five
> rounds, all 13 benchmarks) reads every instructions-retired ratio within
> 0.7%, and every timed row not slower once `benchmark`'s spawned
> throughput was re-run at seven rounds. What the fixes found is registered
> below, marked *(found 2026-10-18)*. DEP-4 (datavalue's `==` over a key
> held twice) was fixed upstream in datavalue-rs 0.3.2, which the engine
> takes with datalogic-rs 5.8.2. VB-5 (a key twice in a context) is fixed
> and removed: the wire reader refuses an object holding a key twice, and
> `parse_json` folds one to its first position and last value, as 3.x and
> `parse_xml` read one. A host-built value can still hold one.
>
> **Re-verified at `8e1a568` (2026-10-18).** Every remaining entry was
> re-checked against the source. No whole entry is fixed; every location
> was corrected where it had moved, and the partial fixes are recorded in
> place: PERF-12's throwaway `Engine` in `build()` (removed from the row),
> DX-2's documentation, TST-3's single integration golden module, TST-8's
> `ran >= 60`, TST-9's weekly deep job, DOC-1's `message.rs` hit, DUP-9's
> `builtins/tests.rs` tables and private `CHECK_EVERY`, and API-4's public
> `def::is_reserved_function`.

## 0. Executive summary

### Overall assessment

The engine is in good structural health for a codebase under construction:

- `#![forbid(unsafe_code)]`.
- Almost no panics on library paths outside `#[cfg(test)]` and documented
  invariants.
- Two duplicate crates in the lockfile.
- A strict lint and doc gate, and an MSRV check in CI.
- Typed, versioned fingerprints.
- A single-writer state design backed by fault-injected model tests.

The debt is not sloppiness. It follows from the way 4.0 was built (fast,
phase by phase, against a v3 oracle that has since been deleted). It falls
into six themes:

1. **One rule, several hand-synchronised copies.** The path rules now have
   one statement (`expr::rules`), with two hand-inlined hot walks held to it
   by an exhaustive test. Call settlement, the recorder's ordering,
   expression evaluation, the raw-JSON expression walks, the issue-code
   tables and the progress writers are each still written 2–6 times. This
   is the largest source of future silent bugs, and it contradicts the
   project's own principle that "each rule lives once in the code".
2. **Parse misses a few rules.** Definitions are read-only now (VB-1
   fixed), so every compiled definition has passed `parse`. But `parse`
   accepts a rollout range that matches no traffic (VB-2), and a host's
   spec mistakes surface as the author's `INPUT_PARSE` (VB-3).
3. **The 4.0 public surface still carries scaffolding.** Unwritten
   `ErrorRecord` retry fields, records with no ids, stringly-typed `Rewrite`,
   raw re-exports of 0.x/5.x dependency crates, overlay internals in
   `testing`, and `String`-heavy `ErrorRecord`. All of these become semver
   commitments at the phase-8 tag.
4. **The tests pin more than they own.** Goldens pin third-party error
   text, fixtures are written up to 11 times with three argument orders,
   and the property tests run fixed seeds. (The corpus's expectations are
   enforced and CI runs every example at smoke counts, since `8e1a568`.)
5. **Developer experience gaps in diagnostics.** A deadline panics off
   Tokio. Wasm cancellation differs from native and is untested. A run
   limit travels as an untyped `Error`.
6. **Provenance rot.** About 90 references to the deleted `design-v4/`
   documents (I-numbers, "break N", "phase N", "T2-C3"), four in public
   rustdoc. There are also comments naming v3 symbols that are no longer
   in the tree, and hand-maintained test counts in CLAUDE.md that have been
   edited 61 times.

### Top items by priority

No High item is open. Ranked on 2026-10-18 after re-verification. Items the
4.0.0 tag makes permanent come first, ordered by Orion's priorities
(performance, then developer experience, then a compact engine). Cheap
silent-failure fixes come next.

| # | Item | Severity | Effort | Why now |
|---|---|---|---|---|
| 1 | **The `ErrorRecord` wire shape, as one change:** [API-5](#api-5) ids on every record, [API-3](#api-3) `Arc<str>` ids and one stamp, [API-1](#api-1) drop or document the retry fields | Medium | M | All three change message bytes and `bindings/dataflow.d.ts`, which becomes permanent at the tag 🔒. API-3 is also a per-record allocation on validation-heavy runs (performance comes first). Doing them together means one bindings regeneration, one CHANGELOG line and one Orion follow-up. |
| 2 | [API-2](#api-2) Dependency types on the public API | Medium-High | M (was L) | Becomes semver at the tag 🔒. Orion's check settles the largest question: keep both re-exports. What is left is sealing the `#[doc(hidden)]` `ResolveExpr`/`EvalRoot` methods, settling the `OperatorInfo`/`Family` clash, and documenting the commitment. |
| 3 | [API-4](#api-4) Stringly or leaky public types | Low-Medium | S each | Free only before the tag 🔒. First: `try_register` (Orion's plugin manifests), `testing::Write`, the built-in configs to `pub(crate)`, one canonical path per item, and the `PARSE_FAILED` split. |
| 4 | [DOC-1](#doc-1) Design-doc IDs in public rustdoc | Medium | S | Four hits ship to docs.rs at 4.0. A CI grep stops new ones. |
| 5 | [DUP-8](#dup-8) `IssueCode` as a literal union in the bindings | Low | S | A bindings change 🔒. Do it after API-4's new codes so they land in the union. |
| 6 | [VB-2](#vb-2) A rollout range that matches no traffic loads cleanly | Medium | S | A silent blackhole on a routing path. The check already exists, uncalled. A new Defect is easier to ship before Orion pins 4.0. |
| 7 | [DX-2](#dx-2) A deadline panics outside a Tokio runtime with timers | Medium | S | A crash, not an error. The documentation half is done. |

### Metrics snapshot

| Metric | Value |
|---|---|
| Rust under `src/`, `derive/src`, `wasm/src`, `corpus/src` | ≈37.8k lines (inline tests included; `record.rs` is about half tests) |
| Integration tests | 3 binaries (`it`, `secrets_isolation`, `ts_types`), ≈23.8k lines, plus 83 corpus cases and their 83 goldens |
| `unsafe` | 0 (`forbid(unsafe_code)`) |
| Panics in non-test library code | A handful, each a documented invariant (`link`, `registry`, interp `unreachable!`) |
| Duplicate crates in `Cargo.lock` | 2 (`bitflags` 1/2, `syn` 2/3) |
| Lint suppressions in `src/` | 6, all with `reason =` |
| References to the deleted `design-v4/` | ≈92 lines across 46 files, 4 in public rustdoc |
| CI feature configurations | 12, run serially in one job |
| UI | ≈9.5k lines TS/TSX, 10 tests |

---

## 1. Correctness defects (from `/code-review`)

These are bugs rather than debt. None was confirmed by running code.

<a id="cr-7"></a>
### CR-7 · `parse_json` source: `"data."` names the root, but `"metadata."` and `"temp_data."` name the key `""`
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **Where:** `src/function/builtins/paths.rs:85-97` (`ParseFrom::parse`); the
  target-side twin is `DataPath::resolve` (`paths.rs:236-238`), where target
  `data.` resolves to `data.data.""`.
- **Reasoning:** the v3 "empty remainder names the root" rule is special-cased
  for `payload.` and `data.` only, so spellings that look equivalent behave
  differently.
- **Remediation:** apply the rule to all three roots, or refuse a trailing `.`,
  on both the source and the target side.

<a id="cr-8"></a>
### CR-8 · Trace replay accepts changes that produce invalid contexts, and skips unparsable paths silently
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **Where:** `src/observe/trace.rs:127-196` (`contexts`, `apply`; the silent
  skip at `:178-180`); wasm `read_trace` (`wasm/src/lib.rs:685-705`) checks
  only `mapping_ends`.
- **Reasoning:** an edited or foreign trace can remove `data` or set
  `metadata: 5`, and `contexts()` returns it rather than throwing
  `INVALID_TRACE`. A malformed change path is skipped (`let Ok(at) = … else
  { return; }`), so the replay diverges without any signal.
- **Remediation:** validate root writes and path parses during replay, and
  surface failures as `INVALID_TRACE`.

---

## 2. Duplicated rules (hand-synchronised copies)

The project's stated principle is "each rule lives once in the code". Each
item below breaks that principle. The common justification for all of them:
every copy is a place where a future change can land in only N−1 of N
copies, and the tests that hold the copies together are either randomised
(they can miss a case) or hand-written (they don't grow with the tables).

<a id="dup-2"></a>
### DUP-2 · Call settlement and the recorder's ordering are written 3–5 times
**Severity:** Medium · **Effort:** M · **Verified:** ✔

- **Where:**
  - Settlement: `src/run/host.rs:245-262`, `src/run/fan_out.rs:209, 277, 320-349`, `src/run/interp.rs:655-702`, `src/function/testing.rs:325-346`
  - Recorder: `src/run/record.rs:172-196` (`plain`) and `:240-352` (`watched`)
- **Reasoning:**
  - The settle rules are duplicated in each copy, and the copies order their
    statements differently:
    - read `errors_before` before keeping the call's records;
    - compute `marked` after apply;
    - drop the overlay on `Err`.
  - `Completed {…}` is built by hand in five places.
  - The recorder's mandatory order (audit, progress, error records, mirror,
    step, event) is coded twice.
  - The fast path's correctness rests on
    `debug_assert!(env.watched || (!env.watch.timed && env.capture == env.capture_audit))`
    (`engine.rs:286-288`), which release builds never check.
  - `testing::call` publicly promises to run "through the path a run takes",
    but keeps its own copy of the settle. If the copies drift, host unit
    tests pass on behaviour production doesn't have.
- **Justification:** a policy change, say to which records the mirror sees,
  must land identically in every copy. This is the most likely place for an
  ordering regression.
- **Remediation:**
  - `fn settle(rc, env, Invoked, element: Option<usize>) -> Settled` and
    `Completed::new`, used by host, fan-out and `testing`.
  - `record_core(...)` for the first three recorder steps, which both paths
    call.
  - Make `CaptureMode` an enum computed once per run, so the `debug_assert`
    becomes a type.

<a id="dup-4"></a>
### DUP-4 · Expression evaluation ("lend constant, else arena eval and convert") written six times
**Severity:** Medium · **Effort:** S · **Verified:** ✔

- **Where:** `src/expr/engine.rs:205-263` (`eval`, `eval_in`), `src/function/sync.rs:102-134`, `src/function/ctx.rs:412-435`, `src/function/builtins/map.rs:323-330, 365`.
- **Reasoning:** the copies have already diverged. Only
  `ExprEngine::eval` uses `evaluate_metered` and reports ops; the run path
  drops the count.
- **Justification:** metering, error classification and arena discipline
  should each have one home. Tracing or limits added later would otherwise
  need six edits.
- **Remediation:** one crate-private `ExprEngine::eval_view<T>(&Expr<T>,
  root, arena)` primitive that `Ctx`, `SyncCtx` and `map` all call.

<a id="dup-7"></a>
### DUP-7 · Expressions are analysed by four walks over raw JSON, parallel to the compiled `Logic`
**Severity:** Medium · **Effort:** M–L · **Verified:** ✔

- **Where:** `src/expr/walk.rs:12-60` (`escaped_keys`, `secret_refs`, `reads_secret`); `src/expr/document.rs:318-345` (its own `var` parsing and payload test); `src/program/checks.rs:103-118, 287-352` (`classify`, `walk_payload`, `payload_read`: a second `var`/`val` parser).
- **Reasoning:** each walk re-derives what the compiler knows (operators
  used, paths read under which scope frame) and re-implements
  templating-mode quirks. `checks.rs`'s `per_element` copies datalogic's
  `frames_pushed_for_child`, and its unit tests use a hand-written predicate
  rather than the real engine, so the copy is tested against itself.
- **Justification:** the `PAYLOAD_VAR` and secrets lints are safety rules,
  and a drift makes them quietly weaker.
- **Update (2026-10-18):** the engine is on datalogic 5.8.2, and
  `per_element` now reads datalogic's own `scoped_arg`, tested against the
  real engine's operators. The other walks remain. A gap found on the way:
  `missing`, `missing_some` and `exists` on a `payload.` path escape
  `PAYLOAD_VAR`, since only `var` and `val` are walked.
- **Remediation:** derive reads and operators from `Logic::facts()`, which
  also closes the `missing`/`exists` gap. Keep one raw walk only for what
  datalogic can't see (escaped keys, `secret`). Merge `reads_secret` into
  `secret_refs(..).next().is_some()`.

<a id="dup-8"></a>
### DUP-8 · `IssueCode` is maintained in five places; the TypeScript type is a bare `string`
**Severity:** Low · **Effort:** S–M · **Verified:** ✔

- **Where:** `src/base/issue.rs` (the enum, `ALL: [Self; 38]` at `:350`, `as_str` `:392`, `severity` `:441`, the test's `ordinal` `:764`, the `ts` declaration `|_cfg| "string"` at `:714-719`), the docs table, and `bindings/dataflow.d.ts:435` (`export type IssueCode = string;`).
- **Reasoning:** adding a code means six edits. `as_str`'s arm order differs
  from `ALL`'s. TypeScript consumers lose the closed set that `Severity`
  already gets as a literal union.
- **Remediation:** a `macro_rules!` table with a mandatory severity per row,
  which keeps the deliberate no-wildcard guard. Emit a literal union under
  `ts`.

<a id="dup-9"></a>
### DUP-9 · Smaller duplications
**Severity:** Low · **Effort:** S each · **Verified:** ✔

- **Two size estimators:** `src/state/size.rs:9` (`NODE = 32`, hand
  constant) vs `src/run/trace.rs:24, 289-305` (`size_of::<usize>()`, an
  uncapped walk per change). Derive the constant from `size_of`, share one
  walker, and cap the trace walk at the remaining budget.
- **Two clocks:** `record.rs:82-86` uses `Instant::now()` and `cancel.rs:135-147`
  uses Tokio's clock, each with its own `CLOCK_READS` thread-local. Under
  paused time, elapsed timings and the deadline read different clocks. Use
  one `clock` module.
- **Four ways to write `metadata.progress`:** `Edit::overwrite<const N>`
  (one caller, contract checked only by `debug_assert`), `engine_set`,
  `engine_update`, and record's in-place/in-view writers (`state/run.rs:284, 547, 567`;
  `run/record.rs:459, 511, 538`). Narrow these to one `overwrite_progress`, and
  write down which benchmark justifies each path.
- **"Trivially true" has two definitions:** the `UNGUARDED_VALIDATION` lint
  tests the raw literal (`checks.rs:356-358`), while lowering tests the folded
  value (`compile.rs:484`). So `{"==":[1,1]}` counts as a guard but guards
  nothing. Feed the lint from the compile walk.
- **The `for_each` message is built by editing the loop's text**
  (`for_each.rs:59`, `problem.replacen("loop over", "for_each over", 1)`).
  Pass the owner name instead.
- **Constants re-spelled:** `"$"`/`"$$"` (`walk.rs:27`) vs the private
  `TEMPLATE_KEY_ESCAPE` (`engine.rs:18`); `"data" | "metadata" | "temp_data"`
  (`path.rs:150-152`) vs `base::ROOTS`; `Root as usize` indexing `ROOTS` by
  declaration order (`path.rs:46`).
- **The 64-op check interval** is now one private `const CHECK_EVERY`
  (`interp.rs:197`), but the number is still restated in prose
  (`interp.rs:17, 128, 201`, `cancel.rs:5, 263`, the public `options.rs:26`
  doc) and as `tests/it/deadline.rs:283`'s `63`. Make it a `pub const`, link
  to it from the docs, and derive the test from it.
- **Left from the second batch** *(2026-10-18)*: the path rules are stated
  once in `expr::rules`, but the two hot walks (the owned `path::set`/`get`
  and the arena splice) stay hand-inlined, since asking `rules::step` per
  segment cost `map`-heavy benchmarks 1.5 to 2% of their instructions; an
  exhaustive test holds both to the rules' reference walk. `merge_root` is
  still written twice (owned `path.rs:475`, arena `splice.rs:266`, plus
  `state/run.rs:386`); and `def/keys.rs:135` spells `map`'s and
  `validation`'s element tables by hand, since `FieldSpec` carries no nested
  fields (`builtins/tests.rs` now derives its expectations from
  `Mapping::fields()`/`Rule::fields()`).

---

## 3. Validation boundary

<a id="vb-2"></a>
### VB-2 · A single workflow's rollout range is never validated
**Severity:** Medium · **Effort:** S · **Verified:** ✔ (`validate_set` has no non-test caller)

- **Where:** `src/def/parse.rs:756-782` (`Walk::rollout`, `bucket`); `src/def/rollout.rs:140-164`.
- **Reasoning:** parse checks only that each bound is a `u8`. So
  `{"bucket_start":60,"bucket_end":20}` or `{"bucket_end":200}` loads
  cleanly and silently matches no traffic: the "blackhole" that rollout.rs's
  own docs warn about. The per-range check exists, but only inside the
  public `validate_set`, which nothing calls.
- **Remediation:** extract `Rollout::check()`, call it from `Walk::rollout`,
  and report a Defect at `rollout`.

<a id="vb-3"></a>
### VB-3 · Host spec mistakes surface late, and get blamed on the workflow author
**Severity:** Medium · **Effort:** S–M · **Verified:** ✔

- **Where:** `src/function/templated.rs:184-185, 287-296`; `derive/src/lib.rs`; `src/function/registry.rs:179-206` (`check_spec`).
- **Reasoning:**
  - A `Templated` runtime default that isn't JSON, an unsupported runtime
    field kind, or a derive `default = "abc"` on an `Expr<u64>` is reported
    as the *author's* `INPUT_PARSE` on every compile that leaves the key out.
    (The derive now refuses a non-JSON `default_json` at macro time,
    `json_text` at `:390-400`, but does not type-check a `default` against
    its field.)
  - `register` already panics on other spec bugs ("best found once") but
    skips these.
- **Remediation:** in `check_spec`, compile each declared default through its
  field kind once with a scratch `CompileCx`, and fail at `register`.

<a id="vb-4"></a>
### VB-4 · `holds_steps` accepts any path ending in `tasks`; `null` means different things on different keys
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **`holds_steps`** (`src/def/parse.rs:164-170`): `check_steps(steps,
  "loop.tasks")` or `"rollout.tasks"` is accepted, and its issues are
  reported at paths no definition can have. Require the grammar
  `(loop.setup | tasks)([i].tasks)*`.
- **`null` semantics:**
  - Treated as absent on `name`, `loop`, `rollout` and `for_each`.
  - `PARSE_FAILED` on `channel`, `priority`, `tags` and `ext`.
  - Refused on `over`.
  - Kept on `condition` as a falsy expression, which silently disables the
    step.

  Document one policy, and add at least an Advisory for a literal `null`
  condition.

---

## 4. Public API and wire surface before the 4.0 freeze 🔒

Every item here is free to change today and a semver break after the
phase-8 tag. That timing is the justification common to the whole section.

<a id="api-1"></a>
### API-1 · `ErrorRecord`'s retry fields are never written; two issue-code docs are wrong
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **Where:** `ErrorRecord::retry_attempted`/`retry_count` (`src/base/error.rs:199, 203`;
  `bindings/dataflow.d.ts:159, 163`), written only by `tests/ts_types.rs:306-307`;
  the `GroupContinueOnError` (`src/base/issue.rs:221-222`) and `ParseFailed`
  (`:306-307`) doc comments.
- **Reasoning:** v3 had the fields and never wrote them either (its
  `retry.rs` says no engine path acts on them), so they are wire shape with
  no producer. `GroupContinueOnError` says "reported by `Compiler::check`",
  but the reader emits it; `ParseFailed` describes v3's serde path.
- **Remediation:** decide with [API-3](#api-3): drop the fields if no stored
  3.x message needs them to read back byte for byte, else say so in their
  docs. Correct the two doc comments.

<a id="api-5"></a>
### API-5 · `validation`'s records and a host's own records carry no ids *(found 2026-10-18)*
**Severity:** Medium · **Effort:** S · **Verified:** ✔ (corpus goldens) · **Freeze:** 🔒

- **Where:** `src/function/builtins/validation.rs:33, 69-76` (`ErrorRecord::new`,
  "the records carry no ids, as v3's do"); `Ctx::record_error`
  (`src/function/ctx.rs:574-576`); `src/run/host.rs:187-212`
  (`stamp_record` fills only `timestamp` and `element_index`).
- **Reasoning:** every other record (`TASK_STATUS_ERROR`, `MAPPING_ERROR`,
  `filter`'s `EVALUATION_ERROR`, a classified `Err`) carries
  `workflow_id` and `task_id`; `VALIDATION_ERROR`, validation's
  `EVALUATION_ERROR` and a host's `Ctx::record_error` do not. A consumer
  grouping errors by task (Orion's summaries, the debugger) has to match
  them by position. v3 byte compatibility is the only reason, and 4.0
  already changes the error list.
- **Remediation:** stamp the ids on every record (one Changed line), or
  record the v3 reason in the docs and keep it.

<a id="api-2"></a>
### API-2 · Raw re-exports of datalogic-rs 5.x and datavalue 0.x; dependency types in signatures
**Severity:** Medium-High · **Effort:** M (was L; Orion settles the re-exports) · **Verified:** ✔
- **Validated (Orion):** Orion's custom operators need `dataflow_rs::datalogic_rs` at O14, and use `datavalue` in 37 files. Keep both re-exports and commit to them; don't drop them. Checked against Orion `fafd753d` on 2026-10-17.

- **Where:** `src/lib.rs:52-53` (`pub use datalogic_rs; pub use datavalue;`); the `#[doc(hidden)]` (still semver) `EvalRoot::view_in` (`src/expr/engine.rs:68-71`) and `ResolveExpr`'s `logic()`, `constant_value()` and `value_of(&DataValue)` (`src/expr/expr.rs:325-336`); `ExprEngineBuilder::operator(impl CustomOperator)` (`engine.rs:304`); `OwnedDataValue` in `Secrets` (`secrets.rs:43, 55`), `Resolver` (`config.rs:341`), `ConfigField::resolve`, `FromValue`.
- **Reasoning:**
  - Any major bump of either dependency is a major bump of dataflow-rs 4.x.
  - The MSRV is already "inherited, not chosen" from them.
  - datalogic 5.8 has its own public `OperatorInfo` and `Family`, which
    clash by name with `dataflow_rs::expr::{OperatorInfo, Family}`
    (`src/expr/operators.rs:10, 88`, wrapping datalogic's through
    `from_logic`/`Family::of`) for hosts that import both.
- **Justification:** this is the largest semver lever in the crate, and it
  is set implicitly.
- **Remediation:** decide deliberately, before 4.0.0:
  - Keep `OwnedDataValue` as a committed type, and document the coupling.
  - Wrap `CustomOperator` behind a dataflow trait.
  - Seal the doc-hidden methods so they expose no datalogic types.
  - Drop the wholesale `pub use datalogic_rs` unless a host truly needs it.

<a id="api-3"></a>
### API-3 · `ErrorRecord` is five owned `String`s; `Decision`'s `Cow` saves nothing
**Severity:** Medium · **Effort:** M · **Verified:** ✔

- **Where:** `src/base/error.rs:177-203`; `src/run/policy.rs:44, 132-142, 323`; timestamp stamping in four homes (`sync.rs:147`, `builtins/validation.rs:70`, `host.rs:192-193`, `record.rs:371`).
- **Reasoning:**
  - Each record allocates its ids and copies the run's timestamp text again.
  - `AuditEntry` already shares `Arc<str>` ids, so the two types disagree.
  - `Cow::Borrowed(TASK_STATUS_ERROR)` is turned into a `String` anyway
    (`policy.rs:323`, `code.into_owned()`).
  - Validation-heavy workloads pay this per rule, and traced runs pay it
    twice (`record.rs:338`, `to_vec()`).
- **Remediation:** use `Arc<str>` ids and timestamp (one per run), stamp in
  one function, and regenerate the bindings.

<a id="api-4"></a>
### API-4 · Stringly or leaky types on the public surface
**Severity:** Low-Medium · **Effort:** S each · **Verified:** ✔

- **`compat::Rewrite { v3: String, v4: String }`** (`src/def/reader.rs:66-74`)
  is human prose. A host that wants to auto-fix a stored definition has to
  parse it. Replace it with a `RewriteKind` enum plus `Display`.
- **`compat::Migrated`** returns only a `Value` (`compat/mod.rs:73-92`) and
  drops the `WorkflowDef`. (With `preserve_order` on, the `Value` now keeps
  `ser.rs`'s order.) Add `pub def` and return text.
- **`PARSE_FAILED`** covers type errors, mixed v3/v4 spellings and `ext`
  collisions, so hosts can only tell them apart by message text. Add
  `MIXED_SPELLING` and `EXT_COLLISION`.
- **`testing::Write`** re-exports overlay internals (`testing.rs:42`;
  `Blocked(OwnedDataValue)`, `src/function/overlay.rs:42-53`), so restructuring the overlay would become
  a breaking change. Give `testing` its own type.
- **The built-in configs** (`MapConfig` and the rest) are `pub` with
  `#[non_exhaustive]` inside a private module. That suggests a contract that
  doesn't exist. Make them `pub(crate)`.
- **One item, several paths:** `crate::Error`/`run::Error`,
  `crate::Issue`/`program::Issue` (produced by `def`), `observe::RunStatus`
  re-exported from `run`. Pick one canonical home each.
- **`Change`** carries a `pub(crate) trace_only` flag, which forces a manual
  `PartialEq`/`Debug` that must list every wire field (`base/change.rs:43-68`);
  a new field missed in `eq` compares equal. `Error`'s `Debug` drops `source`
  and uses `.finish()` instead of `finish_non_exhaustive()` (`error.rs:153-160`).
- **No `Registry::try_register`** (`registry.rs:106-111` panics). The
  reserved set is now public as `def::is_reserved_function`
  (`def/mod.rs:79`), so a host can pre-check, but a fallible register is
  still missing.
- **The derive detects outputs by type name** (`derive/src/lib.rs:296,
  322-335`). An explicit `#[output]` attribute now marks an alias, but an
  unrelated type named `Output` is still auto-detected as one. Add
  `ConfigField::IS_OUTPUT` and a const assertion per field.
- **`Templated` is special-cased by `TypeId`** (`registry.rs:113`,
  `erased.rs:120`). Use an associated const on `FunctionConfig`.
- **`PublishConfig`** (`builtins/publish.rs:16-31`) is shared by `publish_json` and `publish_xml`, so each
  silently accepts the other's field (`pretty`/`root_element`). Split the
  config, or emit an Advisory.

---

## 5. Developer experience and diagnostics

<a id="dx-7"></a>
### DX-7 · The fan-out's `into` pre-fill is in no audit entry *(found 2026-10-18)*
**Severity:** Low · **Effort:** S · **Verified:** ✔ (`fan_out/fold_halt`'s golden)

- **Where:** `src/run/fan_out.rs:287-299` (the `null` pre-fill, a trace
  prelude only); `AuditMode`'s doc (`src/run/message.rs:37-48`) does not
  mention it.
- **Reasoning:** a halted fan-out leaves `into` as `[0, 1, null, null]`
  while the audit trail holds only `.0` and `.1`, so replaying the audit
  does not rebuild the context. Documented, and v3-compatible, but a host
  rebuilding state from the audit trail meets it.
- **Remediation:** document it beside `AuditMode`, or record the pre-fill
  as a `Change` the audit keeps (a Changed line).

<a id="dx-2"></a>
### DX-2 · A run with a deadline panics outside a Tokio runtime with timers
**Severity:** Medium · **Effort:** S · **Verified:** ✔ (`cancel.rs:112`)

- **Where:** `src/run/cancel.rs:112` (`tokio::time::sleep_until`); `src/run/host.rs:152`.
- **Reasoning:** `tokio::time::sleep_until` panics without a time driver.
  `futures::executor::block_on`, smol, or a Tokio runtime without
  `enable_time` crashes at the first host call with a deadline. The wasm
  build shows that "checks only, no race" works as a mode.
- **Done:** the requirement is now documented (`src/run/options.rs:25-31`).
- **Remediation:** use `Handle::try_current()` with a timer probe, fall back
  to checks only (logging once), and test it under `block_on`.

<a id="dx-3"></a>
### DX-3 · Native and wasm cancellation diverge, and the wasm side is untested
**Severity:** Medium · **Effort:** S–M · **Verified:** ✔

- **Where:** `src/run/cancel.rs:96-153`; `src/run/host.rs:151-152`; `wasm/tests/web.rs:372` (the only wasm limit test: `timeout_ms: 0` on a built-in loop).
- **Reasoning:**
  - On wasm a host call that overruns is never cut short, while natively the
    run ends `TimedOut` mid-call.
  - The wasm package registers no host functions and exposes no token, so
    the wasm `race`, `clock()`, mid-run deadline expiry and the "`Err` after
    the limit" rule are never exercised.
  - There are also small redundancies: a double `is_set` check
    (`host.rs:151`, `cancel.rs:101`), and an ungated `Box::pin` of a no-op
    race on wasm (`host.rs:152`, `cancel.rs:126-132`).
  - `RunOptions::deadline`'s doc (`options.rs:27`) only implies the
    divergence; the `cancel.rs:8-11` module doc states it.
- **Remediation:** add `wasm_bindgen_test`s (non-zero deadline mid-loop,
  pre-cancelled token, a slow host call), state the divergence in
  `RunOptions::deadline`'s doc, and remove the redundancies.

<a id="dx-4"></a>
### DX-4 · A run limit is erased to `Error` and recovered by inspecting its kind
**Severity:** Medium · **Effort:** S–M · **Verified:** ✔

- **Where:** `src/run/engine.rs:301-306` (any non-`Cancelled` kind becomes `TimedOut`); `policy.rs:35` (`Flow::Stop(Error)`) into `interp.rs:70-76` (`Exit::Limited(Error)`); `host.rs:101-120` (`Invoked`); `fan_out.rs:254` (`expect_err("a call cut short is an Err")`); `policy.rs:176, 274, 293, 430` (`unreachable!`); `report.rs:129`, `cancel.rs:40`.
- **Reasoning:** `cancel::Stop` is the typed fact, but it travels as
  `Flow::Stop(Error)`. `Invoked { result, cut_short: bool }` admits the
  invalid pair (`Ok`, `true`). The messages also diverge: "the run timed
  out" vs "the run's deadline passed".
- **Remediation:** `Flow::Stop(Stop, Error)`, an exhaustive mapping to
  `RunStatus`, and `Invoked.result` as an enum `Returned | CutShort(Stop, Error)`.

<a id="dx-5"></a>
### DX-5 · Error messages embed whole expressions and offending values
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **Where:** `src/expr/expr.rs:119-120, 146-147, 175-176, 203, 229, 296` (``format!("`{}` did not give {}: {why}", self.raw, …)``).
- **Reasoning:** secrets are masked, but context data such as PII, and
  arbitrarily large expressions, flow into `metadata.errors`, traces and
  logs.
- **Remediation:** truncate the raw JSON, and report the value's *kind*
  rather than the value.

<a id="dx-6"></a>
### DX-6 · datalogic's typed errors are flattened to strings and re-classified
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **Where:** `src/expr/engine.rs:273` (`if e.tag() == "BudgetExceeded"`), `:167, 174` (`e.to_string()`); `src/expr/config.rs:563-569` (the issue code inferred from `ErrorKind::Config` plus the field type).
- **Remediation:** match `datalogic_rs::ErrorKind::BudgetExceeded` under
  `budget`, carry a structured cause, and have `compile` return the issue
  code directly.

---

## 6. Performance debt (needs gate confirmation)

Performance is design priority 1, so these are tracked even where each is
small. None has been measured yet. Run `bench/gate.py compare` with A/B
interleaving before acting on any of them.

| ID | Item | Where | Reasoning | Remediation | Sev / Effort |
|---|---|---|---|---|---|
| PERF-2 | `reads_secret` re-walks raw JSON twice per field on every `Ctx::input()` | `src/function/ctx.rs:278-283, 605-628`; `src/expr/expr.rs:293, 346` | Fixed at compile, recomputed per call; layers call `input()` on every host call | Store a `reads_secret: bool` per `Expr`/hole at compile; `constant_value` returns a borrow | Medium / S |
| PERF-3 | `Templated::get` does a linear string scan per access; `resolve_fields` allocates names per call | `src/function/templated.rs:231-262` | Orion plugin hot path | Resolve field handles in `prepare` | Low-Med / S–M |
| PERF-4 | Parse deep-copies the document under the default (compat-on) path, then clones fields again | `src/def/parse.rs:108` (`object.clone()`) plus per-field `.cloned()` (`:380, 484, 552, 606`) | Every v4-native definition pays for a v3 rewrite pass; costs load and hot-reload time | `parse_owned(Value)`, or skip the clone when a pre-scan finds no v3 keys | Med-Low / M |
| PERF-5 | The write-log trie scans children as a linked list; each splice copies whole pair slices | `src/state/writes.rs:416-423, 552-562, 609-613` (docs at `:13-14, 440-441`); `splice.rs:276-288` | The documented "cost = path depth" doesn't hold for wide objects: k keys under one parent cost O(k²) and garbage grows O(width), which may force near-constant checkpoints | Add a wide-object gate benchmark; if confirmed, a hash index past N children; fix the complexity docs | Medium / M |
| PERF-6 | Trace replay clones the full context per step; wasm `contexts()` collects all of them; `mapping_contexts` replays from `initial` on every call | `src/observe/trace.rs:127-170`; `wasm/src/lib.rs:714-743`; UI `MappingContent.tsx:32` | O(steps × size) memory: a 2 MiB context over 500 steps is ≈1 GB in the browser; per-click work is O(steps²) | A wasm `Replay` handle parsed once, `context_at(i)`, checkpoints every N steps, cached parsed paths | Medium / M |
| PERF-7 | The wasm boundary goes JsValue → text → `Value` → `OwnedDataValue` | `wasm/src/lib.rs:215-238, 285-295` | Three representations per payload; the cost has never been measured | Deserialize straight to `OwnedDataValue`; add a wasm benchmark | Low / S |
| PERF-8 | Per fan-out element: `format!` plus `Arc` plus deep clone for `into` under default capture | `src/run/fan_out.rs:296-299, 340` | Small next to the call, but on the `fan_out` gate path; the null array is also built twice (trace and context) | Reuse one path buffer; build the null array once | Low / S |
| PERF-9 | `Run` clones the `Program` `Arc` per message | `src/run/engine.rs:351` | One contended atomic pair per message, used only to resolve ids | Borrow, or store `Arc<str>` ids in the record; measure on spawned rows | Low / S–M |
| PERF-10 | `RunEnv` (25 fields) is rebuilt per workflow, with the op counter threaded in and out by hand | `src/run/interp.rs:112-165`; `engine.rs:254-290` | A cost and a correctness risk (the 64-op cadence) | Split `RunScope` (once per run) from `WfScope` | Low / M |
| PERF-11 | Ids stored three times, as `String` and `Arc<str>` | `src/program/link.rs:125-128, 232, 252`; `ops.rs:151-153` | Two sources of truth on the run path | Store `Arc<str>` once | Low / S–M |
| PERF-12 | Smaller items | `SharedOperator` double vtable (`engine.rs:20-34`); a dynamic `Expr<Path>` re-parses per eval (`expr.rs:226`); secrets deep-copied per read (`secrets.rs:114`); `WRITES_POOLED = 16` per thread with a per-node `Vec` (`writes.rs:456`); `Interpolated` literal subtrees cloned O(n·depth) at compile (`document.rs:296-309`) | Each is small | Fix opportunistically | Low / S |

---

## 7. Dependency coupling

<a id="dep-2"></a>
### DEP-2 · `ExactValue` re-implements datavalue's deserializer to keep `2.0` a float
**Severity:** Medium · **Effort:** M (upstream) · **Verified:** ✔

- **Where:** `src/base/context.rs:159-250`.
- **Reasoning:** a full visitor copy that has to track every new datavalue
  variant (datetime, tensor) by hand. The upstream crate is the right place
  for float preservation.
- **Remediation:** add a `preserve_float` mode upstream, then delete
  `ExactValue`.

<a id="dep-3"></a>
### DEP-3 · Operator families as cargo features multiply the build matrix
**Severity:** Medium · **Effort:** M · **Verified:** ✔

- **Where:** `Cargo.toml:34-62`; `src/expr/operators.rs:66-83` (`FAMILIES`, built by `#[cfg(feature)]`); `ci.yml` (the configuration list near `:63`, the family loop near `:145`).
- **Reasoning:**
  - 12 CI configurations.
  - Test counts depend on features (859 vs 869).
  - Feature unification lets *another* crate change a host's operator set.
  - The engine already calls datalogic's `with_families(...)`
    (`engine.rs:343`, `operators.rs:185`), but only with the cfg-built
    const, so the runtime switch exists and goes unused.
- **Justification:** the *rule* (families are opt-in, and enabling one is not
  a no-op) is intentional and stays. Only selecting families at compile time
  is the costly part.
- **Remediation:** compile the families in and select them at runtime through
  `ExprEngineBuilder`, defaulting to Core only. First verify that leaving
  `datetime` out at runtime also reverts its string comparison on `==`,
  which is pinned by `datetime_feature_changes_plain_string_comparison`.
  Keep `tensor` and `budget` as features.

---

## 8. Test-suite debt

<a id="tst-3"></a>
### TST-3 · Two golden mechanisms; orphans detected for the corpus only
**Severity:** Low-Medium · **Effort:** S · **Verified:** ✔

- **Done (2026-10-18):** the integration suite has one golden module,
  `tests/it/support/golden.rs` (`writing()` reads
  `DATAFLOW_GOLDEN_WRITE == "1"`), which every integration golden uses; the
  lib copy (`src/function/builtins/one_task.rs:295-296`) applies the same
  predicate. The corpus fails on a golden with no case
  (`tests/it/corpus.rs:83-88`).
- **Still open:**
  - The lib copy is a second mechanism with a different format (entries
    keyed by test name inside one file, `one_task.rs:368-438`).
  - Names are hand-typed (`golden::file("trace", name)`), not derived via
    `module_path!`.
  - Nothing detects an orphan under `tests/golden/{error_context,fan_out,run_loops,trace,lib}`.
- **Remediation:** names from `module_path!`, and one test that fails on any
  unclaimed golden file or entry, the lib's included.

<a id="tst-4"></a>
### TST-4 · Fixtures duplicated across test files, with inconsistent signatures
**Severity:** Medium · **Effort:** M · **Verified:** ✔

- **Where:**
  - `fn engine` is defined 11 times (9 in `tests/it/`, plus `ts_types.rs`
    and `secrets_isolation.rs`), `message` 11, `map` 10, `workflow` 8.
  - `workflow`'s argument order is `(id, tasks, extra)` in `fan_out`,
    `observer` and `trace`; `(id, extra, tasks)` in `run_basics`,
    `host_calls` and `run_json`; and `(id, tasks)` in `error_context` and
    `deadline`.
  - The test functions `Slow` (trace, deadline), `Put` (trace, wire),
    `Fetch` (registry, checks, compile) and `Echo` (registry, fan_out) are
    each defined 2–3 times.
- **Reasoning:** a builder or `Message` API change means about 10 edits, and
  the swapped argument orders invite mistakes when a test moves.
- **Remediation:** consolidate into `tests/it/support/`, now an ordinary
  module of the one integration binary (with `golden` and `corpus_driver`
  already there).

<a id="tst-5"></a>
### TST-5 · Goldens and assertions pin third-party error text
**Severity:** Medium · **Effort:** S–M · **Verified:** ✔

- **Where:**
  - `tests/golden/lib/run/policy.json:8-26`, `builtins/parse.json` (quick-xml messages); `src/function/builtins/parse.rs:364`
  - eleven datalogic "Logic evaluation error: Thrown" strings (lib goldens `parse.json`, `publish.json`, `corpus/expressions/condition_error.json`, `tests/it/run_loops.rs`)
  - `tests/it/derive.rs:286` (serde's `unknown variant`)
  - ≈94 message-text assertions in total
  - `corpus/src/normalise.rs` normalises only timestamps, durations and the engine version
- **Reasoning:** a patch bump of quick-xml, datalogic-rs or serde breaks
  goldens that have nothing to do with the change. The quick-xml hold was
  lifted, so this will recur. It did *(found 2026-10-18)*: the move to
  datalogic 5.8.1 changed one budget error's "in operator: `>`" to
  "`reduce`" in `tests/golden/corpus/expressions/budget.json`.
- **Remediation:** normalise dependency-originated messages in goldens to
  `<dependency message>`, and assert text only where it is our own wire
  contract.

<a id="tst-6"></a>
### TST-6 · Engine config equality is asserted by slicing `Debug` output
**Severity:** Medium · **Effort:** S · **Verified:** ✔

- **Where:** `tests/it/error_context.rs:791`, `tests/it/observer.rs:390`, `tests/it/run_basics.rs:867` (`debug[debug.find("config:")..]`).
- **Reasoning:** these tests are about reload semantics, but they break if a
  field is added to `Engine` or the field order changes.
- **Remediation:** a `#[doc(hidden)] Engine::config()` accessor, comparing
  `EngineConfig` values. (The `secrets_isolation.rs` `Debug` checks are
  correct; leaks through `Debug` are what they test.)

<a id="tst-7"></a>
### TST-7 · The path tests' generators: no 65-segment path, the RNG written twice
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **Where:** `tests/it/path.rs:9-23` and `src/state/tests/random.rs:11-35`
  each write the xorshift RNG; neither generates a path past
  `MAX_PATH_SEGMENTS` (64): they make 1–4 and 1–5 segments.
- **Done (2026-10-18):** the corpus model has the index cap, the random
  test generates `1048576`, a test pins both sides of the cap, and an
  exhaustive small-universe test holds the owned and arena rules to the
  model.
- **Remediation:** a 65-segment path in the generator; share the RNG
  through the corpus crate.

<a id="tst-8"></a>
### TST-8 · Hard-coded counts and magic thresholds
**Severity:** Medium · **Effort:** S · **Verified:** ✔ (61 commits touched CLAUDE.md's count line)

- **Where:** CLAUDE.md "The counts" (1037/887/2/18/130/859/869); `tests/it/repository_definitions.rs:33` (`MINIMUM_DEFINITIONS = 150`); `tests/it/deadline.rs:275, 283` (`63`, coupled to the private `CHECK_EVERY` at `src/run/interp.rs:197` and to one op per `map` task).
- **Done (2026-10-18):** `ts_types`' `ran >= 60` is gone; it now asserts
  that no declared key goes unseen (`tests/ts_types.rs:239-243`).
- **Reasoning:** nothing enforces the counts, so they drift, or every test PR
  has to edit CLAUDE.md, and agents treat the numbers as an oracle. The `63`
  couples a black-box test to lowering internals.
- **Remediation:** drop the absolute counts, or generate and check them in
  CI. Derive `63` from an exported constant, or assert "stopped after at
  least one and before 100" plus a lib unit test for the exact interval.

<a id="tst-9"></a>
### TST-9 · Property-test knobs: fixed seeds, `DATAFLOW_V4_DEEP`, a wall-clock assertion, eager formatting
**Severity:** Medium-Low · **Effort:** S · **Verified:** ✔

- **Done (2026-10-18):** CI's weekly `deep` job sets `DATAFLOW_V4_DEEP=1`
  and runs `cargo test --lib state::` at full depth.
- **Where:**
  - Seeds and counts: `src/state/tests/run_context.rs:232-259`, `writes.rs:486-539`, `splice_property.rs:175-200` (`0xa11ce` at `:177`); no `DATAFLOW_PROPTEST_SEED`.
  - `random.rs:231-234`: the knob is still named `DATAFLOW_V4_DEEP`.
  - `src/state/size.rs:68`: `assert!(per.as_micros() < 50)` in a debug test, on a machine with permanent Docker load.
  - `splice_property.rs:183, 300`: `format!("{op:?} on {owned:?}")` on each of 50k passing cases.
  - A hand-kept feature list in `cfg_attr(any(feature=…), ignore)` (`run_context.rs:232-246`).
- **Reasoning:** fixed seeds turn the property tests into a frozen
  regression set that never explores new inputs, and that set is what holds
  the path rules' appliers together (now beside an exhaustive small-universe
  test). The timing assertion is a flake source.
  The eager formatting spends part of the one-minute budget for nothing.
- **Remediation:** one `random::config()` that reads `DATAFLOW_PROPTEST_SEED`
  and `DATAFLOW_DEEP` (dropping "V4") and prints the seed on failure. Format
  lazily. Replace the timing assertion
  with a node-visit counter. Keep the feature gate in one place.

<a id="tst-10"></a>
### TST-10 · Test scaffolding and fault hooks woven into production modules
**Severity:** Medium-Low · **Effort:** M · **Verified:** ✔

- **Where:** `src/run/interp.rs:684-685` (`#[cfg(test)] let result = record::failing_after_its_writes(result);` on the built-in call path; another `cfg(test)` at `:431`); `record.rs:83-84`, `cancel.rs:136-137` (`CLOCK_READS`); seventeen `#[cfg(test)]` items interleaved in `src/state/writes.rs` (971 lines: `list`, `fault`, `OwnedAt`, `Dirty`…), plus `splice.rs:409-455`.
- **Reasoning:**
  - Lib unit tests run a slightly different interpreter from the shipped
    one.
  - About 30% of `writes.rs` is test scaffolding, which hides the trie's
    real size (≈700 lines).
  - The fault hook simulates "a built-in that writes, then fails, which no
    built-in does today".
- **Remediation:** move scaffolding into `src/**/tests/`. Inject faults with
  a crate-private test built-in rather than a `cfg(test)` branch in `interp`.

<a id="tst-11"></a>
### TST-11 · Very large test files, plus leftovers from the v3 oracle
**Severity:** Low · **Effort:** M · **Verified:** ✔ (line counts)

- **Large files:** `tests/it/trace.rs` 1867, `tests/it/host_calls.rs`
  1812, `tests/it/parse.rs` 1438, `tests/secrets_isolation.rs` 1382 lines,
  and others. `src/run/record.rs` is about half inline tests (1109 lines,
  tests from `:568`). Split by sub-topic, now that each is a module of
  `tests/it/`.
- **v3 leftovers:** `builtins/tests.rs:607-620` keeps a two-column table with
  the v3 column removed (`enum V4`, `struct Row { v4 }`). File headers say
  "v3's … ported (I111)" (`tests/secrets_isolation.rs:2`,
  `tests/it/observer.rs:2`, `tests/it/ops_budget.rs:3`).
- **Other test hygiene:**
  - 10 `compile_fail` doctests in `function/config.rs:46-155` don't pin the
    expected error, so they pass if they fail for an unrelated reason.
    Consider `trybuild`.
  - `filter.rs:99-128` has `async` tests that await only a sync helper.

---

## 9. CI, release and tooling

<a id="ci-2"></a>
### CI-2 · Four artifacts version-locked by hand in eight or more places, checked only at tag time
**Severity:** Medium · **Effort:** M · **Verified:** ✔

- **Where:** root `Cargo.toml:3, 94`, `derive/Cargo.toml:3`, `wasm/Cargo.toml:3, 45`, `ui/package.json`, `ui/package-lock.json`, 7 install snippets. The check lives in `release.yml:42-97` (tags only), using `grep | head -1 | sed`. There is no `[workspace.package]` or `[workspace.dependencies]`.
- **Remediation:** `[workspace.package] version` and `[workspace.dependencies]`.
  Extract the agreement check into a script that also runs on PR CI.

<a id="ci-3"></a>
### CI-3 · The `check` job is one serial job with 18+ cargo invocations and no timeouts
**Severity:** Medium · **Effort:** S–M · **Verified:** ✔

- **Reasoning:** a failure in feature family #9 surfaces only after
  everything before it, and wall-clock time is the sum of all
  configurations (a bash `for` over ten families, and now the examples'
  smoke step too). Each family iteration builds every integration binary,
  example and doctest. No workflow sets `timeout-minutes`. Dropping the
  target cache was a measured decision and is not debt.
- **Remediation:** a `strategy.matrix` job per family (fail-fast off),
  `--tests --lib` there, and timeouts.

<a id="ci-4"></a>
### CI-4 · Checks CLAUDE.md lists are missing from CI; toolchains are unpinned or installed several ways
**Severity:** Medium · **Effort:** S · **Verified:** ✔

- **Missing checks:**
  - `npm run build` (the demo app that `docs.yml` deploys as `/debugger`) is
    not in CI; the `ui` job runs `lint`, `test` and `build:lib`, and
    `build:lib` can't catch a broken `@xyflow/react`.
  - `mdbook build docs` runs only in the manual `docs.yml`.
- **Inconsistent toolchains:**
  - wasm-pack is installed by an unpinned curl script in CI, but by
    `cargo install --version 0.14.0` in release. CI's wasm job exists to
    catch release-time wasm-opt defects, yet it runs a different wasm-pack.
  - Two installer hosts are in use.
  - The docs are built on nightly with mdbook `latest`. A nightly-induced
    docs breakage has already been recorded.
- **Remediation:** add both missing checks to CI. Pin one wasm-pack version
  in one place, using a prebuilt binary. Use stable or a pinned nightly,
  plus a pinned mdbook, for the docs.

<a id="ci-5"></a>
### CI-5 · Smaller release and tooling items
**Severity:** Low-Medium · **Effort:** S each · **Verified:** ✔

- **`publish-wasm`'s cache** restores *after* `cargo install wasm-pack`,
  caches `wasm/target/` (unused, since wasm is a workspace member), and keys
  on `wasm/Cargo.toml` rather than `Cargo.lock`. It saves nothing.
- **`wasm/publish.sh`** is a legacy manual path that skips
  `verify-wasm.mjs`, version agreement and the existence checks, and keeps a
  second `package.json` template. Delete it, or reduce it to calls into the
  shared scripts.
- **Release hardening:**
  - `permissions: contents: write` is granted workflow-wide.
  - `npm publish` runs with a long-lived token and no `--provenance`.
  - There is no `--tag next` for prereleases, so `v4.0.0-rc.1` would become
    `latest`.
- **Bench tooling:** `bench/gate.py`'s `machine()` (`:206-219`) calls
  `sysctl`/`pmset`/`sw_vers` unconditionally, so it crashes off macOS after
  every round with `--out`. Its statistics (`verdict`, `ratio`,
  `half_iqr_pct`, `:285-318`) are untested: `bench/gate_test.py` covers
  only smoke mode and the `GATE` line contract. `orion.sh` holds an 83-line Python heredoc
  that regex-scrapes another repository's human-readable output.
- **`docs/build-with-debugger.sh`** builds a different site from `docs.yml`
  (no wasm overlay, a stale playground wasm). Use one script for both.

---

## 10. UI (`@goplasmatic/dataflow-ui`)

<a id="ui-1"></a>
### UI-1 · The definition types are a hand mirror, and the v3→v4 policy table is re-implemented in TypeScript
**Severity:** Medium · **Effort:** M · **Verified:** ✔

- **Where:** `ui/src/types/workflow.ts` (619 lines; no `ts` derives on `src/def/types.rs`); `workflow.ts:105-128, 370-377` mirror `src/def/reader.rs:397-408`.
- **Reasoning:**
  - "The replay lives once, in Rust" is honoured for traces but not for
    definitions. New keys (`for_each`, `loop.over`, `on_rejection`) are
    mirrored by hand with no drift test.
  - The UI decides v3 policy semantics itself
    (`task.continue_on_error === true && task.halt_on !== 'failure'`), even
    though the wasm `parse()` export already returns the migrated v4
    definition.
- **Justification:** this is the class of bug that wire-type generation
  removed, and it is still open for definitions. If the policy table changes,
  the debugger will show behaviour the engine doesn't run.
- **Drift found (2026-10-18):** `workflow.ts` still requires `name`
  (`:30`, `:48`, `:310`) and carries v3's `path?` (`:320`); the v3 policy
  logic is at `:115` and `:376`.
- **Remediation:** have the UI read definitions through wasm `parse()` and
  render v4 spellings only. Add `ts` derives to `def::types` under the
  existing drift test. A cheaper first step: a `definition_keys` list in
  wasm `engine_facts` and a vitest holding `workflow.ts` to it.

<a id="ui-2"></a>
### UI-2 · 10 tests for ≈9.5k lines, and react-hooks suppressions in the published library
**Severity:** Medium · **Effort:** M · **Verified:** ✔

- **Untested logic:** `buildFlowGraph.ts` (337 lines), `buildGroupFlowGraph.ts`
  (210), `useTreeNodeDebugState.ts` (439) and the `DebuggerContext` reducer
  (594) are pure, easily tested, and untested. There is no coverage floor.
- **Hooks:**
  - `ThemeContext.tsx:32-55` ships in `lib.ts` and syncs derivable state
    through effects, at the cost of an extra render or a flash per theme
    change.
  - `IntegratedDebugToolbar.tsx` leaves `handleExecute` out of its
    dependencies, and dedups on `JSON.stringify(workflows)` (`:118`, and
    again in `DebuggerContext.tsx:402`), so a change of engine doesn't
    re-run.
  - `TreeView.tsx:50, 65` and `App.tsx:117` carry the same TODO pattern.
- **Remediation:** unit-test the graph builders and the reducer against
  corpus-like definitions; add `vitest --coverage`; derive `resolvedTheme`
  via `useSyncExternalStore`; fix the dependency list and the dedup key.

---

## 11. Documentation and provenance hygiene

<a id="doc-1"></a>
### DOC-1 · ≈90 references to the deleted `design-v4/`, four in public rustdoc
**Severity:** Medium (public docs) / Low (internal) · **Effort:** S · **Verified:** ✔ (92 lines in 46 files by a narrow regex, 2026-10-18: `src` 35, `tests` 55, `corpus` 1, `Cargo.toml` 4)

- **Public rustdoc:**
  - `src/base/context.rs:22` ("break 22", on `Context`)
  - `src/observe/trace.rs:212` ("break 5", on `Failed`)
  - `src/def/parse.rs:48` ("(phase 3)", on `ParseOptions::compat`)
  - `src/compat/mod.rs:38` ("(phase 3)", in the module's `//!`)
  - (`src/run/message.rs:75`'s "break 22" is gone.)
- **Internal (examples):**
  - `Cargo.toml:80, 89, 96, 110` (I125, I86, I73, I92)
  - private `///`/`//!`: `state/mod.rs:2-3` ("phase 0 spike … phase 4's
    Task 4"), `expr/config.rs:60` and `expr/output.rs:35` ("break 16"),
    `function/registry.rs:208` ("(M3)"), `run/record.rs:90` (T2-C4),
    `run/interp.rs:510` (T2-C2), `run/error_context.rs:9` ("break 20"),
    `run/host.rs:8` ("break 1"), `corpus/src/replay.rs:32` (I106)
  - test names `the_i65_stop_is_gone` (`tests/it/run_basics.rs:882`) and `run_reports_by_i66` (`src/run/report.rs:280`)
- **Reasoning:** `design-v4/` was deleted in f0d3d98, so none of these can be
  followed. CLAUDE.md requires that "public doc comments cite reasons, never
  a design document's IDs", and four of these violations are about to ship
  to docs.rs. Most comments already state the reason next to the ID, so the
  parenthetical can simply be deleted.
- **Remediation:** replace each ID with its one-clause reason, and rename the
  two tests. Add a CI grep that rejects
  `\((I|R|M)\d+\)|[Bb]reak \d+|[Pp]hase \d|T2-C\d` in `src/`, `tests/` and
  `corpus/`.

<a id="doc-2"></a>
### DOC-2 · Comments and docs name v3 symbols or behaviour that no longer exists
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **v3 names in comments:** about 60 rationales cite v3 functions that are
  only on the `v3.*` tags. That is acceptable as provenance, but the
  ordering contracts read better stated in v4's own terms. The worst cases
  are:
  - references to types listed as removed (`Template::eval`,
    `LogicCompiler`/`TemplateCompiler` at `src/expr/engine.rs:453, 494`);
  - v3 helper names in `src/expr/path.rs:371, 447` (`set_nested_value_parts`,
    `describe_kind`, …);
  - a broken intra-doc link `[SyncCtx::change]` (`builtins/map.rs:271`; the
    method is `write`, `sync.rs:93`), which rustdoc doesn't check on private
    items.
- **Stale comments:**
  - `state/mod.rs:15-16` says `Owner`/`At` will be re-exported by their first
    user, but `Owner` already is (`:12`).
  - `writes.rs:446` says the overlay uses the log, but it doesn't.
  - `docs-tests/Cargo.toml:4` says "three-artifact" (there are four).
  - `docs/book.toml:11` says `mdbook test` compiles the Rust blocks;
    `docs-tests/src/lib.rs:6-9` says it can't.
- **CLAUDE.md inaccuracies:**
  - "the eight built-ins on the crate-private `SyncFunction`": only four
    implement it (`log`, `validation`, `filter`, `map`), and the trait adds
    nothing (see below).
  - The hard-coded test counts (see [TST-8](#tst-8)).
- **`SyncFunction` itself:** it has no generic or `dyn` use, and `SyncCtx` is
  a 12-field struct the built-ins reach into directly, with map-only trace
  plumbing threaded through every built-in. Drop the trait, and put the
  fields behind `write`/`remove`/`record`/`source` with a `TraceSink` that
  `map` borrows.

<a id="doc-3"></a>
### DOC-3 · Most JSON blocks in the docs can't be checked by machine
**Severity:** Low · **Effort:** S · **Verified:** ✔

- **Where:** 153 `json` fences in `docs/src` and `README.md`, of which 49 are
  not valid JSON (elisions such as `"tasks": [...]`, `//` comments).
- **Reasoning:** `tests/it/repository_definitions.rs:657-684` does parse the
  *complete* definitions it finds (it fails on an invalid fence holding
  `"tasks"` and no `...`), so the project note that "docs JSON is untested"
  is partly out of date. Elided fences are counted and skipped, other
  non-JSON fragments are ignored, and semantics are unchecked.
- **Remediation:** tag elided blocks `jsonc` or `text`, test that every `json`
  fence parses, and wrap task-level fragments in a synthetic workflow.

---

## 12. Orion follow-ups (not dataflow-rs work)

`38de619` needs these in Orion's `dataflow-4` when its pin moves past it:

- Delete the arms for the removed `ErrorKind::Parse` and
  `ErrorKind::FunctionNotFound` (`engine/functions/mod.rs:370-372`,
  `engine/error.rs:350-351`). O12's port of `validation/workflows.rs:565-570`
  needs a v4 stand-in for v3's `MissingHandler`; the nearest is
  `UNKNOWN_FUNCTION`.
- Close K30 (key order). Correct the two stale "no `preserve_order`" comments
  (`query/keyset.rs:40-44`, `connector/config.rs:304`).
- **Partial rollouts run both versions on Kafka and `channel_call`.** Both
  pass no routing bucket (`kafka/consumer/process.rs:341-354`,
  `engine/functions/channel_call.rs:302, 642-647`), and every rollout admits
  a run with none, so a 10/90 channel runs both versions on every record and
  every child call. Kafka should hash the record key, as cron does
  (`cron/worker.rs:530-546`). `channel_call` should pass on
  `ctx.info().routing_bucket`, which `38de619` added. The comment at
  `channel_call.rs:302` is wrong. v3 behaves the same way.
- Orion hard-codes `"validate"` beside `BUILTIN_FUNCTIONS`
  (`engine/handlers.rs:572-599`): replace the copy with the new public
  `def::is_reserved_function`.
- **From the second batch (2026-10-18):**
  - A failed `map` mapping records `MAPPING_ERROR` before the
    `TASK_STATUS_ERROR`, and a `filter` condition error records
    `EVALUATION_ERROR`. `has_errors()`, DLQ and retry don't move; error
    summaries, the response envelope and `metadata._orion_errors` gain one
    entry each (the 32-record limit fills faster in loops). A `map` or
    `filter` over the ops budget now fails its task with `BUDGET_EXCEEDED`.
  - The engine is on datalogic-rs 5.8.2 and datavalue-rs 0.3.2: re-lock
    (v3.15's `"5.7"` admits them), fix the `orion-server/Cargo.toml:91` comment, and run
    `operator_names_equal_the_v3_engines`,
    `tensor_operators_are_the_tensor_family`, the forbidden-operator tests
    and `bench/orion.sh`. `expr::Family::Flagd` is gone (Orion does not use
    it); an operator family is on only through dataflow-rs's own feature.
  - Definition fields are read through accessors (`def.id()`,
    `task.function().name()`); O12/O14's reference checker over
    `WorkflowDef::walk` uses them.
- Bump the pin, then run Orion's v3/v4 twin tests and the quarantine-verdict
  test. The batch added one Rejected rule (`INVALID_MAPPING` for a constant
  non-object at a root), which no Orion definition hits.

## 13. Considered and judged *not* debt

These were examined and deliberately left out, so they needn't be
re-audited:

- **Rust safety and lint hygiene:**
  - `#![forbid(unsafe_code)]`, and self_cell + bumpalo with a covariant
    dependent and a documented `'p` proof.
  - Checkpoint panic safety, exercised by fault injection.
  - Library panics limited to documented invariants: `one_task.rs` is
    `#[cfg(test)]`, and the `link` u32 `expect`s are bounded.
  - Each of the 5 lint suppressions carries a `reason`, and the
    `unreachable_patterns` ones are required by feature unification.
- **Single homes that hold:**
  - The policy rules (`policy::decide`, 84-row table tests).
  - The `Error::code` classifier.
  - The progress write's semantics, never gated.
  - The definition rules, `parse`'s alone: a definition cannot be edited
    after it.
  - The path rules (`expr::rules`), which every writer applies.
  - `Ext::keep`.
  - `ser.rs`'s two-pass `Fields` trait.
  - Observer interest read once, and the "one branch per unwatched call"
    claim.
- **Module sizes:** `compile.rs` (≈760 lines of code), `record.rs` (≈570),
  `ctx.rs` (≈657) and `map.rs` (≈441) are mostly inline tests. `Ctx` has 17
  methods in four coherent groups and is not a god object. `erased.rs` is
  two small traits.
- **The overlay vs the write log:** two write logs are justified. The overlay
  replays unresolved paths against a base that may move; the trie keys on
  resolved segments.
- **Concurrency tests:**
  - `deadline.rs` uses `start_paused` throughout, so its timings are
    deterministic.
  - `fan_out.rs` uses `yield_now`, not sleeps.
  - Concurrency invariance is pinned by a golden.
- **Dependency hygiene:** 134 crates, 2 duplicate names, no advisory ignores,
  a standard licence allowlist. No `bench/__pycache__` is committed (it is
  ignored).
- **Intentional decisions in CLAUDE.md:**
  - The UI's wasm requirement lags one release; `release.yml` rewrites it at
    publish.
  - TypeScript 7 is held back.
  - No tags before phase 8.
  - No target cache in the CI `check` job (a measured decision).
  - `opt-level = "s"` for wasm.
  - Operator-family semantics, the `$` escape, `secret` as an operator.
  - `capture_changes` defaults to true (an owner decision, not to be
    re-proposed).
- **Release pipeline structure:** idempotency and ordering (the derive first,
  existence checks, a retry for npm's abbreviated-metadata cache) are sound.

---

## 14. Suggested sequencing

The owner's standing rule is to register known issues and fix them after
phases 7–8, not inside them. Within that rule, the items split by what the
4.0.0 tag makes expensive:

**Before the phase-8 tag (semver or release-blocking):**

1. The `ErrorRecord` wire shape as one change 🔒: [API-5](#api-5) (ids on
   every record), [API-3](#api-3) (representation, one stamp) and
   [API-1](#api-1) (the retry fields and two issue docs).
2. [API-2](#api-2): keep the re-exports (Orion needs them); seal the
   doc-hidden methods and settle the `OperatorInfo`/`Family` clash.
3. Public-surface cleanup 🔒, mostly S: [API-4](#api-4) (`try_register`,
   `testing::Write`, built-in configs, canonical paths, the `PARSE_FAILED`
   split codes, `Rewrite`, `Migrated`), then the [DUP-8](#dup-8) TS
   literal union.
4. [DOC-1](#doc-1): purge the design IDs from public rustdoc at least, and
   add the CI grep.
5. [VB-2](#vb-2): the rollout range check, a new Defect.

**Immediately after 4.0.0 (risk reduction):**

6. Diagnostics: [DX-2](#dx-2) (if not already done before the tag),
   [DX-3](#dx-3), [DX-7](#dx-7).
7. [TST-9](#tst-9) seeds (the deep run is already CI's weekly `deep` job).
8. [DUP-7](#dup-7) on `Logic::facts()`, now that datalogic 5.8.2 is in;
   it closes the `missing`/`exists` gap in `PAYLOAD_VAR`.

**Structural refactors (each behind a gate run):**

9. [DUP-2](#dup-2) settle and recorder once, [DUP-4](#dup-4) eval once.
10. The test-suite follow-ups: [TST-3](#tst-3), [TST-4](#tst-4) and
    [TST-11](#tst-11), in the one integration binary.
11. The performance table in §6, in gate order.
12. CI and UI items, opportunistically.
