# The golden corpus

The corpus records what dataflow-rs 3.x does to a message, case by case, so that v4 can be held to
it. v3 is the oracle: a recording is what v3 did, never what someone expected it to do.

- **Cases** live under `cases/<area>/<name>.json`, and each case's recording sits beside it as
  `<name>.v3.json`.
- **`contract.json`** lists every row the cases must cover:
  - every behaviour row of the feature contract;
  - every break in the compatibility document;
  - every error code.

  A row v3 cannot record is marked `"v3": false`, with the reason.
- **`RECORDED.json`** says what the recordings were made with:
  - the engine version;
  - the hash of the `src/` tree;
  - the datalogic-rs version;
  - the toolchain.

## The two crates

| Crate | What it holds |
| --- | --- |
| `corpus/` (`dataflow-corpus`) | The case format, an order-preserving JSON value, normalisation, the comparator, v3's path rules, trace replay, and the coverage tests. It does not depend on dataflow-rs |
| `corpus/v3/` (`dataflow-corpus-v3`) | v3's driver, which runs a case and records it. Also the `script` host function, the observer recorder, the log capture, `record`, and the green test |

When v4 replaces v3's API on `main`, `corpus/v3` stays on `v3-maintenance`, and a v4 driver reads
the same cases through `dataflow-corpus`.

## v4's driver

`corpus/v4/` (`dataflow-corpus-v4`) reads the same cases through `dataflow-corpus`. In phase 3 it
compiles every definition (`tests/definitions.rs`): v4's `parse` issues, then those of a compiler
holding `script` and the case's operators and secrets. A definition no deviation covers must report
what v3 recorded (`authored` and `checked`), as `(code, severity, path)`. A deviated one must report
exactly its row of `expect::DEVIATED`, unfiltered. Nothing is skipped. Later phases extend it to
runs.

## A case

```json
{
  "description": "One sentence: the behaviour this case pins.",
  "covers": ["schema.priority"],
  "mirrors": ["tests/task_groups.rs::terminal_halt_stops_only_its_own_workflow"],
  "requires": { "on": ["budget"] },
  "engine": { "secrets": {}, "ops_budget": 40, "error_context": { "path": "metadata.failures", "limit": 2 },
              "operators": ["shout"], "observer": true, "logs": "warn" },
  "workflows": [ { "id": "w", "name": "w", "tasks": [] } ],
  "steps": [
    { "run": { "message": { "id": "m1", "payload": {}, "data": {} }, "channel": "c",
               "routing_bucket": 7, "capture_changes": true, "audit_mode": { "last": 2 },
               "trace": { "redact_paths": [] }, "same_as": 0 } },
    { "reload": { "workflows": [] } }
  ],
  "deviations": [ { "break": "4", "paths": ["/steps/0/run/message/errors"], "note": "..." } ]
}
```

**Required fields.** Only `description`, `covers` and `workflows` are required, and unknown keys
are refused.

- **`workflows`** are raw definitions, each parsed on its own, so a refused definition does not
  hide the others.
- **A definition that `build` refuses gets its own case.** One refusal fails the whole build, and
  the case would then record no run.
- **Each run is made twice.** The plain run gives the message, the outcome, and the observer events
  and log lines when the case asks for them. The traced run gives the trace, plus whether it left
  the same message.
- **`same_as: n`** requires this run's message to equal step `n`'s.

**`script` is the one host function.** Its behaviour is all in its config:

- `set`: writes, each a `value` template, a `get` path or a `raw` value;
- `errors`, `identity` and `yields`;
- an `outcome` template, evaluated last: `"success"`, `"skip"`, `"halt"`, `{"status": n}`, or
  `{"error": {"variant", "code", "message", "status", "retryable"}}`.

See `v3/src/script.rs`. v4 ports exactly this function to replay the corpus.

## Recording

```bash
corpus/record.sh            # every case
corpus/record.sh fan_out/   # cases whose name contains the filter
```

**It records twice.**

- **The default build** writes every case that needs no feature on.
- **The all-features build** writes the cases that need a feature, and checks that every other case
  records identically with everything on. A case that silently depends on a feature fails there.

Each case runs twice per build, and a case whose two recordings differ is refused.

**Read every new or changed recording before committing it.** Check that it shows the behaviour
its case describes. If it does not, either the case is wrong or v3 contradicts the contract:

- **A wrong case** is fixed.
- **A contradiction** is a finding. The recording keeps what v3 does, and the contract is amended.

**Once v3 is frozen,** a fix on `v3-maintenance` that changes behaviour re-records its cases there,
and the changed recordings are copied to `main` the same week.

## What a recording compares

**Normalisation.** Each value is replaced in place, so key order is still compared:

| Field | Placeholder |
| --- | --- |
| `metadata.processed_at` | `<timestamp>` |
| `metadata.engine_version` | `<engine_version>` |
| Every audit entry's and error's `timestamp` | `<timestamp>` |
| An error-context record's `timestamp` | `<timestamp>` |
| A trace step's `started_at` | `<timestamp>` |
| A trace step's `duration_us` | `<duration>` |

Every message id is set by the case, so none is generated.

**The comparator compares everything, object key order included.** Key order is wire data. The
only exceptions are diagnostics whose wording v4 owns, listed in `compare::IGNORED`:

- the messages of issues;
- the message of a parse error;
- the message of a build error;
- the message of a reload error.

Codes, severities and paths are compared.

**Deviations are for v4.** Where v4 must answer differently, the case names the break that allows
it and the recording paths it may change, as JSON pointers with `*` for any segment. v3's check
ignores them; v4's ignores those paths and nothing else.

## Features

**Operator families change what a definition means,** so a case that depends on one says so in
`requires`. CI runs the corpus in two builds:

- **all-features,** through `cargo test --workspace --all-features`;
- **default,** through `cargo test -p dataflow-corpus-v3`.

A case must run in one of them, so it never needs features both on and off.

**How the driver reads features.** It detects families from the engine's operator names rather
than from `cfg!`, because `wasm/` turns `all-operators` on in every `--workspace` build. `budget` is
the exception: it adds no operator, so it is read from this crate's own feature.

## Replay

**What v4's trace holds.** It is the initial context plus each step's changes, and the UI rebuilds
each snapshot by replaying them.

**Measuring the gap.** `cargo run -p dataflow-corpus --bin replay_report` replays v3's own recorded
traces and lists every snapshot path the changes do not reproduce. Each one is something v4's trace
must carry, or its runner must derive. `tests/replay.rs` pins that list.
