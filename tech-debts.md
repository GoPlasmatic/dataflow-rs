# Tech debt register: dataflow-rs

## Priority queue

| ID | Tag | Severity | Effort | Summary |
|---|---|---|---|---|
| [Orion follow-ups](#orion-follow-ups) | must | — | — | Required in Orion's `dataflow-4` when its pin moves past the batch. |
| [DEP-3](#dep-3) | should | Medium | M | Select operator families at runtime through `ExprEngineBuilder`. |
| [DEP-2](#dep-2) | should | Medium | M | Upstream float preservation in `datavalue-rs` and remove `ExactValue`. |
| [TST-10](#tst-10) | should | Medium-Low | M | Move `writes.rs` test helpers and fault hooks out of production paths. |
| [DOC-3](#doc-3) | should | Low | S | Tag elided `json` fences as `jsonc`/`text` and validate `json` fences. |

---

## 1. Dependency coupling

<a id="dep-2"></a>
### DEP-2 · `ExactValue` re-implements datavalue's deserializer to keep `2.0` a float
**Severity:** Medium · **Tag:** should · **Effort:** M (upstream)

- **Where:** `src/base/context.rs:159-250`.
- **Reasoning:** a full visitor copy that has to track every new datavalue
  variant (datetime, tensor) by hand. The upstream crate is the right place
  for float preservation.
- **Remediation:** add a `preserve_float` mode upstream, then delete
  `ExactValue`.

<a id="dep-3"></a>
### DEP-3 · Operator families as cargo features multiply the build matrix
**Severity:** Medium · **Tag:** should · **Effort:** M

- **Where:** `Cargo.toml:34-62`; `src/expr/operators.rs:66-83` (`FAMILIES`, built by `#[cfg(feature)]`); `.github/workflows/ci.yml` (`families` matrix at `:154-194`).
- **Reasoning:**
  - 12 CI configurations (2 in `check` plus 10 `families` matrix legs).
  - Feature unification lets another crate change a host's operator set.
  - The engine already calls datalogic's `with_families(...)`
    (`engine.rs:343`, `operators.rs:185`), but only with the cfg-built
    const, so the runtime switch exists and goes unused.
- **Remediation:** compile the families in and select them at runtime through
  `ExprEngineBuilder`, defaulting to Core only. First verify that leaving
  `datetime` out at runtime also reverts its string comparison on `==`, which
  is pinned by `datetime_feature_changes_plain_string_comparison`. Keep
  `tensor` and `budget` as features.

---

## 2. Test-suite & documentation debt

<a id="tst-10"></a>
### TST-10 · Test scaffolding and fault hooks woven into production modules
**Severity:** Medium-Low · **Tag:** should · **Effort:** M

- **Where:** `src/run/interp.rs:684-685` (`#[cfg(test)] let result = record::failing_after_its_writes(result);` on the built-in call path; another `cfg(test)` at `:431`); `record.rs:83-84` (`CLOCK_READS`); seventeen `#[cfg(test)]` items interleaved in `src/state/writes.rs` (971 lines: `list`, `fault`, `OwnedAt`, `Dirty`…), plus `splice.rs:409-455`.
- **Reasoning:**
  - Lib unit tests run a slightly different interpreter from the shipped one.
  - About 30% of `writes.rs` is test scaffolding, which hides the trie's real size (≈700 lines).
  - The fault hook simulates a built-in that writes and then fails.
- **Remediation:** move scaffolding into `src/**/tests/`. Inject faults with
  a crate-private test built-in rather than a `cfg(test)` branch in `interp`.

<a id="doc-3"></a>
### DOC-3 · Elided JSON fences in docs use `json` instead of `jsonc` or `text`
**Severity:** Low · **Tag:** should · **Effort:** S

- **Where:** 49 of 153 `json` fences in `docs/src` and `README.md` use `...` elisions or `//` comments (`tests/it/repository_definitions.rs:657-684`).
- **Remediation:** tag elided blocks `jsonc` or `text`, assert that every `json` fence parses as valid JSON, and wrap task-level fragments in a synthetic workflow.

---

## 3. Performance candidates (gated on `bench/gate.py compare`)

Run `bench/gate.py compare` with A/B interleaving before acting on any item below.

| ID | Item | Where | Remediation | Sev / Effort |
|---|---|---|---|---|
| PERF-2 | `reads_secret` walks raw JSON per field on every `Ctx::input()` | `src/function/ctx.rs:608-632`; `src/expr/expr.rs:335` | Store `reads_secret: bool` per `Expr`/hole at compile; `constant_value` returns a borrow | Medium / S |
| PERF-3 | `Templated::get` linear string scan per access; `resolve_fields` allocates names per call | `src/function/templated.rs:231-262` | Resolve field handles in `prepare` | Low-Med / S–M |
| PERF-4 | Parse deep-copies the document under the default (compat-on) path, then clones fields again | `src/def/parse.rs:108, 380, 484, 552, 606` | `parse_owned(Value)`, or skip the clone when a pre-scan finds no v3 keys | Med-Low / M |
| PERF-5 | Write-log trie scans children as a linked list; each splice copies whole pair slices | `src/state/writes.rs:416-423, 552-562, 609-613`; `splice.rs:276-288` | Add a wide-object gate benchmark; if confirmed, add a hash index past N children | Medium / M |
| PERF-6 | Trace replay clones full context per step; wasm `contexts()` collects all of them | `src/observe/trace.rs:127-170`; `wasm/src/lib.rs:714-743`; `ui/src/.../MappingContent.tsx:32` | Wasm `Replay` handle parsed once, `context_at(i)`, checkpoints every N steps | Medium / M |
| PERF-7 | Wasm boundary goes `JsValue` → text → `Value` → `OwnedDataValue` | `wasm/src/lib.rs:215-238, 285-295` | Deserialize straight to `OwnedDataValue`; add a wasm benchmark | Low / S |
| PERF-8 | Per fan-out element `format!` + `Arc` + deep clone for `into` under default capture | `src/run/fan_out.rs:296-299, 340` | Reuse one path buffer; build the null array once | Low / S |
| PERF-9 | `Run` clones the `Program` `Arc` per message | `src/run/engine.rs:351` | Borrow, or store `Arc<str>` ids in the record | Low / S–M |
| PERF-10 | `RunEnv` (22 fields) rebuilt per workflow, op counter threaded in and out by hand | `src/run/interp.rs:124-169`; `engine.rs:254-286` | Split `RunScope` (once per run) from `WfScope` | Low / M |
| PERF-11 | Ids stored three times, as `String` and `Arc<str>` | `src/program/link.rs:125-128, 232, 252`; `ops.rs:151-153` | Store `Arc<str>` once | Low / S–M |

---

## 4. Orion follow-ups (cross-repo)

<a id="orion-follow-ups"></a>
**Tag:** must · apply in Orion when bumping the `dataflow-rs` pin.

- Delete the arms for the removed `ErrorKind::Parse` and
  `ErrorKind::FunctionNotFound` (`engine/functions/mod.rs:370-372`,
  `engine/error.rs:350-351`). O12's port of `validation/workflows.rs:565-570`
  needs a v4 stand-in for v3's `MissingHandler` (`UNKNOWN_FUNCTION`).
- Close K30 (key order). Correct the two stale "no `preserve_order`" comments
  (`query/keyset.rs:40-44`, `connector/config.rs:304`).
- **Partial rollouts on Kafka and `channel_call`:** both pass no routing bucket
  (`kafka/consumer/process.rs:341-354`, `engine/functions/channel_call.rs:302, 642-647`).
  Kafka should hash the record key (as cron does at `cron/worker.rs:530-546`);
  `channel_call` should forward `ctx.info().routing_bucket`.
- Replace Orion's hard-coded `"validate"` check beside `BUILTIN_FUNCTIONS`
  (`engine/handlers.rs:572-599`) with `def::is_reserved_function`.
- Account for `MAPPING_ERROR` (before `TASK_STATUS_ERROR` on failed `map`),
  `EVALUATION_ERROR` (on failed `filter` condition), and `BUDGET_EXCEEDED`
  (on `map`/`filter` over ops budget).
- Re-lock `datalogic-rs` 5.8.2 and `datavalue-rs` 0.3.2, update
  `orion-server/Cargo.toml:91`, and use `WorkflowDef` / `TaskDef` accessors
  (`def.id()`, `task.function().name()`).
