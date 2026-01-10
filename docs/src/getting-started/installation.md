# Installation

Add dataflow-rs to your Rust project using Cargo.

## Requirements

- Rust 1.70 or later
- Cargo (comes with Rust)

## Add to Cargo.toml

```toml
[dependencies]
dataflow-rs = "2.0"
serde_json = "1.0"
tokio = { version = "1.0", features = ["rt-multi-thread", "macros"] }
```

## Verify Installation

Create a simple test to verify the installation:

```rust
use dataflow_rs::Engine;

fn main() {
    // Create an empty engine
    let engine = Engine::new(vec![], None);
    println!("Engine created with {} workflows", engine.workflows().len());
}
```

Run with:

```bash
cargo run
```

You should see:

```
Engine created with 0 workflows
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

- [Quick Start](./quick-start.md) - Build your first workflow
- [Basic Concepts](./basic-concepts.md) - Understand the core architecture
