# dataflow-rs-derive

Procedural macro crate for [`dataflow-rs`](https://crates.io/crates/dataflow-rs).
`#[derive(FunctionConfig)]` generates the compile-time validation, expression
visitor, field schema, and runtime config resolver for custom host functions.

You do not need to add this crate to your `Cargo.toml` directly. `dataflow-rs`
re-exports the macro as `dataflow_rs::function::FunctionConfig`, and the
generated code references `::dataflow_rs::` paths. Depend on `dataflow-rs` and
write:

```rust
use dataflow_rs::expr::Output;
use dataflow_rs::function::FunctionConfig;
use dataflow_rs::Expr;

#[derive(FunctionConfig)]
struct FetchConfig {
    url: Expr<String>,
    #[config(default = 5000)]
    timeout_ms: Expr<u64>,
    #[output(default = "data.response")]
    output: Output,
}
```

See `dataflow_rs::function::FunctionConfig` and the
[Custom Functions guide](https://goplasmatic.github.io/dataflow-rs/advanced/custom-functions.html)
for supported attributes, field types, and generated methods. Both crates are
released together under the same version number.

## License

Apache-2.0.
