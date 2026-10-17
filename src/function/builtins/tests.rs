//! The built-in configs: the dispatch, the defaults, the two path types, what
//! each config records, and the rows that hold what each one accepts and
//! refuses, and where.

use super::filter::RejectAction;
use super::log::LogLevel;
use super::map::Mapping;
use super::paths::{DataPath, ParseFrom, ParseSource, PublishSource};
use super::validation::Rule;
use super::*;
use crate::base::{AuthoredPath, Issue, IssueCode, Severity};
use crate::def::keys::{PUBLISH_KEYS, builtin_input};
use crate::def::{BUILTIN_FUNCTIONS, ParseOptions, StepDef, parse};
use crate::expr::{CompileCx, ConfigField, ExprEngine, ExprVisitor, FieldSpec, Path};
use crate::function::{FunctionConfig, RetrySafety};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Compiles `input` as `C`, rooted at the config.
fn compile<C: FunctionConfig>(input: &Value) -> (Option<C>, Vec<Issue>) {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let config = {
        let mut cx = CompileCx::new(&engine, &mut issues);
        C::compile(input, &mut cx)
    };
    (config, issues)
}

/// Compiles `raw` as one field `T`, at the key `f`.
fn field<T: ConfigField>(raw: &Value) -> (Option<T>, Vec<Issue>) {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let value = {
        let mut cx = CompileCx::new(&engine, &mut issues);
        cx.at("f", |cx| T::compile(raw, cx))
    };
    (value, issues)
}

/// Each issue as `(code, path)`.
fn codes(issues: &[Issue]) -> Vec<(IssueCode, String)> {
    issues
        .iter()
        .map(|i| (i.code, i.path.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// The dispatch
// ---------------------------------------------------------------------------

#[test]
fn the_kinds_are_the_builtin_names_in_order() {
    assert_eq!(BuiltinKind::ALL.map(BuiltinKind::name), BUILTIN_FUNCTIONS);
    for kind in BuiltinKind::ALL {
        assert_eq!(BuiltinKind::from_name(kind.name()), Some(kind));
    }
    // v3's alias is the compat reader's to rewrite; it is never a kind.
    assert_eq!(BuiltinKind::from_name("validate"), None);
    assert_eq!(BuiltinKind::from_name(""), None);
}

#[test]
fn each_kind_has_a_description_a_category_and_its_configs_fields() {
    let categories = BuiltinKind::ALL.map(|k| k.spec().category);
    assert_eq!(
        categories,
        [
            "transform",
            "validate",
            "control",
            "observe",
            "parse",
            "parse",
            "publish",
            "publish"
        ]
    );
    for kind in BuiltinKind::ALL {
        let spec = kind.spec();
        assert!(spec.description.ends_with('.'), "{}", kind.name());
        assert_eq!(spec.retry, RetrySafety::Unknown, "{}", kind.name());
        assert!(spec.references.is_empty() && spec.runtime_fields.is_none());
    }
    let configs: [&[crate::expr::FieldSpec]; 8] = [
        MapConfig::fields(),
        ValidationConfig::fields(),
        FilterConfig::fields(),
        LogConfig::fields(),
        ParseConfig::fields(),
        ParseConfig::fields(),
        PublishConfig::fields(),
        PublishConfig::fields(),
    ];
    for (kind, fields) in BuiltinKind::ALL.into_iter().zip(configs) {
        assert!(std::ptr::eq(kind.fields(), fields), "{}", kind.name());
    }
}

/// The names of `fields`, as a set.
fn names(fields: &[FieldSpec]) -> BTreeSet<&str> {
    fields.iter().map(|f| &*f.name).collect()
}

/// `keys`, as a set.
fn set<'k>(keys: &[&'k str]) -> BTreeSet<&'k str> {
    keys.iter().copied().collect()
}

#[test]
fn publish_keys_are_the_publish_configs_fields() {
    // The compat reader moves any other key out of a publish input before the
    // config compiles, so the two lists must be one set.
    assert_eq!(set(PUBLISH_KEYS), names(PublishConfig::fields()));
}

#[test]
fn the_readers_key_tables_are_the_configs_fields() {
    // I54: what the reader leaves in a built-in's input is exactly what the
    // config knows, at the input and in its nested configs.
    for kind in BuiltinKind::ALL {
        let input = builtin_input(kind.name()).unwrap_or_else(|| panic!("{}", kind.name()));
        assert_eq!(set(input.keys), names(kind.fields()), "{}", kind.name());
        let elements = input.elements.map(|(key, keys)| (key, set(keys)));
        let expected = match kind {
            BuiltinKind::Map => Some(("mappings", names(Mapping::fields()))),
            BuiltinKind::Validation => Some(("rules", names(Rule::fields()))),
            _ => None,
        };
        assert_eq!(elements, expected, "{}", kind.name());
    }
    assert_eq!(builtin_input("publish_xml").unwrap().keys, PUBLISH_KEYS);
    // `validate` is rewritten before the reader asks; a host function is not
    // a built-in.
    assert!(builtin_input("validate").is_none());
    assert!(builtin_input("http_call").is_none());
}

