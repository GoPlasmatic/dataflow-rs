# Validation Function

`validation` checks rules against the message and records an error for each
rule that fails. It writes nothing to the context.

```json
{
    "function": {
        "name": "validation",
        "input": {
            "rules": [
                {
                    "logic": {"!!": {"var": "data.email"}},
                    "message": "Email is required"
                },
                {
                    "logic": {">": [{"var": "data.age"}, 0]},
                    "message": "Age must be positive"
                }
            ]
        }
    }
}
```

## Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `rules` | array | Yes | The rules, checked in order |

### Rule

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `logic` | JSONLogic | Yes | Passes the rule only when it evaluates to `true` |
| `message` | string \| JSONLogic | Yes | The error recorded when the rule fails, evaluated only then |

Both are required: a rule missing either is `INPUT_PARSE` at its key when
the engine is built.

## How a rule is judged

Every rule runs, in order; no failure stops the rest.

| `logic` evaluates to | Record |
|----------------------|--------|
| `true` | None: the rule passes |
| Anything else: `false`, `null`, `1`, `"yes"`, `[1]`, an object | `VALIDATION_ERROR`, with the rule's `message` |
| An error (the expression does not evaluate) | `EVALUATION_ERROR`, with `Failed to evaluate rule <index>: <reason>` |
| The `ExprEngine`'s ops budget exceeded, by `logic` or `message` | None: the task fails with `BUDGET_EXCEEDED`, under `on_failure` |

Only a literal `true` passes. A truthy value that is not `true` fails the
rule, so `{"var": "data.email"}` is not a presence check; write
`{"!!": {"var": "data.email"}}`.

