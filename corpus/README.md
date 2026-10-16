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
  - `source`, the registry package it ran (`crates.io dataflow-rs <version>, checksum <sha256>`);
  - the datalogic-rs version;
  - the toolchain.

## The three crates

| Crate | What it holds |
| --- | --- |
| `corpus/` (`dataflow-corpus`) | The case format, an order-preserving JSON value, normalisation, the comparator, v3's path rules, trace replay, and the coverage tests. It does not depend on dataflow-rs |
| `corpus/v3/` (`dataflow-corpus-v3`) | v3's driver, which runs a case and records it, against the published 3.15.0 it pins (`dataflow-rs = "=3.15.0"`; I124, amending I14). Also the `script` host function, the observer recorder, the log capture, `record`, and the green test |
| `corpus/v4/` (`dataflow-corpus-v4`) | v4's driver, which holds v4 to the recordings: `script` ported to v4's `Function`, the recording observer, the declared deviations (`expect`), the definitions check, the runs check and the traces check |

`corpus/v3` stays on `main`, on the published 3.15.0 (I124, amending I14), and `record.sh` records
against that pin; `corpus/v4` reads the same cases through `dataflow-corpus`. A v3 fix on
`v3-maintenance` that changes behaviour re-records there, and `main` bumps the pin and re-records.

## v4's driver

`corpus/v4/` (`dataflow-corpus-v4`) reads the same cases through `dataflow-corpus`. It makes three
checks.

**Definitions** (`tests/definitions.rs`, phase 3). It compiles every definition: v4's `parse` issues,
then those of a compiler holding `script` and the case's operators and secrets. A definition no
deviation covers must report what v3 recorded (`authored` and `checked`), as
`(code, severity, path)`. Before that comparison, v4's own Advisories, those of a code v3 does not
have such as `V3_SPELLING`, are filtered out (`expect::compared`). A deviated one must report
exactly its row of `expect::DEVIATED`, unfiltered, and the row must name exactly the breaks of the
deviations covering it. Nothing is skipped.

**Runs** (`tests/runs.rs`, from phase 4). `run::run_case` builds the case, with its engine config,
and runs its steps:

- **The build.** Each definition is parsed and compiled on its own. One v4 refuses is left out
  when v3's `parse` refused it too, as v3's driver left it out; the definitions check holds the two
  refusals together. Any other refusal means there is no engine: v3's build must have failed too,
  or a deviation must cover `/build`, where v4's build error must be its row of
  `expect::RUN_DEVIATED`. No step is compared, so where v3 built and ran steps, a deviation must
  cover `/steps` too.
- **The budget.** On a `budget` build, the case's compiler applies `engine.ops_budget`. A case that
  sets one must require `budget`, so it never runs without its ceiling.
- **The engine's config.** `engine.observer` attaches the recording observer (`observe::Recorder`),
  one per engine, drained per run step. `engine.error_context` sets an `ErrorContext` at its path,
  with its `limit` or v3's default of 32. A reload keeps both, as v3's did.