#[test]
fn compile_dispatches_to_each_config_and_keeps_its_kind() {
    let inputs = [
        json!({"mappings": [{"path": "data.x", "logic": 1}]}),
        json!({"rules": [{"logic": true, "message": "m"}]}),
        json!({"condition": true}),
        json!({"message": "m"}),
        json!({"source": "payload", "target": "in"}),
        json!({"source": "payload", "target": "in"}),
        json!({"source": "out", "target": "text"}),
        json!({"source": "out", "target": "text"}),
    ];
    let engine = ExprEngine::default();
    for (kind, input) in BuiltinKind::ALL.into_iter().zip(&inputs) {
        let mut issues = Vec::new();
        let builtin = {
            let mut cx = CompileCx::new(&engine, &mut issues);
            kind.compile(input, &mut cx)
        };
        assert!(issues.is_empty(), "{}: {issues:?}", kind.name());
        assert_eq!(builtin.map(|b| b.kind()), Some(kind), "{}", kind.name());
    }
    // A refusal is `None`, with the config's own issue.
    let mut issues = Vec::new();
    let builtin = {
        let mut cx = CompileCx::new(&engine, &mut issues);
        BuiltinKind::Filter.compile(&json!({}), &mut cx)
    };
    assert!(builtin.is_none());
    assert_eq!(
        codes(&issues),
        [(IssueCode::InputParse, "condition".to_owned())]
    );
}

// ---------------------------------------------------------------------------
// The defaults
// ---------------------------------------------------------------------------

#[test]
fn every_default_is_v3s() {
    let (filter, issues) = compile::<FilterConfig>(&json!({"condition": true}));
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(filter.unwrap().on_reject, RejectAction::Halt);

    let (log, issues) = compile::<LogConfig>(&json!({"message": "m"}));
    assert!(issues.is_empty(), "{issues:?}");
    let log = log.unwrap();
    assert_eq!(log.level, LogLevel::Info);
    assert!(log.fields.is_empty());

    let (publish, issues) = compile::<PublishConfig>(&json!({"source": "s", "target": "t"}));
    assert!(issues.is_empty(), "{issues:?}");
    let publish = publish.unwrap();
    assert!(!publish.pretty);
    assert_eq!(
        publish.root_element.constant().map(String::as_str),
        Some("root")
    );

    // The enums' own `Default` agrees with the configs' declared defaults.
    assert_eq!(RejectAction::default(), RejectAction::Halt);
    assert_eq!(LogLevel::default(), LogLevel::Info);
}

#[test]
fn the_spellings_are_v3s() {
    let (filter, _) = compile::<FilterConfig>(&json!({"condition": true, "on_reject": "skip"}));
    assert_eq!(filter.unwrap().on_reject, RejectAction::Skip);
    for (text, level) in [
        ("trace", LogLevel::Trace),
        ("debug", LogLevel::Debug),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
    ] {
        let (log, issues) = compile::<LogConfig>(&json!({"message": "m", "level": text}));
        assert!(issues.is_empty(), "{text}: {issues:?}");
        assert_eq!(log.unwrap().level, level);
    }
    let (log, issues) = compile::<LogConfig>(&json!({"message": "m", "level": "verbose"}));
    assert!(log.is_none());
    assert_eq!(
        codes(&issues),
        [(IssueCode::InputParse, "level".to_owned())]
    );
}

// ---------------------------------------------------------------------------
// `DataPath`
// ---------------------------------------------------------------------------

#[test]
fn a_data_path_is_written_relative_to_data() {
    let (path, issues) = field::<DataPath>(&json!("orders"));
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(path.unwrap().constant().unwrap().to_string(), "data.orders");

    // v3's `compute_data_path`: a leading `data.` is a key under `data`.
    let (path, _) = field::<DataPath>(&json!("data.t"));
    assert_eq!(path.unwrap().constant().unwrap().to_string(), "data.data.t");

    let computed = json!({"cat": ["in_", {"var": "data.slot"}]});
    let (path, issues) = field::<DataPath>(&computed);
    assert!(issues.is_empty(), "{issues:?}");
    let path = path.unwrap();
    assert!(path.constant().is_none());
    assert_eq!(path.expr().raw(), &computed);

    // JSONLogic that folds resolves at compile, as a literal does.
    let (path, _) = field::<DataPath>(&json!({"cat": ["or", "ders"]}));
    assert_eq!(path.unwrap().constant().unwrap().to_string(), "data.orders");
}

