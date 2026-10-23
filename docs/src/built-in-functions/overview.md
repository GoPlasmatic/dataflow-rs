# Built-in Functions

Dataflow-rs includes eight built-in functions. They execute inline inside the engine with no external I/O and no registration step required.

| Function | Purpose | Modifies Context |
|----------|--------------|--------------------|
| `parse_json` | Reads or parses JSON from `payload` (or a context path) into `data` | Yes |
| `parse_xml` | Parses an XML string from `payload` (or a context path) into `data` | Yes |
| `map` | Evaluates JSONLogic expressions and writes or removes context fields | Yes |
| `validation` | Evaluates validation rules against the message and records errors for failing rules | No |
| `filter` | Halts the workflow or skips the current task when a condition is not met | No |
| `log` | Emits a structured log message at a configured log level | No |
| `publish_json` | Serializes a value under `data` into a JSON string under `data` | Yes |
| `publish_xml` | Serializes a value under `data` into an XML string under `data` | Yes |

Any other function name referenced by a task must be registered on the `Registry` as a custom [host function](./host-functions.md). All eight built-in names are reserved (`Registry::register` panics if you try to register one). Note that `validate`—the 3.x spelling of `validation`—is automatically rewritten to `validation` when loading 3.x definitions; see [Migrating from 3.x](../migration/from-3.md).

## Calling a Built-in Function

```json
{
    "id": "transform",
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {"path": "data.total", "logic": {"*": [{"var": "data.price"}, {"var": "data.quantity"}]}}
            ]
        }
    }
}
```

The `input` object configures the function and is compiled once when the engine is built. If the configuration is invalid, compilation fails upfront with an `Issue` pointing to the exact JSON path (such as `tasks[0].function.input.mappings[0].path`):

- **Missing required fields, wrong types, or invalid enum variants** are reported as `INPUT_PARSE` (for example, ``missing field `path` `` or ``unknown variant `push`, expected one of `set`, `append`, `extend` ``).
- **Literal context paths outside `data`, `metadata`, or `temp_data`** are reported as `INVALID_PATH`.
- **Unrecognized keys** are preserved in the task's `ext` map and reported as `UNKNOWN_KEY` advisories—or as `LIKELY_TYPO` defects if they are close to a known field name.

