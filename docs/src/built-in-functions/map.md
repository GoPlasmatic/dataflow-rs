# Map Function

The `map` function evaluates JSONLogic expressions and writes the results to paths in the message context (or removes paths). It is the primary built-in function for data transformation.

```json
{
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {
                    "path": "data.full_name",
                    "logic": {"cat": [{"var": "data.first_name"}, " ", {"var": "data.last_name"}]}
                }
            ]
        }
    }
}
```

## Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `mappings` | array | Yes | Ordered list of mappings to apply |

### Mapping Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string \| JSONLogic | Yes | Destination path in the context (`data.*`, `metadata.*`, or `temp_data.*`), or a JSONLogic expression that computes one; see [Paths](#paths) |
| `logic` | JSONLogic | Required unless `unset: true` | Expression computing the value to write. By default, a `null` result is skipped; see [Handling `null` Results](#handling-null-results) |
| `unset` | boolean | No | When `true`, removes the field at `path` instead of writing a value (cannot be combined with `logic`); see [Removing a Path](#removing-a-path) |
| `on_null` | `"skip"` \| `"unset"` | No | Behavior when `logic` evaluates to `null`: `"skip"` (default) leaves `path` unchanged; `"unset"` removes `path` |
| `mode` | `"set"` \| `"append"` \| `"extend"` | No | Write mode: `"set"` (default) assigns the value at `path`; `"append"` and `"extend"` push onto an array at `path`; see [Appending to an Array](#appending-to-an-array) |

Mappings execute sequentially in the order they are listed, and each mapping immediately sees the changes made by earlier mappings in the same task:

```json
{
    "mappings": [
        {"path": "temp_data.full_name", "logic": {"cat": [{"var": "data.first"}, " ", {"var": "data.last"}]}},
        {"path": "data.greeting", "logic": {"cat": ["Hello, ", {"var": "temp_data.full_name"}]}}
    ]
}
```

## Paths

Every destination path must start with one of the three context roots (`data`, `metadata`, or `temp_data`). Missing intermediate objects or arrays are created automatically:

```json
{"path": "data.user.profile.name", "logic": "John"}
```

writes `{"data": {"user": {"profile": {"name": "John"}}}}`.

### Object Keys vs. Array Indices

Path segments behave according to the container they traverse:
- **On an object:** A segment is treated as an object key.
- **On an array (or missing container when the next segment is numeric):** A numeric segment is treated as an array index (`0` through `1,048,575`). Writing to `data.list.2` sets the third element of `data.list`, padding any missing earlier elements with `null`:

  ```jsonc
  {"path": "data.list.0", "logic": "zero"},
  {"path": "data.list.2", "logic": "two"}
  ```

  produces `data.list = ["zero", null, "two"]`.
- **Forcing an object key with `#`:** To write a numeric-looking object key instead of an array index, prefix the segment with `#` (one leading `#` is stripped):

  ```json
  {"path": "data.items.#0", "logic": "first"}
  ```

  produces `{"items": {"0": "first"}}`.

Numbers above `1,048,575` are always treated as object keys so a path can never allocate an unbounded array. Paths can have at most 64 segments.

Literal paths outside `data`, `metadata`, and `temp_data` are rejected at build time with `INVALID_PATH`.

### Writing to a Context Root

Writing directly to a root (`"path": "data"`, `"metadata"`, or `"temp_data"`) **merges** the returned object's keys into that root:

```json
{"path": "data", "logic": {"merged": true}}
```

adds `"merged": true` to `data` while preserving existing keys in `data`. In the audit trail, `new_value` records the merged object (`{"merged": true}`) and `old_value` records the root prior to the merge.

Root paths only accept JSON objects: if `logic` is a non-object constant, compilation rejects the mapping at build time (`INVALID_MAPPING`); if a dynamic expression evaluates to a non-object at runtime, that mapping fails with `MAPPING_ERROR` and leaves the root unchanged.

### Dynamic Destination Paths

Because `path` can be a JSONLogic expression, a single mapping can compute its destination path dynamically from the message:

```json
{
    "path": {"cat": ["data.accounts.", {"var": "data.id"}, ".balance"]},
    "logic": {"var": "data.amount"}
}
```

When `data.id` is `"ACC7"` and `data.amount` is `42`, this writes `data.accounts.ACC7.balance = 42`, and the audit entry records the resolved path (`"data.accounts.ACC7.balance"`). Static string paths are folded at build time with zero per-message cost.

## Expressions (`logic`)

`logic` can be any literal JSON value or JSONLogic expression:

```json
{"path": "data.copy", "logic": {"var": "data.original"}}
```

```json
{"path": "data.status", "logic": "active"}
```

```json
{
    "path": "data.tier",
    "logic": {"if": [
        {">=": [{"var": "data.points"}, 1000]}, "gold",
        {">=": [{"var": "data.points"}, 500]}, "silver",
        "bronze"
    ]}
}
```

```json
{
    "path": "data.count",
    "logic": {"reduce": [{"var": "data.items"}, {"+": [{"var": "accumulator"}, 1]}, 0]}
}
```

```json
{
    "path": "data.name",
    "logic": {"if": [{"!!": {"var": "data.name"}}, {"var": "data.name"}, "Unknown"]}
}
```

### Writing Literal Objects (`$` Escape)

`map` evaluates expressions in templating mode: multi-key objects are treated as object templates whose values are evaluated recursively, while a single-key object whose key matches an enabled JSONLogic operator is evaluated as that operator. To write a literal object whose key collides with an operator name (or starts with `$`), prefix the key with `$`:

| `logic` | Value Written |
|---------|--------|
| `{"cat": ["a", "b"]}` | `"ab"` |
| `{"$cat": ["a", "b"]}` | `{"cat": ["a", "b"]}` |
| `{"$$oid": "x"}` | `{"$oid": "x"}` |
| `{"result": {"var": "data.n"}}` | `{"result": 600}` (when `data.n` is `600`) |

One leading `$` is stripped from any template key, while single-key objects whose key is *not* an operator (like `{"result": ...}`) require no escape. See [Literal keys and the `$` escape](../advanced/jsonlogic.md#literal-keys-and-the--escape).

### Secrets Are Not Allowed in `map`

Because `map` writes its output into the `Message` (which is serialized and traced), neither `logic` nor `path` may reference `{"secret": "name"}`—the compiler rejects it at build time with `SECRET_IN_MESSAGE_WRITE`. To compute a value derived from a secret (such as an HMAC signature), use a [custom host function](./host-functions.md). See [Secrets](../advanced/secrets.md).

## Handling `null` Results

By default (`"on_null": "skip"`), if `logic` evaluates to `null`, the mapping is skipped: the target path retains its existing value (or remains absent) and no audit change is recorded:

```json
{"path": "data.copy", "logic": {"var": "data.optional"}}
```

This default makes `{"if": [cond, value, null]}` naturally express *"set when `cond` is true, otherwise leave unchanged."* Note that a constant `"logic": null` with the default `on_null` is a no-op and is flagged by `Compiler::check` as a `NULL_MAPPING` advisory.

## Removing a Path

To unconditionally delete a field or array element at `path`, set `"unset": true` (with no `logic` key):

```json
{"path": "temp_data.retry", "unset": true}
```

To conditionally delete `path` whenever `logic` evaluates to `null`, keep `logic` and set `"on_null": "unset"`:

```json
{
    "path": "temp_data.reason",
    "logic": {"if": [{"var": "data.ok"}, null, "FAILED"]},
    "on_null": "unset"
}
```

- Removing a key that is already absent is a no-op and records no audit change.
- Removing an existing key records a `Change` with `removed: true`, `old_value` set to the removed value, and `new_value: null`:

  ```json
  {"path": "data.y", "old_value": 2, "new_value": null, "removed": true}
  ```

- Removing an array index (such as `{"path": "data.gone.0", "unset": true}`) removes that element and shifts subsequent elements down.

> **Why use `unset` instead of `false`?** Setting a field to `false` leaves the key present in the object, so `missing` and `exists` report it as present, `{"var": ["temp_data.x", "fallback"]}` and `??` return `false` instead of `"fallback"`, and the key stays in the serialized message. This matters especially in [loops](../advanced/loops.md), where `temp_data` persists across iterations—use `unset: true` or `on_null: "unset"` to clear per-iteration state cleanly.

### Build-Time Mapping Validation Rules

The compiler checks each mapping at build time and reports `INVALID_MAPPING` if any of these rules are violated:

| Rule | Reported Path | Explanation |
|------|-------------|-------|
| Must have `logic` or `"unset": true` | `logic` | Every mapping must either compute a value with `logic` or remove the path with `"unset": true`. |
| Cannot set both `logic` and `"unset": true` | `unset` | `unset: true` takes no `logic`. To remove a path when `logic` is `null`, use `"on_null": "unset"` instead. |
| `on_null` requires `logic` | `on_null` | `on_null` only applies when evaluating `logic`. |
| `mode` requires `logic` | `mode` | `mode` only applies when writing a `logic` result. |
| Cannot unset a context root | `path` | Top-level roots (`data`, `metadata`, `temp_data`) cannot be removed; unset specific keys inside them instead. |
| Cannot append/extend a context root | `path` | Top-level roots are objects, not arrays; specify an array path under a root instead. |
| Root writes must be objects | `logic` | Writing to `data`, `metadata`, or `temp_data` requires a JSON object. |

## Appending to an Array (`mode: "append"` and `"extend"`)

- **`"mode": "append"`** pushes the evaluated result as a single element onto the end of the array at `path`:

  ```json
  {
      "path": "data.log",
      "logic": {"turn": {"var": "temp_data.turn"}, "move": {"var": "temp_data.move"}},
      "mode": "append"
  }
  ```

- **`"mode": "extend"`** takes an array result from `logic` and pushes each of its elements onto the array at `path` (for example, `{"logic": [2, 3], "mode": "extend"}` appends two elements rather than a nested array).

How `append` and `extend` handle the existing value at `path`:
- **Missing or `null`:** Creates a new array at `path` containing the appended element(s).
- **Existing array:** Appends the element(s) in place at the end of the array.
- **Any other type (object, string, number, boolean):** Fails the mapping with `MAPPING_ERROR` without modifying `path`. (Similarly, `extend` fails if `logic` does not evaluate to an array or `null`.)

Each appended element records a compact `Change` at its new index (`data.log.0`, `data.log.1`, etc.):

```jsonc
{"path": "data.log.0", "old_value": null, "new_value": {"turn": 1}},
{"path": "data.log.1", "old_value": null, "new_value": {"turn": 2}}
```

> **Performance tip:** Always prefer `"mode": "append"` over `{"merge": [{"var": "data.log"}, [item]]}` when accumulating items—especially inside a [loop](../advanced/loops.md). `merge` clones the entire array on every iteration ($O(n^2)$ over $n$ iterations), whereas `append` mutates the array in place and records only the newly added element ($O(1)$ per append).

## Handling Mapping Failures

A mapping fails at runtime if:
- Its `logic` expression fails to evaluate.
- A dynamic `path` expression fails to evaluate or resolves outside `data`, `metadata`, and `temp_data`.
- It attempts to unset, append to, or write a non-object to a context root.
- It uses `append` / `extend` on a target path that holds a non-array value.

When a mapping fails:
1. The engine records a `MAPPING_ERROR` on `message.errors()` with `path` set to the mapping's target path and a descriptive message naming the mapping index.
2. Any remaining mappings in the `map` task **still execute**.
3. The `map` task finishes with status `500` (which adds a `TASK_STATUS_ERROR` record and triggers the task's `on_failure` policy).

> **Note:** If a mapping exceeds the configured [operation budget](../advanced/jsonlogic.md) (`ops_budget`), the task aborts immediately with `BUDGET_EXCEEDED` without running subsequent mappings.

Example of a partial mapping failure:

```json
{
    "id": "t",
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {"path": "data.a", "logic": 1},
                {"path": "data.s", "logic": "x", "mode": "append"},
                {"path": "data.c", "logic": 3}
            ]
        }
    }
}
```

If `data.s` holds `"str"` (a string rather than an array), mapping `1` fails, while `data.a = 1` and `data.c = 3` are both written. The audit entry records status `500` with those two changes, `message.errors()` receives `MAPPING_ERROR` (for `mappings[1]`) and `TASK_STATUS_ERROR` (for task `t`), and the task's `on_failure` policy decides whether the workflow continues or stops.

## Audit Trail

When `RunOptions::capture_changes` is enabled (the default), every write or removal made by `map` adds a `Change` to the task's `AuditEntry` containing `path`, `old_value`, `new_value`, and `removed`. See [Audit Trails](../advanced/audit-trails.md).

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"map_demo","name":"Map Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"transform","name":"Transform","function":{"name":"map","input":{"mappings":[{"path":"data.full_name","logic":{"cat":[{"var":"data.input.first_name"}," ",{"var":"data.input.last_name"}]}},{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.full_name"},"!"]}},{"path":"data.is_adult","logic":{">=": [{"var":"data.input.age"},18]}},{"path":"temp_data.processed_at","logic":"2024-01-01T00:00:00Z"}]}}}]}]' data-payload='{"first_name":"John","last_name":"Doe","age":25}'>
</div>

## Best Practices

1. **Use `temp_data` for Intermediate Values** — Keep temporary calculations in `temp_data` and write final outputs to `data`.
2. **Order Mappings Intentionally** — Later mappings in the same task can read fields written by earlier mappings.
3. **Use `unset` to Delete Keys** — Remember that `null` results are skipped by default (`on_null: "skip"`) and `false` keeps the key present; use `"unset": true` or `"on_null": "unset"` to remove a field.
4. **Use `"mode": "append"` for Growing Arrays** — Avoid cloning arrays with `merge` inside loops.
