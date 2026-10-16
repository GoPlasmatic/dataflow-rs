# Fan-Out: One Call per Element

A task's `for_each` runs its function once per element of an array (one model
inference per participant, one HTTP call per recipient) as a single step.

```json
{
  "id": "infer",
  "for_each": {
    "over": { "var": "data.participants" },
    "as": "p",
    "max_concurrency": 8,
    "collect": "temp_data.move",
    "into": "temp_data.moves"
  },
  "on_failure": "continue",
  "function": { "name": "model_infer", "input": {
    "model":  { "var": "temp_data.p.model" },
    "input":  { "var": "temp_data.p.view" },
    "output": "temp_data.move" } }
}
```

Each call sees its participant at `temp_data.p` and its index at
`temp_data.p_index`, writes its answer to `temp_data.move`, and the engine
gathers the answers into `temp_data.moves` in participant order.

## When to use it, and when a loop

A workflow [`loop`](./loops.md) replays the **whole task list** once per
element, and a workflow has only one. `for_each` repeats **one task**. Use it
when a single step needs to fan out, including inside a workflow that already
loops for another reason.

## Fields

| Field | Required | Default | Meaning |
|---|---|---|---|
| `over` | **yes** | none | JSONLogic evaluated once. Must yield an array. |
| `as` | **yes** | none | `temp_data` field holding the element in each call. The index is at `<as>_index` beside it. |
| `max_concurrency` | no | `1` | Calls in flight at once. `1` runs them one at a time. |
| `collect` | with `into` | none | Context path each call writes its result to. |
| `into` | with `collect` | none | Context path that receives the results as an array, in element order. |

Only host functions fan out. The built-ins (`map`, `validation` and the
rest) run inline, and JSONLogic's `map`, `filter` and `reduce` already cover
per-element transforms of an array; `for_each` on a built-in is refused.

## What happens

```text
1. evaluates the task's condition, once   -> false: no call, the task is skipped
2. evaluates `over`, once                  -> not an array: the task fails
3. runs one call per element, up to max_concurrency at a time,
   each over the run's context with its element bound beneath its own writes
4. folds the calls back, in element order, stopping at the first that
   fails the task or halts:
     the errors the call recorded,
     then its writes,
     then its result at into[i],
     then its audit entry
5. applies the task's terminal and halting policies to the fan-out as a whole
```

## Calls are isolated

Every call runs over the run's context **as it was before the fan-out**,
with its own element and index bound beneath its own writes. No call sees
another's writes, even when they run one at a time, and no call copies the
context: the bindings sit in the call's view only, and nothing reaches the
message until every call has finished. Then the engine folds each back in
element order.

Isolation makes `max_concurrency` a timing knob only. At `1` and at `8` the
message comes out the same: the same results, the same writes, the same audit
entries in the same order.

Two consequences for a function author:

- **Write through `Ctx`.** The fold replays the writes a call made with
  `set`, `emit` and `remove`, at their own paths, in order. There is no
  other way to write.
- **Don't count on an earlier element's writes.** Element 3 sees the context
  as it was before the fan-out, not after element 2.

The bindings live only in each call's view, so `temp_data.<as>` and
`temp_data.<as>_index` are not left behind after the task, unless a function
writes under one of them itself; the engine replays that write like any other.
`collect` may not name a path under either.

A function can also ask for its element's index directly:
`ctx.info().element_index` is `Some(i)` inside a fan-out and `None`
everywhere else.

## Results

`collect` names the slot each call writes; `into` names the array the results
land in. Element `i`'s result is always at `into[i]`, and it is `null` when the
element:

- failed: returned an `Err`, or a status of `400` or more,
- never ran, or was never folded, because an earlier element failed the task
  or halted, or
- wrote nothing at `collect`.

`collect`'s own writes are replayed like any other, so after the task it holds
the last element's value.

Without `collect` and `into`, the engine only replays the calls' writes. That
suits a fan-out whose calls have side effects but no result, such as one
message published per recipient.

An empty `over` runs no call and sets `into` to `[]`.

## Failures

Each element is decided under the task's policies with everything that could
halt taken out: an element's `on_failure` is `"fail"` when the task's is
`"fail"` and `"continue"` otherwise, its `on_rejection` is `"continue"`, and
`terminal` is off. The task's own `terminal`, `on_rejection: "halt"` and
`on_failure: "halt"` apply once, to the whole fan-out, after the fold.

- **Under `on_failure: "continue"` or `"halt"`**, a failed element records
  its error, leaves `null` at its index, and the fan-out carries on; with
  `"halt"` the workflow then stops once the fan-out is folded.
