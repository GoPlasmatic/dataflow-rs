//! The issue vocabulary: authored paths, issue codes, severities and issues.

use std::fmt;
use std::str::FromStr;

/// One segment of an [`AuthoredPath`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum PathSeg {
    /// An object key.
    Key(Box<str>),
    /// An array index.
    Index(usize),
}

impl From<&str> for PathSeg {
    fn from(key: &str) -> Self {
        Self::Key(key.into())
    }
}

impl From<String> for PathSeg {
    fn from(key: String) -> Self {
        Self::Key(key.into())
    }
}

impl From<usize> for PathSeg {
    fn from(index: usize) -> Self {
        Self::Index(index)
    }
}

/// A path in the authored JSON: keys and indices, displayed as v3 spelled them,
/// `tasks[1].tasks[0].id`, `function.input.mappings[3].logic`. The root displays
/// as the empty string.
///
/// A key that could not otherwise be read back, one that is empty or holds a
/// `.`, `[`, `]` or `"`, displays as a JSON string in brackets:
/// `function.input.fields["a.b"]`, `[""]`. [`FromStr`] reads every displayed
/// form back, and a path serializes as it displays.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AuthoredPath(Vec<PathSeg>);

impl AuthoredPath {
    /// The empty path, the document itself.
    pub fn root() -> Self {
        Self::default()
    }

    /// The segments, outermost first.
    pub fn segs(&self) -> &[PathSeg] {
        &self.0
    }

    /// Is this the root?
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Appends a segment.
    pub fn push(&mut self, seg: impl Into<PathSeg>) {
        self.0.push(seg.into());
    }

    /// Removes and returns the last segment.
    pub fn pop(&mut self) -> Option<PathSeg> {
        self.0.pop()
    }

    /// A copy with one more segment.
    pub fn join(&self, seg: impl Into<PathSeg>) -> Self {
        let mut path = self.clone();
        path.push(seg);
        path
    }

    /// `self` followed by `rest`.
    pub fn extend(&self, rest: &Self) -> Self {
        let mut path = self.clone();
        path.0.extend(rest.0.iter().cloned());
        path
    }
}

/// Whether `key` displays quoted: bare, it would read back as something else.
fn needs_quotes(key: &str) -> bool {
    key.is_empty() || key.contains(['.', '[', ']', '"'])
}

impl fmt::Display for AuthoredPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, seg) in self.0.iter().enumerate() {
            match seg {
                PathSeg::Key(key) if needs_quotes(key) => {
                    let quoted = serde_json::to_string(&**key).map_err(|_| fmt::Error)?;
                    write!(f, "[{quoted}]")?;
                }
                PathSeg::Key(key) if i == 0 => f.write_str(key)?,
                PathSeg::Key(key) => write!(f, ".{key}")?,
                PathSeg::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}

/// A string that is not an [`AuthoredPath`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsePathError(String);

impl fmt::Display for ParsePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not an authored path: {:?}", self.0)
    }
}

impl std::error::Error for ParsePathError {}

impl FromStr for AuthoredPath {
    type Err = ParsePathError;

    /// The inverse of `Display`: a bare key holds no `.`, `[`, `]` or `"`, and
    /// a bracketed one is a JSON string.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || ParsePathError(text.to_string());
        let mut path = Self::root();
        let mut rest = text;
        let mut first = true;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix("[\"") {
                // The string runs to the first `"` no backslash escapes.
                let mut escaped = false;
                let end = after
                    .char_indices()
                    .find(|&(_, c)| {
                        let close = c == '"' && !escaped;
                        escaped = c == '\\' && !escaped;
                        close
                    })
                    .map(|(at, _)| at)
                    .ok_or_else(bad)?;
                // `after` starts past `["`: the closing quote is `rest[end + 2]`.
                let literal = &rest[1..end + 3];
                let key: String = serde_json::from_str(literal).map_err(|_| bad())?;
                path.push(key);
                rest = after[end + 1..].strip_prefix(']').ok_or_else(bad)?;
            } else if let Some(after) = rest.strip_prefix('[') {
                let end = after.find(']').ok_or_else(bad)?;
                let index = after[..end].parse::<usize>().map_err(|_| bad())?;
                path.push(index);
                rest = &after[end + 1..];
            } else {
                let after = if first {
                    rest
                } else {
                    rest.strip_prefix('.').ok_or_else(bad)?
                };
                let end = after.find(['.', '[']).unwrap_or(after.len());
                let key = &after[..end];
                if needs_quotes(key) {
                    return Err(bad());
                }
                path.push(key);
                rest = &after[end..];
            }
            first = false;
        }
        Ok(path)
    }
}

