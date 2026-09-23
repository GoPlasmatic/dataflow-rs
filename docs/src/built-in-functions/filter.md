# Filter (Pipeline Control Flow)

The `filter` function evaluates a JSONLogic condition and, when it is false, either halts the workflow or skips the task.

## Overview

Filter is a gate function: it controls whether subsequent tasks execute and leaves data untouched. Typical patterns:

- **Guard clauses**: halt a workflow early if prerequisites aren't met
- **Conditional branches**: skip optional processing steps
- **Data quality gates**: stop processing if data doesn't meet criteria

## Configuration

```json
{
    "function": {
        "name": "filter",
        "input": {
            "condition": { "JSONLogic expression" },
            "on_reject": "halt | skip"
        }
    }
}
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `condition` | JSONLogic | Yes | Condition to evaluate against the full message context |
| `on_reject` | string | No | What to do when condition is false: `"halt"` (default) or `"skip"` |

## Rejection Behavior

### `halt` (default)

When the condition is false, the **entire workflow stops**; no further tasks in the workflow execute.

```json
{
    "id": "guard_active_status",
    "name": "Check Active Status",
    "function": {
        "name": "filter",
        "input": {
            "condition": {"==": [{"var": "data.status"}, "active"]},
            "on_reject": "halt"
        }
    }
}
```

If `data.status` is not `"active"`, the workflow halts immediately. The audit trail records the halt.

### `skip`

When the condition is false, only the **current task is skipped**; the workflow continues with the next task.

```json
{
    "id": "optional_premium_check",
    "name": "Check Premium Tier",
    "function": {
        "name": "filter",
        "input": {
            "condition": {"==": [{"var": "data.tier"}, "premium"]},
            "on_reject": "skip"
        }
    }
}
```

If `data.tier` is not `"premium"`, the engine skips this task silently and runs the next one.

## Examples

### Guard Clause Pattern

Stop processing if required data is missing:

```json
{
    "id": "validation_pipeline",
    "name": "Validation Pipeline",
    "tasks": [
        {
            "id": "parse",
            "name": "Parse",
            "function": { "name": "parse_json", "input": {"source": "payload", "target": "input"} }
        },
        {
            "id": "require_email",
            "name": "Require email",
            "function": {
                "name": "filter",
                "input": {
                    "condition": {"!!": {"var": "data.input.email"}},
                    "on_reject": "halt"
                }
            }
        },
        {
            "id": "process",
            "name": "Process",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.result", "logic": {"cat": ["Processed: ", {"var": "data.input.email"}]}}
                    ]
                }
            }
        }
    ]
}
```

### Multi-Condition Gate

Combine conditions with JSONLogic `and`/`or`:

```json
{
    "id": "complex_gate",
    "name": "Complex gate",
    "function": {
        "name": "filter",
        "input": {
            "condition": {
                "and": [
                    {">=": [{"var": "data.order.total"}, 100]},
                    {"==": [{"var": "data.order.currency"}, "USD"]},
                    {"!!": {"var": "data.order.shipping_address"}}
                ]
            },
            "on_reject": "halt"
        }
    }
}
```

### Optional Processing Step

Use `skip` for non-critical conditional logic:

```json
{
    "tasks": [
        {
            "id": "apply_coupon",
            "name": "Apply coupon",
            "function": {
                "name": "filter",
                "input": {
                    "condition": {"!!": {"var": "data.coupon_code"}},
                    "on_reject": "skip"
                }
            }
        },
        {
            "id": "process_coupon",
            "name": "Process coupon",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.discount", "logic": 10}
                    ]
                }
            }
        }
    ]
}
```

## Status Codes

| Code | Meaning | Behavior |
|------|---------|----------|
| `200` | Pass | Condition was true, continue normally |
| *(none)* | Skip | Condition false + `on_reject: skip`: skip task, continue workflow |
| `299` | Halt | Condition false + `on_reject: halt`: stop the remaining tasks in this workflow |

A skip records **no** audit-trail entry and therefore no status code;
`TaskOutcome::Skip` is the one outcome without one. Halt uses
`HALT_STATUS_CODE` (`299`), a public constant you can compare against instead
of a magic number.

## Notes

- The engine **pre-compiles** the filter condition at startup for zero runtime overhead
- Filter never modifies the message; it only controls execution flow
- When a workflow halts, the audit trail records the halt for debugging
- When a task is skipped, no audit trail entry is created
- An expression that **fails to evaluate** is treated exactly like a false
  condition, so `on_reject` fires. With the default `halt` that stops the
  workflow with a `299` and nothing on `message.errors()`; a malformed filter
  is indistinguishable from a legitimate gate closing
- A skipped task writes no `metadata.progress` either, so a downstream rule
  reading `metadata.progress.task_id` still sees the *previous* task
