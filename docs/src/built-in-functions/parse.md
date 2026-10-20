# Parse Functions

`parse_json` and `parse_xml` bring input into the context. A workflow
typically starts with one of them, because the payload is not part of the
JSONLogic evaluation context: an expression reads `data`, `metadata` and
`temp_data`, and nothing else. The parse functions are the only built-ins
that read the payload.

Both take the same two parameters:

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `source` | string \| JSONLogic | Yes | What to read: `payload`, `payload.<path>`, or a context path such as `data.raw` |
| `target` | string \| JSONLogic | Yes | Where to write, relative to `data`: `request` is `data.request` |

`source` names a location, never a value. `target` is always under `data`, so
a target spelled `data.t` lands at `data.data.t`. Either parameter may be
computed from the message; a computed `source` may name the payload too.

## parse_json

Reads the value the source names and writes it at the target.

```json
{
    "function": {
        "name": "parse_json",
        "input": {
            "source": "payload",
            "target": "request"
        }
    }
}
```

What is written depends on what the source holds:

| The source holds | Written at `data.<target>` |
|------------------|----------------------------|
| A string that parses as JSON | The parsed value; a key held twice keeps its first position and its last value |
| Any other string | The string, unchanged |
| An object, array, number, boolean or `null` | A copy of the value |
| Nothing (the path does not exist) | `null` |

So `parse_json` never fails on its input. A payload that arrives as JSON text
is parsed; one that arrives already structured is copied as it is.

### Examples

The whole payload, a path into it, a JSON string in the context, a context
path, and a missing source:

```json
{
    "id": "w",
    "tasks": [
        {"id": "whole", "function": {"name": "parse_json", "input": {"source": "payload", "target": "request"}}},
        {"id": "nested", "function": {"name": "parse_json", "input": {"source": "payload.body.user", "target": "user"}}},
        {"id": "string", "function": {"name": "parse_json", "input": {"source": "data.raw", "target": "parsed"}}},
        {"id": "plain", "function": {"name": "parse_json", "input": {"source": "data.plain", "target": "kept"}}},
        {"id": "context", "function": {"name": "parse_json", "input": {"source": "temp_data.t", "target": "data.t"}}},
        {"id": "missing", "function": {"name": "parse_json", "input": {"source": "payload.nothing", "target": "absent"}}}
    ]
}
```

Run over the payload `{"body": {"user": {"id": 123, "name": "Bob"}}, "n": 1}`
and the data `{"raw": "{\"r\": 1, \"s\": [true]}", "plain": "not json"}`,
every task records `200` and `data` ends as:

```json
{
    "raw": "{\"r\": 1, \"s\": [true]}",
    "plain": "not json",
    "request": {"body": {"user": {"id": 123, "name": "Bob"}}, "n": 1},
    "user": {"id": 123, "name": "Bob"},
    "parsed": {"r": 1, "s": [true]},
    "kept": "not json",
    "data": {"t": null},
    "absent": null
}
```

Note `data.data.t`: the target `data.t` is relative to `data`, like every
other target. And `temp_data.t` was not set, so `null` was written.

### Computed parameters

Both parameters are JSONLogic, so one task can pick its source and target per
message:

```json
{
    "function": {
        "name": "parse_json",
        "input": {
            "source": {"cat": ["data.batches.", {"var": "temp_data.i"}]},
            "target": {"cat": ["batch_", {"var": "temp_data.i"}]}
        }
    }
}
```

With `temp_data.i` at `2` and `data.batches` at `["a", "b", {"k": "c"}]`,
this writes `data.batch_2 = {"k": "c"}`.

A literal `source` is checked when the engine is built: `"source": "extra.x"`
names no payload path and no context root, so the definition is refused with
`INVALID_PATH` at `tasks[0].function.input.source`. A bare root with a trailing
dot (`"payload."`, `"data."`, `"metadata."`, `"temp_data."`) names the root
itself. A computed source is
checked when it is evaluated: one that resolves to `extra.x` fails the call
with `CONFIG_ERROR` (``Task p error: path `extra.x` must start at `data`, `metadata` or `temp_data` ``),
the audit entry records `500`, and the task's `on_failure` decides what
happens next. A computed parameter that fails to
evaluate fails the call the same way, with `LOGIC_ERROR`.

Both parameters name where the engine writes, and the destination is recorded
in each `Change`, so neither may read `{"secret": …}`. See
[Secrets](../advanced/secrets.md).

## parse_xml

Reads the string the source names, parses it as an XML document, and writes
the result at the target.