When any rule recorded an error the call ends with status `400`; otherwise
`200`. A `400` is a rejection, and by default the next task runs; see
[Stopping on a failure](#stopping-on-a-failure).

### Computed messages

A `message` is JSONLogic like every other parameter, so an error can name the
value that caused it:

```json
{
    "logic": {">": [{"var": "data.age"}, 0]},
    "message": {"cat": ["Age ", {"var": "data.age"}, " must be positive"]}
}
```

With `data.age` at `0` the record reads `Age 0 must be positive`. A message
is rendered only when its rule fails, so a computed one costs nothing on the
passing path, and a message that cannot be rendered never fails a passing
rule. A message that fails to render on a failing rule records
`VALIDATION_ERROR` with the text `Validation failed`. A message that is not
a string is recorded as its JSON.

The rendered text lands on the message, which the engine records, so a
`message` may not read `{"secret": …}`: the definition is refused with
`SECRET_IN_MESSAGE_WRITE` at `rules[i].message`. A rule may test a secret in
its `logic`; it may not report one. See [Secrets](../advanced/secrets.md).

## The records

Each failing rule appends one record to the message's errors:

```json
{"code": "VALIDATION_ERROR", "message": "Email is required", "path": null, "workflow_id": "w", "task_id": "check", "timestamp": "2026-10-16T19:12:26.123456+00:00"}
```

Each record carries `workflow_id`, `task_id` and the run's timestamp, as every
other task record does (3.x wrote neither id on validation records). Read them from Rust after a run:

```rust
# fn _demo(message: &dataflow_rs::Message) {
for error in message.errors() {
    println!("{}: {}", error.code, error.message);
}
# }
```

Run over `{"age": 0, "status": "unknown", "word": "not"}`, this task

```json
{
    "id": "check",
    "function": {
        "name": "validation",
        "input": {
            "rules": [
                {"logic": {"!!": {"var": "data.email"}}, "message": "Email is required"},
                {"logic": {">": [{"var": "data.age"}, 0]}, "message": {"cat": ["Age ", {"var": "data.age"}, " must be positive"]}},
                {"logic": {"in": [{"var": "data.status"}, ["active", "pending"]]}, "message": "Invalid status"},
                {"logic": {"+": [{"var": "data.word"}, "x"]}, "message": "never shown"},
                {"logic": 1, "message": "truthy but not true"}
            ]
        }
    }
}
```

records `400` and these five errors, in rule order:

```text
VALIDATION_ERROR  Email is required
VALIDATION_ERROR  Age 0 must be positive
VALIDATION_ERROR  Invalid status
EVALUATION_ERROR  Failed to evaluate rule 3: Thrown: {"type":"NaN"} (in operator: +)
VALIDATION_ERROR  truthy but not true
```

## Stopping on a failure

**A failing `validation` does not stop the workflow on its own.** It returns
`400`, which is a rejection, and a task's `on_rejection` is `continue` by
default: the next task runs on the invalid message. Plenty of workflows
validate to record errors rather than to gate, so this is deliberate. A
`validation` whose rejection nothing after it acts on is reported when the
definition is checked, as the advisory `UNGUARDED_VALIDATION` at the task's
`on_rejection`:

```text
a failing rule here records status 400 and task `process` still runs:
`on_rejection` is `continue`. Add `"on_rejection": "halt"` to stop the
workflow, or gate what follows on `metadata.progress.status_code`
```

`on_failure` does not help: it covers a `500` and an error, never a `400`.

### Within one workflow: `on_rejection: "halt"`

Put `"on_rejection": "halt"` on the validation task. The workflow stops when
a rule failed, a passing message carries on, and the audit entry keeps the
real `400`:

```json
{
    "id": "intake",
    "tasks": [
        {
            "id": "check",
            "on_rejection": "halt",
            "function": {
                "name": "validation",
                "input": {"rules": [{"logic": {"!!": {"var": "data.email"}}, "message": "Email is required"}]}
            }
        },
        {
            "id": "process",
            "function": {"name": "map", "input": {"mappings": [{"path": "data.processed", "logic": true}]}}
        }
    ]
}
```

On a message without `data.email`, `check` records `400` and the error,
`process` does not run, and the run goes on to the next workflow. The run
ends `CompletedWithErrors`, since the message holds a record.

A `"terminal": true` on the task ends the workflow whether or not a rule
failed. See [Control Flow](../advanced/control-flow.md).

### Gating a later task on the status

The engine writes `metadata.progress` (`workflow_id`, `task_id`,
`status_code`) after every task, so a condition can read what the validation
returned:

```json
{
    "id": "process",
    "condition": {"!=": [{"var": "metadata.progress.status_code"}, 400]},
    "function": {"name": "map", "input": {"mappings": [{"path": "data.processed", "logic": true}]}}
}
```

Placed right after the validation, `process` is skipped on a `400` and the
tasks after it still run. Across workflows, have the engine mirror the
records where a condition can read them, with `EngineConfig::error_context`;
see [Error Handling](../core-concepts/error-handling.md).

### 3.x

In 3.x the gate was `"halt_on": "failure"` on the task, and
`continue_on_error` never covered a `400` either. The compat reader rewrites
`halt_on: "failure"` to `on_rejection: "halt"` when a 3.x definition loads,
and `validate`, the 3.x name of this function, to `validation`. See
[Migrating from 3.x](../migration/from-3.md).

## Common patterns

### Required field

```json
{"logic": {"!!": {"var": "data.email"}}, "message": "Email is required"}
```

### Numeric range

```json
{
    "logic": {"and": [
        {">=": [{"var": "data.age"}, 18]},
        {"<=": [{"var": "data.age"}, 120]}
    ]},
    "message": "Age must be between 18 and 120"
}
```

### Value in a list

```json
{
    "logic": {"in": [{"var": "data.status"}, ["active", "pending", "suspended"]]},
    "message": "Invalid status value"
}
```

### Conditional requirement

```json
{
    "logic": {"or": [
        {"!": {"var": "data.is_business"}},
        {"!!": {"var": "data.company_name"}}
    ]},
    "message": "Company name required for business accounts"
}
```

### String length

Needs the `ExtString`
[operator family](../advanced/jsonlogic.md#operator-families);
without it, `{"length": …}` is literal data, which is not `true`, so the rule
fails on every message.

```json
{
    "logic": {">=": [{"length": {"var": "data.password"}}, 8]},
    "message": "Password must be at least 8 characters"
}
```

### Pattern matching

JSONLogic has no regex operator. Substring and prefix checks cover many
cases: `in` is a core operator, `starts_with` and `ends_with` need
`OperatorFamily::ExtString`.

```json
{"logic": {"in": ["@", {"var": "data.email"}]}, "message": "Invalid email format"}
```

For a regex, write a [host function](./host-functions.md) and run it in Rust.

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"validation_demo","name":"Validation Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"check","name":"Validate","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.input.email"}]},"message":"Email is required"},{"logic":{">":[{"var":"data.input.age"},0]},"message":"Age must be positive"},{"logic":{"in":[{"var":"data.input.status"},["active","pending","suspended"]]},"message":"Invalid status"}]}}}]}]' data-payload='{"name":"John","age":0,"status":"unknown"}'>
</div>

Notice the validation errors in the output.

## Best Practices

1. **Validate early**: put the validation task first, after the parse.
2. **Write specific messages**, and compute them where the value matters.
3. **Expect every rule to run**: validation does not short-circuit.
4. **Gate with `on_rejection: "halt"`** when a failure must stop the
   workflow; `on_failure` does not cover a `400`.
5. **Read the errors** after a run; the run's status alone does not say
   which rule failed.
