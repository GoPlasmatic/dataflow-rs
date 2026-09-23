# Fan-Out: One Call per Element

A task's `for_each` runs its function once per element of an array — one model
inference per participant, one HTTP call per recipient — as a single step.

```json
{
  "id": "infer", "name": "One move per participant",
  "for_each": {
    "over": { "var": "data.participants" },
    "as": "p",
    "max_concurrency": 8,
    "collect": "temp_data.move",
    "into": "temp_data.moves"
  },
  "continue_on_error": true,
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
when a single step needs to fan out — including inside a workflow that already
loops for another reason.

## Fields

| Field | Required | Default | Meaning |
|---|---|---|---|
| `over` | **yes** | — | JSONLogic evaluated once. Must yield an array. |
| `as` | **yes** | — | `temp_data` field holding the element in each call. The index is at `<as>_index` beside it. |
| `max_concurrency` | no | `1` | Calls in flight at once. `1` runs them one at a time. |
| `collect` | with `into` | none | Context path each call writes its result to. |
| `into` | with `collect` | none | Context path that receives the results as an array, in element order. |

Only handler-backed functions fan out: `http_call`, `enrich`, `publish_kafka`
and your own handlers. The built-ins — `map`, `validation` and the rest — run
inline, and JSONLogic's `map`, `filter` and `reduce` already cover per-element
transforms of an array.

## What happens

```text
1. evaluates the task's condition, once   -> false: no call, the task is skipped
2. evaluates `over`, once                  -> not an array: the task fails
3. runs one call per element, up to max_concurrency at a time,
   each against its own copy of the message
4. folds the calls back, in element order:
     the errors the call recorded,
     then its writes,
     then its result at into[i],
     then its audit entry
5. applies terminal and halt_on to the fan-out as a whole
```

## Calls are isolated

Every call runs against **its own copy** of the message, taken before the
first call. No call sees another's writes — not even when they run one at a
time. When the calls finish, each is folded back in element order.

That is what makes `max_concurrency` a timing knob only. At `1` and at `8` the
message comes out the same: the same results, the same writes, the same audit
entries in the same order.

Two consequences for a handler author:

- **Write through `ctx.set`.** The fold replays the writes a call recorded. A
  write made by reaching into `ctx.message_mut()` directly is not carried back.
- **Don't count on an earlier element's writes.** Element 3 sees the message as
  it was before the fan-out, not after element 2.

The bindings live only in each call's copy, so `temp_data.<as>` and
`temp_data.<as>_index` are not left behind after the task.

A handler can also ask for its element's index directly:
`TaskContext::element_index()` returns `Some(i)` inside a fan-out and `None`
everywhere else.

## Results

`collect` names the slot each call writes; `into` names the array the results
land in. Element `i`'s result is always at `into[i]`, and it is `null` when the
element:

- failed — returned an `Err`, or a status of `400` or more,
- never ran, because an earlier failure stopped the fan-out, or
- wrote nothing at `collect`.

`collect`'s own writes are replayed like any other, so after the task it holds
the last element's value.

Without `collect` and `into`, the calls' writes are simply replayed — right for
a fan-out whose calls have side effects but no result, such as one
`publish_kafka` per recipient.

An empty `over` runs no call and sets `into` to `[]`.

## Failures

The task's `continue_on_error` applies **per element**, exactly as it applies to
an ordinary task:

- **With it**, a failed element records its error, leaves `null` at its index,
  and the fan-out carries on.
- **Without it**, a failed element fails the task. No further call starts;
  calls already running finish. The fold stops at the failing element, so the
  elements after it contribute nothing — although their external side effects,
  such as an HTTP call already sent, may already have happened.

A call that returns `TaskOutcome::Halt` also stops new calls, and halts the
workflow once the fan-out is folded.

`terminal` and `halt_on` apply to the **whole** fan-out: a terminal task halts
after its last element, and `"halt_on": "failure"` halts after the fan-out if
any element failed.

An `over` that does not evaluate to an array — `null` included — fails the task
the way a handler error would, before any call.

## Records

A fan-out records one entry **per element**, each stamped with its
`element_index`:

```json
{ "workflow_id": "turn", "task_id": "infer", "status": 200,
  "element_index": 3, "changes": [ ] }
```

The same field appears on execution-trace steps and on the errors an element
produced. An ordinary task omits it entirely, so its JSON is unchanged. Inside
a looping workflow an entry carries both `loop_counter` and `element_index`.

`metadata.progress` is written after every element's entry. An empty or
failing `over` still records one entry, without an `element_index`, so the
task always leaves its progress behind.

Observers receive one `task_finished` event per element, each timed on its own
call.

## Example

```rust
use async_trait::async_trait;
use dataflow_rs::prelude::*;
use dataflow_rs::datavalue::OwnedDataValue;
use serde_json::{json, Value};

/// Greets the participant bound at `temp_data.p`.
struct Greet;

#[async_trait]
impl AsyncFunctionHandler for Greet {
    type Input = Value;

    async fn execute(&self, ctx: &mut TaskContext<'_>, _input: &Value) -> Result<TaskOutcome> {
        let name = Value::from(ctx.get("temp_data.p.name").unwrap_or(&OwnedDataValue::Null));
        let greeting = format!("hello, {}", name.as_str().unwrap_or("?"));
        ctx.set("temp_data.greeting", OwnedDataValue::from(&json!(greeting)));
        Ok(TaskOutcome::Success)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let workflow = Workflow::from_json(&json!({
        "id": "greet_all", "name": "Greet everyone",
        "tasks": [{
            "id": "greet", "name": "Greet",
            "for_each": {
                "over": {"var": "data.people"}, "as": "p", "max_concurrency": 4,
                "collect": "temp_data.greeting", "into": "data.greetings"
            },
            "function": {"name": "greet", "input": {}}
        }]
    }).to_string())?;

    let engine = Engine::builder()
        .with_workflows(vec![workflow])
        .register("greet", Greet)
        .build()?;

    let mut message = Message::builder()
        .data(OwnedDataValue::from(&json!({"people": [{"name": "ada"}, {"name": "grace"}]})))
        .build();
    engine.process_message(&mut message).await?;

    assert_eq!(
        Value::from(&message.context["data"]["greetings"]),
        json!(["hello, ada", "hello, grace"])
    );
    Ok(())
}
```

The browser debugger cannot run a fan-out: the wasm build registers no
handlers. It still shows the task and its `for_each`.

## Validation

Refused at `Engine::build()`, and reported by `Workflow::validate_authored` as
`INVALID_FOR_EACH` at the offending key:

- an `over` that is a scalar literal — it could never be an array;
- a missing or malformed `as`;
- `max_concurrency` of `0`;
- `collect` without `into`, or `into` without `collect`;
- a `collect` or `into` that is not a path below `data`, `metadata` or
  `temp_data`, or that names a root;
- an `into` overlapping `collect`, `temp_data.<as>` or `temp_data.<as>_index`;
- `for_each` on a built-in function, or on a task group.
