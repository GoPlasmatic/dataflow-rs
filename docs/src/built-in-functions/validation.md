# Validation Function

The `validation` function evaluates a list of JSONLogic rules against the message context and records an error on `message.errors()` for each rule that fails. It is read-only and never modifies the context.

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
| `rules` | array | Yes | List of validation rules evaluated in order |

### Rule Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `logic` | JSONLogic | Yes | Validation expression; the rule passes only when it evaluates to boolean `true` |
| `message` | string \| JSONLogic | Yes | Error message recorded when the rule fails (evaluated only on failure) |

Both `logic` and `message` are required on every rule; omitting either produces an `INPUT_PARSE` issue at build time.

## How Rules Are Evaluated

All rules in the `rules` list are evaluated in order—a failing rule does not short-circuit subsequent rules in the same task:

| `logic` Result | Recorded on `message.errors()` |
|----------------------|--------|
| `true` | None (rule passes) |
| Any other value (`false`, `null`, `1`, `"yes"`, `[1]`, `{}`) | `VALIDATION_ERROR` with the rule's `message` |
| Expression evaluation error | `EVALUATION_ERROR` (`Failed to evaluate rule <index>: <reason>`) |
| Exceeds `ExprEngine` `ops_budget` (in `logic` or `message`) | Aborts the task immediately with `BUDGET_EXCEEDED` (governed by `on_failure`) |

> **Important:** A rule passes **only** when `logic` evaluates to boolean `true`. Truthy non-boolean values (such as a non-empty string or non-zero number) fail the rule. Always wrap presence checks in `"!!"`: write `{"!!": {"var": "data.email"}}` rather than `{"var": "data.email"}`.

