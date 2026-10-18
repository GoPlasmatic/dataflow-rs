# Tech debt register: dataflow-rs

- **Snapshot:** `main` @ `66d217a` (4.0.0-dev), 2026-10-18. Completed
  entries have been pruned; this file lists open work only. Every entry
  below was re-checked against the tree on 2026-10-18.
- **Scope:** the engine (`src/`), `derive/`, `wasm/`, `corpus/`, `tests/`,
  `examples/`, `bench/`, CI and release workflows, `docs/`, `ui/`
- **Method:** a `/code-review` pass over `src/`, `derive/` and `wasm/src/`,
  plus seven read-only subsystem audits. Each finding had to quote the code
  at a `file:line`; overlapping findings were merged.
- **Not done:** performance items are reasoned from the code and still need
  a gate run (`bench/gate.py`) before anyone acts on them.

### How to read an entry

| Field | Meaning |
|---|---|
| **Severity** | **Critical**: wrong output or a broken release today. **High**: silent-divergence risk, or something the 4.0 API freeze makes permanent. **Medium**: a recurring cost or a likely future bug. **Low**: hygiene. |
| **Tag** | **essential**: the 4.0.0 tag makes it permanent; do it before the tag 🔒. **must**: required for correctness, or to stop a silent divergence; do it next. **should**: a real improvement; schedule after the above. **over-engineering**: the current shape, or the proposed fix, costs more than it returns; simplify or drop it. |
| **Effort** | S ≤ ½ day · M ≤ 3 days · L > 3 days |
| **Verified** | ✔ re-read against the source at `66d217a`. |
| **Freeze** | 🔒 changes public API, wire shape or semver surface, so it is free only before the 4.0.0 tag. |

Each entry gives the reasoning (*why this is debt*) and the justification
(*why fix it, and why this way*). Designs that CLAUDE.md's "Rules that must
hold" pin on purpose are **not** listed as debt. An entry only appears when
the *implementation* of such a rule is duplicated, unenforced or leaky.

---

## 0. Summary

The engine is in good structural health: `#![forbid(unsafe_code)]`, almost no
panics on library paths outside tests, a strict lint/doc gate, typed and
versioned fingerprints, and a single-writer state design under fault
injection. What is left is concentrated in four places: hand-synchronised
copies of rules, a little unvalidated input, deferred public-API hygiene, and
test/CI tooling that has outgrown its first shape.

