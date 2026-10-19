use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::expr::{
    CompileCx, ConfigField, ExprEngine, ExprVisitor, FieldKind, FieldSpec, Interpolated, Output,
    ValueKind, Visit,
};
use dataflow_rs::function::{FunctionConfig, FunctionSpec, Templated, testing};
use dataflow_rs::program::{AuthoredPath, IssueCode, Severity};
use dataflow_rs::run::RunStatus;
use dataflow_rs::{Ctx, Engine, Expr, Function, Issue, Message, Outcome, Path, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::marker::PhantomData;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Level {
    Info,
    Warn,
}

/// A config nested in another.
#[derive(Debug, FunctionConfig)]
struct Limits {
    /// The most items to read.
    #[config(default = 100)]
    max: Expr<u64>,
}

/// The test struct: every attribute the derive knows.
#[derive(Debug, FunctionConfig)]
#[config(check = Probe::check)]
struct Probe {
    /// Where the result goes.
    #[output(primary, default = "data")]
    target: Output,
    /// Where a failure is recorded.
    errors: Output,
    /// What to read.
    ///
    ///   Any expression.
    #[config(records)]
    source: Expr,
    #[config(rename = "max_items", default = 100)]
    max: Expr<u64>,
    #[config(default = true)]
    strict: bool,
    #[config(default = "a \"quoted\" name")]
    label: String,
    #[config(default = -1.5)]
    ratio: f64,
    #[config(serde)]
    level: Option<Level>,
    #[config(default_json = r#"{"a": [1, 2]}"#)]
    extra: Value,
    #[config(records)]
    limits: Option<Limits>,
    body: Option<Interpolated>,
}

impl Probe {
    fn check(&self, cx: &mut CompileCx<'_>) {
        if self.max.constant().is_some_and(|max| *max > 1000) {
            cx.at("max_items", |cx| {
                cx.issue(IssueCode::InputParse, "at most 1000");
            });
        }
    }
}

/// One output, so it is the primary without saying so.
#[derive(Debug, FunctionConfig)]
struct OneOut {
    out: Output,
}

/// Outputs inside an `Option`, a list and a map.
#[derive(Debug, FunctionConfig)]
struct Outputs {
    maybe: Option<Output>,
    many: Vec<Output>,
    keyed: BTreeMap<String, Output>,
    /// Not an output, for contrast.
    plain: Option<Expr<String>>,
}

/// A serde field that cannot be read from `null` is required.
#[derive(Debug, FunctionConfig)]
struct Strict {
    #[config(serde)]
    level: Level,
}

fn root() -> AuthoredPath {
    "tasks[0].function.input".parse().unwrap()
}

fn compile<C: FunctionConfig>(raw: &Value) -> (Option<C>, Vec<Issue>) {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let out = {
        let mut cx = CompileCx::new(&engine, &mut issues).at_path(root());
        C::compile(raw, &mut cx)
    };
    (out, issues)
}

fn summary(issues: &[Issue]) -> Vec<(IssueCode, String)> {
    issues
        .iter()
        .map(|i| (i.code, i.path.to_string()))
        .collect()
}

use crate::support::too_deep;

fn valid() -> Value {
    json!({"errors": "temp_data.err", "source": 1})
}

#[test]
fn every_issue_is_reported_at_once_each_at_its_path() {
    let input = json!({
        "bogus": 1,
        "target": "results.x",
        "errors": "temp_data.err",
        "max_items": too_deep(),
        "strict": "yes",
    });
    let (out, issues) = compile::<Probe>(&input);
    assert!(out.is_none());
    assert_eq!(
        summary(&issues),
        [
            (
                IssueCode::UnknownKey,
                "tasks[0].function.input.bogus".into()
            ),
            (
                IssueCode::InvalidPath,
                "tasks[0].function.input.target".into()
            ),
            (
                IssueCode::InputParse,
                "tasks[0].function.input.source".into()
            ),
            (
                IssueCode::TemplateCompile,
                "tasks[0].function.input.max_items".into()
            ),
            (
                IssueCode::InputParse,
                "tasks[0].function.input.strict".into()
            ),
        ]
    );
    assert_eq!(issues[0].severity, Severity::Rejected);
    assert_eq!(issues[2].message, "missing field `source`");
    assert!(issues[4].message.contains("expected a boolean"));
    assert_eq!(issues[1].field.as_deref(), Some("target"));
}

#[test]
fn a_typo_names_the_near_key_and_unknown_keys_keep_input_order() {
    let mut input = valid();
    input["sourse"] = json!(1);
    input["zzz"] = json!(1);
    input["aaa"] = json!(1);
    let (out, issues) = compile::<Probe>(&input);
    assert!(out.is_none());
    let keys: Vec<String> = input
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| ["sourse", "zzz", "aaa"].contains(&k.as_str()))
        .map(|k| format!("tasks[0].function.input.{k}"))
        .collect();
    let reported: Vec<String> = issues.iter().map(|i| i.path.to_string()).collect();
    assert_eq!(reported, keys);
    let typo = issues
        .iter()
        .find(|i| i.path.to_string().ends_with("sourse"))
        .unwrap();
    assert_eq!(typo.code, IssueCode::LikelyTypo);
    assert!(typo.message.contains("`source`"));
}

