# Test Corpus (`dataflow-corpus`)

The corpus is a collection of declarative workflow test cases—each pinning a
specific engine behavior—along with the internal `dataflow-corpus` crate that
loads and models them. `dataflow-corpus` is never published and does not depend
on `dataflow-rs`: its order-preserving JSON value, dot-path resolver, and trace
replayer act as an independent reference model that the test suite checks the
engine against.

- **Cases** live under `cases/<area>/<name>.json`, grouped by area: `breaks`,
  `builtins`, `expressions`, `fan_out`, `loop`, `rollout`, `run`, and `schema`.
- **The crate** (`src/`) provides the case schema, an order-preserving `Json`
  value, output normalization, the reference dot-path model, and the reference
  trace replay model.

## Where the Corpus Is Used

| Test / Module | How it uses the corpus |
| --- | --- |
| `tests/it/corpus.rs` | Runs every case matching the active feature flags through `tests/it/support/corpus_driver.rs` and compares the output against its golden file (`tests/golden/corpus/<area>/<name>.json`). |
| `tests/it/compat.rs` | Runs every case's workflow definitions (including reload steps) through the 3.x compatibility reader and `migrate_v3`. |
| `tests/ts_types.rs` | Runs every case with tracing and timings enabled to verify that all serialized `Message`, `Trace`, `Run`, and `Issue` payloads conform to the generated TypeScript declarations. |
| `tests/it/repository_definitions.rs` | Verifies that every workflow definition under `cases/` parses and compiles cleanly. |
| `tests/it/path.rs` | Property-tests `dataflow_rs::Path` against `dataflow_corpus::path` across randomized contexts and paths. |
| `tests/it/trace.rs`, `tests/it/error_context.rs` | Verifies `Trace::contexts` against `dataflow_corpus::replay`. |
| `src/run/policy.rs` (unit tests) | Uses `normalise` and `Json` for deterministic assertions. |

`load_cases()` returns all cases sorted by file path and panics if any file
fails to parse, ensuring broken test cases are never silently ignored.

## Invariants Checked on Every Corpus Run

For each case, the test driver parses and compiles every workflow definition
independently against a registry containing the test `script` host function and
an expression engine configured with the case's secrets, custom operators, and
operation budget. Valid workflows are then linked into an `Engine`.

Every `run` step is executed twice—once without tracing and once with tracing—and
the driver verifies that:

- The traced run produces the exact same final `Message` as the untraced run.
- The final context reconstructed by `Trace::contexts` and by
  `dataflow_corpus::replay` matches `message.context()` (including object key order).
- Serializing and deserializing the `Message` and `Trace` round-trips byte-for-byte
  and replays identically.
- Any step with `"same_as": n` produces the exact same message as step `n`.

Finally, the full output (compilation issues, `Run` report, normalized message,
trace steps, and observer events) is compared against the golden file in
`tests/golden/corpus/`. Set `DATAFLOW_GOLDEN_WRITE=1` to update golden files when
intentionally changing engine behavior.

## Case Format

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

Only `description` and `workflows` are required. All structs use
`#[serde(deny_unknown_fields)]` so typos in case files fail immediately.

- **`workflows`**: Raw JSON workflow definitions. Each workflow is parsed and
  compiled individually so a rejected definition does not prevent valid sibling
  workflows from linking.
- **`requires`**: Cargo features that must be enabled (`on`) or disabled (`off`)
  for the case to run (`datetime`, `tensor`, `budget`).
- **`engine`**: Configures optional `families`, `secrets`, `ops_budget`,
  `error_context`, custom `operators` (such as `shout`), and whether to attach a
  recording `observer`.
- **`steps`**:
  - **`run`**: Runs a message (`id` defaults to `"m1"`, `payload` to `{}`) with
    optional `channel`, `routing_bucket`, `capture_changes`, `audit_mode`
    (`"full"`, `"off"`, or `{"last": n}`), `trace` (defaults to `true`), and
    `same_as`.
  - **`reload`**: Replaces the engine's compiled program with a new list of
    `workflows`.

### The `script` Test Host Function

Corpus cases use a single configurable host function named `script`. Its behavior
is driven entirely by its task `input`:
- `set`: Writes to perform (`value` expression, `get` path, or `raw` value)
- `errors`: Error records to push onto the message
- `identity`: Asserts the current `CallIdentity`
- `yields`: Number of async yields to perform before returning
- `outcome`: Outcome to return (`"success"`, `"skip"`, `"halt"`, `{"status": n}`,
  or `{"error": {"variant", "code", "message", "status", "retryable"}}`)

## Reference Modules in `dataflow-corpus`

- **`json`**: An order-preserving JSON value type. Because `dataflow-rs` preserves
  object key insertion order on the wire, `Json` stores objects as ordered key-value
  pairs so equality checks verify key ordering as well.
- **`normalise`**: Replaces non-deterministic or environment-specific fields in
  place (preserving key order) before golden-file comparison:

  | Field | Placeholder |
  | --- | --- |
  | `metadata.processed_at` | `<timestamp>` |
  | `metadata.engine_version` | `<engine_version>` |
  | Audit entry and error `timestamp` | `<timestamp>` |
  | Error-context record `timestamp` | `<timestamp>` |
  | External library error details | `<dependency message>` |

- **`path`**: Reference implementation of dot-path resolution on `Json`. Leading
  `#` escapes force object-key lookup (so `#20` is treated as object key `"20"`
  rather than array index `20`). Missing intermediate containers are created on
  demand as arrays when the next segment parses as an index, or objects otherwise.
- **`replay`**: Reference implementation of trace replay. Reconstructs per-step
  contexts from `trace.initial` and each step's `changes`. Changes at a context
  root (`data`, `metadata`, `temp_data`) merge keys when `replaced` is `false`
  (as emitted by `map`) and replace the entire root when `replaced` is `true`
  (as emitted by `ctx.set` at a root).
