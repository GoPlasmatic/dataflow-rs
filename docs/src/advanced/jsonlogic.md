# JSONLogic

Dataflow-rs uses [JSONLogic](https://jsonlogic.com/) for conditions, for
`map`'s transformations, and for every parameter of every function's input.

## Overview

JSONLogic is a way to write rules as JSON. Dataflow-rs uses it for:

- **Workflow conditions**, which decide whether a message enters a workflow,
  evaluated against the full context (`data`, `metadata`, `temp_data`)
- **Task and group conditions**, which decide whether a step runs
- **`map` mappings**, which transform and copy data
- **Every config parameter**, built-in or host function: a JSON literal is
  JSONLogic for itself, so `"path": "data.out"` and `"timeout_ms": 5000`
  mean what they say, and an object with an operator key is evaluated

Every expression is compiled once, when the workflow is compiled, by the
`ExprEngine` the engine's registry holds. An expression that reads nothing
folds to a constant then and is never evaluated again.

## Basic Syntax

JSONLogic operations are objects with a single key (the operator) and value (the arguments):

```json
{"operator": [argument1, argument2, ...]}
```

## Data Access

### var - Access Data

```json
// Access top-level field
{"var": "data.name"}

// Access nested field
{"var": "data.user.profile.email"}

// Access array element
{"var": "data.items.0"}

// Default value if missing
{"var": ["data.optional", "default value"]}
```

### Context Structure

The context an expression evaluates against is one object with three roots:

```json
{
    "data": { ... },
    "metadata": { ... },
    "temp_data": { ... }
}
```

Access fields with:

```json
{"var": "data.field"}
{"var": "metadata.type"}
{"var": "temp_data.intermediate"}
```

The message's **payload is not in the context**. `{"var": "payload.x"}`
resolves to nothing, silently, on every message; parse the payload into
`data` first (the `parse_json` function does). `Compiler::check` reports a
literal `payload` read as `PAYLOAD_VAR`, a defect, so the mistake is caught
at authoring time rather than found in production.

Values the engine must never record (signing keys, partner tokens) are
deliberately not in this tree either. They live in a store on the
expression engine and are read with the reserved operator
`{"secret": "name"}`; see [Secrets](./secrets.md) for where that is allowed
and what it guarantees.

## Comparison Operators

### Equality

```json
{"==": [{"var": "data.status"}, "active"]}
{"===": [{"var": "data.count"}, 0]}  // Strict equality
{"!=": [{"var": "data.status"}, "deleted"]}
{"!==": [{"var": "data.count"}, null]}  // Strict inequality
```

### Numeric Comparisons

```json
{">": [{"var": "data.age"}, 18]}
{">=": [{"var": "data.score"}, 60]}
{"<": [{"var": "data.price"}, 100]}
{"<=": [{"var": "data.quantity"}, 10]}
```

### Between

```json
{"<=": [1, {"var": "data.x"}, 10]}  // 1 <= x <= 10
{"<": [1, {"var": "data.x"}, 10]}   // 1 < x < 10
```

## Boolean Logic

### and, or, not

```json
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

### Truthy/Falsy

```json
// Check if value is truthy (not null, false, 0, "", [], {})
{"!!": {"var": "data.email"}}

// Check if value is falsy
{"!": {"var": "data.deleted"}}
```

An empty object `{}` is falsy here, as an empty array is. Some JSONLogic
implementations treat any object as truthy; this one does not.

## Conditionals

### if-then-else

```json
{"if": [
    {">=": [{"var": "data.score"}, 90]}, "A",
    {">=": [{"var": "data.score"}, 80]}, "B",
    {">=": [{"var": "data.score"}, 70]}, "C",
    "F"
]}
```

### Ternary

```json
{"if": [
    {"var": "data.premium"},
    "VIP Customer",
    "Standard Customer"
]}
```

## String Operations

### cat - Concatenation

```json
{"cat": ["Hello, ", {"var": "data.name"}, "!"]}
```

### substr - Substring

```json
{"substr": [{"var": "data.text"}, 0, 10]}  // First 10 characters
{"substr": [{"var": "data.text"}, -5]}     // Last 5 characters
```

### in - Contains

```json
// Check if substring exists
{"in": ["@", {"var": "data.email"}]}

// Check if value in array
{"in": [{"var": "data.status"}, ["active", "pending"]]}
```

## Numeric Operations

### Arithmetic

```json
{"+": [{"var": "data.a"}, {"var": "data.b"}]}
{"-": [{"var": "data.total"}, {"var": "data.discount"}]}
{"*": [{"var": "data.price"}, {"var": "data.quantity"}]}
{"/": [{"var": "data.total"}, {"var": "data.count"}]}
{"%": [{"var": "data.n"}, 2]}  // Modulo
```

### Min/Max

```json
{"min": [{"var": "data.a"}, {"var": "data.b"}, 100]}
{"max": [{"var": "data.x"}, 0]}  // Ensure non-negative
```

## Array Operations

### merge - Combine Arrays

```json
{"merge": [
    {"var": "data.list1"},
    {"var": "data.list2"}
]}
```

### map - Transform Array

```json
{"map": [
    {"var": "data.items"},
    {"*": [{"var": ""}, 2]}  // Double each item
]}
```

### filter - Filter Array

```json
{"filter": [
    {"var": "data.items"},
    {">=": [{"var": ""}, 10]}  // Items >= 10
]}
```

### reduce - Aggregate Array

```json
{"reduce": [
    {"var": "data.items"},
    {"+": [{"var": "accumulator"}, {"var": "current"}]},
    0  // Initial value
]}
```

### all/some/none

```json
{"all": [{"var": "data.items"}, {">=": [{"var": ""}, 0]}]}
{"some": [{"var": "data.items"}, {"==": [{"var": ""}, "special"]}]}
{"none": [{"var": "data.items"}, {"<": [{"var": ""}, 0]}]}
```

Inside the body of `map`, `filter`, `reduce`, `all`, `some` and `none`,
`var` reads the current element, not the context. These are the *scoping*
operators; `OperatorInfo::scoping` names them, and the `PAYLOAD_VAR` check
skips their bodies for that reason.

## Try It

> **Want more features?** Try the [Full Debugger UI](/dataflow-rs/debugger/) with step-by-step execution and workflow visualization.

<div class="playground-widget" data-workflows='[{"id":"jsonlogic_demo","name":"JSONLogic Demo","tasks":[{"id":"parse","name":"Parse Payload","function":{"name":"parse_json","input":{"source":"payload","target":"input"}}},{"id":"transform","name":"Transform","function":{"name":"map","input":{"mappings":[{"path":"data.full_name","logic":{"cat":[{"var":"data.input.first_name"}," ",{"var":"data.input.last_name"}]}},{"path":"data.grade","logic":{"if":[{">=": [{"var":"data.input.score"},90]},"A",{">=": [{"var":"data.input.score"},80]},"B",{">=": [{"var":"data.input.score"},70]},"C","F"]}},{"path":"data.total","logic":{"*":[{"var":"data.input.price"},{"var":"data.input.quantity"}]}},{"path":"data.has_email","logic":{"!!":[{"var":"data.input.email"}]}}]}}}]}]' data-payload='{"first_name":"John","last_name":"Doe","score":85,"price":10,"quantity":5,"email":"john@example.com"}'>
</div>

## Common Patterns

### Safe Field Access

```json
// Default to empty string
{"var": ["data.optional", ""]}

// Check existence first
{"if": [
    {"!!": {"var": "data.optional"}},
    {"var": "data.optional"},
    "default"
]}
```

### Null Coalescing

```json
{"if": [
    {"!!": {"var": "data.primary"}},
    {"var": "data.primary"},
    {"var": "data.fallback"}
]}
```

### Type Checking

The operator is `type`, not `typeof`, and it requires the `ext-control`
[operator family](#operator-families-cargo-features). It returns `"null"`,
`"boolean"`, `"number"`, `"string"`, `"array"` or `"object"`.

```json
// Check if string
{"===": [{"type": {"var": "data.field"}}, "string"]}

// Check if array
{"===": [{"type": {"var": "data.items"}}, "array"]}
```

With the `datetime` family also enabled, `type` classifies date-shaped strings
as `"datetime"` or `"duration"` rather than `"string"`.

## Operator Families (Cargo Features)

Everything documented above this section is core JSONLogic and is always
available. The remaining operators ship behind cargo features, all **off by
default**:

| Feature | Operators |
|---|---|
| `ext-string` | `length`, `starts_with`, `ends_with`, `upper`, `lower`, `trim`, `split` |
| `ext-array` | `sort`, `slice`, `group_by`, `distinct` |
| `ext-math` | `abs`, `ceil`, `floor` |
| `ext-control` | `exists`, `??`, `switch` (alias `match`), `type` |
| `ext-object` | `keys`, `values`, `entries` |
| `error-handling` | `try`, `throw` |
| `datetime` | `datetime`, `timestamp`, `parse_date`, `format_date`, `date_diff`, `now` |
| `all-operators` | every family above |
| `tensor` | `tensor`, `zeros`, `full`, `scatter`, `rle_expand`, `one_hot`, `stack`, `concat`, `unstack`, `reshape`, `transpose`, `pad`, `crop`, `cast`, `normalize`, `argmax`, `gather`, `to_list`, `shape`, `dtype` (**not** in `all-operators`, see below) |

```toml
[dependencies]
dataflow-rs = { version = "4.0", features = ["ext-string", "ext-control"] }
```

### `tensor` is opt-in separately

`tensor` is the one family `all-operators` leaves out. A third of its names are
ordinary JSON keys (`shape`, `full`, `cast`, `pad`, `crop`, `concat`, `stack`),
and under templating mode a single-key object whose key is a live operator is
*evaluated*, not passed through. Folding it into `all-operators` would quietly
change what `{"shape": ...}` means in workflows that already run, so you opt
in to it deliberately:

```toml
[dependencies]
dataflow-rs = { version = "4.0", features = ["tensor"] }
```

With it on, write `{"$shape": ...}` wherever you need the literal object.

`error-handling` names the JSONLogic `try`/`throw` operators. It has nothing to
do with dataflow-rs's own [error handling](../core-concepts/error-handling.md),
which is always on.

Three placements are easy to get wrong: `length` is in `ext-string`, not
`ext-array`, even though it counts array elements as well as string characters;
`type` is in `ext-control`, not a family of its own; and the split between the
last two array-flavoured families follows the *input*, not the output:
`entries` is in `ext-object` even though it produces an array (of
`{key, value}` rows), while `group_by` and `distinct` are in `ext-array` even
though they are natural companions to it. Iterating an object's entries with
`group_by` therefore needs both families enabled.

### Enabling a family can change existing rules

dataflow-rs runs JSONLogic in **templating mode**, where an unrecognised
operator name is not an error: the object passes through as literal data. So
these features are not additive.

Before `ext-string`, a mapping that produces `{"length": {"var": "data.x"}}`
stores that object verbatim. After `ext-string`, the same mapping stores a
number. Audit your rules for object keys matching any operator in the table
above before enabling its family.

The fix is to say which you meant. `{"$length": …}` is a *literal* object with a
`length` field, whatever families are enabled. See
[Literal keys and the `$` escape](#literal-keys-and-the--escape) below.

`datetime` goes further and changes **core** operators. With it on, `==`, `<`,
`<=`, `>` and `>=` first try to parse plain string operands as datetimes or
durations:

```json
{"==": ["2024-01-15T00:00:00Z", "2024-01-15T01:00:00+01:00"]}
```

This is `false` without `datetime` and `true` with it: the two strings are
different bytes naming the same instant.

### Literal keys and the `$` escape

Templating mode makes every single-key object whose key is a live operator
an operator invocation. So `{"cat": ["a", "b"]}` is the `cat` operator and
evaluates to `"ab"`, and an object with a field called `cat` needs a way to
say so.

Prefixing a key with `$` says "this is data, not a call":

| You write | You get |
|---|---|
| `{"cat": ["a", "b"]}` | `"ab"` (the operator) |
| `{"$cat": ["a", "b"]}` | `{"cat": ["a", "b"]}` (the object) |
| `{"$$oid": "abc"}` | `{"$oid": "abc"}` |
| `{"$total": 1}` | `{"total": 1}` |

Three things to know:

1. **Exactly one prefix is stripped from every key**, not only from keys that
   collide with an operator. `$total` is not an operator name and is still
   stripped. So a template that emits `$`-prefixed keys (MongoDB's
   `$set` and `$oid`, JSON Schema's `$schema` and `$ref`) must double them.
   `Compiler::check` reports `ESCAPED_TEMPLATE_KEY`, an advisory, for every
   escaped key, which is how you find them all.
2. **It applies at every depth**, anywhere a template key appears, including
   inside a `map` body or an `if` branch.
3. **Two keys may not collapse to the same name.** `{"$a": 1, "a": 2}` would
   emit `a` twice, so the workflow is refused (`DUPLICATE_TEMPLATE_KEY`).

The prefix is `$` on every build, but an authoring tool that renders or
validates the spelling should read it from the engine rather than hardcode it:

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

A *multi-key* object is always an output template, so its keys need no escape
unless one of them starts with `$`:

```json
{"amount": {"var": "data.total"}, "currency": "EUR"}
```

The same is true of a single-key object whose key names no operator:
`{"result": {"var": "data.x"}}` evaluates its argument and emits
`{"result": …}`. You need the escape only when the key *is* an operator name, or
when the key starts with `$`.

An `Interpolated` document (a config field for payloads such as a MongoDB
filter) is the other way to write literal data with live parts: there only
`var` is live, every other key is data as written, and `$` keys need no
doubling. See [Documents with holes](./custom-functions.md#documents-with-holes).

### Checking what a build evaluates

Because a disabled operator is inert rather than an error, you cannot answer
"does this expression do anything?" by reading the rule alone.
`ExprEngine::operators` reports the exact vocabulary of an engine: the core
operators, whichever families were compiled in, the host's own operators,
and `secret`:

```rust
use dataflow_rs::expr::{ExprEngine, Family};

let exprs = ExprEngine::default();
let names: Vec<&str> = exprs.operators().iter().map(|op| &*op.name).collect();

assert!(names.contains(&"var"));      // core: always present
assert!(names.contains(&"secret"));   // the engine's own, on every build
// `length` appears here only when the `ext-string` feature is enabled.

// Each entry says which family it belongs to, whether it scopes `var` to an
// element (`map`, `filter`, `reduce`, ...), and whether it is deterministic.
let map = exprs.operators().iter().find(|op| op.name == "map").unwrap();
assert_eq!(map.family, Family::Core);
assert!(map.scoping && map.deterministic);
```

An authoring tool should run this check before telling someone their
expression is fine. A compiled workflow also carries a `fingerprint` of the
operators and data keys its expressions use (`CompiledWorkflow::fingerprint`),
which a host can keep beside the definition.

> **Note for JavaScript users:** the WASM package is built with
> `all-operators`, so every family in the table is live in the browser and in
> the [Playground](../playground.md). A default `cargo add dataflow-rs` build
> has none of them, so an expression using `length` or `switch` can work in the
> playground and be silently inert in your Rust service.

## Compiling and evaluating expressions yourself

The engine does this for every condition and config parameter. A host that
evaluates rules of its own, in a layer or a function, uses the same
`ExprEngine`:

```rust
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::ExprEngine;
use dataflow_rs::Expr;
use serde_json::json;

let exprs = ExprEngine::default();

// `Expr<T>` says what the result must be; `T` defaults to `OwnedDataValue`.
let limit: Expr<u64> = exprs.compile(&json!({"*": [{"var": "data.n"}, 2]})).unwrap();

// The root is an object holding the three context roots, as JSON or as an
// `OwnedDataValue`; either is viewed in place, nothing is converted first.
let context = json!({"data": {"n": 21}, "metadata": {}, "temp_data": {}});
assert_eq!(exprs.eval(&limit, &context).unwrap().value, 42);
let owned = OwnedDataValue::from(&context);
assert_eq!(exprs.eval(&limit, &owned).unwrap().value, 42);

// An expression that reads nothing folds at compile and is never evaluated.
let fixed: Expr<u64> = exprs.compile(&json!({"+": [40, 2]})).unwrap();
assert_eq!(fixed.constant(), Some(&42));

// A folded constant of the wrong type is refused at compile, with a
// `Config` error; a wrong type at evaluation is an `Eval` error.
assert!(exprs.compile::<u64>(&json!("forty-two")).is_err());
```

Inside a function, `Ctx::eval` does the same against the context as the
call sees it, and `Ctx::exprs` is the engine the program was compiled with,
for an expression the function compiles itself.

### Custom operators

A host registers its own operators on the `ExprEngine`, then builds the
registry over that engine. `operator(name, op)` registers one as neither
scoping nor deterministic; `operator_with(OperatorInfo, op)` says which it is.
The engine refuses a name that is `secret`, that is a built-in operator this
build enables, or that is registered twice.

```rust
# tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
use dataflow_rs::datalogic_rs::{self, CustomOperator};
use dataflow_rs::datavalue::NumberValue;
use dataflow_rs::expr::{ExprEngine, Family, OperatorInfo};
use dataflow_rs::{Engine, Message, Registry};
use serde_json::json;

/// `{"answer": []}`: always 42.
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
    .operator_with(OperatorInfo::custom("answer").deterministic(true), Answer42)
    .build()
    .unwrap();
let answer = exprs.operators().iter().find(|op| op.name == "answer").unwrap();
assert_eq!(answer.family, Family::Custom);

// A name the engine refuses: `secret` is reserved.
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

A registry, and so an engine, is bound to one `ExprEngine`: every workflow
it compiles shares the same operators and the same secrets. Changing either
means building a new expression engine and a new registry.

### Bounding an evaluation

With the `budget` feature, `ExprEngineBuilder::ops_budget(n)` sets a ceiling
on the operations one evaluation may spend. Merely enabling the feature
changes nothing; only `ops_budget` installs a ceiling. Exceeding it fails the
evaluation with an error of kind `Budget`, recorded as `BUDGET_EXCEEDED`, and
`Evaluated::ops` reports what an evaluation spent (`Some(0)` for a folded
constant).

```text
let exprs = ExprEngine::builder().ops_budget(2_000).build()?;
let fold: Expr<u64> = exprs.compile(&json!({"reduce": [
    {"var": "data.xs"}, {"+": [{"var": "current"}, {"var": "accumulator"}]}, 0
]}))?;
// Over 20,000 elements this crosses the ceiling:
let err = exprs.eval(&fold, &big_context).unwrap_err();
assert_eq!(err.code(), "BUDGET_EXCEEDED");
```

The ceiling binds every evaluation in the engine, but what happens next
depends on who evaluated: a condition that fails to evaluate counts as false
and a `map` mapping over the ceiling is logged and skipped, failing its task
with a `500`, while a host function's `Ctx::eval` returns the `Budget` error
to the function. The ceiling lives on the `ExprEngine`, so it survives a
reload through `Engine::compiler` and `with_program`.

## Best Practices

1. **Use var Defaults** - Provide defaults for optional fields
2. **Check Existence** - Use `!!` to verify field exists before use
3. **Keep It Simple** - Complex logic may be better in a host function
4. **Check the vocabulary** - Read `ExprEngine::operators` before relying on
   an operator from a family; a disabled one is inert, not an error
5. **Test Expressions** - Use the playground to test JSONLogic before deploying