#[test]
fn a_non_object_is_input_parse_at_the_root() {
    for raw in [json!([1]), json!(3), Value::Null] {
        let (out, issues) = compile::<Probe>(&raw);
        assert!(out.is_none());
        assert_eq!(
            summary(&issues),
            [(IssueCode::InputParse, "tasks[0].function.input".into())]
        );
        assert_eq!(issues[0].message, "expected an object");
    }
}

#[test]
fn absent_keys_take_their_defaults() {
    let (out, issues) = compile::<Probe>(&valid());
    assert!(issues.is_empty(), "{issues:?}");
    let p = out.unwrap();
    assert_eq!(p.max.constant(), Some(&100));
    assert!(p.strict);
    assert_eq!(p.label, "a \"quoted\" name");
    assert!((p.ratio + 1.5).abs() < f64::EPSILON);
    assert_eq!(p.level, None);
    assert_eq!(p.extra, json!({"a": [1, 2]}));
    assert!(p.limits.is_none() && p.body.is_none());
    assert_eq!(p.source.constant(), Some(&OwnedDataValue::from(&json!(1))));
    assert_eq!(p.errors.constant().unwrap().to_string(), "temp_data.err");
}

#[test]
fn present_keys_override_the_defaults() {
    let mut input = valid();
    input["max_items"] = json!({"var": "data.n"});
    input["level"] = json!("warn");
    input["extra"] = json!([]);
    input["limits"] = json!({"max": 7});
    let (out, issues) = compile::<Probe>(&input);
    assert!(issues.is_empty(), "{issues:?}");
    let p = out.unwrap();
    assert!(p.max.constant().is_none());
    assert_eq!(p.level, Some(Level::Warn));
    assert_eq!(p.extra, json!([]));
    assert_eq!(p.limits.unwrap().max.constant(), Some(&7));
}

#[test]
fn an_absent_or_null_output_takes_its_default() {
    for target in [None, Some(Value::Null)] {
        let mut input = valid();
        if let Some(t) = target {
            input["target"] = t;
        }
        let (out, issues) = compile::<Probe>(&input);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(out.unwrap().target.constant().unwrap().to_string(), "data");
    }
}

#[test]
fn a_required_output_is_missing_when_absent_or_null() {
    for errors in [None, Some(Value::Null)] {
        let mut input = json!({"source": 1});
        if let Some(e) = errors {
            input["errors"] = e;
        }
        let (out, issues) = compile::<Probe>(&input);
        assert!(out.is_none());
        assert_eq!(
            summary(&issues),
            [(
                IssueCode::InputParse,
                "tasks[0].function.input.errors".into()
            )]
        );
        assert_eq!(issues[0].message, "missing field `errors`");
    }
}

#[test]
fn a_serde_field_reports_input_parse_and_reads_absent_from_null() {
    let mut input = valid();
    input["level"] = json!("loud");
    let (out, issues) = compile::<Probe>(&input);
    assert!(out.is_none());
    assert_eq!(
        summary(&issues),
        [(
            IssueCode::InputParse,
            "tasks[0].function.input.level".into()
        )]
    );
    assert!(
        issues[0].message.contains("`loud`"),
        "{}",
        issues[0].message
    );

    let (out, issues) = compile::<Strict>(&json!({}));
    assert!(out.is_none());
    assert_eq!(issues[0].message, "missing field `level`");
    let (out, issues) = compile::<Strict>(&json!({"level": "info"}));
    assert!(issues.is_empty());
    assert_eq!(out.unwrap().level, Level::Info);
}