If every rule passes, the `validation` task finishes with status `200`. If one or more rules fail, the task finishes with status `400` (a **rejection**). By default, `on_rejection` is `"continue"`, so the next task still runs unless you set `"on_rejection": "halt"`; see [Stopping the Workflow on Validation Failure](#stopping-the-workflow-on-validation-failure).

### Dynamic Error Messages

Because `message` can be a JSONLogic expression, error messages can include dynamic values from the message context:

```json
{
    "logic": {">": [{"var": "data.age"}, 0]},
    "message": {"cat": ["Age ", {"var": "data.age"}, " must be positive"]}
}
```

When `data.age` is `0`, this records `"Age 0 must be positive"`. The `message` expression is evaluated lazily only when `logic` fails, so dynamic messages add zero cost when validation passes. (If a dynamic `message` expression fails to evaluate, `"Validation failed"` is recorded; if it evaluates to a non-string value, it is formatted as JSON.)

> **No secrets in error messages:** Because validation error messages are stored in `message.errors()`, a rule's `message` cannot reference `{"secret": "..."}` (`SECRET_IN_MESSAGE_WRITE`). A rule's `logic` *can* compare against a secret, since `logic` reduces to a boolean. See [Secrets](../advanced/secrets.md).

## Error Records

Each failing rule appends an `ErrorRecord` to `message.errors()`:

```json
{"code": "VALIDATION_ERROR", "message": "Email is required", "path": null, "workflow_id": "w", "task_id": "check", "timestamp": "2026-10-16T19:12:26.123456+00:00"}
```

You can inspect all recorded validation errors in Rust after a run:

```rust
# fn _demo(message: &dataflow_rs::Message) {
for error in message.errors() {
    println!("{}: {}", error.code, error.message);
}
# }
```

For example, running the following task against `data = {"age": 0, "status": "unknown", "word": "not"}`:

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

returns status `400` and appends five error records in rule order:

```text
VALIDATION_ERROR  Email is required
VALIDATION_ERROR  Age 0 must be positive
VALIDATION_ERROR  Invalid status
EVALUATION_ERROR  Failed to evaluate rule 3: Thrown: {"type":"NaN"} (in operator: +)
VALIDATION_ERROR  truthy but not true
```

## Stopping the Workflow on Validation Failure

**By default, a failing `validation` task does not stop the workflow.** It returns status `400` (a rejection), and a task's default `on_rejection` policy is `"continue"` so that workflows can collect validation errors across multiple tasks.

If a `validation` task is followed by other tasks that do not check `metadata.progress.status_code`, `Compiler::check` emits an `UNGUARDED_VALIDATION` advisory on the validation task to remind you:

```text
a failing rule here records status 400 and task `process` still runs:
`on_rejection` is `continue`. Add `"on_rejection": "halt"` to stop the
workflow, or gate what follows on `metadata.progress.status_code`
```

Note that setting `on_failure` on a validation task has no effect on failed validation rules, because `on_failure` applies only to `5xx` statuses and `Err` outcomes—not `400` rejections.

### Option 1: Set `"on_rejection": "halt"` on the Validation Task

To stop the current workflow as soon as validation fails, set `"on_rejection": "halt"` on the validation task:

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

When `data.email` is missing, `check` records status `400` and the `VALIDATION_ERROR`, `process` is skipped, the workflow exits with `WorkflowExit::Halted`, and the engine moves on to the next workflow (finishing with `RunStatus::CompletedWithErrors`).

### Option 2: Gate a Subsequent Task on `metadata.progress.status_code`

Because the engine updates `metadata.progress` (`{workflow_id, task_id, status_code}`) after every task, a subsequent task can check whether the preceding validation returned `400`:

```json
{
    "id": "process",
    "condition": {"!=": [{"var": "metadata.progress.status_code"}, 400]},
    "function": {"name": "map", "input": {"mappings": [{"path": "data.processed", "logic": true}]}}
}
```

If you want a later *workflow* to branch on validation errors, configure `EngineConfig::error_context`; see [Error Handling → Branching on Task Failures](../core-concepts/error-handling.md#branching-on-task-failures).

### Migrating from 3.x

In 3.x, this function was named `validate` and halting on failure used `"halt_on": "failure"`. When a 3.x definition is loaded in 4.0, the compatibility reader automatically rewrites `validate` to `validation` and `"halt_on": "failure"` to `"on_rejection": "halt"`. See [Migrating from 3.x](../migration/from-3.md).

## Common Validation Patterns

### Required Field

```json
{"logic": {"!!": {"var": "data.email"}}, "message": "Email is required"}
```

### Numeric Range

```json
{
    "logic": {"and": [
        {">=": [{"var": "data.age"}, 18]},
        {"<=": [{"var": "data.age"}, 120]}
    ]},
    "message": "Age must be between 18 and 120"
}
```

### Allowed Values (`in`)

```json
{
    "logic": {"in": [{"var": "data.status"}, ["active", "pending", "suspended"]]},
    "message": "Invalid status value"
}
```

### Conditional Requirement

```json
{
    "logic": {"or": [
        {"!": {"var": "data.is_business"}},
        {"!!": {"var": "data.company_name"}}
    ]},
    "message": "Company name required for business accounts"
}
```

### String Length

Requires enabling `OperatorFamily::ExtString` on `ExprEngineBuilder` (see [Operator Families](../advanced/jsonlogic.md#operator-families)):

```json
{
    "logic": {">=": [{"length": {"var": "data.password"}}, 8]},
    "message": "Password must be at least 8 characters"
}
```

### Substring / Prefix Matching

Core JSONLogic provides `in` for substring checks, and `OperatorFamily::ExtString` adds `starts_with` and `ends_with`:

```json
{"logic": {"in": ["@", {"var": "data.email"}]}, "message": "Invalid email format"}
```

For full regular expression matching, register a custom [host function](./host-functions.md) or custom operator in Rust.

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"validation_demo","name":"Validation Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"check","name":"Validate","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.input.email"}]},"message":"Email is required"},{"logic":{">":[{"var":"data.input.age"},0]},"message":"Age must be positive"},{"logic":{"in":[{"var":"data.input.status"},["active","pending","suspended"]]},"message":"Invalid status"}]}}}]}]' data-payload='{"name":"John","age":0,"status":"unknown"}'>
</div>

## Best Practices

1. **Validate Early** — Place `validation` tasks right after parsing input data.
2. **Use `"on_rejection": "halt"` When Gating Downstream Tasks** — Remember that `validation` returns `400`, which continues by default unless `"on_rejection": "halt"` is set.
3. **Wrap Presence Checks in `"!!"`** — Rules require boolean `true` to pass.
4. **Include Field Values in Error Messages** — Use dynamic JSONLogic expressions in `message` when helpful for debugging.