#[test]
fn a_data_path_that_is_not_a_string_is_input_parse() {
    for bad in [
        json!(7),
        json!(true),
        json!(null),
        json!(["a"]),
        json!({"+": [1, 2]}),
    ] {
        let (path, issues) = field::<DataPath>(&bad);
        assert!(path.is_none(), "{bad}");
        assert_eq!(
            codes(&issues),
            [(IssueCode::InputParse, "f".to_owned())],
            "{bad}"
        );
    }
}

#[test]
fn data_path_resolve_is_the_one_rule() {
    assert_eq!(DataPath::resolve("a.b").unwrap().to_string(), "data.a.b");
    assert_eq!(DataPath::resolve("#20").unwrap().to_string(), "data.#20");
    // An empty name is the key `""` under `data`, as v3's split made it.
    assert_eq!(DataPath::resolve("").unwrap().to_string(), "data.");
}

// ---------------------------------------------------------------------------
// `PublishSource`
// ---------------------------------------------------------------------------

/// The primary and the fallback, displayed.
fn pair(primary: Option<&Path>, fallback: Option<&Path>) -> (Option<String>, Option<String>) {
    (
        primary.map(ToString::to_string),
        fallback.map(ToString::to_string),
    )
}

#[test]
fn a_publish_source_keeps_v3s_data_fallback() {
    for (raw, primary, fallback) in [
        ("obj", "data.obj", None),
        ("data.obj", "data.data.obj", Some("data.obj")),
        ("data", "data.data", None),
        ("data.a.b", "data.data.a.b", Some("data.a.b")),
        ("metadata.x", "data.metadata.x", None),
    ] {
        let (source, issues) = field::<PublishSource>(&json!(raw));
        assert!(issues.is_empty(), "{raw}: {issues:?}");
        let source = source.unwrap();
        let expected = (Some(primary.to_owned()), fallback.map(str::to_owned));
        assert_eq!(
            pair(source.constant(), source.fallback()),
            expected,
            "{raw}"
        );
        // The run resolves a computed value as the compile resolved the literal.
        let (p, f) = PublishSource::resolve(raw).unwrap();
        assert_eq!(pair(Some(&p), f.as_ref()), expected, "{raw}");
        // The primary is `DataPath`'s, exactly.
        assert_eq!(Some(&DataPath::resolve(raw).unwrap()), source.constant());
    }
}

#[test]
fn a_computed_publish_source_resolves_at_run_time() {
    let raw = json!({"cat": ["data.", {"var": "data.kind"}]});
    let (source, issues) = field::<PublishSource>(&raw);
    assert!(issues.is_empty(), "{issues:?}");
    let source = source.unwrap();
    assert_eq!(pair(source.constant(), source.fallback()), (None, None));
    assert_eq!(source.expr().raw(), &raw);
    // What the run does with the value `data.obj`.
    let (primary, fallback) = PublishSource::resolve("data.obj").unwrap();
    assert_eq!(
        pair(Some(&primary), fallback.as_ref()),
        (
            Some("data.data.obj".to_owned()),
            Some("data.obj".to_owned())
        )
    );

    // One that folds resolves at compile, fallback included.
    let (source, _) = field::<PublishSource>(&json!({"cat": ["data.", "obj"]}));
    let source = source.unwrap();
    assert_eq!(
        pair(source.constant(), source.fallback()),
        (
            Some("data.data.obj".to_owned()),
            Some("data.obj".to_owned())
        )
    );

    // As a `DataPath` refuses it, a source that is not a string.
    let (source, issues) = field::<PublishSource>(&json!(7));
    assert!(source.is_none());
    assert_eq!(codes(&issues), [(IssueCode::InputParse, "f".to_owned())]);
}

// ---------------------------------------------------------------------------
// `ParseSource`
// ---------------------------------------------------------------------------

/// The variant, and its path where it has one.
fn shape_from(from: &ParseFrom) -> String {
    match from {
        ParseFrom::Payload => "Payload".to_owned(),
        ParseFrom::PayloadPath(p) => format!("PayloadPath({p})"),
        ParseFrom::Context(p) => format!("Context({p})"),
    }
}

/// [`shape_from`] for a literal; a folded or computed source says so.
fn shape(source: &ParseSource) -> String {
    match source {
        ParseSource::Literal(from) => shape_from(from),
        ParseSource::Folded { raw, from } => format!("Folded({raw}: {})", shape_from(from)),
        ParseSource::Computed(e) => format!("Computed({})", e.raw()),
    }
}

#[test]
fn a_parse_source_reads_v3s_spellings() {
    for (raw, expected) in [
        ("payload", "Payload"),
        ("payload.a.b", "PayloadPath(data.a.b)"),
        ("data.x", "Context(data.x)"),
        ("metadata.y", "Context(metadata.y)"),
        ("temp_data.z", "Context(temp_data.z)"),
    ] {
        let (source, issues) = field::<ParseSource>(&json!(raw));
        assert!(issues.is_empty(), "{raw}: {issues:?}");
        assert_eq!(shape(&source.unwrap()), expected);
    }
}