- **The skip rule.** A case whose `requires` this build does not satisfy is skipped.
- **A run step** runs on a fresh message with its channel, bucket, `capture_changes` and
  `audit_mode`. It records `{outcome, message, observer, logs, trace}` as v3's driver does,
  normalised the same way, the message with the case's error-context path:
  - The outcome is `"ok"` for a run that completed, with or without error records.
  - `observer`, when the case attaches one, is v3's five events, key for key, made from v4's
    `Event`s, each object's keys in the sorted order v3's recordings hold. `message_started` is `{message_id, workflows_considered}` from
    `RunStarted`. `workflow_started` is `{workflow_id}`. `task_finished` is `{workflow_id,
    task_id, function, status}` from the call's `CallInfo`. `workflow_finished` is `{workflow_id,
    sweeps, halted}`, `halted` when the workflow's exit is `Halted`. `message_finished` is
    `{message_id, errors, stopped_early}`, `stopped_early` when the run ended `Stopped`, `TimedOut`
    or `Cancelled`.
  - `logs`, when the case captures them, is the `log` built-in's lines.
  - `trace`, when the step's trace is on, comes from a second run of the same input, traced with
    no timings, the spec's `changes` and `redact_paths`, and its `max_snapshot_bytes` as the byte
    budget when it is not 0. A traced case keeps `changes` on and sets no `redact_paths` or
    `max_snapshot_bytes`, and the driver asserts it: the trace check replays every trace to v3's
    snapshots and to the message, which a trace without changes, a redacted one or one past a
    budget cannot do, so no deviation could cover one (v4's own tests hold those three). v3's `snapshots`, `mapping_contexts` and `snapshot_audit_trail` have no
    v4 counterpart. It records `{outcome, initial, steps, truncated, message_matches, contexts,
    read_back}`: the traced run's outcome; `initial` and `steps` exactly as v4 serializes them;
    `truncated`; whether the traced run's normalised message equals the plain run's; v4's own
    replay, `Trace::contexts`, one context per step; and whether the trace reads back, below. The
    traced run's events and log lines are dropped, as v3's driver dropped them.
  - `read_back`, v4's own key, says whether the message reads back (I125): `"ok"` when the
    `Message` read from the text v4 wrote writes that text again, byte for byte, or else what
    failed. A trace's `read_back` says the same of the `Trace`, which must also replay
    (`Trace::contexts`) to the contexts the original does, compared as text.

  `script` is v3's, ported to `Function` (`src/script.rs`). Each error it returns carries v3's code
  and v3's text, so its records are v3's.
- **A reload step** parses, compiles and links its definitions through the case's compiler, then
  swaps the engine's program (`Engine::with_program`). It records `"ok"`, or the error of a refused
  build, whose code and message are not compared: v4's code is `REJECTED`, v3's its own error's. A
  reload one engine refuses and the other accepts is still a difference.

The record is compared with `compare::diff`, ignoring:

- `IGNORED`;
- the run outcome's message (v4's `Error` has its own wording; its code and `retryable` are
  compared);
- a refused reload's code;
- the trace, which the traces check compares;
- the run's `read_back`, which v3's recording has no twin of: every one must be `"ok"`;
- every path a deviation covers, except one under a run's trace, which the traces check holds.

At each deviated path, v4's value must be exactly its row of `expect::RUN_DEVIATED`, so a deviation
says what v4 does, not only that it differs. A deviated path where v4's value is v3's fails as
stale: the deviation would declare a difference that is not there. These counts are pinned:

| Count | Default build | `budget` or `--all-features` |
| --- | --- | --- |
| Cases run and compared | 71 | 72 |
| Cases with no build | 10 | 10 |
| Cases needing features this build has the other way | 2 | 1 |
| Deviated pointers, each held to its `RUN_DEVIATED` row | 28 | 28 |
| Run steps whose message reads back byte for byte: every one | 85 | 87 |

`expressions/budget` runs only on a `budget` build. Exactly one of `families_on` and `families_off`
runs in each build. The cases hold 95 run steps: 7 are in cases with no build, one is in the
family case this build skips, and `expressions/budget`'s two run only on a `budget` build.
Every count, here and in the traces table, is the same with serde_json's `preserve_order` on, which
Orion's graph turns on and CI runs: no recording's order follows serde_json's map type.

**Traces** (`tests/traces.rs`, phase 6). For every run step whose recording holds a trace, in a case
that builds and runs in this build, v4's traced run is held to v3's: its outcome (code and
`retryable`, as the plain outcome is), `truncated`, `message_matches`, and its steps, aligned with
v3's by replay (see [Traces](#traces) under Replay). Each traced run must also replay to its own
message, and its trace must read back from the text v4 wrote, byte for byte, replaying to the same
contexts (`read_back`). A deviated pointer under a run's trace is held to its row of
`expect::TRACE_DEVIATED`, each row used exactly once, with the same stale check. These counts are
pinned, the same in every
build (`expressions/budget` turns its trace off, and `families_on` and `families_off` trace alike):

| Count | Every build |
| --- | --- |
| Traced runs compared | 85 |
| Of those, runs whose step list a deviation declares | 1 |
| v3 steps matched by a v4 step, in the other 84 | 239 |
| Of those, snapshots compared with v4's rebuilt context | 227 |
| Of those, snapshots whose records were held to v4's step's `errors`, with a record on either side | 43 |
| v4 steps matched to nothing: the engine's step at a sweep | 41 |
| v4 steps matched to nothing: a task's failure the workflow's policy took | 9 |
| v4 steps matched to nothing: a workflow's own failure | 1 |
| Deviated trace pointers, each held to its `TRACE_DEVIATED` row | 15 |
| Of the matched steps, those whose `status` is held to v3's own audit entry | 215 |
| Traced runs the replay invariant held: every one | 85 |
| v4 steps whose `Trace::contexts` entry equals the corpus replay: every one | 294 |
| Traced runs whose trace reads back byte for byte and replays the same: every one | 85 |
| Traced runs whose steps' records are the message's errors: every one | 85 |

In each build, v3's recordings hold 89 traced runs: the 85 compared, and four in cases v4 does not
build (breaks 2a, 3 and 15), whose `/steps` a deviation covers.

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

See `corpus/v3/src/script.rs`. v4 ports this function to replay the corpus (`corpus/v4/src/script.rs`), with
four differences its module doc lists: one is break 2a, and no case exercises the other three.

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
- **default,** through `cargo test -p dataflow-corpus-v3` and `cargo test -p dataflow-corpus-v4`.

A case must run in one of them, so it never needs features both on and off.

**How the driver reads features.** It detects families from the engine's operator names rather
than from `cfg!`, because `wasm/` turns `all-operators` on in every `--workspace` build. `budget` is
the exception: it adds no operator, so it is read from this crate's own feature.

## Replay

**What v4's trace holds.** It is the initial context plus each step's changes, and the UI rebuilds
each snapshot by replaying them.

**Measuring the gap.** `cargo run -p dataflow-corpus --bin replay-report` replays v3's own recorded
traces and lists every snapshot path the changes do not reproduce. Each one is something v4's trace
must carry, or its runner must derive. `tests/replay.rs` pins that list.

### Traces

v4's driver compares each of v4's traces with v3's by replay (I109). v4's steps are replayed from
its `initial` with `replay::apply` over each step's changes, in order, and walked in order:

- **An engine step** (`task_id: null`, `result: executed`, with a `loop_counter` and every change
  under `temp_data`: the counter, `as` and `scratch` written as a sweep is entered, a loop exits
  or a limit stops the run at a sweep) is applied and matched to nothing. A workflow's own
  `executed` step of any other shape is matched, so it fails against v3's steps.
- **A `failed` step** is applied and matched to nothing. It is the step v3's trace drops (break 5):
  a call whose failure went to its workflow's policy, or a workflow's own failure. Its records,
  like an engine step's, are carried to the next step whose records are compared.
- **Every other step** is matched to v3's next step. The two must agree on `(workflow_id, task_id,
  result, loop_counter, element_index)`. When that v3 step's snapshot `audit_trail` ends in its own
  task's entry (the recordings' `snapshot_audit_trail: "own"`), v4's step `status` must equal that
  entry's `status`. When the v3 step has a snapshot, the context rebuilt after v4's step,
  normalised with the case's error-context path, must equal the snapshot's `context`, key order
  included, and v4's step `errors`, after the records carried to it, normalised, must equal the
  records the snapshot's `errors` gained over the snapshot before it, in order.

Every v3 step must be matched. v3's `mapping_contexts`, `started_at`, `duration_us`, `changes`, and
the rest of its snapshot's `audit_trail`, are not compared.

**A step's records.** v4's step `errors` is every record the step added to the message, the call's
own and then the recorder's (I127), so it is held to what v3's snapshot gained at the matched step.
A step matched to nothing (an engine step, a `failed` step) and a matched step v3 recorded with no
snapshot (a skip) carry their records forward: v3's next snapshot gained them, so they are held
there, before that step's own. The two are compared at the indices the snapshot holds them, after
the earlier steps' records, which are `null` on both sides, so a deviation's pointer, such as
`/steps/0/run/trace/steps/2/message/errors/1`, names one record in v3's recording and in the aligned
trace. 43 matched steps carry a record on either side. They differ in break 4's shape alone, where
v3 wrapped a `WORKFLOW_ERROR` (`loop/continue_on_error_next_sweep`, `loop/setup_error_continues`),
recorded `TASK_ERROR` for a non-array `for_each.over` (`fan_out/over_shapes`), or recorded a
`WORKFLOW_ERROR` for a workflow's own failure, where v4's `failed` step holds one `LOGIC_ERROR`
(`loop/over_shapes`).

**The records invariant.** For every traced run, the records of all of v4's steps, in step order,
normalised, must equal the traced message's `errors`, normalised. A workflow's own `skipped` step
is left out: its record is the failure of a condition that could not be evaluated (R34), which the
message never gets. Like the replay invariant below, it does not depend on v3 and takes no
deviation, and it holds the records on the `failed` steps that end a run, which no v3 snapshot
follows. The count of runs it held is pinned at the traced-run count.

**The replay invariant.** For every traced run, the replay of all of v4's steps from `initial`,
normalised, must equal the traced message's own final context, normalised (I19: a trace replays to
the context on every status). The traced message is the plain run's whenever `message_matches`
holds, which is itself compared. The invariant does not depend on v3 and takes no deviation. A
traced run it cannot hold, a truncated trace or a traced message that differs from the plain
run's, fails outright, and the count of runs it held is pinned at the traced-run count.

**v4's own replay.** `Trace::contexts`, which a host or the UI calls, is a second implementation of
the replay rule (`Path::set`'s, except that a root takes any value whole, against the corpus's
`path::set`). Each context it yields must equal the corpus's replay after the same step,
normalised, so a drift between the two fails. It takes no deviation.

**Trace deviations.** Both sides are compared in v3's coordinates. Each trace is read as
`{outcome, steps, truncated, message_matches}`, each step as `{workflow_id, task_id, result,
status?, message?: {context, errors}, loop_counter?, element_index?}`, with v4's steps as aligned. So a deviation
names a pointer into v3's recorded trace, such as
`/steps/0/run/trace/steps/0/message/context/data/partial`, and v4's JSON at that pointer in the
aligned trace is its row (`MISSING` where the aligned trace has nothing).

**A step list v4 shortens at its tail** (it skips the last steps v3 ran and matches every one
before them, as break 7 does in `schema/status_without_channel`) is deviated per missing index:
`/steps/<i>/run/trace/steps/<j>`, each `MISSING`, so the steps both ran are still compared in
full, contexts and statuses included. **A step list that differs otherwise** (v4 runs workflows
v3 did not, as break 6 lets it) is deviated at the trace's `steps` itself:
`/steps/<i>/run/trace/steps`. Both sides' `steps` are then read as the keys of the steps the
alignment would match, in order, each as the string `"workflow_id/task_id/result"`, with an empty
task id for a workflow's own step. The row holds v4's list. No context is compared with v3's in
that run, and the replay invariant still applies.