#[test]
fn a_nested_config_reports_at_its_path() {
    let mut input = valid();
    input["limits"] = json!({"max": "x", "min": 1});
    let (out, issues) = compile::<Probe>(&input);
    assert!(out.is_none());
    assert_eq!(
        summary(&issues),
        [
            (
                IssueCode::UnknownKey,
                "tasks[0].function.input.limits.min".into()
            ),
            (
                IssueCode::InputParse,
                "tasks[0].function.input.limits.max".into()
            ),
        ]
    );
    // The field is the first key below the root, however deep the issue.
    assert_eq!(issues[1].field.as_deref(), Some("limits"));
}

#[test]
fn check_runs_only_when_every_field_compiled() {
    let mut input = valid();
    input["max_items"] = json!(5000);
    let (out, issues) = compile::<Probe>(&input);
    assert!(
        out.is_none(),
        "a Rejected issue from check refuses the config"
    );
    assert_eq!(
        summary(&issues),
        [(
            IssueCode::InputParse,
            "tasks[0].function.input.max_items".into()
        )]
    );
    assert_eq!(issues[0].message, "at most 1000");

    input["strict"] = json!("yes");
    let (out, issues) = compile::<Probe>(&input);
    assert!(out.is_none());
    assert_eq!(
        summary(&issues),
        [(
            IssueCode::InputParse,
            "tasks[0].function.input.strict".into()
        )]
    );
}

#[test]
fn check_runs_beside_an_unknown_key() {
    let mut input = valid();
    input["max_items"] = json!(5000);
    input["bogus"] = json!(1);
    let (out, issues) = compile::<Probe>(&input);
    assert!(out.is_none());
    let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
    assert_eq!(codes, [IssueCode::UnknownKey, IssueCode::InputParse]);
}

#[test]
fn a_valid_default_json_round_trips() {
    let spec = field(Probe::fields(), "extra");
    let text = spec.default.as_deref().unwrap();
    assert_eq!(text, r#"{"a": [1, 2]}"#);
    let (out, _) = compile::<Probe>(&valid());
    assert_eq!(
        out.unwrap().extra,
        serde_json::from_str::<Value>(text).unwrap()
    );
}

fn field<'f>(fields: &'f [FieldSpec], name: &str) -> &'f FieldSpec {
    fields
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
}

#[test]
fn fields_lists_every_field_in_order() {
    let fields = Probe::fields();
    let names: Vec<&str> = fields.iter().map(|f| &*f.name).collect();
    assert_eq!(
        names,
        [
            "target",
            "errors",
            "source",
            "max_items",
            "strict",
            "label",
            "ratio",
            "level",
            "extra",
            "limits",
            "body"
        ]
    );
    // Built once.
    assert!(std::ptr::eq(fields, Probe::fields()));
}

