# dataflow-rs-derive

Procedural macro crate for [`dataflow-rs`](https://crates.io/crates/dataflow-rs).
`#[derive(FunctionConfig)]` generates a host function config's compile
function, expression visitor, field specification, and runtime resolver.

Do not depend on this crate on its own. `dataflow-rs` re-exports the derive as
`dataflow_rs::function::FunctionConfig`, and the generated code references
`::dataflow_rs::` paths. Add `dataflow-rs` to your dependencies and write:

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

See `dataflow_rs::function::FunctionConfig` for supported attributes, field
types, and generated methods. Both crates release together at the same version.

## License

Apache-2.0.
