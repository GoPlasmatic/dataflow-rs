# Filter Function

The `filter` function acts as a control-flow gate inside a workflow: it evaluates a JSONLogic `condition` and, when the condition is not `true`, either halts the rest of the workflow or skips the current task. It is read-only and never modifies the message context.

```json
{
    "function": {
        "name": "filter",
        "input": {
            "condition": {"==": [{"var": "data.status"}, "active"]},
            "on_reject": "halt"
        }
    }
}
```

## Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `condition` | JSONLogic | Yes | Condition expression; passes only when it evaluates to boolean `true` |
| `on_reject` | `"halt"` \| `"skip"` | No | Action when `condition` is not `true`: `"halt"` (default) stops the current workflow; `"skip"` skips only this task |

- **Strict boolean check:** The filter passes only when `condition` evaluates to boolean `true`. Values like `false`, `null`, `1`, `"true"`, or `[1]` are rejected. Always write presence checks as `{"!!": {"var": "data.email"}}` rather than `{"var": "data.email"}`.
- **Evaluation errors:** If `condition` fails to evaluate, the filter rejects the message and records an `EVALUATION_ERROR` (`Filter condition failed to evaluate: ...`) on `message.errors()` so broken expressions are visible.
- **Operation budget:** If `condition` exceeds the `ExprEngine`'s [operation budget](../advanced/jsonlogic.md) (`ops_budget`), the task fails with `BUDGET_EXCEEDED` under `on_failure`.

## Rejection Behavior (`on_reject`)

### `"halt"` (Default)

Stops the current workflow immediately—no subsequent tasks in the workflow run. The audit trail records status `299`, the workflow exits with `WorkflowExit::Halted`, and execution continues to the next workflow in the program:

```json
[
    {
        "id": "orders",
        "priority": 1,
        "tasks": [
            {
                "id": "require_active",
                "function": {
                    "name": "filter",
                    "input": {"condition": {"==": [{"var": "data.status"}, "active"]}}
                }
            },
            {
                "id": "process",
                "function": {"name": "map", "input": {"mappings": [{"path": "data.processed", "logic": true}]}}
            }
        ]
    },
    {
        "id": "audit",
        "priority": 2,
        "tasks": [
            {
                "id": "mark",
                "function": {"name": "map", "input": {"mappings": [{"path": "data.audited", "logic": true}]}}
            }
        ]
    }
]
```

When `data.status` is `"closed"`, `require_active` records status `299`, `process` is skipped, and the `audit` workflow runs normally, leaving `data` as `{"status": "closed", "audited": true}`.

### `"skip"`

Skips only the `filter` task itself and continues to the next task in the workflow. A skipped filter writes no audit entry and does not update `metadata.progress` (so `metadata.progress.task_id` still reflects the task that ran before the filter). Note that `terminal: true` on a skipped filter does not trigger.

```json
{
    "id": "w",
    "tasks": [
        {
            "id": "first",
            "function": {"name": "map", "input": {"mappings": [{"path": "data.first", "logic": true}]}}
        },
        {
            "id": "coupon",
            "function": {
                "name": "filter",
                "input": {"condition": {"!!": {"var": "data.coupon_code"}}, "on_reject": "skip"}
            }
        },
        {
            "id": "next",
            "function": {"name": "map", "input": {"mappings": [{"path": "data.next", "logic": true}]}}
        }
    ]
}
```

When `data.coupon_code` is absent, the audit trail contains `first` (`200`) and `next` (`200`), with no entry for `coupon`.

## Status Codes

| Audit Status | Meaning |
|--------|---------|
| `200` | `condition` evaluated to `true`; workflow execution continues |
| `299` | Rejected with `on_reject: "halt"`; remaining tasks in the current workflow are skipped |
| *(no entry)* | Rejected with `on_reject: "skip"`; no audit entry is written and the next task runs |

## Examples

### Guard Clause

Stop a workflow early if a required field is missing:

```json
{
    "id": "pipeline",
    "tasks": [
        {
            "id": "parse",
            "function": {"name": "parse_json", "input": {"source": "payload", "target": "input"}}
        },
        {
            "id": "require_email",
            "function": {"name": "filter", "input": {"condition": {"!!": {"var": "data.input.email"}}}}
        },
        {
            "id": "process",
            "function": {
                "name": "map",
                "input": {"mappings": [{"path": "data.result", "logic": {"cat": ["Processed: ", {"var": "data.input.email"}]}}]}
            }
        }
    ]
}
```

### Combining Multiple Conditions

Use `and` and `or` for compound guard conditions:

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
            }
        }
    }
}
```

## Choosing Between `filter`, Task `condition`, and `validation`

- **To skip a single task:** Put a `condition` directly on that task (or wrap a span of tasks in a [task group](../advanced/control-flow.md#groups)).
- **To exit the rest of a workflow silently when a precondition is not met:** Use a `filter` task with `"on_reject": "halt"` (or a `terminal: true` task/group).
- **To validate data and record error messages when rules fail:** Use a [`validation`](./validation.md) task with `"on_rejection": "halt"`. Do not place a `filter` after `validation` just to halt, as the filter's `299` status would overwrite the validation's `400` status in `metadata.progress`.