impl serde::Serialize for AuthoredPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// What an issue is about. v3's 34 codes in v3's order, then the eight v4 adds.
///
/// `#[non_exhaustive]`: use [`Self::as_str`] to serialize.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IssueCode {
    /// `id` is missing or empty.
    EmptyWorkflowId,
    /// `name` is missing or empty.
    EmptyWorkflowName,
    /// `tasks` is missing, not an array, or empty.
    NoTasks,
    /// A step carries no `id`.
    MissingStepId,
    /// Two steps share an id. Groups share the task id namespace.
    DuplicateStepId,
    /// A group's `tasks` is not a non-empty array.
    EmptyGroup,
    /// A group is nested at or beyond
    /// `MAX_GROUP_DEPTH`.
    GroupTooDeep,
    /// A task carries no `function`.
    MissingFunction,
    /// `function` is not an object, or its `name` is missing or empty.
    InvalidFunctionName,
    /// `terminal` is present but not a boolean.
    InvalidTerminal,
    /// A `validation` task whose failure cannot stop anything: it is not
    /// `terminal`, has no `halt_on`, and every task after it in the workflow is
    /// unguarded. A failing rule returns `400`, which `continue_on_error` does
    /// not cover, so the pipeline proceeds as if it had passed.
    ///
    /// [`Severity::Advisory`] — reported by
    /// `check_workflow` and never by
    /// `build`. Validating purely to record errors is a
    /// legitimate shape; this only says the assertion is not acting as a gate.
    UnguardedValidation,
    /// `halt_on` is present and is not one of the accepted strings, or is
    /// present on a *group* — a group has no outcome of its own, so the
    /// executor could not honour it.
    InvalidHaltOn,
    /// A task group carries `continue_on_error`, which the engine drops. Error
    /// handling is per task and per workflow; a group only gates a span.
    ///
    /// [`Severity::Advisory`] — reported by
    /// `check_workflow` and never by
    /// `build`. Unlike [`Self::InvalidHaltOn`], which
    /// the parser refuses outright, this key is real at the other two levels
    /// and may already sit on a host's group nodes; refusing it would abort
    /// every workflow in the build.
    GroupContinueOnError,
    /// A `map` mapping breaks a rule between its keys: it has neither `logic`
    /// nor `"unset": true`, or both; it carries `on_null` or `mode` without
    /// `logic`; or it would remove or append to a context root (`data`,
    /// `metadata`, `temp_data`) by a literal path. Reported at the offending key, e.g.
    /// `tasks[0].function.input.mappings[2].unset`.
    InvalidMapping,
    /// A `map` mapping whose `logic` always evaluates to `null` — `"logic":
    /// null`, or anything that folds to it — under the default
    /// `on_null: "skip"`. A null result is skipped, so the mapping can never
    /// write anything; it usually stands where the author meant to clear the
    /// path, which is `"unset": true`.
    ///
    /// [`Severity::Advisory`] — reported by
    /// `check_workflow` and never by
    /// `build`. The mapping is a no-op, not an error,
    /// and refusing it would fail every build that already carries one.
    NullMapping,
    /// `loop.increment` is below 1 — the counter would never reach `max`.
    LoopIncrementTooSmall,
    /// `loop.max` is not greater than `loop.init` — no sweep could ever run.
    LoopBoundEmpty,
    /// `loop.counter` is not a non-empty dotted path.
    LoopCounterInvalid,
    /// `loop.as` or `loop.scratch` is not a non-empty dotted path.
    LoopSlotInvalid,
    /// `loop.as` names a slot for the current element, but there is no
    /// `loop.over` to take elements from.
    LoopItemWithoutOver,
    /// Two of `loop.counter`, `loop.as` and `loop.scratch` name the same
    /// `temp_data` path, or one lies inside the other — a per-iteration reset
    /// of one would wipe the other. Reported at the later of the two fields.
    LoopSlotCollision,
    /// `loop.over` can never yield an array: it is a scalar literal (a string,
    /// number, boolean or `null`), or `loop.init` is negative, which no array
    /// index can be. Reported at `loop.over` or `loop.init` respectively.
    LoopOverInvalid,
    /// A task's `for_each` breaks one of its rules — `over` a scalar literal,
    /// `as` not a slot path, `max_concurrency` of `0`, `collect` without
    /// `into` or the reverse, a result path outside the context, a `collect`
    /// or `into` overlapping a binding — or sits on a built-in function, or on
    /// a task group. Reported at the offending key, e.g.
    /// `tasks[0].for_each.max_concurrency`, or at `…for_each` itself.
    InvalidForEach,
    /// No handler will dispatch this function name, and it is not a built-in.
    /// Usually a typo or a handler the host forgot to register.
    UnknownFunction,
    /// The name *is* a built-in, but one that ships as a config schema only
    /// (`http_call`, `enrich`, `publish_kafka`) and no handler is registered
    /// under it. The workflow builds cleanly and then fails every message.
    MissingHandler,
    /// A custom task's `input` does not deserialize into its handler's declared
    /// `Input` type.
    InputParse,
    /// A `Template` field of a custom task's input does not compile.
    TemplateCompile,
    /// An expression reads `{"secret": "name"}` and no secret of that name is
    /// declared on the engine (`EngineBuilder::with_secrets`). Only literal
    /// names are checked; a dynamic name fails at evaluation instead.
    UnknownSecret,
    /// An expression whose result the engine writes to the message or emits to
    /// a log reads a secret — a `map` mapping, or a `log` message or field. The
    /// store exists so a value is never recorded; an expression that would
    /// record it is refused outright, derived or not. Compute derived values in
    /// a custom handler.
    SecretInMessageWrite,
    /// The store passed to
    /// `EngineBuilder::with_secrets` is
    /// not a JSON object, so no name resolves and
    /// `EngineBuilder::build` will fail.
    /// Reported by
    /// `EngineBuilder::check_workflow`
    /// *instead of* the [`Self::UnknownSecret`] issues every literal name would
    /// otherwise produce — the workflow is not what is wrong.
    ///
    /// [`Severity::Rejected`], but the odd one out: quarantining the workflow
    /// will not make the build succeed, because the store is what is broken.
    InvalidSecretStore,
    /// Two keys in one template object collapse to the same name once the
    /// template-key escape is stripped — `{"$a": 1, "a": 2}` emits `a` twice.
    /// The context is a `Vec` of pairs, so both survive: a later read sees only
    /// the first while serialization emits both. Always a bug, so
    /// `build` refuses it.
    DuplicateTemplateKey,
    /// A template key carries the escape prefix, so it is emitted with one
    /// prefix stripped: `$type` emits `type`. [`Severity::Advisory`] — reported
    /// by `check_workflow` and never by
    /// `build`.
    ///
    /// Exists for migration. The escape strips uniformly from every template
    /// key, so a workflow written before 3.9 that emits genuinely `$`-prefixed
    /// keys — MongoDB's `$set`/`$oid`, JSON Schema's `$schema`/`$ref` — changes
    /// what it produces, silently. This lists every one so the audit is
    /// mechanical rather than archaeological.
    EscapedTemplateKey,
    /// The document does not deserialize into a workflow. Carries the
    /// parser's own message, which names the offending field and type.
    ParseFailed,
    /// The document parses but workflow validation rejects it. A backstop:
    /// reaching this means a rule exists that the checks above do not model.
    ValidateFailed,
    /// An unknown key within edit distance 1 (keys of up to five characters) or 2 (longer) of a known key at its level: almost certainly a misspelling, so the setting it was meant to carry is silently lost.
    ///
    /// Default severity [`Severity::Defect`].
    LikelyTypo,
    /// An unknown key that resembles no known key at its level. It moves into `ext` under the compat reader and is refused without it.
    ///
    /// Default severity [`Severity::Advisory`].
    UnknownKey,
    /// The compat reader rewrote a v3 spelling to its v4 counterpart.
    ///
    /// Default severity [`Severity::Advisory`].
    V3Spelling,
    /// A literal `var` rooted at `payload`. The evaluation context is `{data, metadata, temp_data}`, so it resolves to nothing.
    ///
    /// Default severity [`Severity::Defect`].
    PayloadVar,
    /// A single-key object inside an interpolated document whose key is a live operator name. A document is never evaluated, so it stays data.
    ///
    /// Default severity [`Severity::Advisory`].
    OperatorInDocument,
    /// A `{"secret": ...}` node inside an interpolated document. A document is never evaluated, so the secret would never be substituted.
    ///
    /// Default severity [`Severity::Rejected`].
    SecretInDocument,
    /// A literal context path that is not rooted at `data`, `metadata` or `temp_data`, or a document hole whose path is not a literal.
    ///
    /// Default severity [`Severity::Rejected`].
    InvalidPath,
    /// A `Reference` field whose value is a literal string the host's
    /// `ResourceOracle` does not resolve, such as a connector no deployment
    /// declares. A computed or absent value is not checked.
    ///
    /// Default severity [`Severity::Defect`]: the workflow loads, then fails
    /// every call.
    UnresolvedReference,
}

