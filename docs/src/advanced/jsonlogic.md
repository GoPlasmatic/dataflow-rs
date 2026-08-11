# JSONLogic

Dataflow-rs uses [JSONLogic](https://jsonlogic.com/) for conditions and data transformations.

## Overview

JSONLogic is a way to write rules as JSON. It's used in dataflow-rs for:

- **Rule Conditions** - Control when rules (workflows) execute, evaluated against the full context (`data`, `metadata`, `temp_data`)
- **Action Conditions** - Control when actions (tasks) execute
- **Map Function** - Transform and copy data

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

In dataflow-rs, the context available to JSONLogic is:

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
// Check if value is truthy (not null, false, 0, "")
{"!!": {"var": "data.email"}}

// Check if value is falsy
{"!": {"var": "data.deleted"}}
```

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
| `ext-array` | `sort`, `slice` |
| `ext-math` | `abs`, `ceil`, `floor` |
| `ext-control` | `exists`, `??`, `switch` (alias `match`), `type` |
| `error-handling` | `try`, `throw` |
| `datetime` | `datetime`, `timestamp`, `parse_date`, `format_date`, `date_diff`, `now` |
| `all-operators` | every family above |

```toml
[dependencies]
dataflow-rs = { version = "3.3", features = ["ext-string", "ext-control"] }
```

`error-handling` names the JSONLogic `try`/`throw` operators. It has nothing to
do with dataflow-rs's own [error handling](../core-concepts/error-handling.md),
which is always on.

Two placements are easy to get wrong: `length` is in `ext-string`, not
`ext-array`, even though it counts array elements as well as string characters;
and `type` is in `ext-control`, not a family of its own.

### Enabling a family can change existing rules

dataflow-rs runs JSONLogic in **templating mode**, where an unrecognised
operator name is not an error — the object passes through as literal data. That
is what makes these features non-additive.

Before `ext-string`, a mapping that produces `{"length": {"var": "data.x"}}`
stores that object verbatim. After `ext-string`, the same mapping stores a
number. Audit your rules for object keys matching any operator in the table
above before enabling its family.

`datetime` goes further and changes **core** operators. With it on, `==`, `<`,
`<=`, `>` and `>=` first try to parse plain string operands as datetimes or
durations:

```json
{"==": ["2024-01-15T00:00:00Z", "2024-01-15T01:00:00+01:00"]}
```

This is `false` without `datetime` and `true` with it — the two strings are
different bytes naming the same instant.

## Best Practices

1. **Use var Defaults** - Provide defaults for optional fields
2. **Check Existence** - Use `!!` to verify field exists before use
3. **Keep It Simple** - Complex logic may be better in custom functions
4. **Test Expressions** - Use the playground to test JSONLogic before deploying
