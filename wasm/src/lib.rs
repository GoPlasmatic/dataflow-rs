//! WebAssembly bindings for dataflow-rs 4: the engine in JavaScript and
//! TypeScript, as `@goplasmatic/dataflow-wasm`.
//!
//! A thin binding over [`Engine::run_with`]: one `WasmEngine` class, one
//! `run`, the trace's replay and the engine's facts. JSON crosses the
//! boundary as text (`JSON.stringify` in, `JSON.parse` out), so `null` stays
//! `null` and a plain object stays a plain object. Every result is typed: the
//! package's `.d.ts` carries the crate's wire shapes (`Message`, `Run`,
//! `Trace`, `Issue` and the rest) and the package's own option and result
//! types.
//!
//! # Usage
//!
//! ```javascript
//! import init, { WasmEngine, contexts } from '@goplasmatic/dataflow-wasm';
//!
//! await init();
//!
//! const engine = new WasmEngine([{
//!   id: "example",
//!   tasks: [{
//!     id: "greet",
//!     function: {
//!       name: "map",
//!       input: { mappings: [{ path: "data.greeting", logic: { cat: ["hello ", { var: "data.name" }] } }] }
//!     }
//!   }]
//! }]);
//!
//! const { message, run, trace } = await engine.run({ name: "world" }, {
//!   data: { name: "world" },
//!   trace: true,
//! });
//! console.log(run.status);                    // "completed"
//! console.log(message.context.data.greeting); // "hello world"
//! console.log(contexts(trace).at(-1).data);   // the context after the last step
//! ```
//!
//! A definition the engine refuses throws an `EngineError`: a JS `Error` whose
//! `code` is `"REJECTED"` and whose `issues` are the engine's, each at its
//! authored path. A run never rejects for what the message does: a stopped,
//! timed-out or failing run resolves, with `run.status` saying how it ended
//! and `trace` holding the failing step.

use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::def::{self, ParseOptions, Rollout};
use dataflow_rs::expr::{ExprEngine, Path, Secrets};
use dataflow_rs::observe::{Trace, TraceOptions};
use dataflow_rs::run::{AuditMode, Context, Engine, Instant, Message, Run, RunOptions};
use dataflow_rs::{Issue, Registry};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

/// The crate's wire shapes, embedded whole: what `message`, `run`, `trace`
/// and an issue are, as `bindings/dataflow.d.ts` declares them.
#[wasm_bindgen(typescript_custom_section)]
const WIRE_TYPES: &str = include_str!("../../bindings/dataflow.d.ts");

/// The package's own types: what each export takes and gives.
#[wasm_bindgen(typescript_custom_section)]
const PACKAGE_TYPES: &str = r#"
/**
 * What `new WasmEngine(workflows, options)` takes beside the definitions.
 */
export interface EngineOptions {
  /**
   * A secret store the workflows read through `{"secret": "name"}`. The
   * values are held by the engine, never by a message, so they appear in no
   * message, trace or error this engine returns.
   */
  secrets?: { [name: string]: JsonValue };
}

/**
 * Which audit entries a run keeps on the message: every one (`"full"`, the
 * default), none (`"off"`), or the most recent `last`.
 */
export type AuditMode = "full" | "off" | { last: number };

/**
 * What a run's trace records.
 */
export interface TraceOptions {
  /** Time each call, into its step's `elapsed_us`. Off by default. */
  timings?: boolean;
  /** Record each step's changes, so the trace replays. On by default. */
  changes?: boolean;
  /**
   * Context paths whose values read `null` in `initial` and in every change,
   * such as `"metadata.headers"`. The message keeps the real values.
   */
  redact?: string[];
  /**
   * A ceiling on what the trace holds. The step that would cross it, and
   * every later one, keeps everything but its changes, and `truncated` is set.
   */
  max_bytes?: number;
}

/**
 * What `engine.run(payload, options)` takes beside the payload.
 */