impl IssueCode {
    /// The stable string form, SCREAMING_SNAKE, v3's spellings unchanged.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmptyWorkflowId => "EMPTY_WORKFLOW_ID",
            Self::EmptyWorkflowName => "EMPTY_WORKFLOW_NAME",
            Self::NoTasks => "NO_TASKS",
            Self::MissingStepId => "MISSING_STEP_ID",
            Self::DuplicateStepId => "DUPLICATE_STEP_ID",
            Self::EmptyGroup => "EMPTY_GROUP",
            Self::GroupTooDeep => "GROUP_TOO_DEEP",
            Self::MissingFunction => "MISSING_FUNCTION",
            Self::InvalidFunctionName => "INVALID_FUNCTION_NAME",
            Self::InvalidTerminal => "INVALID_TERMINAL",
            Self::InvalidHaltOn => "INVALID_HALT_ON",
            Self::GroupContinueOnError => "GROUP_CONTINUE_ON_ERROR",
            Self::InvalidMapping => "INVALID_MAPPING",
            Self::NullMapping => "NULL_MAPPING",
            Self::UnguardedValidation => "UNGUARDED_VALIDATION",
            Self::LoopIncrementTooSmall => "LOOP_INCREMENT_TOO_SMALL",
            Self::LoopBoundEmpty => "LOOP_BOUND_EMPTY",
            Self::LoopCounterInvalid => "LOOP_COUNTER_INVALID",
            Self::LoopSlotInvalid => "LOOP_SLOT_INVALID",
            Self::LoopItemWithoutOver => "LOOP_ITEM_WITHOUT_OVER",
            Self::LoopSlotCollision => "LOOP_SLOT_COLLISION",
            Self::LoopOverInvalid => "LOOP_OVER_INVALID",
            Self::InvalidForEach => "INVALID_FOR_EACH",
            Self::UnknownFunction => "UNKNOWN_FUNCTION",
            Self::MissingHandler => "MISSING_HANDLER",
            Self::InputParse => "INPUT_PARSE",
            Self::TemplateCompile => "TEMPLATE_COMPILE",
            Self::UnknownSecret => "UNKNOWN_SECRET",
            Self::SecretInMessageWrite => "SECRET_IN_MESSAGE_WRITE",
            Self::InvalidSecretStore => "INVALID_SECRET_STORE",
            Self::DuplicateTemplateKey => "DUPLICATE_TEMPLATE_KEY",
            Self::EscapedTemplateKey => "ESCAPED_TEMPLATE_KEY",
            Self::ParseFailed => "PARSE_FAILED",
            Self::ValidateFailed => "VALIDATE_FAILED",
            Self::LikelyTypo => "LIKELY_TYPO",
            Self::UnknownKey => "UNKNOWN_KEY",
            Self::V3Spelling => "V3_SPELLING",
            Self::PayloadVar => "PAYLOAD_VAR",
            Self::OperatorInDocument => "OPERATOR_IN_DOCUMENT",
            Self::SecretInDocument => "SECRET_IN_DOCUMENT",
            Self::InvalidPath => "INVALID_PATH",
            Self::UnresolvedReference => "UNRESOLVED_REFERENCE",
        }
    }

    /// The default severity of an issue carrying this code.
    ///
    /// The match has **no wildcard arm**, and must never gain one: a code added
    /// later stops this compiling until it is deliberately classified, so a new
    /// advisory cannot arrive as a silent rejection in a host's screen. An issue
    /// may carry a different severity ([`Issue::with_severity`]).
    pub fn severity(self) -> Severity {
        match self {
            // Reported for an author's attention; the definition loads.
            Self::UnguardedValidation
            | Self::GroupContinueOnError
            | Self::EscapedTemplateKey
            | Self::NullMapping
            | Self::UnknownKey
            | Self::V3Spelling
            | Self::OperatorInDocument => Severity::Advisory,

            // Loads, then does not do what it says.
            Self::MissingHandler
            | Self::LikelyTypo
            | Self::PayloadVar
            | Self::UnresolvedReference => Severity::Defect,

            // Everything else never loads.
            Self::EmptyWorkflowId
            | Self::EmptyWorkflowName
            | Self::NoTasks
            | Self::MissingStepId
            | Self::DuplicateStepId
            | Self::EmptyGroup
            | Self::GroupTooDeep
            | Self::MissingFunction
            | Self::InvalidFunctionName
            | Self::InvalidTerminal
            | Self::InvalidHaltOn
            | Self::InvalidMapping
            | Self::LoopIncrementTooSmall
            | Self::LoopBoundEmpty
            | Self::LoopCounterInvalid
            | Self::LoopSlotInvalid
            | Self::LoopItemWithoutOver
            | Self::LoopSlotCollision
            | Self::LoopOverInvalid
            | Self::InvalidForEach
            | Self::UnknownFunction
            | Self::InputParse
            | Self::TemplateCompile
            | Self::UnknownSecret
            | Self::SecretInMessageWrite
            | Self::InvalidSecretStore
            | Self::DuplicateTemplateKey
            | Self::ParseFailed
            | Self::ValidateFailed
            | Self::SecretInDocument
            | Self::InvalidPath => Severity::Rejected,
        }
    }
}

