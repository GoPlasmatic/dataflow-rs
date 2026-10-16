# Installation

Add dataflow-rs to your Rust project using Cargo.

## Requirements

- Rust 1.98 or later (Edition 2024)
- Cargo (comes with Rust)

## Add to Cargo.toml

```toml
[dependencies]
dataflow-rs = "4.0"
serde_json = "1.0"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

`serde_json` gives you the `json!` macro the examples write definitions and
payloads with. `tokio` runs the async `Engine::run`; a run with a deadline
races each host call on a Tokio timer, which the `time` feature provides.

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
| `datetime` | `datetime`, `timestamp`, `parse_date`, `format_date`, `date_diff`, `now`; also changes `==` and the ordering operators on date-shaped strings |
| `all-operators` | every family above |
| `tensor` | `tensor`, `zeros`, `full`, `scatter`, `rle_expand`, `one_hot`, `stack`, `concat`, `unstack`, `reshape`, `transpose`, `pad`, `crop`, `cast`, `normalize`, `argmax`, `gather`, `to_list`, `shape`, `dtype`. Deliberately **not** in `all-operators`: several names collide with ordinary JSON keys |
| `budget` | `ExprEngineBuilder::ops_budget`: a per-evaluation operation ceiling with a hard abort |
| `wasm-web` | required when targeting `wasm32-unknown-unknown` |
| `ts` | derives TypeScript declarations for the wire shapes. Outside semver: the generator and its output's layout may change in any release |

```toml
[dependencies]
dataflow-rs = { version = "4.0", features = ["ext-string"] }
```

Enabling an operator family is not a no-op. The engine always runs in
templating mode, so an operator whose family is off is literal data:
`{"length": …}` echoes back as an object until `ext-string` is on, after which
it evaluates. `{"$length": …}` pins the literal reading. Read
[JSONLogic → Operator Families](../advanced/jsonlogic.md#operator-families-cargo-features)
before enabling one.

## Verify Installation

Create a small program to verify the installation:

```rust
use dataflow_rs::Engine;

fn main() {
    // An engine with no workflows. `build` fails only when a workflow is
    // rejected, so there is nothing here it can refuse.
    let engine = Engine::builder().build().expect("no workflow to reject");
    println!(
        "Engine built with {} workflows",
        engine.program().workflows().len()
    );
}
```

Run with:

```bash
cargo run
```

You should see:

```text
Engine built with 0 workflows
```

## Optional Dependencies

Depending on your use case, you may want to add:

```toml
[dependencies]
# To see the lines the `log` built-in emits
log = "0.4"
env_logger = "0.11"
```

Host functions are written with `async fn` directly on the `Function` trait,
so no `async-trait` crate is needed.

## Next Steps

- [Quick Start](./quick-start.md) - Build your first workflow
- [Basic Concepts](./basic-concepts.md) - Understand the core architecture
