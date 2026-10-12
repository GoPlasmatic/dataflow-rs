extern crate dataflow_rs_v4 as dataflow_rs;

#[test]
fn the_alias_resolves() {
    assert_eq!(dataflow_rs::VERSION, env!("CARGO_PKG_VERSION"));
}
