# Validation Function

The `validation` function evaluates rules against message data and collects validation errors.

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
| `message` | string | No | Error message (default: "Validation failed") |

## How Validation Works

1. Each rule's `logic` is evaluated against the message context
2. If the result is exactly `true`, the rule passes
3. Any other result (false, null, etc.) is a failure
4. Failed rules add errors to `message.errors`

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

```json
{
    "logic": {">=": [
        {"strlen": {"var": "data.password"}},
        8
    ]},
    "message": "Password must be at least 8 characters"
}
```

### Pattern Matching (with Regex)

```json
{
    "logic": {"regex_match": [
        {"var": "data.email"},
        "^[^@]+@[^@]+\\.[^@]+$"
    ]},
    "message": "Invalid email format"
}
```

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

After processing, check `message.errors`:

```rust
for error in &message.errors {
    println!("{}: {}", error.code, error.message);
}
```

Error structure:
- `code`: "VALIDATION_ERROR" for failed rules
- `message`: The error message from the rule

## Try It

> **Want more features?** Try the [Full Debugger UI](/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"validation_demo","name":"Validation Demo","tasks":[{"id":"validate","name":"Validate","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.email"}]},"message":"Email is required"},{"logic":{">":[{"var":"data.age"},0]},"message":"Age must be positive"},{"logic":{"in":[{"var":"data.status"},["active","pending","suspended"]]},"message":"Invalid status"}]}}}]}]' data-message='{"data":{"name":"John","age":0,"status":"unknown"},"metadata":{}}'>
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

With `continue_on_error: true`, transformation proceeds even if validation fails.

## Stop on Validation Failure

For strict validation (stop on failure):

```json
{
    "continue_on_error": false,
    "tasks": [
        {
            "id": "validate",
            "function": {"name": "validation", "input": {...}}
        },
        {
            "id": "process",
            "function": {"name": "map", "input": {...}}
        }
    ]
}
```

If validation fails, subsequent tasks are skipped.

## Best Practices

1. **Validate Early** - Add validation as the first task
2. **Clear Messages** - Write specific, actionable error messages
3. **Check All Rules** - Validation evaluates all rules (doesn't short-circuit)
4. **Use continue_on_error** - Decide if processing should continue on failure
5. **Handle Errors** - Always check `message.errors` after processing
