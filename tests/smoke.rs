#[test]
fn version_is_the_package_version() {
    assert_eq!(dataflow_rs::VERSION, env!("CARGO_PKG_VERSION"));
}