```json
{
    "function": {
        "name": "parse_xml",
        "input": {
            "source": "payload",
            "target": "request"
        }
    }
}
```

The source must hold a string, and the string must be a document. Anything
else fails the call with `VALIDATION_ERROR`:

| The source holds | Result |
|------------------|--------|
| A string holding an XML document | The document, converted as below |
| A string that is not a document | `Validation error: Failed to parse XML: …` |
| Anything but a string, or nothing | `Validation error: ParseXml: Source 'data.obj' is not a string` |

The audit entry records `500`, the message gets the record as
`Task <id> error: <message>`, and the task's `on_failure` decides: by default
the failure reaches the workflow and stops the run; with
`"on_failure": "continue"` the next task runs.

### XML to JSON conversion

The document is read with [`quick-xml`](https://docs.rs/quick-xml)'s serde
reader. Text lands under `$text` and attributes under `@name`; the root
element is consumed, so paths into the result start at its children. Each row
shows a whole document, since that is what the source holds:

| XML document | Written at `data.<target>` |
|--------------|----------------------------|
| `<root><name>Alice</name></root>` | `{"name": {"$text": "Alice"}}` |
| `<root><person id="1" role="admin"/></root>` | `{"person": {"@id": "1", "@role": "admin"}}` |
| `<root><person id="7">Ann</person></root>` | `{"person": {"@id": "7", "$text": "Ann"}}` |
| `<root><empty/></root>` | `{"empty": {}}` |
| `<root><a><b>x</b></a></root>` | `{"a": {"b": {"$text": "x"}}}` |
| `<root><z>1</z><a>2</a><m>3</m></root>` | `{"z": {"$text": "1"}, "a": {"$text": "2"}, "m": {"$text": "3"}}` |
| `<root><item>a</item><item>b</item></root>` | `{"item": {"$text": "b"}}` |

Four consequences to plan for before writing the mappings that read the
result:

- **A leaf is an object, not a string.** The path to Alice's name is
  `data.request.name.$text`. A condition comparing `data.request.name` to
  `"Alice"` is false, since the value there is `{"$text": "Alice"}`.
- **Every value is a string.** `<age>30</age>` gives `{"$text": "30"}`. XML
  carries no types; compare against `"30"` or convert.
- **Elements keep document order, in every build.** The keys of the sixth
  row come out as `z`, `a`, `m`, however the build's `serde_json` is
  configured. 3.x's default build sorted them.
- **A repeated element does not become an array.** It keeps its first
  position and its last value, so the last row drops `a`. For a document with
  repeated elements, parse it in a [host function](./host-functions.md) that
  controls its own XML reader.

### Example

```json
{
    "id": "w",
    "tasks": [
        {
            "id": "parse",
            "function": {"name": "parse_xml", "input": {"source": "payload", "target": "request"}}
        },
        {
            "id": "flatten",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {"path": "data.user.name", "logic": {"var": "data.request.name.$text"}},
                        {"path": "data.user.email", "logic": {"var": "data.request.email.$text"}}
                    ]
                }
            }
        }
    ]
}
```

With the payload `"<user><name>Alice</name><email>alice@example.com</email></user>"`,
`data` ends as:

```json
{
    "request": {
        "name": {"$text": "Alice"},
        "email": {"$text": "alice@example.com"}
    },
    "user": {
        "name": "Alice",
        "email": "alice@example.com"
    }
}
```

The `map` lifts the `$text` leaves into plain strings, which is the usual
second step after `parse_xml`.

## Failures

`parse_json` fails only on a computed source that names no source, or a
computed parameter that does not evaluate. `parse_xml` fails on a source that
is not a string or not a document. Each failure records `500` on the audit
trail and a record on the message; `on_failure` on the task decides whether
the workflow goes on:

```json
{
    "id": "w",
    "tasks": [
        {
            "id": "broken",
            "on_failure": "continue",
            "function": {"name": "parse_xml", "input": {"source": "data.broken", "target": "doc"}}
        },
        {
            "id": "after",
            "function": {"name": "map", "input": {"mappings": [{"path": "data.after", "logic": true}]}}
        }
    ]
}
```

With `data.broken` at `"<a><b></a>"`, `broken` records `500`, the message
gets a `VALIDATION_ERROR` record with the text

```text
Task broken error: Validation error: Failed to parse XML: ill-formed document: expected `</b>`, but `</a>` was found
```

and `after` still runs. Without `"on_failure": "continue"` the run stops at
`broken`, with that error as the run's status.

## Next Steps

- [Map](./map.md): transform the parsed data
- [Validation](./validation.md): check it
- [Publish](./publish.md): serialize it for output