impl fmt::Display for IssueCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A string that is not an [`IssueCode`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseIssueCodeError(String);

impl fmt::Display for ParseIssueCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown issue code: {:?}", self.0)
    }
}

impl std::error::Error for ParseIssueCodeError {}

impl FromStr for IssueCode {
    type Err = ParseIssueCodeError;

    /// The inverse of [`IssueCode::as_str`], for reading recordings.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "EMPTY_WORKFLOW_ID" => Ok(Self::EmptyWorkflowId),
            "EMPTY_WORKFLOW_NAME" => Ok(Self::EmptyWorkflowName),
            "NO_TASKS" => Ok(Self::NoTasks),
            "MISSING_STEP_ID" => Ok(Self::MissingStepId),
            "DUPLICATE_STEP_ID" => Ok(Self::DuplicateStepId),
            "EMPTY_GROUP" => Ok(Self::EmptyGroup),
            "GROUP_TOO_DEEP" => Ok(Self::GroupTooDeep),
            "MISSING_FUNCTION" => Ok(Self::MissingFunction),
            "INVALID_FUNCTION_NAME" => Ok(Self::InvalidFunctionName),
            "INVALID_TERMINAL" => Ok(Self::InvalidTerminal),
            "INVALID_HALT_ON" => Ok(Self::InvalidHaltOn),
            "GROUP_CONTINUE_ON_ERROR" => Ok(Self::GroupContinueOnError),
            "INVALID_MAPPING" => Ok(Self::InvalidMapping),
            "NULL_MAPPING" => Ok(Self::NullMapping),
            "UNGUARDED_VALIDATION" => Ok(Self::UnguardedValidation),
            "LOOP_INCREMENT_TOO_SMALL" => Ok(Self::LoopIncrementTooSmall),
            "LOOP_BOUND_EMPTY" => Ok(Self::LoopBoundEmpty),
            "LOOP_COUNTER_INVALID" => Ok(Self::LoopCounterInvalid),
            "LOOP_SLOT_INVALID" => Ok(Self::LoopSlotInvalid),
            "LOOP_ITEM_WITHOUT_OVER" => Ok(Self::LoopItemWithoutOver),
            "LOOP_SLOT_COLLISION" => Ok(Self::LoopSlotCollision),
            "LOOP_OVER_INVALID" => Ok(Self::LoopOverInvalid),
            "INVALID_FOR_EACH" => Ok(Self::InvalidForEach),
            "UNKNOWN_FUNCTION" => Ok(Self::UnknownFunction),
            "MISSING_HANDLER" => Ok(Self::MissingHandler),
            "INPUT_PARSE" => Ok(Self::InputParse),
            "TEMPLATE_COMPILE" => Ok(Self::TemplateCompile),
            "UNKNOWN_SECRET" => Ok(Self::UnknownSecret),
            "SECRET_IN_MESSAGE_WRITE" => Ok(Self::SecretInMessageWrite),
            "INVALID_SECRET_STORE" => Ok(Self::InvalidSecretStore),
            "DUPLICATE_TEMPLATE_KEY" => Ok(Self::DuplicateTemplateKey),
            "ESCAPED_TEMPLATE_KEY" => Ok(Self::EscapedTemplateKey),
            "PARSE_FAILED" => Ok(Self::ParseFailed),
            "VALIDATE_FAILED" => Ok(Self::ValidateFailed),
            "LIKELY_TYPO" => Ok(Self::LikelyTypo),
            "UNKNOWN_KEY" => Ok(Self::UnknownKey),
            "V3_SPELLING" => Ok(Self::V3Spelling),
            "PAYLOAD_VAR" => Ok(Self::PayloadVar),
            "OPERATOR_IN_DOCUMENT" => Ok(Self::OperatorInDocument),
            "SECRET_IN_DOCUMENT" => Ok(Self::SecretInDocument),
            "INVALID_PATH" => Ok(Self::InvalidPath),
            "UNRESOLVED_REFERENCE" => Ok(Self::UnresolvedReference),
            _ => Err(ParseIssueCodeError(text.to_string())),
        }
    }
}

