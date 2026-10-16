# Map Function

`map` writes the results of JSONLogic expressions to context paths, or
removes paths. It is the built-in that does the transforming.

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
| `mappings` | array | Yes | The mappings, applied in order |

### Mapping

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string \| JSONLogic | Yes | Where the result is written: a context path, or JSONLogic computing one; see [Paths](#paths) |
| `logic` | JSONLogic | Unless `unset` | The value to write. A `null` result is skipped; see [Null results](#null-results) |
| `unset` | boolean | No | `true` removes the key at `path` instead of writing it. Takes no `logic`; see [Removing a path](#removing-a-path) |
| `on_null` | `"skip"` \| `"unset"` | No | What a `null` result does: `skip` (the default) leaves the path as it was, `unset` removes it |
| `mode` | `"set"` \| `"append"` \| `"extend"` | No | How a result is written: `set` (the default) assigns it, `append` and `extend` add to the array at `path`; see [Appending to an array](#appending-to-an-array) |

Mappings run in order, each against the context as the ones before it left
it, so a later mapping reads an earlier one's write:

```json
{
    "mappings": [
        {"path": "temp_data.full_name", "logic": {"cat": [{"var": "data.first"}, " ", {"var": "data.last"}]}},
        {"path": "data.greeting", "logic": {"cat": ["Hello, ", {"var": "temp_data.full_name"}]}}
    ]
}
```

## Paths

A path starts at one of the three roots, `data`, `metadata` or `temp_data`,
and names a key per dot. Missing containers are created on the way:

```json
{"path": "data.user.profile.name", "logic": "John"}
```

writes `{"data": {"user": {"profile": {"name": "John"}}}}`.

A segment means one thing on an object and another on an array. On an object
it is a key; on an array a numeric segment is an index, so `data.list.2`
writes the third element of `data.list`, padding with `null` as needed, and
creates `data.list` as an array when it is missing:

```json
{"path": "data.list.0", "logic": "zero"},
{"path": "data.list.2", "logic": "two"}
```

leaves `data.list` as `["zero", null, "two"]`. To name a key that looks like
a number, prefix it with `#`: one leading `#` is stripped, and the segment is
never an index, so

```json
{"path": "data.items.#0", "logic": "first"}
```

writes `{"items": {"0": "first"}}`. A number above 1,048,575 is a key too,
never an index, so no path can grow an array without bound. A path holds at
most 64 segments.

A literal path outside the three roots is refused when the engine is built,
as `INVALID_PATH` at the mapping's `path`; a path that is not a string, and
not JSONLogic computing one, is `INPUT_PARSE`.

### Root paths

Writing a root merges an object into it:

```json
{"path": "data", "logic": {"merged": true}}
```

adds `merged` to `data` and keeps the keys already there. The change's
`new_value` is the object written, `{"merged": true}`, not the root after the
merge; its `old_value` is the root before it. A result that is not an object replaces
the root with it, which leaves the context with a root that is no longer an
object; write an object at a root.

### Computed destinations

`path` is JSONLogic like every other parameter, so one mapping can write
somewhere different for each message:

```json
{
    "path": {"cat": ["data.accounts.", {"var": "data.id"}, ".balance"]},
    "logic": {"var": "data.amount"}
}
```

With `data.id` at `"ACC7"` and `data.amount` at `42`, this writes
`data.accounts.ACC7.balance = 42`, and the change records that path, not the
expression. A plain string is JSONLogic for itself, so the static spelling
folds to a path when the engine is built and costs nothing per message. Only
a destination that reads the message is evaluated per write.

A computed path is checked when it is evaluated: one that resolves to
something outside the three roots fails that mapping (the rest still run, and
the call ends with `500`; see [Failures](#failures)).

## Expressions

`logic` is JSONLogic evaluated against the context. A few shapes:

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

### Literal objects

An expression is evaluated in templating mode: a multi-key object is an
output template, and a single-key object whose key names an operator is that
operator. Prefix the key with `$` to write it as data:

| `logic` | Writes |
|---------|--------|
| `{"cat": ["a", "b"]}` | `"ab"` |
| `{"$cat": ["a", "b"]}` | `{"cat": ["a", "b"]}` |
| `{"$$oid": "x"}` | `{"$oid": "x"}` |
| `{"result": {"var": "data.n"}}` | `{"result": 600}`, with `data.n` at `600` |

Exactly one `$` is stripped from every template key, not only from keys that
collide with an operator, so a key that must start with `$` is doubled. A key
naming no operator needs no escape: `{"result": …}` is the ordinary single-key
output template, evaluating its argument. See
[Literal keys and the `$` escape](../advanced/jsonlogic.md#literal-keys-and-the--escape).

### Secrets are refused

A mapping's result is written to the message, which the engine records, so a
mapping may not read `{"secret": "name"}`: not in `logic`, not in a computed
`path`. The definition is refused with `SECRET_IN_MESSAGE_WRITE` at the
expression. Compute a derived value (an HMAC, a signed URL) in a
[host function](./host-functions.md), which reads a secret through its own
config. See [Secrets](../advanced/secrets.md).

## Null results

A `logic` that evaluates to `null` is skipped: the path keeps whatever it
held, and no change is recorded.

```json
{"path": "data.copy", "logic": {"var": "data.optional"}}
```

The skip is what makes `{"if": [cond, value, null]}` mean "set or keep". It
also means `null` can never clear anything: `"logic": null` is a no-op, kept
loadable for the installed base and reported as the advisory `NULL_MAPPING`
(any `logic` that folds to a constant `null` is, under the default
`on_null`). Removal is explicit.

## Removing a path

`"unset": true` removes the key at `path`. It takes no `logic`:

```json
{"path": "temp_data.retry", "unset": true}
```

`"on_null": "unset"` keeps the `logic` and makes a `null` result remove the
path instead of being skipped, so `{"if": [cond, value, null]}` means "set or
clear":

```json
{
    "path": "temp_data.reason",
    "logic": {"if": [{"var": "data.ok"}, null, "FAILED"]},
    "on_null": "unset"
}
```

Removing an absent key does nothing and records nothing. A removal that does
happen records a `Change` with `removed: true` and the old value:

```json
{"path": "data.y", "old_value": 2, "new_value": null, "removed": true}
```

An index removes that element and shifts the later ones down: with
`data.gone` at `["a", "b", "c"]`, `{"path": "data.gone.0", "unset": true}`
leaves `["b", "c"]`.

Clearing with `false` is not the same thing. The key stays present, so
`missing` and `exists` report it, `{"var": ["temp_data.x", "fallback"]}` and
`??` return `false` rather than the fallback, and the value travels with the
message. The difference shows most in a [loop](../advanced/loops.md), where
`temp_data` carries over between sweeps: a per-item slot set with "set or
keep" still holds the previous item's value in every sweep that does not set
it. Clear it at the end of the sweep with `unset`, or set it with
`on_null: "unset"`.

### The rules between the keys

A mapping that breaks one of these is refused when the engine is built, as
`INVALID_MAPPING` at the key to change:

| Rule | Reported at | Means |
|------|-------------|-------|
| A mapping has `logic` or `"unset": true` | `logic` | a mapping needs `logic`, or `"unset": true` to remove the path |
| Never both | `unset` | `unset` removes the path and takes no `logic`; to remove it only when the result is null, keep `logic` and use `"on_null": "unset"` |
| `on_null` needs `logic` | `on_null` | `on_null` says what a null `logic` result does, and this mapping has no `logic` |
| `mode` needs `logic` | `mode` | `mode` says how a `logic` result is written, and this mapping has no `logic` |
| A root cannot be removed | `path` | a context root (`data`, `metadata`, `temp_data`) cannot be removed; remove the keys under it instead |
| A root cannot be appended to | `path` | a context root (`data`, `metadata`, `temp_data`) is an object and cannot be appended to; name an array under it instead |

Only a literal path names a root here. A computed path that resolves to a
root fails that mapping when it runs, leaving the root in place.

## Appending to an array

`"mode": "append"` pushes the result onto the array at `path`, in place:

```json
{
    "path": "data.log",
    "logic": {"turn": {"var": "temp_data.turn"}, "move": {"var": "temp_data.move"}},
    "mode": "append"
}
```

`"mode": "extend"` pushes each element of an array result instead, so
`{"logic": [2, 3], "mode": "extend"}` adds two elements where `append` would
add one two-element array.

What the target may hold:

- missing or `null`: it becomes a new array holding the result;
- an array: the result is added at the end;
- anything else: the mapping fails, and the value is not wrapped into an
  array.

`extend` with a result that is not an array fails the mapping. A `null`
result follows `on_null` as for any other mapping: skipped by default, or
removing the path under `on_null: "unset"`.

Each element added records one `Change`, at its index:

```json
{"path": "data.log.0", "old_value": null, "new_value": {"turn": 1}},
{"path": "data.log.1", "old_value": null, "new_value": {"turn": 2}}
```

That is why the modes exist. Rebuilding the array with
`{"merge": [{"var": "data.log"}, [{"var": "temp_data.entry"}]]}` gives the
same result, but copies the whole array on every append and records all of
it, so a [loop](../advanced/loops.md) that accumulates a log pays for the
square of its length. `append` converts and copies only the new element.

## Failures

A mapping fails when its `logic` does not evaluate, when a computed `path`
does not resolve to a context path, when it would remove or append to a
root, or when it appends to a target that is not an array. The failure is
logged, the remaining mappings still run, and the call ends with status
`500`:

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

With `data.s` at `"str"`, the second mapping fails (`Map: Cannot append to
data.s: it holds a string, not an array`), `data.a` and `data.c` are both
written, the audit entry records `500` with those two changes, and the
message gets the record `TASK_STATUS_ERROR` with the text
`Task t returned status 500`. Then the task's `on_failure` decides: by
default the failure reaches the workflow and stops the run, with `TASK_ERROR`
(`Task t failed with status 500`) as the run's status; with
`"on_failure": "continue"` on the task the next task runs. See
[Error Handling](../core-concepts/error-handling.md).

## Audit

Every write records a `Change` on the task's audit entry, when the run
captures changes (on by default): the dotted path written, `old_value`
(`null` when the path was absent) and `new_value`; `removed: true` on a
removal. A root write's `new_value` is the object written. See
[Audit Trails](../advanced/audit-trails.md).

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"map_demo","name":"Map Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"transform","name":"Transform","function":{"name":"map","input":{"mappings":[{"path":"data.full_name","logic":{"cat":[{"var":"data.input.first_name"}," ",{"var":"data.input.last_name"}]}},{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.full_name"},"!"]}},{"path":"data.is_adult","logic":{">=": [{"var":"data.input.age"},18]}},{"path":"temp_data.processed_at","logic":"2024-01-01T00:00:00Z"}]}}}]}]' data-payload='{"first_name":"John","last_name":"Doe","age":25}'>
</div>

## Best Practices

1. **Use `temp_data` for intermediate results**, and `data` for what the
   message carries out.
2. **Order matters**: a mapping sees the writes of the mappings before it.
3. **Handle missing fields** with `if` or `!!`, or let a `null` result skip.
4. **Write an object at a root**, so the write merges.
5. **Clear with `unset`, not `false` or `null`**: `null` is skipped and
   `false` is a value; only `unset` makes a path absent.