export interface RunOptions {
  /** The message id. A UUID v7 unless set. */
  id?: string;
  /** The `data` root the message starts with. An object, else ignored. */
  data?: JsonValue;
  /** The `metadata` root the message starts with. An object, else ignored. */
  metadata?: JsonValue;
  /** The channel whose workflows run. Every active workflow when unset. */
  channel?: string;
  /** The rollout bucket, 0 to 99 (a hash `% 100`). Every rollout admits an unset one. */
  routing_bucket?: number;
  /** Record each task's changes in the audit trail. On by default. */
  capture_changes?: boolean;
  /** Which audit entries the message keeps. `"full"` by default. */
  audit?: AuditMode;
  /** Trace the run: `true` for the default options, or the options. Off by default. */
  trace?: boolean | TraceOptions;
  /**
   * A deadline, in milliseconds from the call. The run then ends
   * `"timed_out"`, with its partial message and trace, rather than rejecting.
   */
  timeout_ms?: number;
}

/**
 * What `engine.run` resolves with, on every status.
 */
export interface RunResult {
  /** The message after the run. */
  message: Message;
  /** How the run ended and what each workflow did. */
  run: Run;
  /** The trace, when `trace` was asked for. */
  trace?: Trace;
}

/**
 * What the package throws: a JS `Error` with a `code`, and, for a refused
 * definition, the engine's issues.
 *
 * - `"REJECTED"`: a definition did not load; `issues` holds every issue, the
 *   Advisories too.
 * - `"INVALID_ARGUMENT"`: an argument is not what the export takes; the
 *   message names it.
 * - `"INVALID_TRACE"`: a trace handed to `contexts` or `mapping_contexts` is
 *   malformed.
 */
export interface EngineError extends Error {
  code: "REJECTED" | "INVALID_ARGUMENT" | "INVALID_TRACE";
  issues?: Issue[];
}

/**
 * Facts about the engine a tool would otherwise keep a copy of.
 */
export interface EngineFacts {
  /** The engine's version. */
  version: string;
  /** The built-in function names, which no registration may shadow. */
  builtin_functions: string[];
  /** The deepest group nesting a definition may use. */
  max_group_depth: number;
  /** What a `loop` reads for an absent `init` and `increment`. */
  loop_defaults: { init: number; increment: number };
  /** What a `for_each` reads for an absent `max_concurrency`. */
  for_each_defaults: { max_concurrency: number };
  /** The keys the engine knows at each level of a definition. */
  definition_keys: {
    workflow: string[];
    task: string[];
    group: string[];
    loop: string[];
    for_each: string[];
    rollout: string[];
    function: string[];
  };
}

/**
 * What `parse(definition)` gives: the definition in the 4.0 spelling, or
 * `null` when it was rejected, and every issue at its authored path.
 */
export interface ParseResult {
  definition: JsonValue | null;
  issues: Issue[];
}
"#;

/// Initialize the module: installs the panic hook, so a panic reaches the
/// console as a message rather than an `unreachable` trap. Called when the
/// module loads.
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

// ---------------------------------------------------------------------------
// The boundary: JSON as text, errors as `EngineError`
// ---------------------------------------------------------------------------

/// An `EngineError`: a JS `Error` carrying `code` and, when given, `issues`.
fn engine_error(code: &str, message: impl AsRef<str>, issues: Option<&[Issue]>) -> JsValue {
    let error = js_sys::Error::new(message.as_ref());
    // `Reflect::set` fails only on a frozen object, which a fresh `Error` is
    // not; an unlikely failure would lose the property, not the error.
    let _ = js_sys::Reflect::set(&error, &JsValue::from_str("code"), &JsValue::from_str(code));
    if let Some(issues) = issues
        && let Ok(issues) = to_js(issues)
    {
        let _ = js_sys::Reflect::set(&error, &JsValue::from_str("issues"), &issues);
    }
    error.into()
}

fn invalid(what: &str, detail: impl std::fmt::Display) -> JsValue {
    engine_error("INVALID_ARGUMENT", format!("{what}: {detail}"), None)
}

/// The JSON text of a JS value named `what`: `JSON.stringify`, with
/// `undefined` and `null` read as `null`.
fn stringify(v: &JsValue, what: &str) -> Result<String, JsValue> {
    if v.is_undefined() || v.is_null() {
        return Ok("null".to_owned());
    }
    let text = js_sys::JSON::stringify(v).map_err(|e| {
        invalid(
            what,
            js_sys::Error::from(e)
                .message()
                .as_string()
                .unwrap_or_else(|| "is not JSON".to_owned()),
        )
    })?;
    // `JSON.stringify` of a function or a symbol is `undefined`, not text.
    text.as_string().ok_or_else(|| invalid(what, "is not JSON"))
}

