# Installation

Add dataflow-rs to your Rust project using Cargo.

## Requirements

- Rust 1.98 or later (Edition 2024)
- Cargo (comes with Rust)

## Add to Cargo.toml

```toml
[dependencies]
dataflow-rs = "3.13"
serde_json = "1.0"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

## Cargo Features

All features are off by default; the default build has core JSONLogic only.

| Feature | Unlocks |
|---|---|
| `ext-string` | `length`, `starts_with`, `ends_with`, `upper`, `lower`, `trim`, `split` |
| `ext-array` | `sort`, `slice`, `group_by`, `distinct` |
| `ext-math` | `abs`, `ceil`, `floor` |
| `ext-control` | `exists`, `??`, `switch` (alias `match`), `type` |
| `ext-object` | `keys`, `values`, `entries` |
| `error-handling` | `try`, `throw` (the JSONLogic operators; unrelated to dataflow-rs error handling, which is always on) |
| `datetime` | `datetime`, `timestamp`, `parse_date`, `format_date`, `date_diff`, `now` |
| `all-operators` | every family above |
| `tensor` | `tensor`, `zeros`, `full`, `scatter`, `rle_expand`, `one_hot`, `stack`, `concat`, `unstack`, `reshape`, `transpose`, `pad`, `crop`, `cast`, `normalize`, `argmax`, `gather`, `to_list`, `shape`, `dtype`. Deliberately **not** in `all-operators`: several names collide with ordinary JSON keys |
| `budget` | `EngineBuilder::with_ops_budget`: a per-evaluation operation ceiling with a hard abort |
| `wasm-web` | required when targeting `wasm32-unknown-unknown` |

```toml
[dependencies]
dataflow-rs = { version = "3.13", features = ["ext-string"] }
```

Read [JSONLogic → Operator Families](../advanced/jsonlogic.md#operator-families-cargo-features)
before enabling one: turning a family on can change how an existing rule
behaves.

## Verify Installation

Create a small program to verify the installation:

```rust
use dataflow_rs::Engine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create an empty rules engine via the builder.
    let engine = Engine::builder().build()?;
    println!("Rules engine created with {} rules", engine.workflows().len());
    Ok(())
}
```

Run with:

```bash
cargo run
```

You should see:

```text
Rules engine created with 0 rules
```

## Optional Dependencies

Depending on your use case, you may want to add:

```toml
[dependencies]
# For async operations
async-trait = "0.1"

# For custom error handling
thiserror = "2.0"

# For logging
log = "0.4"
env_logger = "0.11"
```

## Next Steps

- [Quick Start](./quick-start.md) - Build your first rule
- [Basic Concepts](./basic-concepts.md) - Understand the core architecture