The 4.0.0 freeze itself is almost clean: the wire shape (`ErrorRecord` ids
and `Arc<str>` sharing) and the dependency re-exports are settled. One item
remains pre-tag — [API-4](#api-4)'s leftover public-surface hygiene.

### Highest priority (essential and must)

| Item | Tag | Severity | Effort | Why now |
|---|---|---|---|---|
| [API-4](#api-4) Public-surface hygiene | essential | Low-Medium | S each | The last pre-tag 🔒 item: `testing::Write`, `Migrated::json`, `Change`'s manual `PartialEq`/`Debug`, `Error`'s `Debug`. |
| [DUP-2](#dup-2) Call settlement and the recorder's ordering | must | Medium | M | The most likely place for an ordering regression: the settle rules are written 3–5 times, in different statement orders. |
| [DUP-7](#dup-7) Raw-JSON expression walks | must | Medium | M–L | The `PAYLOAD_VAR`/secrets safety lints drift, and `missing`/`exists` already escape `PAYLOAD_VAR`. |
| [VB-3](#vb-3) Host spec mistakes surface late | must | Medium | S–M | A host's spec bug is reported as the *author's* `INPUT_PARSE`, on every compile. |
| [DX-3](#dx-3) Native and wasm cancellation diverge | must | Medium | S–M | Untested wasm cancellation: a host call that overruns is never cut short there. |
| [UI-1](#ui-1) Definition types are a hand mirror | must | Medium | M | The debugger can show v3 policy behaviour the engine no longer runs. |
| [CI-2](#ci-2) Four artifacts version-locked by hand | must | Medium | M | Agreement is checked only at tag time; a miss ships a mismatched release. |
| [Orion follow-ups](#orion-follow-ups) | must | — | — | Needed in Orion's `dataflow-4` when its pin moves past the batch. |

### Metrics snapshot

| Metric | Value |
|---|---|
| Rust under `src/`, `derive/src`, `wasm/src`, `corpus/src` | ≈37.8k lines (inline tests included; `record.rs` is about half tests) |
| Integration tests | 3 binaries (`it`, `secrets_isolation`, `ts_types`), ≈23.8k lines, plus 83 corpus cases and their 83 goldens |
| `unsafe` | 0 (`forbid(unsafe_code)`) |
| Panics in non-test library code | A handful, each a documented invariant (`link`, `registry`, interp `unreachable!`) |
| Duplicate crates in `Cargo.lock` | 2 (`bitflags` 1/2, `syn` 2/3) |
| Lint suppressions in `src/` | 6, all with `reason =` |
| CI feature configurations | 12, run serially in one job |
| UI | ≈9.5k lines TS/TSX, 10 tests |

---

## 1. Duplicated rules (hand-synchronised copies)

The project's stated principle is "each rule lives once in the code". Each
item below breaks that principle. The common justification: every copy is a
place where a future change can land in only N−1 of N copies, and the tests
that hold the copies together are either randomised (they can miss a case) or
hand-written (they don't grow with the tables).

<a id="dup-2"></a>
### DUP-2 · Call settlement and the recorder's ordering are written 3–5 times
**Severity:** Medium · **Tag:** must · **Effort:** M · **Verified:** ✔

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
**Severity:** Medium · **Tag:** should · **Effort:** S · **Verified:** ✔

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
**Severity:** Medium · **Tag:** must · **Effort:** M–L · **Verified:** ✔

- **Where:** `src/expr/walk.rs:12-60` (`escaped_keys`, `secret_refs`, `reads_secret`); `src/expr/document.rs:318-345` (its own `var` parsing and payload test); `src/program/checks.rs:103-118, 287-352` (`classify`, `walk_payload`, `payload_read`: a second `var`/`val` parser).
- **Reasoning:** each walk re-derives what the compiler knows (operators
  used, paths read under which scope frame) and re-implements
  templating-mode quirks.
- **Justification:** the `PAYLOAD_VAR` and secrets lints are safety rules,
  and a drift makes them quietly weaker. `missing`, `missing_some` and
  `exists` on a `payload.` path already escape `PAYLOAD_VAR`, since only
  `var` and `val` are walked.
- **Remediation:** derive reads and operators from `Logic::facts()`, which
  also closes the `missing`/`exists` gap. Keep one raw walk only for what
  datalogic can't see (escaped keys, `secret`). Merge `reads_secret` into
  `secret_refs(..).next().is_some()`.

<a id="dup-9"></a>
### DUP-9 · Smaller duplications
**Severity:** Low · **Tag:** should · **Effort:** S each · **Verified:** ✔

- **Two size estimators:** `src/state/size.rs:9` (`NODE = 32`, hand
  constant) vs `src/run/trace.rs:24, 289-305` (`size_of::<usize>()`, an
  uncapped walk per change). Derive the constant from `size_of`, share one
  walker, and cap the trace walk at the remaining budget.
- **Two clocks:** `record.rs:82-86` uses `Instant::now()` and `cancel.rs:135-147`
  uses Tokio's clock, each with its own `CLOCK_READS` thread-local. Under
  paused time, elapsed timings and the deadline read different clocks. Use
  one `clock` module.
- **"Trivially true" has two definitions:** the `UNGUARDED_VALIDATION` lint
  tests the raw literal (`checks.rs:356-358`), while lowering tests the folded
  value (`compile.rs:484`). So `{"==":[1,1]}` counts as a guard but guards
  nothing. Feed the lint from the compile walk.
- **`merge_root` is written twice** (owned `path.rs:475`, arena
  `splice.rs:266`, plus `state/run.rs:386`).
- **`def/keys.rs:135`** spells `map`'s and `validation`'s element tables by
  hand, since `FieldSpec` carries no nested fields.
- **Left from the second batch:** the path rules are stated once in
  `expr::rules`, but the two hot walks (the owned `path::set`/`get` and the
  arena splice) stay hand-inlined, since asking `rules::step` per segment
  cost `map`-heavy benchmarks 1.5 to 2% of their instructions; an exhaustive
  test holds both to the rules' reference walk.

---

## 2. Validation boundary

<a id="vb-3"></a>
### VB-3 · Host spec mistakes surface late, and get blamed on the workflow author
**Severity:** Medium · **Tag:** must · **Effort:** S–M · **Verified:** ✔

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

---

## 3. Public API and wire surface before the 4.0 freeze 🔒

<a id="api-4"></a>
### API-4 · Stringly or leaky types on the public surface
**Severity:** Low-Medium · **Tag:** essential · **Effort:** S each · **Verified:** ✔

The canonical paths and the first round of leaky types are fixed. What
remains:

- **`compat::Migrated`** still returns `json: Value` (`compat/mod.rs:85-99`);
  it now also carries `pub def`, but a host that wants the written text must
  re-serialize. Return text, or document why `Value` is the contract.
- **`testing::Write`** re-exports overlay internals (`testing.rs:42, 46-54`;
  `Blocked(OwnedDataValue)`, `src/function/overlay.rs:42-53`), so restructuring
  the overlay would become a breaking change. Give `testing` its own type.
- **`Change`** carries a `pub(crate) trace_only` flag, which forces a manual
  `PartialEq`/`Debug` that must list every wire field (`base/change.rs:43-68`);
  a new field missed in `eq` compares equal. Derive the comparison, or keep
  `trace_only` out of the wire type.
- **`Error`'s `Debug`** uses `.finish()` instead of
  `finish_non_exhaustive()` (`error.rs:154-163`); it is a public type with
  private fields. *(Lowest priority; arguably over-engineering.)*

---

## 4. Developer experience and diagnostics

<a id="dx-3"></a>
### DX-3 · Native and wasm cancellation diverge, and the wasm side is untested
**Severity:** Medium · **Tag:** must · **Effort:** S–M · **Verified:** ✔

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
**Severity:** Medium · **Tag:** should · **Effort:** S–M · **Verified:** ✔

- **Where:** `src/run/engine.rs:301-306` (any non-`Cancelled` kind becomes `TimedOut`); `policy.rs:35` (`Flow::Stop(Error)`) into `interp.rs:70-76` (`Exit::Limited(Error)`); `host.rs:101-120` (`Invoked`); `fan_out.rs:254` (`expect_err("a call cut short is an Err")`); `policy.rs:176, 274, 293, 430` (`unreachable!`); `report.rs:129`, `cancel.rs:40`.
- **Reasoning:** `cancel::Stop` is the typed fact, but it travels as
  `Flow::Stop(Error)`. `Invoked { result, cut_short: bool }` admits the
  invalid pair (`Ok`, `true`). The messages also diverge: "the run timed
  out" vs "the run's deadline passed".
- **Remediation:** `Flow::Stop(Stop, Error)`, an exhaustive mapping to
  `RunStatus`, and `Invoked.result` as an enum `Returned | CutShort(Stop, Error)`.

<a id="dx-5"></a>
### DX-5 · Error messages embed whole expressions and offending values
**Severity:** Low · **Tag:** should · **Effort:** S · **Verified:** ✔

- **Where:** `src/expr/expr.rs:119-120, 146-147, 175-176, 203, 229, 296` (``format!("`{}` did not give {}: {why}", self.raw, …)``).
- **Reasoning:** secrets are masked, but context data such as PII, and
  arbitrarily large expressions, flow into `metadata.errors`, traces and
  logs.
- **Remediation:** truncate the raw JSON, and report the value's *kind*
  rather than the value.

<a id="dx-6"></a>
### DX-6 · datalogic's typed errors are flattened to strings and re-classified
**Severity:** Low · **Tag:** should · **Effort:** S · **Verified:** ✔

- **Where:** `src/expr/engine.rs:273` (`if e.tag() == "BudgetExceeded"`), `:167, 174` (`e.to_string()`); `src/expr/config.rs:563-569` (the issue code inferred from `ErrorKind::Config` plus the field type).
- **Remediation:** match `datalogic_rs::ErrorKind::BudgetExceeded` under
  `budget`, carry a structured cause, and have `compile` return the issue
  code directly.

---

## 5. Performance debt (needs gate confirmation)

Performance is design priority 1, so these are tracked even where each is
small. None has been measured yet. Run `bench/gate.py compare` with A/B
interleaving before acting on any of them.

| ID | Tag | Item | Where | Reasoning | Remediation | Sev / Effort |
|---|---|---|---|---|---|---|
| PERF-2 | should | `reads_secret` re-walks raw JSON twice per field on every `Ctx::input()` | `src/function/ctx.rs:278-283, 605-628`; `src/expr/expr.rs:293, 346` | Fixed at compile, recomputed per call; layers call `input()` on every host call | Store a `reads_secret: bool` per `Expr`/hole at compile; `constant_value` returns a borrow | Medium / S |
| PERF-3 | should | `Templated::get` does a linear string scan per access; `resolve_fields` allocates names per call | `src/function/templated.rs:231-262` | Orion plugin hot path | Resolve field handles in `prepare` | Low-Med / S–M |
| PERF-4 | should | Parse deep-copies the document under the default (compat-on) path, then clones fields again | `src/def/parse.rs:108` (`object.clone()`) plus per-field `.cloned()` (`:380, 484, 552, 606`) | Every v4-native definition pays for a v3 rewrite pass; costs load and hot-reload time | `parse_owned(Value)`, or skip the clone when a pre-scan finds no v3 keys | Med-Low / M |
| PERF-5 | should | The write-log trie scans children as a linked list; each splice copies whole pair slices | `src/state/writes.rs:416-423, 552-562, 609-613` (docs at `:13-14, 440-441`); `splice.rs:276-288` | The documented "cost = path depth" doesn't hold for wide objects: k keys under one parent cost O(k²) and garbage grows O(width), which may force near-constant checkpoints | Add a wide-object gate benchmark; if confirmed, a hash index past N children; fix the complexity docs | Medium / M |
| PERF-6 | should | Trace replay clones the full context per step; wasm `contexts()` collects all of them; `mapping_contexts` replays from `initial` on every call | `src/observe/trace.rs:127-170`; `wasm/src/lib.rs:714-743`; UI `MappingContent.tsx:32` | O(steps × size) memory: a 2 MiB context over 500 steps is ≈1 GB in the browser; per-click work is O(steps²) | A wasm `Replay` handle parsed once, `context_at(i)`, checkpoints every N steps, cached parsed paths | Medium / M |
| PERF-7 | should | The wasm boundary goes JsValue → text → `Value` → `OwnedDataValue` | `wasm/src/lib.rs:215-238, 285-295` | Three representations per payload; the cost has never been measured | Deserialize straight to `OwnedDataValue`; add a wasm benchmark | Low / S |
| PERF-8 | should | Per fan-out element: `format!` plus `Arc` plus deep clone for `into` under default capture | `src/run/fan_out.rs:296-299, 340` | Small next to the call, but on the `fan_out` gate path; the null array is also built twice (trace and context) | Reuse one path buffer; build the null array once | Low / S |
| PERF-9 | should | `Run` clones the `Program` `Arc` per message | `src/run/engine.rs:351` | One contended atomic pair per message, used only to resolve ids | Borrow, or store `Arc<str>` ids in the record; measure on spawned rows | Low / S–M |
| PERF-10 | should | `RunEnv` (25 fields) is rebuilt per workflow, with the op counter threaded in and out by hand | `src/run/interp.rs:112-165`; `engine.rs:254-290` | A cost and a correctness risk (the 64-op cadence) | Split `RunScope` (once per run) from `WfScope` | Low / M |
| PERF-11 | should | Ids stored three times, as `String` and `Arc<str>` | `src/program/link.rs:125-128, 232, 252`; `ops.rs:151-153` | Two sources of truth on the run path | Store `Arc<str>` once | Low / S–M |
| PERF-12 | over-engineering | Smaller items | `SharedOperator` double vtable (`engine.rs:20-34`); a dynamic `Expr<Path>` re-parses per eval (`expr.rs:226`); secrets deep-copied per read (`secrets.rs:114`); `WRITES_POOLED = 16` per thread with a per-node `Vec` (`writes.rs:456`); `Interpolated` literal subtrees cloned O(n·depth) at compile (`document.rs:296-309`) | Each is small | Fix opportunistically; do not schedule | Low / S |

---

## 6. Dependency coupling

<a id="dep-2"></a>
### DEP-2 · `ExactValue` re-implements datavalue's deserializer to keep `2.0` a float
**Severity:** Medium · **Tag:** should · **Effort:** M (upstream) · **Verified:** ✔

- **Where:** `src/base/context.rs:159-250`.
- **Reasoning:** a full visitor copy that has to track every new datavalue
  variant (datetime, tensor) by hand. The upstream crate is the right place
  for float preservation.
- **Remediation:** add a `preserve_float` mode upstream, then delete
  `ExactValue`.

<a id="dep-3"></a>
### DEP-3 · Operator families as cargo features multiply the build matrix
**Severity:** Medium · **Tag:** should · **Effort:** M · **Verified:** ✔

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
  `datetime` out at runtime also reverts its string comparison on `==`, which
  is pinned by `datetime_feature_changes_plain_string_comparison`. Keep
  `tensor` and `budget` as features.

---

## 7. Test-suite debt

<a id="tst-3"></a>
### TST-3 · Two golden mechanisms; orphans detected for the corpus only
**Severity:** Low-Medium · **Tag:** should · **Effort:** S · **Verified:** ✔

- The integration suite has one golden module,
  `tests/it/support/golden.rs` (`writing()` reads
  `DATAFLOW_GOLDEN_WRITE == "1"`), and the corpus fails on a golden with no
  case (`tests/it/corpus.rs:83-88`).
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
**Severity:** Medium · **Tag:** should · **Effort:** M · **Verified:** ✔

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
**Severity:** Medium · **Tag:** should · **Effort:** S–M · **Verified:** ✔

- **Where:**
  - `tests/golden/lib/run/policy.json:8-26`, `builtins/parse.json` (quick-xml messages); `src/function/builtins/parse.rs:364`
  - eleven datalogic "Logic evaluation error: Thrown" strings (lib goldens `parse.json`, `publish.json`, `corpus/expressions/condition_error.json`, `tests/it/run_loops.rs`)
  - `tests/it/derive.rs:286` (serde's `unknown variant`)
  - ≈94 message-text assertions in total
  - `corpus/src/normalise.rs` normalises only timestamps, durations and the engine version
- **Reasoning:** a patch bump of quick-xml, datalogic-rs or serde breaks
  goldens that have nothing to do with the change. This has already recurred:
  datalogic 5.8.1 changed one budget error's "in operator: `>`" to
  "`reduce`" in `tests/golden/corpus/expressions/budget.json`.
- **Remediation:** normalise dependency-originated messages in goldens to
  `<dependency message>`, and assert text only where it is our own wire
  contract.

<a id="tst-6"></a>
### TST-6 · Engine config equality is asserted by slicing `Debug` output
**Severity:** Medium · **Tag:** should · **Effort:** S · **Verified:** ✔

- **Where:** `tests/it/error_context.rs:791`, `tests/it/observer.rs:390`, `tests/it/run_basics.rs:867` (`debug[debug.find("config:")..]`).
- **Reasoning:** these tests are about reload semantics, but they break if a
  field is added to `Engine` or the field order changes.
- **Remediation:** a `#[doc(hidden)] Engine::config()` accessor, comparing
  `EngineConfig` values. (The `secrets_isolation.rs` `Debug` checks are
  correct; leaks through `Debug` are what they test.)

<a id="tst-7"></a>
### TST-7 · The path tests' generators: no 65-segment path, the RNG written twice
**Severity:** Low · **Tag:** should · **Effort:** S · **Verified:** ✔

- **Where:** `tests/it/path.rs:9-23` and `src/state/tests/random.rs:11-35`
  each write the xorshift RNG; neither generates a path past
  `MAX_PATH_SEGMENTS` (64): they make 1–4 and 1–5 segments.
- The corpus model has the index cap, the random test generates `1048576`,
  and an exhaustive small-universe test holds the owned and arena rules to
  the model.
- **Remediation:** a 65-segment path in the generator; share the RNG
  through the corpus crate.

<a id="tst-8"></a>
### TST-8 · Hard-coded counts and magic thresholds
**Severity:** Medium · **Tag:** should · **Effort:** S · **Verified:** ✔ (61 commits touched CLAUDE.md's count line)

- **Where:** CLAUDE.md "The counts" (1037/887/2/18/130/859/869);
  `tests/it/repository_definitions.rs:33` (`MINIMUM_DEFINITIONS = 150`).
- **Reasoning:** nothing enforces the counts, so they drift, or every test PR
  has to edit CLAUDE.md, and agents treat the numbers as an oracle.
- **Remediation:** drop the absolute counts, or generate and check them in
  CI.

<a id="tst-9"></a>
### TST-9 · Property-test knobs: fixed seeds, `DATAFLOW_V4_DEEP`, a wall-clock assertion, eager formatting
**Severity:** Medium-Low · **Tag:** should · **Effort:** S · **Verified:** ✔

- **Where:**
  - Seeds and counts: `src/state/tests/run_context.rs:232-259`, `writes.rs:486-539`, `splice_property.rs:175-200` (`0xa11ce` at `:177`); no `DATAFLOW_PROPTEST_SEED`.
  - `random.rs:231-234`: the knob is still named `DATAFLOW_V4_DEEP`.
  - `src/state/size.rs:68`: `assert!(per.as_micros() < 50)` in a debug test, on a machine with permanent Docker load.
  - `splice_property.rs:183, 300`: `format!("{op:?} on {owned:?}")` on each of 50k passing cases.
  - A hand-kept feature list in `cfg_attr(any(feature=…), ignore)` (`run_context.rs:232-246`).
- **Reasoning:** fixed seeds turn the property tests into a frozen
  regression set that never explores new inputs, and that set is what holds
  the path rules' appliers together. The timing assertion is a flake source.
  The eager formatting spends part of the one-minute budget for nothing.
- **Remediation:** one `random::config()` that reads `DATAFLOW_PROPTEST_SEED`
  and `DATAFLOW_DEEP` (dropping "V4") and prints the seed on failure. Format
  lazily. Replace the timing assertion with a node-visit counter. Keep the
  feature gate in one place.

<a id="tst-10"></a>
### TST-10 · Test scaffolding and fault hooks woven into production modules
**Severity:** Medium-Low · **Tag:** should · **Effort:** M · **Verified:** ✔

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
**Severity:** Low · **Tag:** over-engineering · **Effort:** M · **Verified:** ✔ (line counts)

- **Large files:** `tests/it/trace.rs` 1867, `tests/it/host_calls.rs`
  1812, `tests/it/parse.rs` 1438, `tests/secrets_isolation.rs` 1382 lines.
  Splitting them by sub-topic is churn for little gain; leave them unless a
  file is being edited anyway.
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

## 8. CI, release and tooling

<a id="ci-2"></a>
### CI-2 · Four artifacts version-locked by hand in eight or more places, checked only at tag time
**Severity:** Medium · **Tag:** must · **Effort:** M · **Verified:** ✔

- **Where:** root `Cargo.toml:3, 94`, `derive/Cargo.toml:3`, `wasm/Cargo.toml:3, 45`, `ui/package.json`, `ui/package-lock.json`, 7 install snippets. The check lives in `release.yml:42-97` (tags only), using `grep | head -1 | sed`. There is no `[workspace.package]` or `[workspace.dependencies]`.
- **Remediation:** `[workspace.package] version` and `[workspace.dependencies]`.
  Extract the agreement check into a script that also runs on PR CI.

<a id="ci-3"></a>
### CI-3 · The `check` job is one serial job with 18+ cargo invocations and no timeouts
**Severity:** Medium · **Tag:** should · **Effort:** S–M · **Verified:** ✔

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
**Severity:** Medium · **Tag:** should · **Effort:** S · **Verified:** ✔

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
**Severity:** Low-Medium · **Tag:** should · **Effort:** S each · **Verified:** ✔

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
  only smoke mode and the `GATE` line contract. `orion.sh` holds an 83-line
  Python heredoc that regex-scrapes another repository's human-readable
  output. *(The `orion.sh` heredoc is over-engineering; replace or delete
  it rather than hardening it.)*
- **`docs/build-with-debugger.sh`** builds a different site from `docs.yml`
  (no wasm overlay, a stale playground wasm). Use one script for both.

---

## 9. UI (`@goplasmatic/dataflow-ui`)

<a id="ui-1"></a>
### UI-1 · The definition types are a hand mirror, and the v3→v4 policy table is re-implemented in TypeScript
**Severity:** Medium · **Tag:** must · **Effort:** M · **Verified:** ✔

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
**Severity:** Medium · **Tag:** should · **Effort:** M · **Verified:** ✔

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

## 10. Documentation and provenance hygiene

<a id="doc-2"></a>
### DOC-2 · Comments and docs name v3 symbols or behaviour that no longer exists
**Severity:** Low · **Tag:** should · **Effort:** S · **Verified:** ✔

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
**Severity:** Low · **Tag:** should · **Effort:** S · **Verified:** ✔

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

## 11. Orion follow-ups (not dataflow-rs work)

<a id="orion-follow-ups"></a>
**Tag:** must · the pin must move through the 4.0 batches.

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
  `ctx.info().routing_bucket`, which the batch added. The comment at
  `channel_call.rs:302` is wrong. v3 behaves the same way.
- Orion hard-codes `"validate"` beside `BUILTIN_FUNCTIONS`
  (`engine/handlers.rs:572-599`): replace the copy with the new public
  `def::is_reserved_function`.
- **From the second batch:**
  - A failed `map` mapping records `MAPPING_ERROR` before the
    `TASK_STATUS_ERROR`, and a `filter` condition error records
    `EVALUATION_ERROR`. `has_errors()`, DLQ and retry don't move; error
    summaries, the response envelope and `metadata._orion_errors` gain one
    entry each (the 32-record limit fills faster in loops). A `map` or
    `filter` over the ops budget now fails its task with `BUDGET_EXCEEDED`.
  - The engine is on datalogic-rs 5.8.2 and datavalue-rs 0.3.2: re-lock
    (v3.15's `"5.7"` admits them), fix the `orion-server/Cargo.toml:91`
    comment, and run `operator_names_equal_the_v3_engines`,
    `tensor_operators_are_the_tensor_family`, the forbidden-operator tests
    and `bench/orion.sh`. `expr::Family::Flagd` is gone (Orion does not use
    it); an operator family is on only through dataflow-rs's own feature.
  - Definition fields are read through accessors (`def.id()`,
    `task.function().name()`); O12/O14's reference checker over
    `WorkflowDef::walk` uses them.
- Bump the pin, then run Orion's v3/v4 twin tests and the quarantine-verdict
  test. The batch added one Rejected rule (`INVALID_MAPPING` for a constant
  non-object at a root), which no Orion definition hits.
