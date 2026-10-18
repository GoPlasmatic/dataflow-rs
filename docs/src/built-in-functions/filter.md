# Filter Function

`filter` is a gate: it evaluates a condition and, when the condition is not
met, either halts the workflow or skips the task. It writes nothing.

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
| `condition` | JSONLogic | Yes | Lets the message through only when it evaluates to `true` |
| `on_reject` | `"halt"` \| `"skip"` | No | What a rejected message does: `halt` (the default) stops the workflow, `skip` skips this task only |

Only a literal `true` passes. `false`, `null`, a missing path, `1`, `"true"`
and `[1]` all reject, and record nothing. A condition that fails to evaluate
rejects too, and records `EVALUATION_ERROR` (`Filter condition failed to
evaluate: …`) on the message's errors, so a malformed filter is told apart
from a gate closing; the run then completes with errors. A condition over the
[ops budget](../advanced/jsonlogic.md) is no rejection: it fails the task with
`BUDGET_EXCEEDED`, under its `on_failure`. Write a presence check as
`{"!!": {"var": "data.email"}}`, not `{"var": "data.email"}`.

## What a rejection does

### `halt` (the default)

The workflow stops: no later task in it runs. The audit entry records status
`299`, and the run goes on to the next workflow.

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

With `data.status` at `"closed"`, `require_active` records `299`, `process`
never runs, and `audit` runs: `data` ends as
`{"status": "closed", "audited": true}`.

### `skip`

Only this task is skipped; the next one runs. A skip records no audit entry
and writes no `metadata.progress`, so a later condition reading
`metadata.progress.task_id` sees the task before the filter. A `terminal` on
a skipped filter does not fire.

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

Without `data.coupon_code`, the audit trail holds `first` and `next`, each
`200`, and nothing for `coupon`. A skip on its own gates nothing: pair the
filter with the task it guards through a [group](../advanced/control-flow.md)
or a `condition` on that task, or use `halt`.

## Status codes

| Status | Meaning |
|--------|---------|
| `200` | The condition was `true`; the workflow continues |
| `299` | Rejected under `halt`: the rest of the workflow does not run |
| none | Rejected under `skip`: no audit entry, the next task runs |

`299` is the status every `Halt` records, a host function's included, so a
host can compare against it. See [Host Functions](./host-functions.md).

## Examples

### Guard clause

Stop a workflow whose input lacks what the rest of it needs:

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

### Several conditions

Combine them with `and` and `or`:

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

## Compared with a task condition

A task's `condition` decides whether that one task runs; a `filter` under
`halt` decides whether the rest of the workflow runs. A `filter` under `skip`
is a task condition that records nothing, which is why a condition on the
task itself is usually clearer. A `validation` that must stop the workflow is
gated with `"on_rejection": "halt"` on the validation task, not with a
`filter` after it: the filter's `299` would replace the `400` on
`metadata.progress`. See [Validation](./validation.md#stopping-on-a-failure)
and [Control Flow](../advanced/control-flow.md).

## Notes

- The condition is compiled when the engine is built; a condition that reads
  nothing folds to a constant, so a literal `true` or `false` costs nothing.
- `filter` never modifies the message; it only controls what runs.
- An evaluation error rejects, with the default `halt` that is a `299` and
  nothing on the message's errors.