/// When an issue bites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Severity {
    /// The definition never runs.
    Rejected,
    /// It loads and then fails or misbehaves on messages.
    Defect,
    /// It loads and runs; the shape deserves an author's attention.
    Advisory,
}

impl Severity {
    const ALL: [Self; 3] = [Self::Rejected, Self::Defect, Self::Advisory];

    /// The stable string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "REJECTED",
            Self::Defect => "DEFECT",
            Self::Advisory => "ADVISORY",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What kind of step an issue sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StepKind {
    /// A task.
    Task,
    /// A task group.
    Group,
    /// Any step under `loop.setup`.
    Setup,
}

impl StepKind {
    const ALL: [Self; 3] = [Self::Task, Self::Group, Self::Setup];

    /// The stable string form: `task`, `group` or `setup`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Group => "group",
            Self::Setup => "setup",
        }
    }
}

/// Reads a value from the string it serializes as.
fn from_text<'de, D, T, E>(
    deserializer: D,
    read: impl FnOnce(&str) -> Result<T, E>,
) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    E: fmt::Display,
{
    let text = <String as serde::Deserialize>::deserialize(deserializer)?;
    read(&text).map_err(serde::de::Error::custom)
}

/// The variant of `all` whose string is `text`.
fn named<T: Copy>(
    all: &[T],
    as_str: fn(T) -> &'static str,
    what: &str,
    text: &str,
) -> Result<T, String> {
    all.iter()
        .copied()
        .find(|v| as_str(*v) == text)
        .ok_or_else(|| format!("unknown {what}: {text:?}"))
}

impl<'de> serde::Deserialize<'de> for AuthoredPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_text(deserializer, str::parse)
    }
}

impl serde::Serialize for IssueCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for IssueCode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_text(deserializer, str::parse)
    }
}

impl serde::Serialize for Severity {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Severity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_text(deserializer, |text| {
            named(&Self::ALL, Self::as_str, "severity", text)
        })
    }
}

impl serde::Serialize for StepKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for StepKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_text(deserializer, |text| {
            named(&Self::ALL, Self::as_str, "step kind", text)
        })
    }
}

/// One finding about an authored definition.
///
/// It serializes as an object of its seven fields, under their own names: the
/// code and severity as their strings, the path as it displays, the step kind
/// as `task`, `group` or `setup`, and an absent part as `null`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Issue {
    /// What it is about.
    pub code: IssueCode,
    /// When it bites. [`IssueCode::severity`] unless overridden.
    pub severity: Severity,
    /// Where in the authored JSON.
    pub path: AuthoredPath,
    /// The kind of step it sits in, if any.
    pub step_kind: Option<StepKind>,
    /// The id of the step it sits in, if known.
    pub step_id: Option<String>,
    /// The config field (the first key under `function.input`), if inside one.
    pub field: Option<String>,
    /// A human-readable explanation.
    pub message: String,
}