#[test]
fn a_parse_source_outside_the_roots_is_invalid_path() {
    // Break 15: v3 read any other name as a top-level context key.
    for bad in ["extra.x", "", "payloads.x"] {
        let (source, issues) = field::<ParseSource>(&json!(bad));
        assert!(source.is_none(), "{bad}");
        assert_eq!(
            codes(&issues),
            [(IssueCode::InvalidPath, "f".to_owned())],
            "{bad}"
        );
    }
}

#[test]
fn a_computed_parse_source_is_computed_unless_it_folds() {
    let raw = json!({"cat": ["data.batches.", {"var": "temp_data.i"}]});
    let (source, issues) = field::<ParseSource>(&raw);
    assert!(issues.is_empty(), "{issues:?}");
    let source = source.unwrap();
    assert!(matches!(&source, ParseSource::Computed(e) if e.raw() == &raw));

    // One that folds is resolved at compile (I71), by the resolver the run
    // uses: a constant that is not a string, or names no source, is refused
    // as `Expr<Path>` refuses it.
    let folds = json!({"cat": ["pay", "load"]});
    let (source, issues) = field::<ParseSource>(&folds);
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(shape(&source.unwrap()), format!("Folded({folds}: Payload)"));
    for bad in [
        json!({"cat": ["ext", "ra.x"]}),
        json!(7),
        json!(null),
        json!(["data.x"]),
    ] {
        let (source, issues) = field::<ParseSource>(&bad);
        assert!(source.is_none(), "{bad}");
        assert_eq!(
            codes(&issues),
            [(IssueCode::InputParse, "f".to_owned())],
            "{bad}"
        );
    }
}

#[test]
fn the_run_resolves_a_name_as_the_compile_resolves_a_literal() {
    // A computed source's value goes through `ParseSource::literal` at run
    // time; it must give the answer the same text gets as a literal.
    for name in [
        "payload",
        "payload.a.b",
        "payload.",
        "data",
        "data.x",
        "metadata.y.#3",
        "temp_data.z",
        "extra.x",
        "",
        "payloadx",
    ] {
        let (compiled, _) = field::<ParseSource>(&json!(name));
        let compiled = compiled.as_ref().map(shape);
        let resolved = ParseFrom::parse(name).ok().as_ref().map(shape_from);
        assert_eq!(compiled, resolved, "{name}");
    }
}

// ---------------------------------------------------------------------------
// What each config records
// ---------------------------------------------------------------------------

/// Each expression's path, its raw JSON and its `records` flag.
#[derive(Default)]
struct Recorded(Vec<(String, Value, bool)>);

impl ExprVisitor for Recorded {
    fn expr(&mut self, at: &AuthoredPath, raw: &Value, records: bool) {
        self.0.push((at.to_string(), raw.clone(), records));
    }
}

/// What `Builtin::visit` reports for `input` compiled as `kind`.
fn visited(kind: BuiltinKind, input: &Value) -> Vec<(String, Value, bool)> {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let builtin = {
        let mut cx = CompileCx::new(&engine, &mut issues);
        kind.compile(input, &mut cx)
    };
    assert!(issues.is_empty(), "{issues:?}");
    let mut recorded = Recorded::default();
    builtin.unwrap().visit(&mut recorded);
    recorded.0
}

#[test]
fn what_the_engine_records_is_visited_as_recorded() {
    let v = |path: &str, raw: Value, records: bool| (path.to_owned(), raw, records);
    assert_eq!(
        visited(
            BuiltinKind::Validation,
            &json!({"rules": [{"logic": {"var": "data.ok"}, "message": "m"}]})
        ),
        [
            v("rules[0].logic", json!({"var": "data.ok"}), false),
            v("rules[0].message", json!("m"), true),
        ]
    );
    assert_eq!(
        visited(
            BuiltinKind::Filter,
            &json!({"condition": true, "on_reject": "skip"})
        ),
        [v("condition", json!(true), false)]
    );
    assert_eq!(
        visited(
            BuiltinKind::Log,
            &json!({"message": "m", "level": "warn", "fields": {"a": 1, "b.c": {"var": "data.b"}}})
        ),
        [
            v("message", json!("m"), true),
            v("fields.a", json!(1), true),
            v("fields[\"b.c\"]", json!({"var": "data.b"}), true),
        ]
    );
    for kind in [BuiltinKind::ParseJson, BuiltinKind::ParseXml] {
        assert_eq!(
            visited(kind, &json!({"source": "payload.a", "target": "in"})),
            [
                v("source", json!("payload.a"), true),
                v("target", json!("in"), true),
            ]
        );
    }
    let computed = json!({"cat": ["data.", {"var": "data.k"}]});
    assert_eq!(
        visited(
            BuiltinKind::ParseJson,
            &json!({"source": computed, "target": "in"})
        )[0],
        v("source", computed, true)
    );
    for kind in [BuiltinKind::PublishJson, BuiltinKind::PublishXml] {
        assert_eq!(
            visited(
                kind,
                &json!({"source": "out", "target": "text", "pretty": true})
            ),
            [
                v("source", json!("out"), true),
                v("target", json!("text"), true),
                v("root_element", json!("root"), true),
            ]
        );
    }
}

