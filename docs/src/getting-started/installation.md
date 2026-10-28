# Installation

Add `dataflow-rs` to your Rust project using Cargo.

## Requirements

- Rust 1.98 or later (Edition 2024)
- Cargo (included with Rust)

## Add to `Cargo.toml`

```toml
[dependencies]
dataflow-rs = "4.1"
serde_json = "1.0"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

- `serde_json` provides the `json!` macro used to construct workflow definitions and payloads in the examples.
- `tokio` provides the async runtime for `Engine::run`. Enabling Tokio's `time` feature is required if you use `RunOptions::deadline` to enforce timeouts on async host calls.

## Operator Families and Cargo Features

By default, optional Cargo features are disabled and `ExprEngine::default()` enables only the standard JSONLogic operators. Six pure-operator extension families (`ExtString`, `ExtArray`, `ExtMath`, `ExtControl`, `ExtObject`, `ErrorHandling`) are compiled in by default and can be enabled at runtime on `ExprEngineBuilder` via `.family(...)`, `.families(...)`, or `.all_operators()`.

| Cargo Feature | Description |
|---|---|
| `datetime` |Unlocks `OperatorFamily::Datetime` (`datetime`, `timestamp`, `parse_date`, `format_date`, `date_diff`, `now`). Also changes `==` and comparison operators when comparing ISO-8601 date/duration strings. |
| `tensor` | Unlocks `OperatorFamily::Tensor` (`tensor`, `zeros`, `full`, `scatter`, `rle_expand`, `one_hot`, `stack`, `concat`, `unstack`, `reshape`, `transpose`, `pad`, `crop`, `cast`, `normalize`, `argmax`, `gather`, `to_list`, `shape`, `dtype`). Not included in `ExprEngineBuilder::all_operators()` because several operator names collide with common JSON object keys. |
| `budget` | Unlocks `ExprEngineBuilder::ops_budget`, which enforces a maximum operation count per JSONLogic evaluation. |
| `wasm-web` | Required when compiling for `wasm32-unknown-unknown`. |
| `ts` | Generates TypeScript type declarations for wire types (internal tooling; not covered by semver guarantees). |

```toml
[dependencies]
dataflow-rs = { version = "4.1", features = ["datetime", "budget"] }
```

> **Note:** Enabling an operator family changes how single-key JSON objects with matching keys are evaluated. When a family is disabled, its operator names are treated as literal object keys (for example, `{"length": ...}` produces a literal object until `OperatorFamily::ExtString` is enabled, after which `length` is evaluated as an operator). You can always prefix a key with `$` (for example, `{"$length": ...}`) to force a literal object. See [JSONLogic → Operator Families](../advanced/jsonlogic.md#operator-families) for details.

## Verify Installation

Create a minimal program to verify that everything compiles:

```rust
use dataflow_rs::Engine;

fn main() {
    // Build an engine with no workflows. `build()` fails only when a workflow
    // definition is rejected.
    let engine = Engine::builder().build().expect("no workflow to reject");
    println!(
        "Engine built with {} workflows",
        engine.program().workflows().len()
    );
}
```

Run it with:

```bash
cargo run
```

You should see:

```text
Engine built with 0 workflows
```

## Optional Dependencies

If your workflows use the built-in `log` function, add a `log` implementation such as `env_logger` to view its output:

```toml
[dependencies]
log = "0.4"
env_logger = "0.11"
```

Custom host functions use native `async fn` in the `Function` trait, so you do not need the `async-trait` crate.

## Next Steps

- [Quick Start](./quick-start.md) — Build and run your first workflow
- [Basic Concepts](./basic-concepts.md) — Learn the core architecture