impl Issue {
    /// An issue at its code's default severity.
    pub fn new(code: IssueCode, path: AuthoredPath, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: code.severity(),
            path,
            step_kind: None,
            step_id: None,
            field: None,
            message: message.into(),
        }
    }

    /// Overrides the severity.
    #[must_use]
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Names the step the issue sits in.
    #[must_use]
    pub fn with_step(mut self, kind: StepKind, id: Option<&str>) -> Self {
        self.step_kind = Some(kind);
        self.step_id = id.map(str::to_string);
        self
    }

    /// Names the config field the issue sits in.
    #[must_use]
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_root() {
            write!(f, "{} [{}]", self.message, self.code)
        } else {
            write!(f, "{}: {} [{}]", self.path, self.message, self.code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::HashSet;

    /// Every [`IssueCode`]; kept complete by [`ordinal`].
    const ALL_CODES: [IssueCode; 42] = [
        IssueCode::EmptyWorkflowId,
        IssueCode::EmptyWorkflowName,
        IssueCode::NoTasks,
        IssueCode::MissingStepId,
        IssueCode::DuplicateStepId,
        IssueCode::EmptyGroup,
        IssueCode::GroupTooDeep,
        IssueCode::MissingFunction,
        IssueCode::InvalidFunctionName,
        IssueCode::InvalidTerminal,
        IssueCode::UnguardedValidation,
        IssueCode::InvalidHaltOn,
        IssueCode::GroupContinueOnError,
        IssueCode::InvalidMapping,
        IssueCode::NullMapping,
        IssueCode::LoopIncrementTooSmall,
        IssueCode::LoopBoundEmpty,
        IssueCode::LoopCounterInvalid,
        IssueCode::LoopSlotInvalid,
        IssueCode::LoopItemWithoutOver,
        IssueCode::LoopSlotCollision,
        IssueCode::LoopOverInvalid,
        IssueCode::InvalidForEach,
        IssueCode::UnknownFunction,
        IssueCode::MissingHandler,
        IssueCode::InputParse,
        IssueCode::TemplateCompile,
        IssueCode::UnknownSecret,
        IssueCode::SecretInMessageWrite,
        IssueCode::InvalidSecretStore,
        IssueCode::DuplicateTemplateKey,
        IssueCode::EscapedTemplateKey,
        IssueCode::ParseFailed,
        IssueCode::ValidateFailed,
        IssueCode::LikelyTypo,
        IssueCode::UnknownKey,
        IssueCode::V3Spelling,
        IssueCode::PayloadVar,
        IssueCode::OperatorInDocument,
        IssueCode::SecretInDocument,
        IssueCode::InvalidPath,
        IssueCode::UnresolvedReference,
    ];

    /// A distinct index per variant, with no wildcard arm: adding a code stops
    /// this compiling until it is given one.
    fn ordinal(code: IssueCode) -> usize {
        match code {
            IssueCode::EmptyWorkflowId => 0,
            IssueCode::EmptyWorkflowName => 1,
            IssueCode::NoTasks => 2,
            IssueCode::MissingStepId => 3,
            IssueCode::DuplicateStepId => 4,
            IssueCode::EmptyGroup => 5,
            IssueCode::GroupTooDeep => 6,
            IssueCode::MissingFunction => 7,
            IssueCode::InvalidFunctionName => 8,
            IssueCode::InvalidTerminal => 9,
            IssueCode::UnguardedValidation => 10,
            IssueCode::InvalidHaltOn => 11,
            IssueCode::GroupContinueOnError => 12,
            IssueCode::InvalidMapping => 13,
            IssueCode::NullMapping => 14,
            IssueCode::LoopIncrementTooSmall => 15,
            IssueCode::LoopBoundEmpty => 16,
            IssueCode::LoopCounterInvalid => 17,
            IssueCode::LoopSlotInvalid => 18,
            IssueCode::LoopItemWithoutOver => 19,
            IssueCode::LoopSlotCollision => 20,
            IssueCode::LoopOverInvalid => 21,
            IssueCode::InvalidForEach => 22,
            IssueCode::UnknownFunction => 23,
            IssueCode::MissingHandler => 24,
            IssueCode::InputParse => 25,
            IssueCode::TemplateCompile => 26,
            IssueCode::UnknownSecret => 27,
            IssueCode::SecretInMessageWrite => 28,
            IssueCode::InvalidSecretStore => 29,
            IssueCode::DuplicateTemplateKey => 30,
            IssueCode::EscapedTemplateKey => 31,
            IssueCode::ParseFailed => 32,
            IssueCode::ValidateFailed => 33,
            IssueCode::LikelyTypo => 34,
            IssueCode::UnknownKey => 35,
            IssueCode::V3Spelling => 36,
            IssueCode::PayloadVar => 37,
            IssueCode::OperatorInDocument => 38,
            IssueCode::SecretInDocument => 39,
            IssueCode::InvalidPath => 40,
            IssueCode::UnresolvedReference => 41,
        }
    }

    #[test]
    fn all_codes_lists_every_variant() {
        assert_eq!(ALL_CODES.len(), 42);
        let indices: HashSet<usize> = ALL_CODES.iter().copied().map(ordinal).collect();
        assert_eq!(indices, (0..ALL_CODES.len()).collect::<HashSet<_>>());
    }

    #[test]
    fn every_code_round_trips_through_its_string() {
        for code in ALL_CODES {
            assert_eq!(code.as_str().parse::<IssueCode>(), Ok(code), "{code:?}");
        }
        assert!("NOT_A_CODE".parse::<IssueCode>().is_err());
    }

    #[test]
    fn code_strings_are_distinct_and_screaming_snake() {
        let mut seen = HashSet::new();
        for code in ALL_CODES {
            assert!(seen.insert(code.as_str()), "duplicate string for {code:?}");
            assert_eq!(code.to_string(), code.as_str());
            assert!(
                code.as_str()
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "{code:?} is not SCREAMING_SNAKE"
            );
        }
    }

    #[test]
    fn the_advisory_and_defect_sets_are_exactly_these() {
        let by = |want: Severity| -> HashSet<&'static str> {
            ALL_CODES
                .iter()
                .filter(|c| c.severity() == want)
                .map(|c| c.as_str())
                .collect()
        };
        assert_eq!(
            by(Severity::Advisory),
            HashSet::from([
                "UNGUARDED_VALIDATION",
                "GROUP_CONTINUE_ON_ERROR",
                "ESCAPED_TEMPLATE_KEY",
                "NULL_MAPPING",
                "UNKNOWN_KEY",
                "V3_SPELLING",
                "OPERATOR_IN_DOCUMENT",
            ])
        );
        assert_eq!(
            by(Severity::Defect),
            HashSet::from([
                "MISSING_HANDLER",
                "LIKELY_TYPO",
                "PAYLOAD_VAR",
                "UNRESOLVED_REFERENCE",
            ])
        );
        assert_eq!(by(Severity::Rejected).len(), ALL_CODES.len() - 11);
        assert!(by(Severity::Rejected).contains("SECRET_IN_DOCUMENT"));
        assert!(by(Severity::Rejected).contains("INVALID_PATH"));
    }

    #[test]
    fn severities_have_distinct_strings() {
        let all = [Severity::Rejected, Severity::Defect, Severity::Advisory];
        let strings: HashSet<_> = all.iter().map(|s| s.as_str()).collect();
        assert_eq!(strings.len(), 3);
        assert_eq!(Severity::Defect.to_string(), "DEFECT");
    }

    #[test]
    fn an_authored_path_round_trips() {
        for text in [
            "tasks[1].tasks[0].id",
            "loop.setup[0].id",
            "function.input.fields.a",
            "[2]",
            "",
            "a[0][1].b",
        ] {
            let path: AuthoredPath = text.parse().unwrap();
            assert_eq!(path.to_string(), text);
            assert_eq!(path.is_root(), text.is_empty());
        }
        assert!("a..b".parse::<AuthoredPath>().is_err());
        assert!("a[x]".parse::<AuthoredPath>().is_err());
        assert!("a[1".parse::<AuthoredPath>().is_err());
    }

    #[test]
    fn path_building_and_serialization() {
        let mut p = AuthoredPath::root().join("tasks").join(1usize);
        assert_eq!(p.to_string(), "tasks[1]");
        let q = p.extend(&"function.name".parse().unwrap());
        assert_eq!(q.to_string(), "tasks[1].function.name");
        assert_eq!(p.pop(), Some(PathSeg::Index(1)));
        assert_eq!(
            serde_json::to_string(&q).unwrap(),
            "\"tasks[1].function.name\""
        );
    }

    #[test]
    fn an_issue_takes_its_codes_severity_and_displays() {
        let issue = Issue::new(IssueCode::UnknownKey, "tasks[0].x".parse().unwrap(), "m");
        assert_eq!(issue.severity, Severity::Advisory);
        assert_eq!(issue.to_string(), "tasks[0].x: m [UNKNOWN_KEY]");
        let root = Issue::new(IssueCode::NoTasks, AuthoredPath::root(), "m")
            .with_severity(Severity::Defect)
            .with_step(StepKind::Setup, Some("a"))
            .with_field("f");
        assert_eq!(root.severity, Severity::Defect);
        assert_eq!(root.step_kind, Some(StepKind::Setup));
        assert_eq!(root.step_id.as_deref(), Some("a"));
        assert_eq!(root.field.as_deref(), Some("f"));
        assert_eq!(root.to_string(), "m [NO_TASKS]");
    }
    #[test]
    fn a_key_it_could_not_otherwise_print_is_quoted() {
        for (key, shown) in [
            ("a.b", r#"["a.b"]"#),
            ("", r#"[""]"#),
            ("x[0]", r#"["x[0]"]"#),
            ("q\"t", r#"["q\"t"]"#),
        ] {
            let path = AuthoredPath::root().join(key);
            assert_eq!(path.to_string(), shown, "{key:?}");
            assert_eq!(shown.parse::<AuthoredPath>(), Ok(path), "{shown}");
            let deeper = AuthoredPath::root().join("log").join(key).join(0usize);
            let shown = format!("log{shown}[0]");
            assert_eq!(deeper.to_string(), shown, "{key:?}");
            assert_eq!(shown.parse::<AuthoredPath>(), Ok(deeper), "{shown}");
        }
    }

    #[test]
    fn a_quoted_key_round_trips_anywhere_in_a_path() {
        let text = r#"tasks[0].function.input.fields["a.b"]"#;
        let path: AuthoredPath = text.parse().unwrap();
        assert_eq!(path.segs().len(), 6);
        assert_eq!(path.segs()[5], PathSeg::Key("a.b".into()));
        assert_eq!(path.to_string(), text);

        let text = r#"["a.b"].c["]"]["\"\\"][3]"#;
        let path: AuthoredPath = text.parse().unwrap();
        assert_eq!(
            path.segs(),
            [
                PathSeg::Key("a.b".into()),
                PathSeg::Key("c".into()),
                PathSeg::Key("]".into()),
                PathSeg::Key("\"\\".into()),
                PathSeg::Index(3),
            ]
        );
        assert_eq!(path.to_string(), text);

        // A key quoted when it need not be reads back, and displays bare: a
        // backslash alone reads back unquoted.
        let path: AuthoredPath = r#"a["b"]["\\"]"#.parse().unwrap();
        assert_eq!(
            path.segs(),
            [
                PathSeg::Key("a".into()),
                PathSeg::Key("b".into()),
                PathSeg::Key("\\".into()),
            ]
        );
        assert_eq!(path.to_string(), r"a.b.\");
        assert_eq!(path.to_string().parse::<AuthoredPath>(), Ok(path));

        // An ordinary path displays unchanged.
        let ordinary = AuthoredPath::root()
            .join("tasks")
            .join(1usize)
            .join("function")
            .join("input");
        assert_eq!(ordinary.to_string(), "tasks[1].function.input");

        for bad in [
            r#"a.["b"]"#,
            r#"a["b"#,
            r#"a["b""#,
            r#"a["b"x]"#,
            r#"a[b"]"#,
            r#"a"b"#,
            r#"["\q"]"#,
        ] {
            assert!(bad.parse::<AuthoredPath>().is_err(), "{bad}");
        }
    }

    #[test]
    fn an_issue_serializes_with_its_parts_as_strings() {
        let issue = Issue::new(
            IssueCode::UnknownFunction,
            r#"tasks[0].function.input["a.b"]"#.parse().unwrap(),
            "no function named `x`",
        )
        .with_step(StepKind::Task, Some("t"))
        .with_field("a.b");
        let value = serde_json::to_value(&issue).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut want = [
            "code",
            "severity",
            "path",
            "step_kind",
            "step_id",
            "field",
            "message",
        ];
        want.sort_unstable();
        assert_eq!(keys, want);
        assert_eq!(value["code"], "UNKNOWN_FUNCTION");
        assert_eq!(value["severity"], "REJECTED");
        assert_eq!(value["path"], r#"tasks[0].function.input["a.b"]"#);
        assert_eq!(value["step_kind"], "task");
        assert_eq!(value["step_id"], "t");
        assert_eq!(value["field"], "a.b");
        assert_eq!(value["message"], "no function named `x`");
        assert_eq!(serde_json::from_value::<Issue>(value).unwrap(), issue);

        // The parts that are absent are `null`, and read back absent.
        let bare = Issue::new(IssueCode::NoTasks, AuthoredPath::root(), "m");
        let value = serde_json::to_value(&bare).unwrap();
        assert_eq!(value["step_kind"], Value::Null);
        assert_eq!(value["path"], "");
        assert_eq!(serde_json::from_value::<Issue>(value).unwrap(), bare);
    }

    #[test]
    fn the_parts_of_an_issue_read_back_from_their_strings() {
        for kind in [StepKind::Task, StepKind::Group, StepKind::Setup] {
            let value = serde_json::to_value(kind).unwrap();
            assert_eq!(value, kind.as_str());
            assert_eq!(serde_json::from_value::<StepKind>(value).unwrap(), kind);
        }
        for severity in [Severity::Rejected, Severity::Defect, Severity::Advisory] {
            let value = serde_json::to_value(severity).unwrap();
            assert_eq!(serde_json::from_value::<Severity>(value).unwrap(), severity);
        }
        for code in ALL_CODES {
            let value = serde_json::to_value(code).unwrap();
            assert_eq!(value, code.as_str());
            assert_eq!(serde_json::from_value::<IssueCode>(value).unwrap(), code);
        }
        assert!(serde_json::from_value::<StepKind>(json!("Task")).is_err());
        assert!(serde_json::from_value::<Severity>(json!("rejected")).is_err());
        assert!(serde_json::from_value::<IssueCode>(json!("NOT_A_CODE")).is_err());
        assert!(serde_json::from_value::<AuthoredPath>(json!("a..b")).is_err());
    }
}
