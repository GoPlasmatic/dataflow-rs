//! The grid of `map` mappings both differentials walk: `tests/map_vs_v3.rs`
//! holds v4's compile to v3's over it, and the runtime differential in
//! `src/function/builtins/map.rs` holds v4's call to v3's run.

use serde_json::{Map, Value, json};

/// Every combination of `logic` (absent, present, `null`), `unset` (absent,
/// `true`, `false`), `on_null` (absent, `skip`, `unset`), `mode` (absent, `set`,
/// `append`, `extend`) and `path` (`data`, `data.x`).
pub fn mappings() -> Vec<Value> {
    let logics = [None, Some(json!({"var": "data.y"})), Some(Value::Null)];
    let unsets = [None, Some(true), Some(false)];
    let on_nulls = [None, Some("skip"), Some("unset")];
    let modes = [None, Some("set"), Some("append"), Some("extend")];
    let paths = ["data", "data.x"];
    let mut all = Vec::new();
    for logic in &logics {
        for unset in unsets {
            for on_null in on_nulls {
                for mode in modes {
                    for path in paths {
                        let mut m = Map::new();
                        m.insert("path".into(), json!(path));
                        if let Some(logic) = logic {
                            m.insert("logic".into(), logic.clone());
                        }
                        if let Some(unset) = unset {
                            m.insert("unset".into(), json!(unset));
                        }
                        if let Some(on_null) = on_null {
                            m.insert("on_null".into(), json!(on_null));
                        }
                        if let Some(mode) = mode {
                            m.insert("mode".into(), json!(mode));
                        }
                        all.push(Value::Object(m));
                    }
                }
            }
        }
    }
    assert_eq!(all.len(), 216);
    all
}