- **Under the default `on_failure: "fail"`**, a failed element fails the task.
  No further call starts; calls already running finish. The fold stops at
  the failing element, so the elements after it contribute nothing, although
  their external side effects, such as an HTTP call already sent, may already
  have happened.

A call that returns `Outcome::Halt` also stops new calls, and the fold
stops at the halting element: its own errors, writes, result and audit entry
land, and the elements after it contribute nothing, however many had already
finished under `max_concurrency`, side effects included. The workflow halts
once the fan-out is folded.

`terminal` applies to the **whole** fan-out: a terminal task halts after its
last element. `"on_rejection": "halt"` halts after the fan-out if any
element was rejected.

An `over` that does not evaluate to an array (`null` included) fails the task
the way a function error would, before any call.

A fan-out the run's deadline or cancellation token cuts short folds
nothing, finished elements included: the records each call added are kept,
the task is recorded once, and the run ends `TimedOut` or `Cancelled`.

## Records

A fan-out records one entry **per element**, each stamped with its
`element_index`:

```json
{ "workflow_id": "turn", "task_id": "infer", "status": 200,
  "element_index": 3, "changes": [ ] }
```

The same field appears on trace steps and on the errors an element
produced. An ordinary task omits it entirely, so its JSON is unchanged. Inside
a looping workflow an entry carries both `loop_counter` and `element_index`.

While `capture_changes` is on, each element's entry carries that element's
writes, so a large fan-out inside a long loop multiplies the cost described in
[Memory in long loops](./loops.md#memory-in-long-loops).

`metadata.progress` is written after every element's entry. An empty or
failing `over` still records one entry, without an `element_index`, so the
task always leaves its progress behind.

Observers receive one `TaskFinished` event per element, each timed on its own
call.

## Example

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Result};
use serde_json::json;

#[derive(FunctionConfig)]
struct GreetConfig {
    /// Who to greet: in the workflow, `{"var": "temp_data.p.name"}`.
    name: Expr<String>,
}

/// Greets the participant bound at `temp_data.p`.
struct Greet;

impl Function for Greet {
    type Config = GreetConfig;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &GreetConfig) -> Result<Outcome> {
        let name = ctx.eval(&cfg.name)?;
        let greeting = OwnedDataValue::from(&json!(format!("hello, {name}")));
        ctx.set(&"temp_data.greeting".parse()?, greeting)?;
        Ok(Outcome::Ok)
    }
}

let engine = Engine::builder()
    .register("greet", Greet)
    .workflow_json(&json!({
        "id": "greet_all",
        "tasks": [{
            "id": "greet",
            "for_each": {
                "over": {"var": "data.people"}, "as": "p", "max_concurrency": 4,
                "collect": "temp_data.greeting", "into": "data.greetings"
            },
            "function": {"name": "greet", "input": {"name": {"var": "temp_data.p.name"}}}
        }]
    }))
    .build()
    .unwrap();

let data = OwnedDataValue::from(&json!({"people": [{"name": "ada"}, {"name": "grace"}]}));
let mut message = Message::from_parts(OwnedDataValue::Null, data, OwnedDataValue::Null);
engine.run(&mut message).await;

let context = message.to_json()["context"].clone();
assert_eq!(context["data"]["greetings"], json!(["hello, ada", "hello, grace"]));
// The bindings were never written to the message.
assert!(context["temp_data"].get("p").is_none());
// One entry per element, in element order.
let elements: Vec<Option<usize>> = message.audit().iter().map(|e| e.element_index).collect();
assert_eq!(elements, [Some(0), Some(1)]);
# });
```

## Performance

A fan-out copies nothing: each call reads the run's one view with its element
bound beneath it, and the fold splices each call's writes in. The `fan_out`
benchmark, 100 elements over a 2 MiB context, measured at the 4.0 gate
against 3.15 at well under 1% of 3.15's time per message, since 3.15 copied
the context once per element. See [Performance](./performance.md).

## Validation

`def::parse` reports each of these as `INVALID_FOR_EACH`, Rejected, at the
offending key, so `Engine::builder().build()` refuses the workflow:

- an `over` that is a scalar literal, which could never be an array;
- a missing or malformed `as`;
- `max_concurrency` of `0`;
- `collect` without `into`, or `into` without `collect`;
- a `collect` or `into` that is not a path below `data`, `metadata` or
  `temp_data`, or that names a root;
- a `collect` overlapping `temp_data.<as>` or `temp_data.<as>_index`, since the
  result would be replayed into the message and leave the binding behind;
- an `into` overlapping `collect`, `temp_data.<as>` or `temp_data.<as>_index`;
- `for_each` on a built-in function, or on a task group.

The compiler checks the rules again, since a host may edit a parsed
definition before compiling it. See
[Authoring-Time Validation](./authoring-validation.md).
