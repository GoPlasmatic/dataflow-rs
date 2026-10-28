# JSONLogic

Dataflow-rs uses [JSONLogic](https://jsonlogic.com/) for workflow and task
conditions, `map` transformations, and function input parameters.

## Overview

JSONLogic lets you write expressions and rules as standard JSON. Dataflow-rs
uses JSONLogic for:

- **Workflow conditions**, which decide whether a message enters a workflow,
  evaluated against the full message context (`data`, `metadata`, `temp_data`)
- **Task and group conditions**, which decide whether a step executes
- **`map` mappings**, which compute and transform values
- **Function input parameters**, for both built-in and custom host functions:
  any plain JSON literal evaluates to itself (so `"path": "data.out"` and
  `"timeout_ms": 5000` work as literals), while an object with a recognized
  operator key is evaluated dynamically

Every expression is compiled once when the workflow is built using the
`ExprEngine` owned by the engine's `Registry`. Any expression that does not read
from the message context or secrets is constant-folded at compile time and never
re-evaluated at runtime.

## Basic Syntax

A JSONLogic operation is a single-key JSON object where the key is the operator
name and the value is its argument (or array of arguments):

```jsonc
{"operator": [argument1, argument2, ...]}
```

## Accessing Data

### `var` — Read Context Fields

```jsonc
// Read a top-level field
{"var": "data.name"}

// Read a nested field
{"var": "data.user.profile.email"}

// Read an array element by index
{"var": "data.items.0"}

// Provide a default value if the path is missing or null
{"var": ["data.optional", "default value"]}
```

### Context Structure

Expressions evaluate against a single context object with three root namespaces:

```jsonc
{
    "data": { ... },
    "metadata": { ... },
    "temp_data": { ... }
}
```

Reference fields by starting the path with one of the three roots:

```jsonc
{"var": "data.field"}
{"var": "metadata.type"}
{"var": "temp_data.intermediate"}
```

The message's **raw `payload` is not part of the evaluation context**. Writing
`{"var": "payload.x"}` always resolves to `null`. Parse the payload into `data`
first using `parse_json` or `parse_xml`. `Compiler::check` flags any literal
`payload` variable read as a `PAYLOAD_VAR` defect so you can catch it at
authoring time.

Secrets (such as signing keys and API tokens) are also kept out of the context
tree so they are never serialized or recorded in traces. Store secrets in the
expression engine's secret store and read them with `{"secret": "name"}`; see
[Secrets](./secrets.md).

## Comparison Operators

### Equality

```jsonc
{"==": [{"var": "data.status"}, "active"]}
{"===": [{"var": "data.count"}, 0]}      // Strict equality
{"!=": [{"var": "data.status"}, "deleted"]}
{"!==": [{"var": "data.count"}, null]}   // Strict inequality
```

### Numeric Comparisons

```jsonc
{">": [{"var": "data.age"}, 18]}
{">=": [{"var": "data.score"}, 60]}
{"<": [{"var": "data.price"}, 100]}
{"<=": [{"var": "data.quantity"}, 10]}
```

### Range Comparisons (Between)

```jsonc
{"<=": [1, {"var": "data.x"}, 10]}  // 1 <= x <= 10
{"<": [1, {"var": "data.x"}, 10]}   // 1 < x < 10
```

## Boolean Logic

### `and`, `or`, `!`

```jsonc
{"and": [
    {">=": [{"var": "data.age"}, 18]},
    {"==": [{"var": "data.verified"}, true]}
]}

{"or": [
    {"==": [{"var": "data.status"}, "active"]},
    {"==": [{"var": "data.status"}, "pending"]}
]}

{"!": {"var": "data.disabled"}}
```

### Truthiness (`!!` and `!`)

```jsonc
// Check if a value is truthy (not null, false, 0, "", [], or {})
{"!!": {"var": "data.email"}}

// Check if a value is falsy
{"!": {"var": "data.deleted"}}
```

Note that an empty object `{}` is treated as falsy (just like an empty array
`[]`). Some other JSONLogic implementations treat all objects as truthy.

## Conditionals

### Multi-Branch `if`

```json
{"if": [
    {">=": [{"var": "data.score"}, 90]}, "A",
    {">=": [{"var": "data.score"}, 80]}, "B",
    {">=": [{"var": "data.score"}, 70]}, "C",
    "F"
]}
```

### Ternary `if`

```json
{"if": [
    {"var": "data.premium"},
    "VIP Customer",
    "Standard Customer"
]}
```

## String Operations

### `cat` — Concatenation

```json
{"cat": ["Hello, ", {"var": "data.name"}, "!"]}
```

### `substr` — Substring

```jsonc
{"substr": [{"var": "data.text"}, 0, 10]}  // First 10 characters
{"substr": [{"var": "data.text"}, -5]}     // Last 5 characters
```

### `in` — Substring or Element Membership

```jsonc
// Check if a substring exists in a string
{"in": ["@", {"var": "data.email"}]}

// Check if a value exists in an array
{"in": [{"var": "data.status"}, ["active", "pending"]]}
```

## Numeric Operations

### Arithmetic

```jsonc
{"+": [{"var": "data.a"}, {"var": "data.b"}]}
{"-": [{"var": "data.total"}, {"var": "data.discount"}]}
{"*": [{"var": "data.price"}, {"var": "data.quantity"}]}
{"/": [{"var": "data.total"}, {"var": "data.count"}]}
{"%": [{"var": "data.n"}, 2]}  // Modulo
```

### `min` and `max`

```jsonc
{"min": [{"var": "data.a"}, {"var": "data.b"}, 100]}
{"max": [{"var": "data.x"}, 0]}  // Ensure non-negative
```

## Array Operations

### `merge` — Combine Arrays

```json
{"merge": [
    {"var": "data.list1"},
    {"var": "data.list2"}
]}
```

### `map` — Transform Elements

```jsonc
{"map": [
    {"var": "data.items"},
    {"*": [{"var": ""}, 2]}  // Double each item
]}
```

### `filter` — Keep Matching Elements

```jsonc
{"filter": [
    {"var": "data.items"},
    {">=": [{"var": ""}, 10]}  // Keep items >= 10
]}
```

### `reduce` — Fold an Array into a Single Value

```jsonc
{"reduce": [
    {"var": "data.items"},
    {"+": [{"var": "accumulator"}, {"var": "current"}]},
    0  // Initial value
]}
```

### `all`, `some`, `none`

```jsonc
{"all": [{"var": "data.items"}, {">=": [{"var": ""}, 0]}]}
{"some": [{"var": "data.items"}, {"==": [{"var": ""}, "special"]}]}
{"none": [{"var": "data.items"}, {"<": [{"var": ""}, 0]}]}
```

Inside the body of `map`, `filter`, `reduce`, `all`, `some`, and `none`, `var`
reads from the current array element rather than the top-level message context.
These are called *scoping* operators (`Operator::scoping`), and the compiler
skips `PAYLOAD_VAR` checks inside their bodies for that reason.

## Try It

> Open the [Full Debugger UI](/dataflow-rs/debugger/) for step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"jsonlogic_demo","name":"JSONLogic Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"transform","name":"Transform","function":{"name":"map","input":{"mappings":[{"path":"data.full_name","logic":{"cat":[{"var":"data.input.first_name"}," ",{"var":"data.input.last_name"}]}},{"path":"data.grade","logic":{"if":[{">=": [{"var":"data.input.score"},90]},"A",{">=": [{"var":"data.input.score"},80]},"B",{">=": [{"var":"data.input.score"},70]},"C","F"]}},{"path":"data.total","logic":{"*":[{"var":"data.input.price"},{"var":"data.input.quantity"}]}},{"path":"data.has_email","logic":{"!!":[{"var":"data.input.email"}]}}]}}}]}]' data-payload='{"first_name":"John","last_name":"Doe","score":85,"price":10,"quantity":5,"email":"john@example.com"}'>
</div>

## Common Patterns

### Safe Field Access

```jsonc
// Default to an empty string if missing
{"var": ["data.optional", ""]}

// Check if truthy before using
{"if": [
    {"!!": {"var": "data.optional"}},
    {"var": "data.optional"},
    "default"
]}
```

### Fallback Values

```json
{"if": [
    {"!!": {"var": "data.primary"}},
    {"var": "data.primary"},
    {"var": "data.fallback"}
]}
```

### Type Checking

The type-inspection operator is `type` (not `typeof`), and it requires enabling
the `ExtControl` [operator family](#operator-families). It returns `"null"`,
`"boolean"`, `"number"`, `"string"`, `"array"`, or `"object"`.

```jsonc
// Check if a field is a string
{"===": [{"type": {"var": "data.field"}}, "string"]}

// Check if a field is an array
{"===": [{"type": {"var": "data.items"}}, "array"]}
```

When the `Datetime` family is also enabled, `type` classifies ISO-8601 date and
duration strings as `"datetime"` or `"duration"` instead of `"string"`.

## Operator Families

All operators shown in the sections above belong to core JSONLogic and are
always enabled. Extended operators are organized into [`OperatorFamily`]
variants. They are **disabled by default** in `ExprEngine::default()` and can be
enabled selectively on `ExprEngineBuilder`:

| `OperatorFamily` | Cargo Feature | Operators |
|---|---|---|
| `ExtString` | *(always compiled in)* | `length`, `starts_with`, `ends_with`, `upper`, `lower`, `trim`, `split` |
| `ExtArray` | *(always compiled in)* | `sort`, `slice`, `group_by`, `distinct` |
| `ExtMath` | *(always compiled in)* | `abs`, `ceil`, `floor` |
| `ExtControl` | *(always compiled in)* | `exists`, `??`, `switch` (alias `match`), `type` |
| `ExtObject` | *(always compiled in)* | `keys`, `values`, `entries` |
| `ErrorHandling` | *(always compiled in)* | `try`, `throw` |
| `Datetime` | `datetime` | `datetime`, `timestamp`, `parse_date`, `format_date`, `date_diff`, `now` |
| *(via `all_operators()`)* | None | Enables every compiled-in family listed above |
| `Tensor` | `tensor` | `tensor`, `zeros`, `full`, `scatter`, `rle_expand`, `one_hot`, `stack`, `concat`, `unstack`, `reshape`, `transpose`, `pad`, `crop`, `cast`, `normalize`, `argmax`, `gather`, `to_list`, `shape`, `dtype` (**excluded** from `all_operators()`; see below) |

```rust
use dataflow_rs::expr::{ExprEngine, OperatorFamily};
use dataflow_rs::Registry;

let exprs = ExprEngine::builder()
    .families([OperatorFamily::ExtString, OperatorFamily::ExtControl])
    .build()
    .unwrap();
let registry = Registry::new(exprs);
# let _ = registry;
```

### Why `Tensor` Is Opt-In

`ExprEngineBuilder::all_operators` intentionally excludes `Tensor`. Several
tensor operator names overlap with common JSON field names (`shape`, `full`,
`cast`, `pad`, `crop`, `concat`, `stack`), and in templating mode any
single-key object matching an active operator name is evaluated as an operator
call rather than emitted as a literal object.

To use tensor operators, enable the `tensor` Cargo feature and add
`OperatorFamily::Tensor` explicitly:

```toml
[dependencies]
dataflow-rs = { version = "4.1", features = ["tensor"] }
```

When `Tensor` is enabled, prefix literal single-key objects with `$` (for
example, `{"$shape": ...}`) when you want a literal `"shape"` field.

### Notes on Specific Families

- `ErrorHandling` provides the JSONLogic `try` and `throw` expression operators.
  It is separate from dataflow-rs's task and workflow
  [error handling](../core-concepts/error-handling.md), which is always active.
- `length` belongs to `ExtString` (not `ExtArray`), even though it works on both
  strings and arrays.
- `type` belongs to `ExtControl`.
- `entries` belongs to `ExtObject` (because its input is an object, even though
  it returns an array of `{key, value}` pairs), while `group_by` and `distinct`
  belong to `ExtArray`. To turn an object into entries and group them with
  `group_by`, enable both `ExtObject` and `ExtArray`.

### Enabling a Family Can Change Existing Rules

Dataflow-rs evaluates JSONLogic in **templating mode**: if a single-key object's
key is not an active operator, the object is treated as literal JSON output
rather than causing an error. As a result, enabling additional operator families
can change the behavior of existing workflows.

For example, when `OperatorFamily::ExtString` is disabled, a mapping with
`{"length": {"var": "data.x"}}` writes an object with a `"length"` key. Once
`ExtString` is enabled, that same expression calls the `length` operator and
writes a number. Before enabling a new operator family, check your workflows for
single-key objects whose keys match the operators in that family.

To guarantee that a key is always treated as literal data regardless of which
families are enabled, prefix the key with `$` (for example, `{"$length": …}`).

Enabling the `datetime` Cargo feature also affects core comparison operators:
`==`, `<`, `<=`, `>`, and `>=` will attempt to parse string operands as ISO-8601
datetimes or durations before comparing them:

```json
{"==": ["2024-01-15T00:00:00Z", "2024-01-15T01:00:00+01:00"]}
```

Without `datetime`, this comparison is `false` (two different strings); with
`datetime` enabled, it evaluates to `true` because both strings represent the
same instant in time.

### Literal Keys and the `$` Escape Prefix

In templating mode, any single-key object whose key matches an active operator
is evaluated as an operator call. For example, `{"cat": ["a", "b"]}` calls the
`cat` operator and produces `"ab"`.

To output a literal object with a key named `cat`, prefix the key with `$`:

| Expression Written | Evaluated Output |
|---|---|
| `{"cat": ["a", "b"]}` | `"ab"` (evaluates the `cat` operator) |
| `{"$cat": ["a", "b"]}` | `{"cat": ["a", "b"]}` (emits a literal object) |
| `{"$$oid": "abc"}` | `{"$oid": "abc"}` |
| `{"$total": 1}` | `{"total": 1}` |

Three rules govern `$` escaping:

1. **One leading `$` is stripped from every template key**, whether or not the
   key matches an operator name (`$total` becomes `total`). If your output JSON
   needs a literal leading `$` (such as `$$set`, `$$oid`, `$$schema`, or
   `$$ref`), double the prefix. `Compiler::check` reports an
   `ESCAPED_TEMPLATE_KEY` advisory for every escaped key so you can review them.
2. **Escaping applies at every nesting depth**, including inside `map` bodies
   and `if` branches.
3. **Two keys in the same object cannot resolve to the same unescaped name.**
   The compiler rejects `{"$a": 1, "a": 2}` with `DUPLICATE_TEMPLATE_KEY`.

Although the escape character is always `$`, authoring tools can query it via
`ExprEngine::escape()`:

```rust
use dataflow_rs::expr::ExprEngine;
use dataflow_rs::Expr;
use serde_json::{Value, json};

let exprs = ExprEngine::default();
assert_eq!(exprs.escape(), '$');

let context = json!({"data": {}, "metadata": {}, "temp_data": {}});
let operator: Expr = exprs.compile(&json!({"cat": ["a", "b"]})).unwrap();
let literal: Expr = exprs.compile(&json!({"$cat": ["a", "b"]})).unwrap();
assert_eq!(Value::from(&*exprs.eval(&operator, &context).unwrap().value), json!("ab"));
assert_eq!(
    Value::from(&*exprs.eval(&literal, &context).unwrap().value),
    json!({"cat": ["a", "b"]})
);
```

Multi-key objects are always treated as output templates, so their keys never
need escaping unless a key itself begins with `$`:

```json
{"amount": {"var": "data.total"}, "currency": "EUR"}
```

Similarly, a single-key object whose key is not an active operator name (such as
`{"result": {"var": "data.x"}}`) evaluates its inner value and emits
`{"result": …}`. You only need the `$` prefix when a single-key object's key is
an operator name or when any template key starts with a literal `$`.

If you are authoring a custom host function that takes a JSON template with
literal `$` keys (like a MongoDB query), consider using an `Interpolated` field
instead: inside `Interpolated`, only `{"var": …}` nodes are evaluated and `$`
keys require no escaping. See
[Interpolated Documents](./custom-functions.md#interpolated-documents).

### Inspecting Active Operators

Because disabled operators simply pass through as literal JSON objects, you
cannot tell from a workflow's JSON alone whether an operator is active. Use
`ExprEngine::operators()` to list every operator active in a given engine
(including core operators, enabled families, custom host operators, and
`secret`):

```rust
use dataflow_rs::expr::{ExprEngine, OperatorFamily};

let exprs = ExprEngine::default();
let names: Vec<&str> = exprs.operators().iter().map(|op| &*op.name).collect();

assert!(names.contains(&"var"));      // Core operator: always present
assert!(names.contains(&"secret"));   // Built-in engine operator: always present
// `length` only appears when `OperatorFamily::ExtString` is enabled.

// Each operator entry reports its family, whether it scopes `var` to an
// array element (`map`, `filter`, `reduce`, etc.), and whether it is deterministic.
let map = exprs.operators().iter().find(|op| op.name == "map").unwrap();
assert_eq!(map.family, OperatorFamily::Core);
assert!(map.scoping && map.deterministic);
```

Authoring tools can inspect `ExprEngine::operators()` to validate operator usage
in workflows, and `CompiledWorkflow::fingerprint` records all operators and data
keys referenced by a compiled workflow.

> **Note for JavaScript/TypeScript users:** The WASM package is compiled with
> the `datetime` feature and `ExprEngineBuilder::all_operators()`, so every
> operator family in the table above (except `Tensor`) is enabled out of the box
> in the browser and [Playground](../playground.md). In Rust,
> `ExprEngine::default()` enables core operators only, so remember to enable any
> extended families you use on `ExprEngineBuilder`.

## Compiling and Evaluating Expressions Directly

The engine automatically compiles and evaluates conditions and function
configurations. If you want to evaluate standalone JSONLogic expressions in your
own application code, layers, or custom functions, use `ExprEngine` directly:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::ExprEngine;
use dataflow_rs::Expr;
use serde_json::json;

let exprs = ExprEngine::default();

// `Expr<T>` specifies the expected result type (`T` defaults to `OwnedDataValue`).
let limit: Expr<u64> = exprs.compile(&json!({"*": [{"var": "data.n"}, 2]})).unwrap();

// Pass an object containing the three context roots as either `serde_json::Value`
// or `OwnedDataValue`; both are read in place without conversion.
let context = json!({"data": {"n": 21}, "metadata": {}, "temp_data": {}});
assert_eq!(exprs.eval(&limit, &context).unwrap().value, 42);
let owned = OwnedDataValue::from(&context);
assert_eq!(exprs.eval(&limit, &owned).unwrap().value, 42);

// Expressions that do not read context or secrets fold to a constant at compile time.
let fixed: Expr<u64> = exprs.compile(&json!({"+": [40, 2]})).unwrap();
assert_eq!(fixed.constant(), Some(&42));

// A constant with an incompatible type fails at compile time with a `Config` error,
// whereas a runtime type mismatch returns an `Eval` error.
assert!(exprs.compile::<u64>(&json!("forty-two")).is_err());
```

Inside a custom host function, call `ctx.eval(&expr)` to evaluate against the
current call's context view, and use `ctx.exprs()` to access the program's
`ExprEngine` if you need to compile expressions dynamically.

### Custom Operators

You can register custom JSONLogic operators on `ExprEngineBuilder` before
creating your `Registry`. Use `.operator(name, op)` to register an operator that
is neither scoping nor deterministic, or `.operator_with(Operator, op)` to
configure those properties explicitly. The builder rejects operator names that
conflict with `secret`, any enabled built-in operator, or an already-registered
custom operator.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datalogic_rs::{self, CustomOperator};
use dataflow_rs::datavalue::NumberValue;
use dataflow_rs::expr::{ExprEngine, Operator, OperatorFamily};
use dataflow_rs::{Engine, Message, Registry};
use serde_json::json;

/// `{"answer": []}`: always returns 42.
struct Answer42;

impl CustomOperator for Answer42 {
    fn evaluate<'a>(
        &self,
        _args: &[&'a datalogic_rs::DataValue<'a>],
        _ctx: &mut datalogic_rs::operator::EvalContext<'_, 'a>,
        arena: &'a datalogic_rs::bumpalo::Bump,
    ) -> datalogic_rs::Result<&'a datalogic_rs::DataValue<'a>> {
        Ok(arena.alloc(datalogic_rs::DataValue::Number(NumberValue::from_i64(42))))
    }
}

let exprs = ExprEngine::builder()
    .operator_with(Operator::custom("answer").deterministic(true), Answer42)
    .build()
    .unwrap();
let answer = exprs.operators().iter().find(|op| op.name == "answer").unwrap();
assert_eq!(answer.family, OperatorFamily::Custom);

// Reserved names like `secret` are rejected:
assert!(ExprEngine::builder().operator("secret", Answer42).build().is_err());

let engine = Engine::builder()
    .registry(Registry::new(exprs))
    .workflow_json(&json!({"id": "w", "tasks": [
        {"id": "t", "function": {"name": "map", "input": {
            "mappings": [{"path": "data.x", "logic": {"answer": []}}]}}}
    ]}))
    .build()
    .unwrap();
let mut message = Message::from_json(&json!({}));
engine.run(&mut message).await;
assert_eq!(message.to_json()["context"]["data"]["x"], json!(42));
# });
```

Each `Registry` (and the `Engine` built from it) is bound to a single
`ExprEngine`, meaning all workflows compiled by that registry share the same
operators and secret store. To change operators or rotate secrets, build a new
`ExprEngine` and `Registry`.

### Limiting Evaluation Cost (`ops_budget`)

When the `budget` Cargo feature is enabled, `ExprEngineBuilder::ops_budget(n)`
caps the number of operations a single expression evaluation can perform.
(Enabling the feature flag alone sets no limit until you call `ops_budget`.) If
an evaluation exceeds the budget, it fails with a `Budget` error (error code
`BUDGET_EXCEEDED`). You can inspect how many operations an evaluation used via
`Evaluated::ops` (which returns `Some(0)` for constant-folded expressions).

```text
let exprs = ExprEngine::builder().ops_budget(2_000).build()?;
let fold: Expr<u64> = exprs.compile(&json!({"reduce": [
    {"var": "data.xs"}, {"+": [{"var": "current"}, {"var": "accumulator"}]}, 0
]}))?;
// Evaluating over 20,000 elements exceeds the 2,000-op budget:
let err = exprs.eval(&fold, &big_context).unwrap_err();
assert_eq!(err.code(), "BUDGET_EXCEEDED");
```

The operation budget applies to every evaluation in the engine:
- If a workflow, group, or task `condition` exceeds the budget, it evaluates to
  `false` (and workflow selection records `ConditionFailed`).
- If a built-in function (`map`, `filter`, `validation`, etc.) exceeds the
  budget, the task fails with `BUDGET_EXCEEDED` and follows its `on_failure`
  policy.
- If a custom function calls `ctx.eval` and exceeds the budget, `ctx.eval`
  returns the `Budget` error to the function.

Because the budget is configured on `ExprEngine`, it is preserved across
workflow reloads via `Engine::compiler()` and `Engine::with_program()`.

## Best Practices

1. **Use `var` defaults:** Provide fallback values for optional fields using
   `{"var": ["data.field", "default"]}`.
2. **Check truthiness with `!!`:** Verify that optional fields or arrays are
   present and non-empty before operating on them.
3. **Move complex logic into custom functions:** Keep JSONLogic expressions
   focused on routing, validation, and data mapping; implement heavy business
   logic or external I/O in Rust host functions.
4. **Verify enabled operator families:** Check `ExprEngine::operators()` to
   confirm which extended families are enabled, since disabled operators pass
   through as literal objects instead of raising errors.
5. **Test in the Playground:** Use the interactive Playground or Debugger UI to
   test and step through complex expressions before deploying.
