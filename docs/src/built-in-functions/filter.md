# Filter (Pipeline Control Flow)

The `filter` function provides pipeline control flow by evaluating a JSONLogic condition and either halting the workflow or skipping the task when the condition is false.

## Overview

Filter is a gate function — it doesn't modify data but controls whether subsequent tasks execute. This enables patterns like:

- **Guard clauses** — halt a workflow early if prerequisites aren't met
- **Conditional branches** — skip optional processing steps
- **Data quality gates** — stop processing if data doesn't meet criteria

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

When the condition is false, the **entire workflow stops** — no further tasks in the workflow execute.

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

If `data.status` is not `"active"`, the workflow halts immediately. The halt is recorded in the audit trail.

### `skip`

When the condition is false, only the **current task is skipped** — the workflow continues with the next task.

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

If the user is not premium, this task is skipped silently and the next task runs.

## Examples

### Guard Clause Pattern

Stop processing if required data is missing:

```json
{
    "id": "validation_pipeline",
    "tasks": [
        {
            "id": "parse",
            "function": { "name": "parse_json", "input": {"source": "payload", "target": "input"} }
        },
        {
            "id": "require_email",
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
| `298` | Skip | Condition false + `on_reject: skip` — skip task, continue workflow |
| `299` | Halt | Condition false + `on_reject: halt` — stop workflow execution |

## Notes

- The filter condition is **pre-compiled** at engine startup for zero runtime overhead
- Filter never modifies the message — it only controls execution flow
- When a workflow halts, the halt is recorded in the audit trail for debugging
- When a task is skipped, no audit trail entry is created