#[test]
fn fields_carry_kind_required_records_default_and_doc() {
    let fields = Probe::fields();

    let target = field(fields, "target");
    assert!(matches!(target.kind, FieldKind::Output));
    // The engine records every output, so its field says so.
    assert!(!target.required && target.records && target.primary);
    assert_eq!(target.default.as_deref(), Some("\"data\""));
    assert_eq!(
        target.description.as_deref(),
        Some("Where the result goes.")
    );

    let errors = field(fields, "errors");
    assert!(matches!(errors.kind, FieldKind::Output));
    assert!(errors.required && errors.records && !errors.primary);
    assert_eq!(errors.default, None);

    let source = field(fields, "source");
    assert!(matches!(source.kind, FieldKind::Expr(ValueKind::Any)));
    assert!(source.required && source.records);
    assert_eq!(
        source.description.as_deref(),
        Some("What to read. Any expression.")
    );

    let max = field(fields, "max_items");
    assert!(matches!(max.kind, FieldKind::Expr(ValueKind::Number)));
    assert!(!max.required && !max.records);
    assert_eq!(max.default.as_deref(), Some("100"));
    assert_eq!(max.description, None);

    let strict = field(fields, "strict");
    assert!(matches!(strict.kind, FieldKind::Literal(ValueKind::Bool)));
    assert_eq!(strict.default.as_deref(), Some("true"));

    let label = field(fields, "label");
    assert_eq!(label.default.as_deref(), Some(r#""a \"quoted\" name""#));
    assert_eq!(field(fields, "ratio").default.as_deref(), Some("-1.5"));

    let level = field(fields, "level");
    assert!(matches!(level.kind, FieldKind::Literal(ValueKind::Any)));
    assert!(!level.required);
    let strict_level = field(Strict::fields(), "level");
    assert!(strict_level.required);

    let limits = field(fields, "limits");
    assert!(!limits.required && limits.records);
    match limits.kind {
        FieldKind::Nested(inner) => {
            let max = field(inner(), "max");
            assert!(!max.required);
            assert_eq!(max.default.as_deref(), Some("100"));
            assert_eq!(max.description.as_deref(), Some("The most items to read."));
        }
        other => panic!("a nested config, got {other:?}"),
    }
    assert!(matches!(
        <Limits as ConfigField>::kind(),
        FieldKind::Nested(_)
    ));

    let body = field(fields, "body");
    assert!(matches!(body.kind, FieldKind::Interpolated));
    assert!(!body.required);
}

#[test]
fn a_field_holding_an_output_at_any_depth_records() {
    let fields = Outputs::fields();
    let records = |name: &str| field(fields, name).records;
    // `Option` forwards its element's kind; a list and a map hold outputs.
    assert!(matches!(field(fields, "maybe").kind, FieldKind::Output));
    assert!(records("maybe"));
    assert!(matches!(field(fields, "many").kind, FieldKind::List(_)));
    assert!(records("many"));
    assert!(matches!(field(fields, "keyed").kind, FieldKind::Map(_)));
    assert!(records("keyed"));
    assert!(!records("plain"));
}

#[test]
fn a_lone_output_is_the_primary() {
    let out = field(OneOut::fields(), "out");
    assert!(out.primary && out.required);
    let (cfg, issues) = compile::<OneOut>(&json!({"out": "data.r"}));
    assert!(issues.is_empty());
    assert_eq!(cfg.unwrap().out.constant().unwrap().to_string(), "data.r");
}

/// A raw identifier: the field `r#type` is the key `type`.
#[derive(Debug, FunctionConfig)]
struct Raw {
    r#type: Expr<String>,
}

#[test]
fn a_raw_field_name_keeps_its_key() {
    let (cfg, issues) = compile::<Raw>(&json!({"type": "x"}));
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(
        cfg.unwrap().r#type.constant().map(String::as_str),
        Some("x")
    );
    assert_eq!(Raw::fields()[0].name, "type");

    let (cfg, issues) = compile::<Raw>(&json!({}));
    assert!(cfg.is_none());
    assert_eq!(
        summary(&issues),
        [(IssueCode::InputParse, "tasks[0].function.input.type".into())]
    );
    assert_eq!(issues[0].message, "missing field `type`");

    // The Rust spelling is not a key.
    let (cfg, issues) = compile::<Raw>(&json!({"r#type": "x"}));
    assert!(cfg.is_none());
    let codes: Vec<IssueCode> = issues.iter().map(|i| i.code).collect();
    assert_eq!(codes, [IssueCode::LikelyTypo, IssueCode::InputParse]);
    assert!(issues[0].message.contains("did you mean `type`"));

    let (cfg, _) = compile::<Raw>(&json!({"type": {"var": "data.t"}}));
    let mut seen = Collect::default();
    FunctionConfig::visit(&cfg.unwrap(), &mut seen);
    assert_eq!(seen.exprs[0].0, "type");
}

#[derive(Default)]
struct Collect {
    exprs: Vec<(String, Value, bool)>,
    docs: Vec<String>,
}

impl ExprVisitor for Collect {
    fn expr(&mut self, at: &AuthoredPath, raw: &Value, records: bool) {
        self.exprs.push((at.to_string(), raw.clone(), records));
    }
    fn interpolated(&mut self, at: &AuthoredPath, _doc: &Interpolated) {
        self.docs.push(at.to_string());
    }
}

#[test]
fn visit_reports_every_expression_with_its_relative_path() {
    let mut input = valid();
    input["limits"] = json!({"max": {"var": "data.n"}});
    input["body"] = json!({"x": {"var": "data.y"}});
    let (out, issues) = compile::<Probe>(&input);
    assert!(issues.is_empty(), "{issues:?}");
    let mut seen = Collect::default();
    FunctionConfig::visit(&out.unwrap(), &mut seen);
    assert_eq!(
        seen.exprs,
        [
            // An `Output` is recorded without saying so.
            ("target".into(), json!("data"), true),
            ("errors".into(), json!("temp_data.err"), true),
            ("source".into(), json!(1), true),
            ("max_items".into(), json!(100), false),
            // `records` on `limits` is inherited by everything below it.
            ("limits.max".into(), json!({"var": "data.n"}), true),
        ]
    );
    assert_eq!(seen.docs, ["body"]);
}

#[test]
fn a_nested_config_visits_below_its_parent_path() {
    let engine = ExprEngine::default();
    let mut issues = Vec::new();
    let limits = {
        let mut cx = CompileCx::new(&engine, &mut issues);
        <Vec<Limits> as ConfigField>::compile(&json!([{}, {"max": {"var": "data.n"}}]), &mut cx)
    }
    .unwrap();
    let mut seen = Collect::default();
    let mut v = Visit::new(&mut seen, "input".parse().unwrap());
    v.recording(true, |v| v.at("limits", |v| limits.visit(v)));
    assert_eq!(
        seen.exprs,
        [
            ("input.limits[0].max".into(), json!(100), true),
            ("input.limits[1].max".into(), json!({"var": "data.n"}), true),
        ]
    );
}

/// The constant path of `config`'s primary output, if it has one.
fn primary<C: FunctionConfig>(config: &C) -> Option<String> {
    config
        .primary_output()
        .map(|out| out.constant().expect("a literal output").to_string())
}

#[test]
fn primary_output_is_the_single_output_or_the_marked_one() {
    // Two outputs: the one marked `#[output(primary)]`.
    let (probe, issues) = compile::<Probe>(&valid());
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(primary(&probe.unwrap()).as_deref(), Some("data"));
    // One output: it, unmarked.
    let (one, _) = compile::<OneOut>(&json!({"out": "data.r"}));
    assert_eq!(primary(&one.unwrap()).as_deref(), Some("data.r"));
    // No output field: none.
    let (limits, _) = compile::<Limits>(&json!({}));
    assert_eq!(primary(&limits.unwrap()), None);
    // Outputs in a list and a map are not outputs themselves; the one in an
    // option is, and the only one, so it is the primary.
    let (outputs, issues) = compile::<Outputs>(&json!({
        "maybe": "data.m", "many": ["data.a"], "keyed": {"k": "data.k"}}));
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(primary(&outputs.unwrap()).as_deref(), Some("data.m"));
    assert!(field(Outputs::fields(), "maybe").primary);
    // A hand-written config has none unless it says so.
    assert_eq!(primary(&()), None);
}

/// An optional primary output beside a required one.
#[derive(Debug, FunctionConfig)]
struct MaybeOut {
    #[output(primary)]
    maybe: Option<Output>,
    always: Output,
}

#[test]
fn an_option_output_primary_is_as_ref() {
    let (cfg, issues) = compile::<MaybeOut>(&json!({"always": "data.a"}));
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(primary(&cfg.unwrap()), None, "absent: no primary output");
    let (cfg, _) = compile::<MaybeOut>(&json!({"maybe": "data.m", "always": "data.a"}));
    let cfg = cfg.unwrap();
    assert_eq!(primary(&cfg).as_deref(), Some("data.m"));
    assert!(std::ptr::eq(
        cfg.primary_output().unwrap(),
        cfg.maybe.as_ref().unwrap()
    ));
}

/// A lone optional output, unmarked: the primary, as a lone `Output` is.
#[derive(Debug, FunctionConfig)]
struct LoneMaybe {
    url: Expr<String>,
    output: Option<Output>,
}

#[test]
fn a_lone_option_output_is_the_primary_without_saying_so() {
    assert!(field(LoneMaybe::fields(), "output").primary);
    let (cfg, issues) = compile::<LoneMaybe>(&json!({"url": "u", "output": "data.r"}));
    assert!(issues.is_empty(), "{issues:?}");
    let cfg = cfg.unwrap();
    assert_eq!(primary(&cfg).as_deref(), Some("data.r"));
    assert!(std::ptr::eq(
        cfg.primary_output().unwrap(),
        cfg.output.as_ref().unwrap()
    ));
    // Unset, or `null`: no primary output for this input.
    for input in [json!({"url": "u"}), json!({"url": "u", "output": null})] {
        let (cfg, issues) = compile::<LoneMaybe>(&input);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(primary(&cfg.unwrap()), None, "{input}");
    }
}

/// Resolves its config over the call's context and writes the object at
/// `temp_data.<task id>`.
struct Resolves<C>(PhantomData<fn() -> C>);

impl<C: FunctionConfig> Function for Resolves<C> {
    type Config = C;

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &C) -> Result<Outcome> {
        let resolved = ctx.resolve(cfg)?;
        let at: Path = format!("temp_data.{}", ctx.info().task_id).parse()?;
        ctx.set(&at, resolved)?;
        Ok(Outcome::Ok)
    }
}

/// The message every resolve below reads.
fn resolve_message() -> Message {
    Message::from_parts(
        OwnedDataValue::Null,
        OwnedDataValue::from(&json!({"n": 4, "name": "ada", "k": "v"})),
        OwnedDataValue::Null,
    )
}

/// What a call wrote at `temp_data.<task>`, as wire text.
fn resolved_at(message: &Message, task: &str) -> String {
    let at: Path = format!("temp_data.{task}").parse().unwrap();
    let value = message.context().get(&at).expect("the call wrote it");
    serde_json::to_string(value).unwrap()
}

/// One field of each kind the derive resolves.
#[derive(FunctionConfig)]
struct Kinds {
    flag: bool,
    count: u32,
    any: Expr,
    text: Expr<String>,
    n: Expr<u64>,
    delta: Expr<i64>,
    ratio: Expr<f64>,
    on: Expr<bool>,
    to: Expr<Path>,
    set: Option<Expr<u64>>,
    unset: Option<Expr<u64>>,
    nulled: Option<Expr>,
    list: Vec<Expr<u64>>,
    keyed: BTreeMap<String, Expr<String>>,
    limits: Limits,
    #[output(primary)]
    literal_out: Output,
    computed_out: Output,
    body: Interpolated,
    #[config(rename = "renamed_key")]
    renamed: String,
    #[config(serde)]
    level: Level,
    #[config(serde)]
    maybe_level: Option<Level>,
}

#[tokio::test]
async fn a_derived_config_resolves_each_field_in_declaration_order() {
    let input = json!({
        "flag": true,
        "count": 7,
        "any": {"var": "data.k"},
        "text": {"cat": ["hi ", {"var": "data.name"}]},
        "n": {"+": [{"var": "data.n"}, 1]},
        "delta": {"-": [{"var": "data.n"}, 10]},
        "ratio": {"/": [{"var": "data.n"}, 8]},
        "on": {"==": [{"var": "data.n"}, 4]},
        "to": {"cat": ["data.", {"var": "data.k"}]},
        "set": 3,
        "nulled": null,
        "list": [1, {"var": "data.n"}],
        "keyed": {"b": {"var": "data.name"}, "a": "lit"},
        "limits": {"max": {"var": "data.n"}},
        "literal_out": "data.out",
        "computed_out": {"cat": ["data.", {"var": "data.k"}]},
        "body": {"$oid": {"var": "data.name"}, "n": 1},
        "renamed_key": "r",
        "level": "warn",
    });
    let called = testing::call(Resolves::<Kinds>(PhantomData), input)
        .message(resolve_message())
        .await;
    assert!(called.issues.is_empty(), "{:?}", called.issues);
    assert_eq!(called.outcome.unwrap(), Outcome::Ok);
    // Every set field, by its authored key, in declaration order: a scalar
    // as itself; an expression evaluated, as a number, a string, a boolean
    // or a path's text; an `Option` left unset has no key, and one set to
    // `null` is `null`; a list and a map element by element (the map in its
    // own key order); a nested config as an object; an output's path; a
    // document rendered; a serde field as it serializes, and absent when
    // that is `null`.
    assert_eq!(
        resolved_at(&called.message, "test"),
        concat!(
            r#"{"flag":true,"count":7,"any":"v","text":"hi ada","n":5,"delta":-6,"#,
            r#""ratio":0.5,"on":true,"to":"data.v","set":3,"nulled":null,"list":[1,4],"#,
            r#""keyed":{"a":"lit","b":"ada"},"limits":{"max":4},"literal_out":"data.out","#,
            r#""computed_out":"data.v","body":{"$oid":"ada","n":1},"renamed_key":"r","#,
            r#""level":"warn"}"#
        )
    );
}

/// Floats a derived config reads three ways: as a scalar, as an expression
/// and through serde.
#[derive(FunctionConfig)]
struct Floats {
    scalar: f64,
    expr: Expr<f64>,
    #[config(serde)]
    weights: Vec<f64>,
}

#[tokio::test]
async fn a_whole_number_float_resolves_as_a_float() {
    // `2.0` is written `2.0` whichever way the field reads it, as the
    // message writes a whole-number float.
    let input = json!({"scalar": 2.0, "expr": {"*": [{"var": "data.n"}, 0.5]},
                       "weights": [1.0, 2.5]});
    let called = testing::call(Resolves::<Floats>(PhantomData), input)
        .message(resolve_message())
        .await;
    assert!(called.issues.is_empty(), "{:?}", called.issues);
    assert_eq!(called.outcome.unwrap(), Outcome::Ok);
    assert_eq!(
        resolved_at(&called.message, "test"),
        r#"{"scalar":2.0,"expr":2.0,"weights":[1.0,2.5]}"#
    );
}

/// A derived config with one field of each kind a `Templated` manifest can
/// declare.
#[derive(FunctionConfig)]
struct Twin {
    expr: Expr,
    n: Expr<u64>,
    to: Expr<Path>,
    doc: Interpolated,
    label: String,
    count: u32,
    ratio: f64,
    any: Value,
    unset: Option<String>,
    out: Output,
}

/// [`Twin`]'s fields as a plugin's manifest: resolved through `Templated`.
struct TwinPlugin;

impl Function for TwinPlugin {
    type Config = Templated;

    fn spec(&self) -> FunctionSpec {
        FunctionSpec::default().with_runtime_fields(Twin::fields().to_vec())
    }

    async fn call(&self, ctx: &mut Ctx<'_>, cfg: &Templated) -> Result<Outcome> {
        let resolved = cfg.resolved(ctx)?;
        // The trait's resolve is `resolved`, by the call's context too.
        assert_eq!(FunctionConfig::resolve(cfg, &mut &*ctx)?, resolved);
        assert_eq!(ctx.resolve(cfg)?, resolved);
        ctx.set(&"temp_data.templated".parse()?, resolved)?;
        Ok(Outcome::Ok)
    }
}

#[tokio::test]
async fn a_derived_config_and_its_templated_twin_resolve_alike() {
    // One resolver: the same fields, derived and declared at runtime,
    // resolve to the same object over one message, a whole-number float
    // written as the float it is.
    for ratio in ["2.5", "2.0"] {
        twins_resolve_alike(serde_json::from_str(ratio).unwrap(), ratio).await;
    }
}

/// [`Twin`] and its manifest over one input, `ratio` its `ratio`, which
/// both write as `written`.
async fn twins_resolve_alike(ratio: Value, written: &str) {
    let input = json!({
        "expr": {"var": "data.k"},
        "n": {"+": [{"var": "data.n"}, 1]},
        "to": {"cat": ["data.", {"var": "data.k"}]},
        "doc": {"$oid": {"var": "data.name"}, "n": 1},
        "label": "l",
        "count": 7,
        "ratio": ratio,
        "any": {"a": [1, {"b": null}]},
        "out": {"cat": ["data.", {"var": "data.k"}]},
    });
    let e = Engine::builder()
        .register("derived", Resolves::<Twin>(PhantomData))
        .register("templated", TwinPlugin)
        .workflow_json(&json!({"id": "w", "tasks": [
            {"id": "derived", "function": {"name": "derived", "input": input}},
            {"id": "templated", "function": {"name": "templated", "input": input}},
        ]}))
        .build()
        .unwrap_or_else(|issues| panic!("the engine builds: {issues:?}"));
    let mut m = resolve_message();
    let run = e.run(&mut m).await;
    assert!(matches!(run.status(), RunStatus::Completed), "{run:?}");
    assert!(m.errors().is_empty(), "{:?}", m.errors());
    let want = format!(
        concat!(
            r#"{{"expr":"v","n":5,"to":"data.v","doc":{{"$oid":"ada","n":1}},"label":"l","#,
            r#""count":7,"ratio":{},"any":{{"a":[1,{{"b":null}}]}},"out":"data.v"}}"#
        ),
        written
    );
    assert_eq!(resolved_at(&m, "derived"), want);
    assert_eq!(resolved_at(&m, "templated"), want);
}