#[test]
fn the_fields_say_what_each_config_records() {
    let records = |fields: &[crate::expr::FieldSpec]| -> Vec<(String, bool, bool)> {
        fields
            .iter()
            .map(|f| (f.name.to_string(), f.records, f.required))
            .collect()
    };
    let r = |name: &str, records: bool, required: bool| (name.to_owned(), records, required);
    assert_eq!(
        records(ValidationConfig::fields()),
        [r("rules", false, true)]
    );
    assert_eq!(
        records(Rule::fields()),
        [r("logic", false, true), r("message", true, true)]
    );
    assert_eq!(
        records(FilterConfig::fields()),
        [r("condition", false, true), r("on_reject", false, false)]
    );
    assert_eq!(
        records(LogConfig::fields()),
        [
            r("level", false, false),
            r("message", true, true),
            r("fields", true, false)
        ]
    );
    assert_eq!(
        records(ParseConfig::fields()),
        [r("source", true, true), r("target", true, true)]
    );
    assert_eq!(
        records(PublishConfig::fields()),
        [
            r("source", true, true),
            r("target", true, true),
            r("pretty", false, false),
            r("root_element", true, false)
        ]
    );
    for f in ParseConfig::fields() {
        assert!(matches!(
            f.kind,
            crate::expr::FieldKind::Expr(crate::expr::ValueKind::String)
        ));
    }
}

// ---------------------------------------------------------------------------
// What each config accepts and refuses
// ---------------------------------------------------------------------------

