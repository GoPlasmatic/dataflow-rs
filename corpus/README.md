# The corpus

The corpus is a set of workflow cases, each pinning one behaviour of the engine, and the crate
that reads them (`dataflow-corpus`, never published). The crate does not depend on `dataflow-rs`:
its JSON value, its path rules and its trace replay are an independent model that the engine's
tests hold the engine to.

- **Cases** live under `cases/<area>/<name>.json`, one behaviour each, grouped by area: `breaks`,
  `builtins`, `expressions`, `fan_out`, `loop`, `rollout`, `run` and `schema`.
- **The crate** (`src/`) holds the case format, an order-preserving JSON value, normalisation, the
  dot-path model and the trace replay model.

## Who reads it

| Reader | What it takes from the corpus |
| --- | --- |
| `tests/it/corpus.rs` | Every case the build satisfies, run through the driver (`tests/it/support/corpus_driver.rs`) and held to its golden file, `tests/golden/corpus/<area>/<name>.json` |
| `tests/it/compat.rs` | Every case's definitions, its reload steps' included, through the compat reader and `migrate_v3` |
| `tests/ts_types.rs` | Every case through the same driver, traced with timings, so every key a corpus message, trace, `Run` and issue writes is held to its TypeScript declaration |
| `tests/it/repository_definitions.rs` | Every definition under `cases/` parses and compiles, as every other definition in the repository does |
| `tests/it/path.rs` | `path`: `Path` against the model, on random contexts and paths |
| `tests/it/trace.rs`, `tests/it/error_context.rs` | `replay` and `normalise`: `Trace::contexts` against the model's replay |
| `src/run/policy.rs`'s tests | `normalise` and `Json` |

`load_cases()` returns every case, sorted by path, and panics naming the file when one does not
parse, because a case that silently drops out tests nothing; `case_name` is a case's path below
`cases/` without `.json`, its name in a report.

## What a run of the corpus checks

The driver parses and compiles each definition on its own, through a registry holding `script`
and an expression engine with the case's secrets, custom operators and ops budget, then links the
compiled ones into one engine. Each run step runs plain and then traced, and every run step,
whatever its case pins, must pass:

- the traced run's message equals the plain run's;
- the trace's last context is the message's, through `Trace::contexts` and through this crate's
  `replay`, key order included;
- the message and the trace read back byte for byte, and the trace read back replays the same;
- a `same_as: n` step's message equals step `n`'s.

A case with run steps fails when a definition is refused as `UNKNOWN_FUNCTION`, when no
definition compiled, or when a reload compiles none: each would run less than the case says. A
case that pins refusals carries no run step. What each case then does (every definition's issues,
each run's `Run`, normalised message and trace steps, the observer's events) is held to its
golden file; `DATAFLOW_GOLDEN_WRITE=1` rewrites them, and the diff is reviewed against each
description. `requires` decides which builds run a case: in the default build
`expressions/budget` and `expressions/families_on` are skipped, and with every feature on
`expressions/families_off` is, both pinned.

## A case

```json
{
  "description": "One sentence: the behaviour this case pins.",
  "requires": { "on": ["budget"] },
  "engine": { "secrets": {}, "ops_budget": 40, "error_context": { "path": "metadata.failures", "limit": 2 },
              "operators": ["shout"], "observer": true },
  "workflows": [ { "id": "w", "name": "w", "tasks": [] } ],
  "steps": [
    { "run": { "message": { "id": "m1", "payload": {}, "data": {} }, "channel": "c",
               "routing_bucket": 7, "capture_changes": true, "audit_mode": { "last": 2 },
               "trace": false, "same_as": 0 } },
    { "reload": { "workflows": [] } }
  ]
}
```

Only `description` and `workflows` are required, and every type refuses unknown keys, so a typo in
a case fails loudly instead of silently testing less (`src/case.rs`). `Case::validate` checks what
serde cannot: the `requires` names, `on` and `off` not both set, an `ops_budget` only with
`budget` required, and a `same_as` naming an earlier run step.

- **`workflows`** are raw definitions, each parsed and compiled on its own, so a refused definition
  does not hide the others: the engine links the ones that compiled.
- **`requires`** names the cargo features a case needs on or off (`case::FEATURES`: `datetime`,
  `tensor` and `budget`). CI runs two builds, default (nothing on) and `--all-features`
  (everything on), so a case never needs features both on and off.
- **`engine`** configures the engine the steps run on: its `families` (`case::FAMILIES`),
  `secrets`, `ops_budget`, an `error_context`, custom `operators` by name (`shout`), and an
  `observer` whose events are kept.
- **A `run` step** runs on a fresh message (`id` defaults to `m1`, `payload` to `{}`) with its
  channel, routing bucket, `capture_changes` and `audit_mode` (`"full"`, `"off"` or
  `{"last": n}`). `trace` is `false` to skip the traced run (default `true`). `same_as: n`
  requires this run's message to equal step `n`'s.
- **A `reload` step** swaps the engine's program for its own `workflows`.

**`script` is the cases' one host function.** Its behaviour is all in its config: `set` (writes,
each a `value` template, a `get` path or a `raw` value), `errors`, `identity`, `yields`, and an
`outcome` template evaluated last (`"success"`, `"skip"`, `"halt"`, `{"status": n}`, or
`{"error": {"variant", "code", "message", "status", "retryable"}}`). The driver registers it,
and refuses any key its config does not know, as every derived config does.

## The tools

**`json`.** An order-preserving JSON value. Key order is wire data: the JSON a message serializes
to keeps its keys in the order they were written, and a removal changes that order. The corpus
keeps objects as ordered pairs, so its equality is order-sensitive whatever map type serde_json
is built with (`dataflow-rs` turns on its `preserve_order`).

**`normalise`.** Replaces the fields that differ between two runs of the same case, each in place,
keeping its key's position, so key order is still compared:

| Field | Placeholder |
| --- | --- |
| `metadata.processed_at` | `<timestamp>` |
| `metadata.engine_version` | `<engine_version>` |
| Every audit entry's and error's `timestamp` | `<timestamp>` |
| An error-context record's `timestamp` | `<timestamp>` |
| A trace step's `started_at` | `<timestamp>` |
| A trace step's `duration_us` | `<duration>` |

Every message id is set by the case, so none is generated.

**`path`.** The engine's dot paths on `Json`. A segment is a key on an object, with one leading `#`
stripped, and an index on an array, parsed from the raw segment, so `#20` is never an index. A
missing container is created on demand: an array when the next segment parses as an index, an
object otherwise. A write either applies in full or does nothing, and the empty path reads the
root and writes nothing.

**`replay`.** Rebuilds a trace's per-step contexts from its initial context and each step's
changes, in order: a `set` of `new_value`, or a removal when `removed` is true. A change at a whole
root (`data`, `metadata`, `temp_data`) merges into it, because that is what `map` does there and
what its change records: only the keys it wrote; a change marked `replaced: true` (a handler's
`ctx.set` at a root) is set whole. `Trace::contexts`, which a host or the UI calls, is a second
implementation of the same rule, and the engine's trace tests hold the two to each other.
