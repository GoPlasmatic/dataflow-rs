//! Compatibility with v3 definitions and behaviour.
//!
//! Every v3 definition loads through the compat reader, which
//! [`parse`](crate::def::parse) runs first when
//! [`ParseOptions::compat`](crate::def::ParseOptions::compat) is on (the
//! default). It rewrites each v3 spelling into v4's and reports the rewrite
//! as a `V3_SPELLING` Advisory at the v3 key:
//!
//! | Where | v3 spelling | v4 spelling |
//! | --- | --- | --- |
//! | Workflow | `continue_on_error: true` / `false` | `on_failure: continue` / `fail` |
//! | Task | `continue_on_error: true` | `on_failure: continue`, `on_rejection: continue` |
//! | Task | `halt_on: "failure"` | `on_failure: fail`, `on_rejection: halt` |
//! | Task | both | `on_failure: halt`, `on_rejection: halt` |
//! | Task | `halt_on: "never"`, `continue_on_error: false` | the defaults, still reported |
//! | `function.name` | `validate` | `validation` |
//!
//! It moves two kinds of key into a node's `ext`, reporting each as the key
//! it is:
//!
//! - a key a built-in's input does not know, which v3's configs ignored, as
//!   `function.input.<key>`, and an unknown key of a `map` mapping or a
//!   `validation` rule, as `function.input.mappings[0].<key>` (an
//!   `UNKNOWN_KEY`, or a `LIKELY_TYPO`);
//! - a group's `continue_on_error` (a `GROUP_CONTINUE_ON_ERROR` when `true`,
//!   which v3 reported and never honoured, otherwise an `UNKNOWN_KEY`).
//!
//! It refuses a `continue_on_error` that is not a bool and a `halt_on` that
//! is not `"never"` or `"failure"`. It refuses a v3 key written beside a v4
//! one, at the v3 key: on one node, any v3 policy key (`continue_on_error`,
//! `halt_on`) beside any v4 policy key (`on_failure`, `on_rejection`).
//!
//! With the reader off, nothing is rewritten. The v3 keys of a workflow, a
//! task and a group (`continue_on_error`, `halt_on`) are unknown keys, and
//! Rejected as every unknown key then is. `validate` and a built-in
//! input's stray keys are not keys of the definition: they stay
//! in the task's `function` as written, for the function's lookup and config
//! to refuse when the workflow is compiled (phase 3).
//!
//! [`migrate_v3`] parses with the reader on and writes the definition in the
//! v4 spelling, which parses with the reader off to an equal definition.
//!
//! ```
//! use dataflow_rs::compat::migrate_v3;
//! use serde_json::json;
//!
//! let migrated = migrate_v3(&json!({
//!     "id": "w",
//!     "tasks": [{"id": "t", "continue_on_error": true, "function": {"name": "f", "input": {}}}]
//! }))
//! .unwrap();
//! assert_eq!(
//!     migrated.json,
//!     json!({"id": "w", "tasks": [{
//!         "id": "t", "on_failure": "continue", "function": {"name": "f", "input": {}}
//!     }]})
//! );
//! assert_eq!(migrated.rewrites[0].path.to_string(), "tasks[0].continue_on_error");
//! assert_eq!(migrated.rewrites[0].v3, "continue_on_error: true");
//! assert_eq!(migrated.rewrites[0].v4, "on_failure: continue, on_rejection: continue");
//! ```

pub use crate::def::reader::Rewrite;

use crate::Issue;
use crate::base::any_rejected;
use crate::def::{ParseOptions, parse_with_rewrites};
use serde_json::Value;

/// A v3 definition in the v4 spelling: what [`migrate_v3`] returns.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Migrated {
    /// The definition in the v4 spelling.
    pub json: Value,
    /// Each v3 spelling rewritten, in document order.
    pub rewrites: Vec<Rewrite>,
    /// Every issue the definition has, none Rejected: the rewrites' own
    /// `V3_SPELLING`s among them.
    pub issues: Vec<Issue>,
}

/// Parse with compat on and serialize the definition in the v4 spelling. Err
/// holds every issue when any is Rejected; otherwise `issues` holds the rest.
pub fn migrate_v3(json: &Value) -> std::result::Result<Migrated, Vec<Issue>> {
    let (parsed, rewrites) = parse_with_rewrites(json, ParseOptions::default().compat(true));
    let Some(def) = parsed.def else {
        debug_assert!(any_rejected(&parsed.issues));
        return Err(parsed.issues);
    };
    let json =
        serde_json::to_value(&def).expect("a definition serializes to JSON: every key is a string");
    Ok(Migrated {
        json,
        rewrites,
        issues: parsed.issues,
    })
}