Because built-in functions run inline over the engine's context view, `for_each` is not supported on built-in tasks (`INVALID_FOR_EACH`). To transform arrays in a built-in task, use JSONLogic's `map`, `filter`, or `reduce` operators; to fan out async work across array elements, use `for_each` on a [custom host function](./host-functions.md#for_each-on-a-host-function).

## Dynamic Parameters with JSONLogic

Every parameter of every built-in function accepts a JSONLogic expression—including target paths (`map` `path`, `parse_*` / `publish_*` `source` and `target`), `validation` error `message`s, and `publish_xml` `root_element`.

Because any plain JSON literal is valid JSONLogic that evaluates to itself, static strings like `"path": "data.total"` are folded to constants at build time with zero runtime overhead. Dynamic expressions are evaluated per message only when they reference message fields.

> **Escaping literal single-key objects:** In templating mode, a single-key JSON object whose key matches an enabled JSONLogic operator is evaluated as that operator. Prefix the key with `$` (for example, `{"$cat": ...}`) when you want to write a literal object. See [Literal keys and the `$` escape](../advanced/jsonlogic.md#literal-keys-and-the--escape).

> **Secrets protection:** Any parameter whose output is written to the message, audit trail, or logs (such as a `map` expression/path, `validation` message, or `log` field) cannot reference `{"secret": "..."}`—compilation rejects it with `SECRET_IN_MESSAGE_WRITE`. See [Secrets](../advanced/secrets.md).

## Return Outcomes and Policies

Each built-in function returns an outcome that is handled by the task's `on_rejection` and `on_failure` policies:

| Function | Outcome |
|----------|---------|
| `map` | Returns `Ok` (`200`), or status `500` if any individual mapping fails (recording `MAPPING_ERROR` for each failed mapping while still executing remaining mappings). |
| `validation` | Returns `Ok` (`200`) if all rules pass, or status `400` (a rejection) if any rule fails, recording one `VALIDATION_ERROR` (or `EVALUATION_ERROR`) per failing rule. |
| `filter` | Returns `Ok` (`200`) if `condition` is `true`; otherwise returns `Halt` (status `299`) or `Skip` (no audit entry) depending on `on_reject` (records `EVALUATION_ERROR` if the condition fails to evaluate). |
| `log` | Always returns `Ok` (`200`). |
| `parse_json` | Returns `Ok` (`200`). Fails only if a dynamic `source` or `target` expression fails to evaluate or resolves to an invalid root. |
| `parse_xml` | Returns `Ok` (`200`), or fails with `VALIDATION_ERROR` (`500`) if the source is not a valid XML string. |
| `publish_json`, `publish_xml` | Returns `Ok` (`200`), or fails with `VALIDATION_ERROR` (`500`) if the source path is missing or `null`. |

- **Rejections (`400`)** are governed by the task's `on_rejection` policy (`"continue"` by default, or `"halt"` to stop the workflow).
- **Failures (`500` or `Err`)** record an error on `message.errors()` and are governed by the task's `on_failure` policy (`"fail"` by default, which escalates to the workflow's `on_failure` policy).

See [Tasks](../core-concepts/task.md) and [Error Handling](../core-concepts/error-handling.md) for full details.

## Example Pipeline: Parse, Transform, Validate, Publish

```json
{
    "id": "intake",
    "tasks": [
        {
            "id": "parse_input",
            "function": {
                "name": "parse_json",
                "input": {"source": "payload", "target": "input"}
            }
        },
        {
            "id": "transform",
            "function": {
                "name": "map",
                "input": {
                    "mappings": [
                        {
                            "path": "data.user.full_name",
                            "logic": {"cat": [{"var": "data.input.first_name"}, " ", {"var": "data.input.last_name"}]}
                        }
                    ]
                }
            }
        },
        {
            "id": "check",
            "on_rejection": "halt",
            "function": {
                "name": "validation",
                "input": {
                    "rules": [
                        {
                            "logic": {"and": [{"!!": {"var": "data.input.first_name"}}, {"!!": {"var": "data.input.last_name"}}]},
                            "message": "Full name required"
                        }
                    ]
                }
            }
        },
        {
            "id": "publish",
            "function": {
                "name": "publish_json",
                "input": {"source": "user", "target": "response", "pretty": true}
            }
        }
    ]
}
```

- **Valid payload (`{"first_name": "Ada", "last_name": "Lovelace"}`):** Every task succeeds with status `200`, and `data` finishes as:

  ```json
  {
      "input": {"first_name": "Ada", "last_name": "Lovelace"},
      "user": {"full_name": "Ada Lovelace"},
      "response": "{\n  \"full_name\": \"Ada Lovelace\"\n}"
  }
  ```

- **Invalid payload (`{"last_name": "Lovelace"}`):** `transform` writes `" Lovelace"`, `check` records a `VALIDATION_ERROR` (`"Full name required"`) with status `400`, and `"on_rejection": "halt"` stops the workflow before `publish` runs.

## Conditional Tasks

Any built-in task can include a `condition` to control whether it executes:

```json
{
    "id": "premium_discount",
    "condition": {"==": [{"var": "data.tier"}, "premium"]},
    "function": {
        "name": "map",
        "input": {
            "mappings": [
                {"path": "data.discount", "logic": 20}
            ]
        }
    }
}
```

## Inspecting the Function Catalogue in Rust

`Registry::catalogue()` lists all eight built-in functions (including their categories and config field schemas) followed by any registered custom host functions:

```rust
use dataflow_rs::Registry;
use dataflow_rs::def::BUILTIN_FUNCTIONS;

let registry = Registry::default();
let builtins: Vec<(&str, &str)> = registry
    .catalogue()
    .filter(|f| f.builtin)
    .map(|f| (f.name, f.spec.category.as_ref()))
    .collect();
assert_eq!(
    builtins,
    [
        ("map", "transform"),
        ("validation", "validate"),
        ("filter", "control"),
        ("log", "observe"),
        ("parse_json", "parse"),
        ("parse_xml", "parse"),
        ("publish_json", "publish"),
        ("publish_xml", "publish"),
    ]
);
let names: Vec<&str> = registry.catalogue().map(|f| f.name).collect();
assert_eq!(names, BUILTIN_FUNCTIONS);

// Inspect config field specifications (useful for UI forms or linters).
let map = registry.catalogue().find(|f| f.name == "map").unwrap();
let fields: Vec<(&str, bool)> = map.fields.iter().map(|f| (&*f.name, f.required)).collect();
assert_eq!(fields, [("mappings", true)]);
```

## Learn More

- [Parse](./parse.md) — `parse_json` and `parse_xml`
- [Map](./map.md) — Transforming fields, removing keys, and appending to arrays
- [Validation](./validation.md) — Rule-based data validation and error records
- [Filter](./filter.md) — Halting workflows or skipping tasks
- [Log](./log.md) — Structured logging with JSONLogic fields
- [Publish](./publish.md) — `publish_json` and `publish_xml`
- [Host Functions](./host-functions.md) — Extending the engine with custom async functions
