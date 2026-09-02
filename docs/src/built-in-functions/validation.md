# Validation Function

The `validation` function — also spelled `validate`, which deserializes to the
same config — evaluates rules against message data and collects validation errors.

## Overview

The validation function:

- Evaluates JSONLogic rules against message context
- Collects errors for failed validations
- Is read-only (doesn't modify message data)
- Returns status 200 (pass) or 400 (fail)

## Basic Usage

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
| `rules` | array | Yes | List of validation rules |

### Rule Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `logic` | JSONLogic | Yes | Expression that must evaluate to `true` |
| `message` | string \| JSONLogic | Yes | Error message recorded when the rule fails. Since 3.9 it may be an expression that names the value that failed — see [Computed Messages](#computed-messages) |

Both fields are required: a rule missing either one fails to load, and the
workflow is rejected when the engine is built rather than when the first
message arrives.

### Computed Messages

A `message` is JSONLogic like every other parameter, so an error can carry the
value that caused it instead of a fixed sentence:

```json
{
    "logic": {"<=": [{"var": "data.age"}, 120]},
    "message": {"cat": ["Age ", {"var": "data.age"}, " is out of range"]}
}
```

A plain string is JSONLogic for itself, so the static spelling folds to a
constant at build time and costs nothing per message. A message is rendered
**only when its rule fails**, so a computed one is free on the passing path.

Because the rendered text lands in `message.errors()`, which is serialized, a
`message` may not read `{"secret": …}` — a rule may *test* a secret in its
`logic`, but it may not *report* one. See [Secrets](../advanced/secrets.md).

## How Validation Works

1. Each rule's `logic` is evaluated against the message context
2. If the result is exactly `true`, the rule passes
3. Any other result (false, null, etc.) is a failure
4. Failed rules add errors to `message.errors()`

## Common Validation Patterns

### Required Field

```json
{
    "logic": {"!!": {"var": "data.email"}},
    "message": "Email is required"
}
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

### String Length

Requires the `ext-string` [operator family](../advanced/jsonlogic.md#operator-families-cargo-features).
Note `length` lives in `ext-string` even though it also counts array elements.

```json
{
    "logic": {">=": [
        {"length": {"var": "data.password"}},
        8
    ]},
    "message": "Password must be at least 8 characters"
}
```

### Pattern Matching

JSONLogic has **no regex operator** — there is no `regex_match`. Substring and
prefix checks cover many cases; `in` is a core operator, while `starts_with` and
`ends_with` need `ext-string`.

```json
{
    "logic": {"in": ["@", {"var": "data.email"}]},
    "message": "Invalid email format"
}
```

For real pattern matching, register a [custom function](../advanced/custom-functions.md)
and run the regex in Rust.

### Conditional Required

```json
{
    "logic": {"or": [
        {"!": {"var": "data.is_business"}},
        {"!!": {"var": "data.company_name"}}
    ]},
    "message": "Company name required for business accounts"
}
```

### Value in List

```json
{
    "logic": {"in": [
        {"var": "data.status"},
        ["active", "pending", "suspended"]
    ]},
    "message": "Invalid status value"
}
```

## Multiple Rules

All rules are evaluated, collecting all errors:

```json
{
    "rules": [
        {
            "logic": {"!!": {"var": "data.name"}},
            "message": "Name is required"
        },
        {
            "logic": {"!!": {"var": "data.email"}},
            "message": "Email is required"
        },
        {
            "logic": {">": [{"var": "data.amount"}, 0]},
            "message": "Amount must be positive"
        }
    ]
}
```

## Accessing Errors

After processing, check `message.errors()`:

```rust
# fn _demo(message: dataflow_rs::Message) {
for error in message.errors() {
    println!("{}: {}", error.code, error.message);
}
# }
```

Error structure:
- `code`: one of three, depending on how the rule failed
  - `VALIDATION_ERROR` — the rule evaluated and did not return `true`
  - `EVALUATION_ERROR` — the rule's own expression failed to evaluate
  - `COMPILATION_ERROR` — the rule's logic was never compiled (an engine-side fault)
- `message`: the rule's `message` for `VALIDATION_ERROR`; a description of the
  failure for the other two

Note that all three carry no `workflow_id` or `task_id` — validation builds its
entries without executor identity, so attribute them by position in
`message.errors()` rather than by id.

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"validation_demo","name":"Validation Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"validate","name":"Validate","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.input.email"}]},"message":"Email is required"},{"logic":{">":[{"var":"data.input.age"},0]},"message":"Age must be positive"},{"logic":{"in":[{"var":"data.input.status"},["active","pending","suspended"]]},"message":"Invalid status"}]}}}]}]' data-payload='{"name":"John","age":0,"status":"unknown"}'>
</div>

Notice the validation errors in the output.

## Validation with Continue on Error

Combine validation with data transformation:

```json
{
    "id": "validated_transform",
    "continue_on_error": true,
    "tasks": [
        {
            "id": "validate",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [...]
                }
            }
        },
        {
            "id": "transform",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [...]
                }
            }
        }
    ]
}
```

Transformation proceeds even if validation fails — but note that this is true
with or without `continue_on_error`, for the reason below. Use `halt_on` if you
meant to stop.

## Stopping on a validation failure

**A failing `validation` task does not stop the workflow by default.** It returns
status `400`, and the engine treats the `4xx` range as "logged as a warning, carry
on"; only `5xx` and a returned `Err` engage `continue_on_error` at all. So this
does *not* do what it looks like:

```json
{
    "continue_on_error": false,
    "tasks": [
        {"id": "validate", "name": "Validate",
         "function": {"name": "validation", "input": {"rules": []}}},
        {"id": "process", "name": "Process",
         "function": {"name": "map", "input": {"mappings": []}}}
    ]
}
```

`process` still runs. `EngineBuilder::check_workflow` reports this shape as
`UNGUARDED_VALIDATION`.

### Within one rule: `halt_on`

Put [`halt_on: "failure"`](../advanced/control-flow.md#halt_on) on the validation
task. It halts only when a rule failed, so a passing message carries on, and the
audit trail keeps the real `400`:

```json
{
    "tasks": [
        {"id": "validate", "name": "Validate", "halt_on": "failure",
         "function": {"name": "validation", "input": {"rules": []}}},
        {"id": "process", "name": "Process",
         "function": {"name": "map", "input": {"mappings": []}}}
    ]
}
```

### Across rules: the error-context path

Halting stops **this rule only** — later rules still process the message, so
`halt_on` is not a rejection. To stop a whole pipeline, have the engine record
failures where a condition can read them, with
[`with_error_context_path`](../core-concepts/error-handling.md#branching-on-why-a-task-failed),
and gate the following rule on it:

```json
{"id": "exchange", "name": "Exchange",
 "condition": {"!": [{"var": "metadata.errors.0.code"}]},
 "tasks": []}
```

This is the one that holds when the work you are guarding lives in a later rule.

### Older alternatives

Before `halt_on` the same gate was written as a `filter` reading
`metadata.progress.status_code`, which the engine rewrites after every task:

```json
{"id": "gate", "name": "Stop if invalid",
 "function": {"name": "filter", "input": {
     "condition": {"!=": [{"var": "metadata.progress.status_code"}, 400]},
     "on_reject": "halt"}}}
```

It still works, at a cost: a `filter` halt records status `299`, so the `400` is
replaced on both the audit trail and `metadata.progress` and the host can no
longer see what the task actually returned. Prefer `halt_on`.

## Best Practices

1. **Validate Early** - Add validation as the first task
2. **Clear Messages** - Write specific, actionable error messages
3. **Check All Rules** - Validation evaluates all rules (doesn't short-circuit)
4. **Gate with `halt_on`** - `continue_on_error` does not cover a `400`; use `halt_on: "failure"` when the assertion must stop the rule
5. **Handle Errors** - Always check `message.errors()` after processing