/// A JS value read as `T`, through its JSON text. A JS string is taken as
/// the string it is, not as JSON text to parse.
fn from_js<T: DeserializeOwned>(v: &JsValue, what: &str) -> Result<T, JsValue> {
    let text = stringify(v, what)?;
    serde_json::from_str(&text).map_err(|e| invalid(what, e))
}

/// A Rust value handed to JS: its JSON text through `JSON.parse`, so the
/// object's keys are in wire order and `null` is `null`.
fn to_js<T: Serialize + ?Sized>(v: &T) -> Result<JsValue, JsValue> {
    let text = serde_json::to_string(v).map_err(|e| invalid("result", e))?;
    js_sys::JSON::parse(&text)
}

/// The definitions argument: JSON text, or an array of definitions.
fn definitions(workflows: &JsValue) -> Result<Vec<Value>, JsValue> {
    let raw: Value = match workflows.as_string() {
        Some(text) => serde_json::from_str(&text).map_err(|e| invalid("workflows", e))?,
        None => from_js(workflows, "workflows")?,
    };
    match raw {
        Value::Array(items) => Ok(items),
        other => Err(invalid(
            "workflows",
            format!("must be an array of definitions, not {}", kind(&other)),
        )),
    }
}

/// A definition argument: one definition, as an object.
fn definition(v: &JsValue, what: &str) -> Result<Value, JsValue> {
    let raw: Value = from_js(v, what)?;
    if raw.is_object() {
        Ok(raw)
    } else {
        Err(invalid(
            what,
            format!("must be a definition object, not {}", kind(&raw)),
        ))
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The payload argument: a JS string is the string it is (so a `parse_json`
/// from `payload` reads it as 3.x did), `undefined` is `null`, and anything
/// else is the JSON it is.
fn payload(v: &JsValue) -> Result<OwnedDataValue, JsValue> {
    if let Some(text) = v.as_string() {
        return Ok(OwnedDataValue::from(&Value::String(text)));
    }
    let raw: Value = from_js(v, "payload")?;
    Ok(OwnedDataValue::from(&raw))
}

// ---------------------------------------------------------------------------
// The options and results, as read and written
// ---------------------------------------------------------------------------

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineOptionsIn {
    secrets: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunOptionsIn {
    id: Option<String>,
    data: Option<Value>,
    metadata: Option<Value>,
    channel: Option<String>,
    routing_bucket: Option<u8>,
    capture_changes: Option<bool>,
    /// `"full"`, `"off"` or `{ last: n }`: read by hand below, so a mistake
    /// inside it is named rather than reported as an unmatched variant.
    audit: Option<Value>,
    /// `true`, `false` or the trace options: read by hand below, as `audit`.
    trace: Option<Value>,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditLast {
    last: usize,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceOptionsIn {
    timings: Option<bool>,
    changes: Option<bool>,
    redact: Option<Vec<String>>,
    max_bytes: Option<usize>,
}

/// `options.audit`: `"full"`, `"off"` or `{ last: n }`.
fn audit_mode(raw: Value) -> Result<AuditMode, JsValue> {
    let shape = || invalid("options.audit", "must be \"full\", \"off\" or { last: n }");
    match raw {
        Value::String(mode) => match mode.as_str() {
            "full" => Ok(AuditMode::Full),
            "off" => Ok(AuditMode::Off),
            _ => Err(shape()),
        },
        Value::Object(_) => serde_json::from_value::<AuditLast>(raw)
            .map(|a| AuditMode::Last(a.last))
            .map_err(|e| invalid("options.audit", e)),
        _ => Err(shape()),
    }
}

/// `options.trace`: `true` (the default options), `false` (no trace) or the
/// options object.
fn trace_options(raw: Value) -> Result<Option<TraceOptions>, JsValue> {
    let opts = match raw {
        Value::Bool(false) => return Ok(None),
        Value::Bool(true) => TraceOptionsIn::default(),
        Value::Object(_) => serde_json::from_value::<TraceOptionsIn>(raw)
            .map_err(|e| invalid("options.trace", e))?,
        _ => {
            return Err(invalid(
                "options.trace",
                "must be true, false or an object of trace options",
            ));
        }
    };
    let mut trace = TraceOptions::default();
    if let Some(on) = opts.timings {
        trace = trace.timings(on);
    }
    if let Some(on) = opts.changes {
        trace = trace.changes(on);
    }
    if let Some(max) = opts.max_bytes {
        trace = trace.max_bytes(max);
    }
    let mut redact = Vec::with_capacity(opts.redact.as_ref().map_or(0, Vec::len));
    for raw in opts.redact.unwrap_or_default() {
        let path: Path = raw
            .parse()
            .map_err(|e| invalid("options.trace.redact", format!("{raw:?}: {e}")))?;
        redact.push(path);
    }
    Ok(Some(trace.redact(redact)))
}

/// The run options, owned, so the future that runs the message holds them.
struct Prepared {
    message: Message,
    channel: Option<String>,
    routing_bucket: Option<u8>,
    capture_changes: Option<bool>,
    audit: Option<AuditMode>,
    trace: Option<TraceOptions>,
    deadline: Option<Instant>,
}

impl RunOptionsIn {
    fn prepare(self, payload: OwnedDataValue) -> Result<Prepared, JsValue> {
        let seed = |v: Option<Value>| v.map_or(OwnedDataValue::Null, |v| OwnedDataValue::from(&v));
        let mut message = Message::from_parts(payload, seed(self.data), seed(self.metadata));
        if let Some(id) = self.id {
            message = message.with_id(id);
        }
        let audit = self.audit.map(audit_mode).transpose()?;
        let trace = self.trace.map(trace_options).transpose()?.flatten();
        let deadline = self
            .timeout_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms));
        // The engine serves a larger bucket as 99, as 3.x's Rust API did;
        // the package is new, so it refuses one rather than skew traffic.
        if let Some(bucket) = self.routing_bucket
            && bucket > Rollout::MAX_BUCKET
        {
            return Err(invalid(
                "options.routing_bucket",
                format!("{bucket} is not a bucket: buckets are 0 to 99"),
            ));
        }
        Ok(Prepared {
            message,
            channel: self.channel,
            routing_bucket: self.routing_bucket,
            capture_changes: self.capture_changes,
            audit,
            trace,
            deadline,
        })
    }
}

/// What `run` resolves with, written in wire order.
#[derive(Serialize)]
struct RunResult<'a> {
    message: &'a Message,
    run: &'a Run,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace: Option<&'a Trace>,
}

#[derive(Serialize)]
struct ParseResult {
    definition: Option<Value>,
    issues: Vec<Issue>,
}

#[derive(Serialize)]
struct EngineFacts {
    version: &'static str,
    builtin_functions: [&'static str; 8],
    max_group_depth: usize,
    loop_defaults: LoopDefaults,
    for_each_defaults: ForEachDefaults,
    definition_keys: dataflow_rs::def::DefinitionKeys,
}

#[derive(Serialize)]
struct LoopDefaults {
    init: i64,
    increment: i64,
}

#[derive(Serialize)]
struct ForEachDefaults {
    max_concurrency: usize,
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// A workflow engine in WebAssembly: a set of definitions, compiled once, that
/// runs messages and returns Promises.
#[derive(Debug)]
#[wasm_bindgen]
pub struct WasmEngine {
    inner: Engine,
}

#[wasm_bindgen]
impl WasmEngine {
    /// `new WasmEngine(workflows, options?)`: the definitions, as JSON text
    /// or an array, with an optional secret store.
    ///
    /// Throws an `EngineError`: `"REJECTED"` with `issues` when a definition
    /// does not load, `"INVALID_ARGUMENT"` when an argument is not what this
    /// takes.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "string | JsonValue[]")] workflows: JsValue,
        #[wasm_bindgen(unchecked_optional_param_type = "EngineOptions")] options: Option<
            js_sys::Object,
        >,
    ) -> Result<Self, JsValue> {
        let definitions = definitions(&workflows)?;
        let options: EngineOptionsIn = match options {
            Some(o) => from_js(&o.into(), "options")?,
            None => EngineOptionsIn::default(),
        };
        let mut exprs = ExprEngine::builder();
        if let Some(secrets) = options.secrets {
            let store = Secrets::new(OwnedDataValue::from(&secrets))
                .map_err(|e| invalid("options.secrets", e))?;
            exprs = exprs.secrets(store);
        }
        let exprs = exprs.build().map_err(|e| invalid("options", e))?;
        let mut builder = Engine::builder().registry(Registry::new(exprs));
        for raw in &definitions {
            builder = builder.workflow_json(raw);
        }
        let inner = builder.build().map_err(|issues| {
            let rejected = issues
                .iter()
                .filter(|i| i.severity == dataflow_rs::program::Severity::Rejected)
                .count();
            let first = issues
                .iter()
                .find(|i| i.severity == dataflow_rs::program::Severity::Rejected)
                .or(issues.first());
            let message = match first {
                Some(issue) => format!(
                    "{rejected} definition issue(s) rejected the workflows; the first: {} at `{}`: {}",
                    issue.code, issue.path, issue.message
                ),
                None => "the workflows were rejected".to_owned(),
            };
            engine_error("REJECTED", message, Some(&issues))
        })?;
        Ok(Self { inner })
    }

    /// `engine.run(payload, options?)`: runs one message and resolves, on
    /// every status, with `{ message, run, trace? }`.
    ///
    /// The payload is any value: a string stays a string, so a `parse_json`
    /// task reading `payload` parses it; an object is stored as the object it
    /// is. The promise rejects only for a malformed argument
    /// (`"INVALID_ARGUMENT"`); a run that stops, times out or fails resolves,
    /// with `run.status` saying how and `trace`, when asked for, holding the
    /// failing step.
    #[wasm_bindgen(unchecked_return_type = "Promise<RunResult>")]
    pub fn run(
        &self,
        #[wasm_bindgen(unchecked_param_type = "JsonValue | undefined")] payload: JsValue,
        #[wasm_bindgen(unchecked_optional_param_type = "RunOptions")] options: Option<
            js_sys::Object,
        >,
    ) -> js_sys::Promise {
        let prepared = (|| {
            let payload = self::payload(&payload)?;
            let options: RunOptionsIn = match options {
                Some(o) => from_js(&o.into(), "options")?,
                None => RunOptionsIn::default(),
            };
            options.prepare(payload)
        })();
        let Prepared {
            mut message,
            channel,
            routing_bucket,
            capture_changes,
            audit,
            trace,
            deadline,
        } = match prepared {
            Ok(p) => p,
            Err(e) => return js_sys::Promise::reject(&e),
        };
        let engine = self.inner.clone();
        future_to_promise(async move {
            let mut opts = RunOptions::default();
            if let Some(channel) = &channel {
                opts = opts.channel(channel);
            }
            if let Some(bucket) = routing_bucket {
                opts = opts.routing_bucket(bucket);
            }
            if let Some(on) = capture_changes {
                opts = opts.capture_changes(on);
            }
            if let Some(mode) = audit {
                opts = opts.audit(mode);
            }
            if let Some(trace) = trace {
                opts = opts.trace(trace);
            }
            if let Some(deadline) = deadline {
                opts = opts.deadline(deadline);
            }
            let run = engine.run_with(&mut message, &opts).await;
            to_js(&RunResult {
                message: &message,
                run: &run,
                trace: run.trace(),
            })
        })
    }

    /// `engine.check(definition)`: every issue the definition has, parsed and
    /// checked against this engine's functions, each at its authored path.
    /// An empty array means it would load.
    #[wasm_bindgen(unchecked_return_type = "Issue[]")]
    pub fn check(
        &self,
        #[wasm_bindgen(unchecked_param_type = "JsonValue")] definition: JsValue,
    ) -> Result<JsValue, JsValue> {
        let raw = self::definition(&definition, "definition")?;
        let mut parsed = def::parse(&raw, ParseOptions::default());
        if let Some(def) = &parsed.def {
            parsed.issues.extend(self.inner.compiler().check(def));
        }
        to_js(&parsed.issues)
    }

    /// How many workflows the engine holds.
    pub fn workflow_count(&self) -> usize {
        self.inner.program().workflows().len()
    }

    /// The workflows' ids, in program order.
    pub fn workflow_ids(&self) -> Vec<String> {
        self.inner
            .program()
            .workflows()
            .iter()
            .map(|w| w.id.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// The version of the engine compiled into this module.
///
/// Matches the published npm package version: the release workflow stamps
/// both from the root `Cargo.toml`. Pair a wasm build with a UI build on it:
/// an older engine reads a key it predates as an unknown key, kept in `ext`
/// and reported, so a workflow using a newer feature loads and does something
/// other than what it says.
#[wasm_bindgen]
pub fn engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Facts about the engine a tool would otherwise keep a copy of: the
/// built-in names, the group depth limit and the loop and fan-out defaults.
#[wasm_bindgen(unchecked_return_type = "EngineFacts")]
pub fn engine_facts() -> Result<JsValue, JsValue> {
    to_js(&EngineFacts {
        version: dataflow_rs::VERSION,
        builtin_functions: def::BUILTIN_FUNCTIONS,
        max_group_depth: def::MAX_GROUP_DEPTH,
        loop_defaults: LoopDefaults {
            init: def::defaults::INIT,
            increment: def::defaults::INCREMENT,
        },
        for_each_defaults: ForEachDefaults {
            max_concurrency: def::defaults::MAX_CONCURRENCY,
        },
        definition_keys: def::definition_keys(),
    })
}

/// `parse(definition)`: the definition read as the engine reads it, 3.x
/// spellings rewritten, and every issue at its authored path. `definition`
/// is `null` when the definition was rejected.
#[wasm_bindgen(unchecked_return_type = "ParseResult")]
pub fn parse(
    #[wasm_bindgen(unchecked_param_type = "JsonValue")] definition: JsValue,
) -> Result<JsValue, JsValue> {
    let raw = self::definition(&definition, "definition")?;
    let (definition, issues) = match dataflow_rs::compat::migrate_v3(&raw) {
        Ok(migrated) => (Some(migrated.json), migrated.issues),
        Err(issues) => (None, issues),
    };
    to_js(&ParseResult { definition, issues })
}

/// A trace read back, validated before anything replays or indexes it.
fn read_trace(trace: &JsValue) -> Result<Trace, JsValue> {
    let trace: Trace = from_js(trace, "trace")?;
    trace
        .validate()
        .map_err(|error| engine_error("INVALID_TRACE", error.message().to_owned(), None))?;
    Ok(trace)
}

/// `contexts(trace)`: the context after each step, replaying the steps'
/// changes from `trace.initial`; one per step, in order. The last one is the
/// message's context after the run, unless the trace is `truncated`.
///
/// Throws `"INVALID_TRACE"` for a trace that cannot be replayed: a step's
/// `mapping_ends` that do not fit its changes, a change whose path does not
/// parse, or a root written to anything but an object or `null`. It throws
/// `"INVALID_ARGUMENT"` for one that does not read as a trace.
#[wasm_bindgen(unchecked_return_type = "Context[]")]
pub fn contexts(
    #[wasm_bindgen(unchecked_param_type = "Trace")] trace: JsValue,
) -> Result<JsValue, JsValue> {
    let trace = read_trace(&trace)?;
    let contexts: Vec<Context> = trace.contexts().collect();
    to_js(&contexts)
}

/// `mapping_contexts(trace, step)`: for a `map` step, the context after each
/// of its mappings, in order; an empty array for any other step.
///
/// Throws `"INVALID_ARGUMENT"` for a step past the trace's last, and
/// `"INVALID_TRACE"` as `contexts` does.
#[wasm_bindgen(unchecked_return_type = "Context[]")]
pub fn mapping_contexts(
    #[wasm_bindgen(unchecked_param_type = "Trace")] trace: JsValue,
    step: f64,
) -> Result<JsValue, JsValue> {
    let trace = read_trace(&trace)?;
    // A JS number, checked here rather than wrapped by the glue: `NaN`, a
    // fraction, a negative number and one past the last step are refused.
    let len = trace.steps.len();
    if !step.is_finite() || step.fract() != 0.0 || step < 0.0 || step >= len as f64 {
        return Err(invalid(
            "step",
            format!("{step} is not a step of the trace, which has {len}"),
        ));
    }
    let contexts = trace.mapping_contexts(step as usize).unwrap_or_default();
    to_js(&contexts)
}
