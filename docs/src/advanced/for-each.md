# Fan-Out (`for_each`)

Adding `for_each` to a task invokes its host function once per element of an
array (for example, running one model inference per participant or sending one
HTTP request per recipient) as a single workflow step.

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

In this example, each invocation sees its element at `temp_data.p` and its zero-based
index at `temp_data.p_index`, writes its output to `temp_data.move`, and has its
output collected by the engine into the `temp_data.moves` array in element
order.

## When to Use `for_each` vs. `loop`

- A workflow [`loop`](./loops.md) repeats the **entire workflow task list**
  sequentially for each iteration, and a workflow can have only one `loop`.
- A task `for_each` repeats a **single host-function task** across an array of
  elements (optionally concurrently via `max_concurrency`), and can be used
  inside a workflow that already has a `loop`.

## Configuration Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `over` | **Yes** | None | JSONLogic expression evaluated once before fan-out. Must produce an array. |
| `as` | **Yes** | None | `temp_data` key bound to the current element in each call. The element's index is bound alongside it at `temp_data.<as>_index`. |
| `max_concurrency` | No | `1` | Maximum number of element calls allowed to run concurrently (`1` runs them sequentially). |
| `collect` | With `into` | None | Context path where each element call writes its result. |
| `into` | With `collect` | None | Context path where the engine writes the collected results array, in element order. |

Only custom host functions support `for_each`. Built-in functions (`map`,
`validation`, etc.) run inline and already have JSONLogic array operators (`map`,
`filter`, `reduce`) for per-element transformations; placing `for_each` on a
built-in function or a task group is rejected at compile time.

## Execution Lifecycle

When a task with `for_each` runs, the engine performs these steps:

```text
1. Evaluate the task's `condition` once  -> if false, skip the entire task
2. Evaluate `over` once                  -> if not an array, fail the task
3. Execute one function call per element (up to `max_concurrency` in flight at once),
   each reading from the shared context with its element bound in an isolated overlay
4. Commit results in element order (0, 1, 2, ...), stopping at the first element
   that fails the task or halts:
     - append any error records from the call
     - apply the call's context writes
     - copy the value at `collect` into `into[i]`
     - append the element's audit entry
5. Apply the task's `terminal` and halting policies to the fan-out as a whole
```

## Call Isolation and Determinism

Every element call sees the message context **as it was before the `for_each`
task started**, with its own `temp_data.<as>` and `temp_data.<as>_index`
bindings layered underneath its own buffered writes.

- **No cross-element visibility:** Element 3 never sees writes made by element
  2, even when `max_concurrency` is `1`.
- **Zero context cloning:** Element bindings live only in each call's read view;
  the message context itself is not cloned per element.
- **Deterministic output regardless of concurrency:** Because writes are buffered
  per call and committed in strict element order (`0..n`) after all calls finish,
  `max_concurrency: 1` and `max_concurrency: 8` produce the exact same context
  state, collected array, and audit trail order.

For custom function authors, this means:
- **Write through `Ctx`:** All writes made with `ctx.set`, `ctx.emit`, and
  `ctx.remove` are replayed in element order during the commit phase.
- **Do not rely on state from earlier elements:** Each element call starts from
  the pre-task context snapshot.
- **Element bindings are temporary:** `temp_data.<as>` and
  `temp_data.<as>_index` exist only in each call's view and are not written to
  the message after the task completes (unless your function explicitly calls
  `ctx.set` on those paths). You can also read the current element index in Rust
  via `ctx.info().element_index`.

## Collecting Results (`collect` and `into`)

When `collect` and `into` are configured, each element call writes its output to
the `collect` path, and the engine gathers those values into an array at `into`.
Element `i`'s result is always placed at `into[i]`.

`into[i]` is set to `null` if element `i`:
- Failed (returned `Err` or a status `>= 400`)
- Did not write a value to `collect`
- Never ran (or was not committed) because an earlier element failed the task or
  halted

Because each call's writes to `collect` are also replayed onto the message in
element order, `collect` holds the last committed element's value after the task
completes. If `over` evaluates to an empty array `[]`, no calls run and `into`
is set to `[]`.

