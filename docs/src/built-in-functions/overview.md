# Built-in Functions

Dataflow-rs ships eight built-in functions. They run inline inside the
engine: nothing to register, no I/O, one interpreter over the compiled
definition.

| Function | What it does | Writes the context |
|----------|--------------|--------------------|
| `parse_json` | Reads a value from the payload or the context into `data` | Yes |
| `parse_xml` | Parses an XML string from the payload or the context into `data` | Yes |
| `map` | Writes the results of JSONLogic to context paths, or removes them | Yes |
| `validation` | Checks rules against the message and records an error for each that fails | No |
| `filter` | Halts the workflow, or skips the task, when its condition is not met | No |
| `log` | Emits a log line, with structured fields, at a chosen level | No |
| `publish_json` | Serializes a value under `data` to a JSON string under `data` | Yes |
| `publish_xml` | Serializes a value under `data` to an XML string under `data` | Yes |

Any other name a task calls is a function the host registered; see
[Host Functions](./host-functions.md). The eight names are reserved:
`Registry::register` panics on one. `validate`, the 3.x spelling of
`validation`, is rewritten to `validation` when a definition loads; see
[Migrating from 3.x](../migration/from-3.md).

## How a task calls one

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

`input` is the function's config. It is compiled once, when the engine is
built, so a malformed one refuses the definition then rather than failing the
first message. Each issue names the key it is about, as in
`tasks[0].function.input.mappings[0].path`:

- a required key that is missing is `INPUT_PARSE` (``missing field `path` ``),
  and so is a value of the wrong type or an unknown spelling of an enum
  (``unknown variant `push`, expected one of `set`, `append`, `extend` ``);
- a literal path outside `data`, `metadata` and `temp_data` is
  `INVALID_PATH`;
- a key the function does not know is kept in the task's `ext` and reported
  as `UNKNOWN_KEY`, or as `LIKELY_TYPO` when it is a near miss of a known
  key. The definition still loads, so a host that screens definitions reads
  the report. 3.x dropped such keys without a word.

`for_each` is refused on a built-in (`INVALID_FOR_EACH`), since a built-in
runs inline: iterate with JSONLogic's `map`, `filter` or `reduce` instead, or
fan out a [host function](./host-functions.md#for_each-on-a-host-function).

## Every parameter is JSONLogic

Every parameter of every built-in is a JSONLogic expression, including the
ones that name a destination: a `map` `path`, a `parse_*` or `publish_*`
`source` and `target`, a `validation` `message`, a `publish_xml`
`root_element`. A JSON literal is JSONLogic for itself, so the static spelling
means what it says and costs nothing: it folds to a constant when the engine
is built, and only a parameter that reads the message does per-message work.

For an author this changes one thing: a single-key object whose key names an
operator evaluates as that operator, so a literal object is written with a
`$` on its key, `{"$cat": …}`. See
[Literal keys and the `$` escape](../advanced/jsonlogic.md#literal-keys-and-the--escape).

A parameter whose result the engine records, such as a destination or a
validation message, may not read `{"secret": …}`: the definition is refused
with `SECRET_IN_MESSAGE_WRITE`. See [Secrets](../advanced/secrets.md).

## What each one returns

A built-in tells the engine how its call went, and the task's policies decide
what that does to the workflow.

| Function | Returns |
|----------|---------|
| `map` | `Ok`. A mapping that fails is logged, the rest still run, and the call ends with status `500` |
| `validation` | `Ok`, or status `400` when any rule failed, with one error record per failing rule |
| `filter` | `Ok`, or `Halt` (status `299`) or `Skip` (no audit entry) by `on_reject` |
| `log` | `Ok`, always |
| `parse_json` | `Ok`. Only a computed `source` that names no source fails the call |
| `parse_xml` | `Ok`, or an error (`VALIDATION_ERROR`) when the source is not a string or not an XML document |
| `publish_json`, `publish_xml` | `Ok`, or an error (`VALIDATION_ERROR`) when the source is missing or `null` |

The audit entry records the status. A `400` is a rejection: the task's
`on_rejection` decides, and by default (`continue`) the next task runs. A
`500` or an error is a failure: the message gets a record (`TASK_STATUS_ERROR`
for a status, the error's own code for an error), and the task's `on_failure`
decides. By default (`fail`) the failure reaches the workflow, whose own
`on_failure` stops the run unless it says `continue`. See
[Actions (Tasks)](../core-concepts/task.md) and
[Error Handling](../core-concepts/error-handling.md).

## A complete pipeline

Parse, transform, validate, publish:

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
                        {"logic": {"!!": {"var": "data.user.full_name"}}, "message": "Full name required"}
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

With the payload `{"first_name": "Ada", "last_name": "Lovelace"}` every task
records `200` and `data.response` holds the pretty JSON of `data.user`:

```json
{
    "input": {"first_name": "Ada", "last_name": "Lovelace"},
    "user": {"full_name": "Ada Lovelace"},
    "response": "{\n  \"full_name\": \"Ada Lovelace\"\n}"
}
```

With a payload that has no `first_name`, the mapping writes an empty name,
`check` records `400` and a `VALIDATION_ERROR` with the message
`Full name required`, and `on_rejection: "halt"` stops the workflow there:
`publish` does not run, and `data.response` is never written. Without
`on_rejection`, `publish` would have run on the invalid message, since a
`400` lets the next task run by default.

## A conditional task

A task's `condition` decides whether it runs at all; a built-in is not told
about it:

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

## The catalogue

A `Registry` lists the eight built-ins first, each with its category and the
fields of its config, then the host's functions by name:

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

// Each config's fields, for a form or a linter.
let map = registry.catalogue().find(|f| f.name == "map").unwrap();
let fields: Vec<(&str, bool)> = map.fields.iter().map(|f| (&*f.name, f.required)).collect();
assert_eq!(fields, [("mappings", true)]);
```

## Learn more

- [Parse](./parse.md): `parse_json` and `parse_xml`
- [Map](./map.md): writes, removals and appends
- [Validation](./validation.md): rules and their records
- [Filter](./filter.md): halting a workflow or skipping a task
- [Log](./log.md): structured log lines
- [Publish](./publish.md): `publish_json` and `publish_xml`
- [Host Functions](./host-functions.md): everything the engine does not ship
