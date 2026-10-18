//! The integration suite: one binary, one module per topic (the table in
//! CLAUDE.md's "Testing"), so a build links the crate once rather than once
//! per topic. `secrets_isolation` and `ts_types` stay their own binaries: the
//! first installs the process's logger and asserts over everything it
//! logged, and the second's file name is written into the bindings' header.
//!
//! Run one topic with `cargo test --test it <topic>::`.

mod support;

mod acceptance;
mod checks;
mod compat;
mod compile;
mod config_fields;
mod corpus;
mod deadline;
mod derive;
mod documents;
mod error_context;
mod expr;
mod fan_out;
mod fingerprint;
mod function_testing;
mod host_calls;
mod observer;
mod operators;
#[cfg(feature = "budget")]
mod ops_budget;
mod parse;
mod path;
mod program;
mod registry;
mod repository_definitions;
mod run_basics;
mod run_json;
mod run_loops;
mod trace;
mod wire;

#[test]
fn version_is_the_package_version() {
    assert_eq!(dataflow_rs::VERSION, env!("CARGO_PKG_VERSION"));
}
