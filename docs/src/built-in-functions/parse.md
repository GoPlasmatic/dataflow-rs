# Parse Functions

`parse_json` and `parse_xml` load input data into the `data` context. Because a message's raw `payload` sits outside the JSONLogic evaluation context (`data`, `metadata`, and `temp_data`), workflows typically begin with one of these two functions. `parse_json` and `parse_xml` are the only built-in functions that can read from `payload`.

Both functions accept the same two parameters:

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `source` | string \| JSONLogic | Yes | Source location to read from: `"payload"`, `"payload.<path>"`, or a context path such as `"data.raw"` |
| `target` | string \| JSONLogic | Yes | Destination path relative to `data` (for example, `"request"` writes to `data.request`) |

- `source` always names a path (not an inline value).
- `target` is always resolved relative to `data` (so `"target": "data.t"` writes to `data.data.t`).
- Both parameters can be static strings or dynamic JSONLogic expressions evaluated against the message context.

## `parse_json`

Reads the value at `source` and writes it to `data.<target>`:

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

How `parse_json` handles different source values:

| Value at `source` | Written to `data.<target>` |
|------------------|----------------------------|
| A valid JSON string | The parsed JSON value (duplicate object keys keep their first position and last value) |
| Any non-JSON string | The original string, unchanged |
| An object, array, number, boolean, or `null` | A copy of the value as-is |
| Missing path (does not exist) | `null` |

Because non-JSON strings and already-parsed objects are copied as-is, `parse_json` never fails on malformed JSON text—a workflow works identically whether the caller passes a raw JSON string or a pre-parsed JSON object in `payload`.

### Examples

Reading the entire payload, a nested path inside the payload, a JSON string inside `data`, a plain string, a `temp_data` field, and a missing path:

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

When run with payload `{"body": {"user": {"id": 123, "name": "Bob"}}, "n": 1}` and initial `data` `{"raw": "{\"r\": 1, \"s\": [true]}", "plain": "not json"}`, every task succeeds with status `200` and `data` finishes as:

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

Notice `data.data.t`: because `target` is always relative to `data`, `"target": "data.t"` writes to `data.data.t`. And because `temp_data.t` was absent, `null` was written.

### Dynamic `source` and `target` Expressions

Because both `source` and `target` accept JSONLogic expressions, a task can compute them dynamically per message:

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

When `temp_data.i` is `2` and `data.batches` is `["a", "b", {"k": "c"}]`, this writes `data.batch_2 = {"k": "c"}`.

- **Static `source` validation:** A literal `source` is validated at build time. For example, `"source": "extra.x"` does not start with `payload`, `data`, `metadata`, or `temp_data`, so compilation rejects the definition with `INVALID_PATH` at `tasks[0].function.input.source`. (A root with a trailing dot—such as `"payload."` or `"data."`—refers to the root itself.)
- **Dynamic `source` validation:** A computed `source` is validated at runtime when evaluated. If it resolves to an invalid root like `"extra.x"`, the task fails with `CONFIG_ERROR` (status `500`, governed by `on_failure`). If a dynamic expression fails to evaluate, the task fails with `LOGIC_ERROR`.
- **No secrets in paths:** Because `source` and `target` determine the destination path recorded in the audit trail, neither expression may reference `{"secret": "..."}`. See [Secrets](../advanced/secrets.md).

## `parse_xml`

Reads an XML string from `source`, parses it into a JSON structure, and writes the result to `data.<target>`:

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

Unlike `parse_json`, `parse_xml` requires the value at `source` to be a valid XML string; otherwise the task fails with `VALIDATION_ERROR` (status `500`, governed by `on_failure`):

| Value at `source` | Result |
|------------------|--------|
| Valid XML document string | Parsed JSON object written to `data.<target>` |
| Malformed XML string | Fails with `VALIDATION_ERROR`: `Validation error: Failed to parse XML: ...` |
| Non-string or missing value | Fails with `VALIDATION_ERROR`: `Validation error: ParseXml: Source '...' is not a string` |

### XML-to-JSON Conversion Rules

`parse_xml` uses [`quick-xml`](https://docs.rs/quick-xml)'s Serde deserializer:
- Element text content is placed under `"$text"`.
- XML attributes are prefixed with `@` (for example, `"@id"`).
- The outer root element tag is stripped, so the object written to `data.<target>` contains the root element's children and attributes.

| XML Input | JSON Written to `data.<target>` |
|--------------|----------------------------|
| `<root><name>Alice</name></root>` | `{"name": {"$text": "Alice"}}` |
| `<root><person id="1" role="admin"/></root>` | `{"person": {"@id": "1", "@role": "admin"}}` |
| `<root><person id="7">Ann</person></root>` | `{"person": {"@id": "7", "$text": "Ann"}}` |
| `<root><empty/></root>` | `{"empty": {}}` |
| `<root><a><b>x</b></a></root>` | `{"a": {"b": {"$text": "x"}}}` |
| `<root><z>1</z><a>2</a><m>3</m></root>` | `{"z": {"$text": "1"}, "a": {"$text": "2"}, "m": {"$text": "3"}}` |
| `<root><item>a</item><item>b</item></root>` | `{"item": {"$text": "b"}}` |

Keep four rules in mind when working with `parse_xml` output:

1. **Leaf elements are objects containing `"$text"`, not bare strings.** To read `<name>Alice</name>`, reference `{"var": "data.request.name.$text"}` (comparing `data.request.name` directly to `"Alice"` evaluates to `false`).
2. **All text and attribute values are strings.** `<age>30</age>` parses as `{"$text": "30"}`.
3. **Child element order is preserved.**
4. **Repeated sibling tags overwrite earlier ones.** As shown in the last row above, repeated `<item>` tags keep the first key position and the last value (`"b"`). If you need to parse repeated XML elements into JSON arrays, register a custom [host function](./host-functions.md) with your own XML schema/parser.

### Example: Parsing and Flattening XML

A common pattern is to follow `parse_xml` with a `map` task that extracts `$text` values into flat fields:

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

Given the payload `"<user><name>Alice</name><email>alice@example.com</email></user>"`, `data` finishes as:

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

## Handling Parse Failures

When `parse_xml` fails (or when a dynamic `source`/`target` expression fails in `parse_json`), the task records status `500` on the audit trail, appends an `ErrorRecord` to `message.errors()`, and follows the task's `on_failure` policy:

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

If `data.broken` is `"<a><b></a>"`, `broken` records status `500` and adds a `VALIDATION_ERROR`:

```text
Task broken error: Validation error: Failed to parse XML: ill-formed document: expected `</b>`, but `</a>` was found
```

Because `"on_failure": "continue"` is set on `broken`, the `after` task still runs. Without `"on_failure": "continue"`, the failure stops the run at `broken`.

## Next Steps

- [Map](./map.md) — Transform parsed data
- [Validation](./validation.md) — Validate parsed fields
- [Publish](./publish.md) — Serialize data back to JSON or XML