`collect` and `into` are optional: if your function performs side effects (such
as publishing a notification per recipient) or writes to distinct paths per
element, you can omit both fields.

## Error Handling and Halting in `for_each`

During fan-out execution, individual element failures and rejections are governed
by the task's `on_failure` and `on_rejection` policies, while workflow-halting
actions (`terminal: true`, `on_rejection: "halt"`, and `on_failure: "halt"`) are
applied once after all elements have been committed:

- **With `on_failure: "continue"` or `"halt"`**: If an element fails, its error
  is recorded, its slot in `into[i]` is set to `null`, and the remaining
  elements continue normally. If the policy is `"halt"`, the workflow stops
  after all elements finish and commit.
- **With `on_failure: "fail"` (default)**: If an element fails, the task fails.
  No new element calls are started, any in-flight calls are allowed to finish,
  and the commit step stops at the first failing element index (subsequent
  elements do not commit their context writes, though external side effects such
  as network requests already in flight may have occurred).
- **When an element returns `Outcome::Halt`**: No new element calls are started,
  the commit step stops after committing the halting element (discarding context
  writes from higher-indexed elements), and the workflow halts.
- **Non-array `over`**: If `over` evaluates to `null` or a non-array value, the
  task fails immediately before invoking any element calls.
- **Deadlines and cancellation**: If the run's deadline expires or its
  cancellation token is triggered during a fan-out, uncommitted element writes
  are discarded, recorded errors are kept, and the run ends with `TimedOut` or
  `Cancelled`.

## Audit Trail and Observability

A `for_each` task records one `AuditEntry` **per committed element**, stamped
with `element_index`:

```json
{ "workflow_id": "turn", "task_id": "infer", "status": 200,
  "element_index": 3, "changes": [ ] }
```

The same `element_index` field is included on trace steps and error records
produced by that element. Inside a looping workflow, entries carry both
`loop_counter` and `element_index`.

When `capture_changes` is enabled, each element's audit entry stores the changes
made by that element. For large fan-outs inside long loops, consider disabling
`capture_changes`; see [Memory in Long Loops](./loops.md#memory-in-long-loops).

`metadata.progress` is updated after each element's entry. If `over` is empty or
fails to evaluate to an array, the task records a single audit entry without
`element_index`. Observers receive one `TaskFinished` event per element call,
each timed individually.

## Complete Example

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::{Ctx, Engine, Expr, Function, Message, Outcome, Result};
use serde_json::json;

#[derive(FunctionConfig)]
struct GreetConfig {
    /// Who to greet: configured as `{"var": "temp_data.p.name"}` in the workflow.
    name: Expr<String>,
}

/// Greets the person bound at `temp_data.p`.
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
// Element bindings are scoped to each call's view and not left in `temp_data`:
assert!(context["temp_data"].get("p").is_none());
// One audit entry per element, in element order:
let elements: Vec<Option<usize>> = message.audit().iter().map(|e| e.element_index).collect();
assert_eq!(elements, [Some(0), Some(1)]);
# });
```

## Performance

Because `for_each` binds each element in a lightweight overlay over the shared
context view rather than cloning the message context per element, fanning out
over 100 elements on a multi-megabyte context allocates only the per-element
writes. See [Performance](./performance.md) for benchmark results.

## Validation

`def::parse` validates `for_each` configurations at compile time and reports any
violation as `INVALID_FOR_EACH` (`Severity::Rejected`) at the offending key:

- `over` is a scalar literal that can never evaluate to an array
- `as` is missing or not a valid non-empty path
- `max_concurrency` is `0`
- `collect` is specified without `into`, or `into` without `collect`
- `collect` or `into` is not a valid sub-path under `data`, `metadata`, or
  `temp_data` (or points directly to a root)
- `collect` overlaps with `temp_data.<as>` or `temp_data.<as>_index`
- `into` overlaps with `collect`, `temp_data.<as>`, or `temp_data.<as>_index`
- `for_each` is placed on a built-in function or on a task group

See [Authoring-Time Validation](./authoring-validation.md) for more details.
