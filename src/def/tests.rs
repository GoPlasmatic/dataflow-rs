//! The definition keys, held together: each level's known-key table, what
//! `parse` reads, what `Serialize` writes back, and the fields of the types.
//! A key added to one and not the others fails here: a table entry parse
//! never reads would be dropped silently, a key parse reads that the table
//! lacks would be read and reported `UNKNOWN_KEY` at once, and a key `ser`
//! skips would be lost by `migrate_v3`.

use super::keys::{self, Level};
use super::{
    FailurePolicy, ForEachDef, FunctionRef, GroupDef, LoopDef, NodeHeader, ParseOptions,
    RejectionPolicy, Status, StepDef, TaskDef, WorkflowDef, WorkflowFailurePolicy, defaults, parse,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// A definition setting every key at every level, each to a value that is
/// not its default, in the order `Serialize` writes them.
fn full() -> Value {
    json!({
        "id": "w",
        "name": "W",
        "description": "d",
        "priority": 7,
        "condition": {"==": [1, 1]},
        "tasks": [
            {"id": "t", "name": "T", "description": "td", "condition": {"var": "data.go"},
             "function": {"name": "f", "input": [1, "x"]},
             "on_failure": "halt", "on_rejection": "halt", "terminal": true,
             "for_each": {"over": [1, 2], "as": "x", "max_concurrency": 4,
                          "collect": "temp_data.r", "into": "data.rs"},
             "ext": {"ui": {"x": 1}}},
            {"id": "g", "name": "G", "description": "gd", "condition": false,
             "terminal": true, "tasks": [{"id": "m", "function": {"name": "f", "input": {}}}],
             "ext": {"continue_on_error": true}}
        ],
        "on_failure": "continue",
        "channel": "orders",
        "status": "paused",
        "version": 3,
        "rollout": {"bucket_start": 10, "bucket_end": 60},
        "loop": {"counter": "i", "init": 2, "increment": 3, "max": 9,
                 "setup": [{"id": "s", "function": {"name": "f", "input": {}}}],
                 "over": {"var": "data.xs"}, "as": "it", "scratch": "sc"},
        "tags": ["a", "b"],
        "created_at": "2026-01-02T03:04:05.123456789+00:00",
        "updated_at": "2026-02-03T04:05:06+00:00",
        "ext": {"path": "ui/folder"}
    })
}

/// Each level: its table, and where its object sits in [`full`], as a JSON
/// pointer.
const LEVELS: [(&str, Level, &str); 7] = [
    ("workflow", keys::WORKFLOW, ""),
    ("task", keys::TASK, "/tasks/0"),
    ("group", keys::GROUP, "/tasks/1"),
    ("loop", keys::LOOP, "/loop"),
    ("for_each", keys::FOR_EACH, "/tasks/0/for_each"),
    ("rollout", keys::ROLLOUT, "/rollout"),
    ("function", keys::FUNCTION, "/tasks/0/function"),
];

fn strict(json: &Value) -> super::Parsed {
    parse(json, ParseOptions::default().compat(false))
}

/// The public `definition_keys()` is the very tables the walk reads: a tool
/// holding its own view to the engine cannot drift from a key added here
/// without this failing.
#[test]
fn the_definition_keys_api_is_the_level_tables() {
    let keys = keys::definition_keys();
    let expected = [
        ("workflow", keys::WORKFLOW, keys.workflow),
        ("task", keys::TASK, keys.task),
        ("group", keys::GROUP, keys.group),
        ("loop", keys::LOOP, keys.r#loop),
        ("for_each", keys::FOR_EACH, keys.for_each),
        ("rollout", keys::ROLLOUT, keys.rollout),
        ("function", keys::FUNCTION, keys.function),
    ];
    for (level, table, api) in expected {
        assert_eq!(api, table.known(), "{level}");
    }
}

#[test]
fn every_level_of_the_full_definition_sets_exactly_its_tables_keys() {
    let full = full();
    for (level, table, at) in LEVELS {
        let object = full.pointer(at).and_then(Value::as_object).expect(level);
        let set: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        let known: BTreeSet<&str> = table.known().iter().copied().collect();
        assert_eq!(set, known, "{level}");
    }
}

#[test]
fn the_full_definition_parses_clean_and_writes_back_as_its_text() {
    let full = full();
    let parsed = strict(&full);
    assert_eq!(parsed.issues, [], "every key is known and valid");
    let def = parsed.def.expect("the definition parses");
    // Text, so key order counts: `Serialize` writes each table's order.
    assert_eq!(
        serde_json::to_string(&def).unwrap(),
        serde_json::to_string(&full).unwrap()
    );
}

#[test]
fn every_field_of_every_type_is_read_from_the_full_definition() {
    // An exhaustive destructure: a field added to a type fails to compile
    // here until the full definition sets it, so a field no key fills cannot
    // be added unnoticed. Each is asserted away from its default.
    let def = strict(&full()).def.expect("the definition parses");
    let WorkflowDef {
        id,
        header,
        priority,
        condition,
        tasks,
        on_failure,
        channel,
        status,
        version,
        rollout,
        r#loop,
        tags,
        created_at,
        updated_at,
    } = &def;
    assert!(!id.is_empty());
    header_set(header);
    assert_ne!(*priority, defaults::PRIORITY);
    assert!(condition.is_some());
    assert_ne!(*on_failure, WorkflowFailurePolicy::default());
    assert_ne!(channel, defaults::CHANNEL);
    assert_ne!(*status, Status::default());
    assert_ne!(*version, defaults::VERSION);
    assert!(rollout.is_some());
    assert!(!tags.is_empty());
    assert!(created_at.is_some() && updated_at.is_some());

    let [StepDef::Task(task), StepDef::Group(group)] = tasks.as_slice() else {
        panic!("a task, then a group: {tasks:?}");
    };
    let TaskDef {
        id,
        header,
        condition,
        function,
        on_failure,
        on_rejection,
        terminal,
        for_each,
    } = task;
    assert!(!id.is_empty());
    header_set(header);
    assert!(condition.is_some());
    let FunctionRef { name, input } = function;
    assert!(!name.is_empty() && *input != json!({}));
    assert_ne!(*on_failure, FailurePolicy::default());
    assert_ne!(*on_rejection, RejectionPolicy::default());
    assert!(*terminal);
    let Some(ForEachDef {
        over: _,
        r#as,
        max_concurrency,
        collect,
        into,
    }) = for_each
    else {
        panic!("the task fans out");
    };
    assert!(!r#as.is_empty());
    assert_ne!(*max_concurrency, defaults::MAX_CONCURRENCY);
    assert!(collect.is_some() && into.is_some());

    let GroupDef {
        id,
        header,
        condition,
        terminal,
        tasks,
    } = group;
    assert!(!id.is_empty());
    header_set(header);
    assert!(condition.is_some() && *terminal && !tasks.is_empty());

    let Some(LoopDef {
        counter,
        init,
        increment,
        max: _,
        setup,
        over,
        r#as,
        scratch,
    }) = r#loop
    else {
        panic!("the workflow loops");
    };
    assert!(counter.is_some() && over.is_some() && r#as.is_some() && scratch.is_some());
    assert_ne!(*init, defaults::INIT);
    assert_ne!(*increment, defaults::INCREMENT);
    assert!(!setup.is_empty());
}

/// Every field of a header is set.
fn header_set(header: &NodeHeader) {
    let NodeHeader {
        name,
        description,
        ext,
    } = header;
    assert!(name.is_some() && description.is_some() && !ext.is_empty());
}

#[test]
fn removing_any_key_changes_the_definition_or_refuses_it() {
    // No key is read and then ignored: without it, the definition is
    // another one, or none.
    let full = full();
    let whole = strict(&full).def.expect("the definition parses");
    for (level, table, at) in LEVELS {
        for &key in table.known() {
            let mut json = full.clone();
            let object = json
                .pointer_mut(at)
                .and_then(Value::as_object_mut)
                .expect(level);
            object.shift_remove(key);
            if let Some(def) = strict(&json).def {
                assert_ne!(def, whole, "{level}.{key} is read and then ignored");
            }
        }
    }
}