/// What v4 says of a row's input: through `def::parse` with the compat reader
/// on, then the config compile. Every path is below the task's input.
#[derive(Clone, Copy, Debug)]
enum V4 {
    /// No issue at all.
    Accepts,
    /// Loads, with exactly one issue, which is not Rejected: this code at
    /// this path.
    AcceptsWith(&'static str, IssueCode),
    /// Refused: every issue is this code at this field.
    Refuses(&'static str, IssueCode),
}

/// One input and what v4 says of it.
struct Row {
    input: Value,
    v4: V4,
}

fn ok(input: Value) -> Row {
    Row {
        input,
        v4: V4::Accepts,
    }
}

fn ok_with(input: Value, at: &'static str, code: IssueCode) -> Row {
    Row {
        input,
        v4: V4::AcceptsWith(at, code),
    }
}

fn refused(input: Value, at: &'static str, code: IssueCode) -> Row {
    Row {
        input,
        v4: V4::Refuses(at, code),
    }
}

/// Where the one task keeps its input.
const INPUT: &str = "tasks[0].function.input";

/// The one-task workflow a row's input is read in.
fn workflow(name: &str, input: &Value) -> Value {
    json!({"id": "w", "name": "w", "tasks": [
        {"id": "t", "name": "t", "function": {"name": name, "input": input}}
    ]})
}

/// Every issue v4 reports: `def::parse` with the compat reader on, then,
/// when it loaded, the config compile of the parsed task's input. Each as
/// `(path below the input, code, severity)`.
fn v4_issues(name: &str, input: &Value) -> Vec<(String, IssueCode, Severity)> {
    let parsed = parse(&workflow(name, input), ParseOptions::default());
    let mut issues = parsed.issues;
    if let Some(def) = parsed.def {
        let StepDef::Task(task) = &def.tasks[0] else {
            panic!("one task: {def:?}");
        };
        let kind = BuiltinKind::from_name(&task.function.name).expect("a built-in");
        let engine = ExprEngine::default();
        let before = issues.len();
        let builtin = {
            let mut cx = CompileCx::new(&engine, &mut issues).at_path(INPUT.parse().unwrap());
            kind.compile(&task.function.input, &mut cx)
        };
        let rejected = issues[before..]
            .iter()
            .any(|i| i.severity == Severity::Rejected);
        assert_eq!(builtin.is_some(), !rejected, "{name} {input}");
    }
    issues
        .into_iter()
        .map(|i| {
            let path = i.path.to_string();
            let below = match path.strip_prefix(INPUT) {
                Some(rest) => rest.strip_prefix('.').unwrap_or(rest).to_owned(),
                None => path,
            };
            (below, i.code, i.severity)
        })
        .collect()
}

fn map_rows() -> Vec<Row> {
    use IssueCode::{InputParse, InvalidMapping, InvalidPath, NullMapping, UnknownKey};
    let mapping = json!({"path": "data.x", "logic": 1});
    vec![
        ok(json!({"mappings": [mapping]})),
        ok(json!({"mappings": []})),
        refused(json!({}), "mappings", InputParse),
        refused(json!({"mappings": null}), "mappings", InputParse),
        refused(json!({"mappings": mapping}), "mappings", InputParse),
        refused(json!({"mappings": [5]}), "mappings[0]", InputParse),
        refused(
            json!({"mappings": [{"logic": 1}]}),
            "mappings[0].path",
            InputParse,
        ),
        refused(
            json!({"mappings": [{"path": "data.x"}]}),
            "mappings[0].logic",
            InvalidMapping,
        ),
        // Break 15: 3.x read a literal path outside the three roots as a
        // top-level context key, and a non-string one as its text.
        refused(
            json!({"mappings": [{"path": "extra.x", "logic": 1}]}),
            "mappings[0].path",
            InvalidPath,
        ),
        refused(
            json!({"mappings": [{"path": 7, "logic": 1}]}),
            "mappings[0].path",
            InputParse,
        ),
        ok(
            json!({"mappings": [{"path": {"cat": ["data.", {"var": "data.k"}]},
                                "logic": {"var": "data.y"}}]}),
        ),
        ok_with(
            json!({"mappings": [{"path": "data.x", "logic": null}]}),
            "mappings[0].logic",
            NullMapping,
        ),
        refused(json!(null), "", InputParse),
        // I54: the reader moves both into the task's `ext`.
        ok_with(
            json!({"mappings": [mapping], "strict": true}),
            "strict",
            UnknownKey,
        ),
        ok_with(
            json!({"mappings": [{"path": "data.x", "logic": 1, "note": "n"}]}),
            "mappings[0].note",
            UnknownKey,
        ),
    ]
}

/// Lists of mappings `map` accepts, as 3.x did: every `on_null` and `mode` with `logic`,
/// under each spelling of a `false` `unset`; a removal and a `null` logic; a
/// context root; `#` paths; computed paths.
fn accepted_mapping_lists() -> Vec<Vec<Value>> {
    let logic = json!({"var": "data.y"});
    let combinations = |unset: Option<bool>| -> Vec<Value> {
        let mut list = Vec::new();
        for on_null in [None, Some("skip"), Some("unset")] {
            for mode in [None, Some("set"), Some("append"), Some("extend")] {
                let mut m = json!({"path": "data.x", "logic": logic});
                for (key, value) in [
                    ("unset", unset.map(Value::from)),
                    ("on_null", on_null.map(Value::from)),
                    ("mode", mode.map(Value::from)),
                ] {
                    if let Some(value) = value {
                        m[key] = value;
                    }
                }
                list.push(m);
            }
        }
        list
    };
    vec![
        combinations(None),
        combinations(Some(false)),
        vec![
            json!({"path": "data.x", "unset": true}),
            json!({"path": "data.x", "logic": null}),
            json!({"path": "data", "logic": logic, "mode": "set", "on_null": "skip"}),
        ],
        vec![
            json!({"path": "data.MT103.#20", "logic": logic}),
            json!({"path": "temp_data.rows.#0", "logic": logic, "mode": "append"}),
        ],
        vec![
            json!({"path": {"cat": ["data.", {"var": "data.k"}]}, "logic": logic}),
            json!({"path": {"cat": ["data.", {"var": "data.k"}]}, "unset": true}),
        ],
    ]
}

#[test]
fn map_keeps_every_accepted_mapping() {
    for list in accepted_mapping_lists() {
        let input = json!({"mappings": list});
        let (config, issues) = compile::<MapConfig>(&input);
        let config = config.unwrap_or_else(|| panic!("{input}: {issues:?}"));
        assert_eq!(config.mappings.len(), list.len(), "{input}");
    }
}

fn validation_rows() -> Vec<Row> {
    use IssueCode::{InputParse, UnknownKey};
    let rule = json!({"logic": true, "message": "m"});
    vec![
        ok(json!({"rules": [rule]})),
        ok(json!({"rules": []})),
        refused(json!({}), "rules", InputParse),
        refused(json!({"rules": null}), "rules", InputParse),
        refused(json!({"rules": rule}), "rules", InputParse),
        refused(json!({"rules": [5]}), "rules[0]", InputParse),
        refused(
            json!({"rules": [{"message": "m"}]}),
            "rules[0].logic",
            InputParse,
        ),
        refused(
            json!({"rules": [{"logic": true}]}),
            "rules[0].message",
            InputParse,
        ),
        ok(json!({"rules": [{"logic": null, "message": "m"}]})),
        ok(json!({"rules": [{"logic": true, "message": null}]})),
        ok(json!({"rules": [{"logic": "yes", "message": "m"}]})),
        ok(json!({"rules": [{"logic": true, "message": 7}]})),
        ok(json!({"rules": [{"logic": {">": [{"var": "data.age"}, 0]}, "message": "m"}]})),
        // tests/jsonlogic_params.rs: a computed message, and one that would error.
        ok(json!({"rules": [{"logic": {">": [{"var": "data.age"}, 0]},
                             "message": {"cat": ["age must be positive, got ", {"var": "data.age"}]}}]})),
        ok(json!({"rules": [{"logic": true, "message": {"+": ["not", "a number"]}}]})),
        refused(json!(null), "", InputParse),
        // I54: the reader moves both into the task's `ext`.
        ok_with(
            json!({"rules": [rule], "strict": true}),
            "strict",
            UnknownKey,
        ),
        ok_with(
            json!({"rules": [{"logic": true, "message": "m", "code": "E1"}]}),
            "rules[0].code",
            UnknownKey,
        ),
    ]
}

fn filter_rows() -> Vec<Row> {
    use IssueCode::{InputParse, LikelyTypo};
    vec![
        ok(json!({"condition": true})),
        refused(json!({}), "condition", InputParse),
        ok(json!({"condition": null})),
        ok(json!({"condition": "yes"})),
        ok(json!({"condition": {"==": [{"var": "data.status"}, "active"]}})),
        ok(json!({"condition": true, "on_reject": "halt"})),
        ok(json!({"condition": true, "on_reject": "skip"})),
        refused(
            json!({"condition": true, "on_reject": null}),
            "on_reject",
            InputParse,
        ),
        refused(
            json!({"condition": true, "on_reject": 1}),
            "on_reject",
            InputParse,
        ),
        refused(
            json!({"condition": true, "on_reject": "maybe"}),
            "on_reject",
            InputParse,
        ),
        refused(
            json!({"condition": true, "on_reject": "Skip"}),
            "on_reject",
            InputParse,
        ),
        refused(json!([]), "", InputParse),
        // I54: moved into the task's `ext`, a Defect since it is a near miss.
        ok_with(
            json!({"condition": true, "on_rejct": "skip"}),
            "on_rejct",
            LikelyTypo,
        ),
    ]
}

fn log_rows() -> Vec<Row> {
    use IssueCode::{InputParse, UnknownKey};
    vec![
        ok(json!({"message": "m"})),
        refused(json!({}), "message", InputParse),
        ok(json!({"message": null})),
        ok(json!({"message": 5})),
        ok(json!({"message": {"cat": ["order ", {"var": "data.id"}]}})),
        ok(json!({"message": "m", "level": "warn"})),
        refused(json!({"message": "m", "level": null}), "level", InputParse),
        refused(json!({"message": "m", "level": 3}), "level", InputParse),
        refused(
            json!({"message": "m", "level": "verbose"}),
            "level",
            InputParse,
        ),
        ok(json!({"message": "m", "fields": {}})),
        refused(
            json!({"message": "m", "fields": null}),
            "fields",
            InputParse,
        ),
        refused(json!({"message": "m", "fields": [1]}), "fields", InputParse),
        ok(json!({"message": "m", "fields": {"a": null, "b": 2}})),
        ok(json!({"message": "m", "fields": {"id": {"var": "data.id"}}})),
        refused(json!("m"), "", InputParse),
        // I54: moved into the task's `ext`.
        ok_with(
            json!({"message": "m", "target": "stderr"}),
            "target",
            UnknownKey,
        ),
    ]
}

fn parse_rows() -> Vec<Row> {
    use IssueCode::{InputParse, InvalidPath, UnknownKey};
    vec![
        ok(json!({"source": "payload", "target": "input"})),
        refused(json!({"target": "input"}), "source", InputParse),
        refused(json!({"source": "payload"}), "target", InputParse),
        // docs/src/built-in-functions/parse.md's literal sources.
        ok(json!({"source": "payload.body.user", "target": "user"})),
        ok(json!({"source": "payload.xmlContent", "target": "x"})),
        ok(json!({"source": "data.raw", "target": "parsed"})),
        ok(json!({"source": "metadata.raw", "target": "parsed"})),
        ok(json!({"source": "temp_data.raw", "target": "parsed"})),
        // Break 15, and its widening to a source that is not a string.
        refused(
            json!({"source": "extra.x", "target": "t"}),
            "source",
            InvalidPath,
        ),
        refused(json!({"source": "", "target": "t"}), "source", InvalidPath),
        refused(json!({"source": null, "target": "t"}), "source", InputParse),
        refused(json!({"source": 7, "target": "t"}), "source", InputParse),
        // docs/src/built-in-functions/parse.md's computed source and target.
        ok(
            json!({"source": {"cat": ["data.batches.", {"var": "temp_data.i"}]},
                  "target": {"cat": ["parsed_", {"var": "temp_data.i"}]}}),
        ),
        // tests/jsonlogic_params.rs's computed target.
        ok(json!({"source": "payload", "target": {"cat": ["in_", {"var": "data.slot"}]}})),
        ok(json!({"source": "payload", "target": "data.t"})),
        ok(json!({"source": "payload", "target": ""})),
        // The widened break 15: a target that is not a string.
        refused(
            json!({"source": "payload", "target": null}),
            "target",
            InputParse,
        ),
        refused(
            json!({"source": "payload", "target": 7}),
            "target",
            InputParse,
        ),
        refused(json!(7), "", InputParse),
        // I54: moved into the task's `ext`.
        ok_with(
            json!({"source": "payload", "target": "t", "format": "json"}),
            "format",
            UnknownKey,
        ),
    ]
}

fn publish_rows() -> Vec<Row> {
    use IssueCode::{InputParse, LikelyTypo};
    vec![
        ok(json!({"source": "output", "target": "json_string"})),
        refused(json!({"target": "t"}), "source", InputParse),
        refused(json!({"source": "s"}), "target", InputParse),
        ok(json!({"source": "response.data", "target": "xmlOutput", "root_element": "Data"})),
        // I56: `data.data.x`, else v3's fallback `data.x`; nothing is rewritten.
        ok(json!({"source": "data.x", "target": "t"})),
        // corpus/cases/builtins/publish.json's spelling.
        ok(json!({"source": "data.obj", "target": "pretty_out", "pretty": true})),
        ok(json!({"source": "", "target": ""})),
        // The widened break 15: a source or target that is not a string.
        refused(json!({"source": null, "target": "t"}), "source", InputParse),
        refused(json!({"source": 7, "target": "t"}), "source", InputParse),
        refused(json!({"source": "s", "target": null}), "target", InputParse),
        refused(json!({"source": "s", "target": 7}), "target", InputParse),
        ok(json!({"source": "s", "target": "t", "pretty": true})),
        refused(
            json!({"source": "s", "target": "t", "pretty": null}),
            "pretty",
            InputParse,
        ),
        refused(
            json!({"source": "s", "target": "t", "pretty": "yes"}),
            "pretty",
            InputParse,
        ),
        ok(json!({"source": "s", "target": "t", "root_element": null})),
        ok(json!({"source": "s", "target": "t", "root_element": 7})),
        // tests/jsonlogic_params.rs's computed source, target and root element.
        ok(json!({"source": {"cat": ["in_", {"var": "data.slot"}]},
                  "target": {"cat": ["out_", {"var": "data.slot"}]}})),
        ok(json!({"source": "doc", "target": "xml", "root_element": {"var": "data.doc_type"}})),
        // docs/src/built-in-functions/publish.md's computed source and target.
        ok(
            json!({"source": {"cat": ["outputs.", {"var": "data.format"}]},
                  "target": {"cat": ["rendered_", {"var": "data.format"}]}}),
        ),
        refused(json!(null), "", InputParse),
        // Moved into the task's `ext` (R41), a Defect since it is a near miss.
        ok_with(
            json!({"source": "s", "target": "t", "targte": "u"}),
            "targte",
            LikelyTypo,
        ),
    ]
}

/// Where a row's v4 issues differ from what the row says, if they do.
fn mismatch(row: &Row, issues: &[(String, IssueCode, Severity)]) -> Option<String> {
    let only = |at: &str, code: IssueCode| issues.iter().all(|(p, c, _)| p == at && *c == code);
    let rejected = issues.iter().any(|(_, _, s)| *s == Severity::Rejected);
    let holds = match row.v4 {
        V4::Accepts => issues.is_empty(),
        V4::AcceptsWith(at, code) => issues.len() == 1 && only(at, code) && !rejected,
        V4::Refuses(at, code) => rejected && only(at, code),
    };
    (!holds).then(|| format!("v4 gave {issues:?}, the row says {:?}", row.v4))
}

#[test]
fn every_config_accepts_and_refuses_what_its_rows_say() {
    let tables: [(&[&str], Vec<Row>); 6] = [
        (&["map"], map_rows()),
        (&["validation"], validation_rows()),
        (&["filter"], filter_rows()),
        (&["log"], log_rows()),
        (&["parse_json", "parse_xml"], parse_rows()),
        (&["publish_json", "publish_xml"], publish_rows()),
    ];
    let mut failures = Vec::new();
    for (names, rows) in &tables {
        assert!(rows.len() >= 12, "{names:?} has {} rows", rows.len());
        for row in rows {
            for name in *names {
                let input = &row.input;
                if let Some(why) = mismatch(row, &v4_issues(name, input)) {
                    failures.push(format!("{name} {input}: {why}"));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} rows:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
