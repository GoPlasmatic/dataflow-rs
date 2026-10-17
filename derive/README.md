# dataflow-rs-derive

The procedural macro behind [`dataflow-rs`](https://crates.io/crates/dataflow-rs):
`#[derive(FunctionConfig)]`, which turns a host function's config struct into
its compile function, its expression visitor, its field spec and its resolver.

This crate is not used on its own. `dataflow-rs` re-exports the derive as
`dataflow_rs::function::FunctionConfig`, and the generated code names
`::dataflow_rs::` paths, so depend on `dataflow-rs` and write:

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

The attributes, the field types and what the derive generates are documented
on `dataflow_rs::function::FunctionConfig`. The two crates are released
together at one version, and `dataflow-rs` pins this one exactly.

## License

Apache-2.0, as `dataflow-rs`.
