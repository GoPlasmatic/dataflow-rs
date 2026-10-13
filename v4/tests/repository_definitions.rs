extern crate dataflow_rs_v4 as dataflow_rs;

// Every workflow definition in the repository, collected from JSON files,
// markdown fences, Rust raw strings and pure-literal `json!` macros, and the
// UI's TypeScript samples, held to `def::parse`:
//
// * no definition gets a `LIKELY_TYPO` (R46), and
// * every definition from the docs, the READMEs, `examples/` and the UI's
//   samples loads, with no Rejected issue.
//
// A definition is any JSON object with a string `id` and an array `tasks`, at
// any depth. Once an object is taken as one the scan does not descend into it
// (its groups carry `id` and `tasks` too).
//
// Not read: `docs/theme/dataflow-playground.js`, whose samples are built by
// string concatenation around a JS variable, so they are not literals.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use dataflow_rs::def::{ParseOptions, StepDef, WorkflowDef, parse};
use dataflow_rs::expr::CompileCx;
use dataflow_rs::function::{ExprVisitor, FieldSpec, FunctionConfig};
use dataflow_rs::program::{CompileOptions, IssueCode, Severity};
use dataflow_rs::{Ctx, Function, Outcome, Registry};
use serde_json::Value;

const MINIMUM_DEFINITIONS: usize = 150;

/// Docs-class definitions that parse Rejected, as `(file, id, reason)`. None is
/// shown as a refusal: each is a sketch whose subject is another key and
/// whose `"tasks": []` is filler, which v3 refuses as `NO_TASKS` too (it is
/// in `authoring::refuse_authoring_issues`). Reported to the owner as a docs
/// finding; v4 is not at fault, so they are named here rather than fixed in
/// this task (nothing under `docs/` is edited in this phase).
const DELIBERATELY_REJECTED: &[(&str, &str, &str)] = &[
    (
        "corpus/README.md",
        "w",
        "FINDING: the case-format sketch carries `\"tasks\": []` as filler; NO_TASKS",
    ),
    (
        "docs/src/advanced/loops.md",
        "per_item",
        "FINDING: loop-shape sketch with `\"tasks\": [ ]`; NO_TASKS",
    ),
    (
        "docs/src/advanced/loops.md",
        "three_times",
        "FINDING: loop-shape sketch with `\"tasks\": [ ]`; NO_TASKS",
    ),
    (
        "docs/src/advanced/loops.md",
        "paginate",
        "FINDING: loop-shape sketch with `\"tasks\": [ ]`; NO_TASKS",
    ),
    (
        "docs/src/built-in-functions/validation.md",
        "exchange",
        "FINDING: condition sketch with `\"tasks\": []`; NO_TASKS",
    ),
];

/// The `LIKELY_TYPO` issues that are expected, as `(file, path)`: the break
/// case for unknown keys carries `conditon`, `maxx`, `ass`, `bucket_strat` and
/// `targte` on purpose, to pin what the rule reports. Each entry is consumed
/// by exactly one issue, so a further typo in that file, or the case ceasing
/// to fire one of these, fails the test.
const DELIBERATE_TYPOS: &[(&str, &str)] = &[
    ("corpus/cases/breaks/unknown_keys.json", "conditon"),
    ("corpus/cases/breaks/unknown_keys.json", "tasks[0].conditon"),
    ("corpus/cases/breaks/unknown_keys.json", "loop.maxx"),
    (
        "corpus/cases/breaks/unknown_keys.json",
        "tasks[0].for_each.ass",
    ),
    (
        "corpus/cases/breaks/unknown_keys.json",
        "rollout.bucket_strat",
    ),
    (
        "corpus/cases/breaks/unknown_keys.json",
        "tasks[0].function.input.targte",
    ),
];

/// Markdown `json` fences holding `"tasks"` that are not valid JSON and are
/// not `...`-elided, as `(file, reason)`. Empty today; anything else fails.
const SKIPPED_FENCES: &[(&str, &str)] = &[];

/// The UI-samples normaliser notes that are expected, as substrings. Empty:
/// every `workflows` array normalises.
const EXPECTED_TS_SKIPS: &[&str] = &[];

/// Per-source floors, set a little below today's counts (corpus 144, src 33,
/// tests 122, examples 13, wasm 9, UI 16, docs 14), so a scanner that goes
/// silent on one source cannot hide behind the others.
const GROUP_FLOORS: &[(&str, usize)] = &[
    ("json: corpus/cases", 130),
    ("rs: src", 25),
    ("rs: tests", 100),
    ("rs: examples", 10),
    ("rs: wasm", 6),
    ("ts: ui samples", 12),
    ("md: docs/src", 10),
    ("md: README.md", 1),
];

/// Directories never read.
const SKIPPED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "design-v4",
    ".superpowers",
    "pkg",
    "book",
    "dist",
];

// ---------------------------------------------------------------------------
// Finding definitions in a parsed value
// ---------------------------------------------------------------------------

fn is_definition(object: &serde_json::Map<String, Value>) -> bool {
    object.get("id").is_some_and(Value::is_string)
        && object.get("tasks").is_some_and(Value::is_array)
}

fn find_definitions(value: &Value, out: &mut Vec<Value>) {
    match value {
        Value::Object(object) => {
            if is_definition(object) {
                out.push(value.clone());
            } else {
                object.values().for_each(|v| find_definitions(v, out));
            }
        }
        Value::Array(items) => items.iter().for_each(|v| find_definitions(v, out)),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Text scanning
// ---------------------------------------------------------------------------

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The end of a `"…"` string starting at `i` (the index after the closing
/// quote), honouring backslash escapes.
fn skip_string(b: &[u8], mut i: usize, quote: u8) -> usize {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
}

/// The end of a Rust raw string starting at `i` (at the `r`), if one starts
/// there.
fn raw_string(b: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    if b.get(i) != Some(&b'r') || (i > 0 && is_ident(b[i - 1]) && b[i - 1] != b'b') {
        return None;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'#') {
        j += 1;
    }
    if b.get(j) != Some(&b'"') {
        return None;
    }
    let hashes = j - i - 1;
    let body_start = j + 1;
    let mut k = body_start;
    while k < b.len() {
        if b[k] == b'"'
            && b.get(k + 1..k + 1 + hashes)
                .is_some_and(|s| s.iter().all(|&c| c == b'#'))
        {
            return Some((body_start, k, k + 1 + hashes));
        }
        k += 1;
    }
    None
}

/// The index after a Rust char literal starting at `i`, or `None` if the
/// quote is a lifetime.
fn char_literal(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i + 1) == Some(&b'\\') {
        let mut j = i + 3;
        while j < b.len() && b[j] != b'\'' {
            j += 1;
        }
        return Some((j + 1).min(b.len()));
    }
    // A (possibly multi-byte) char, then the closing quote.
    let rest = std::str::from_utf8(&b[i + 1..]).ok().or_else(|| {
        let end = (i + 6).min(b.len());
        (i + 1..=end)
            .rev()
            .find_map(|e| std::str::from_utf8(&b[i + 1..e]).ok())
    })?;
    let c = rest.chars().next()?;
    let after = i + 1 + c.len_utf8();
    (b.get(after) == Some(&b'\'')).then_some(after + 1)
}

/// The index just past the delimiter that closes the one at `start`, skipping
/// string literals and comments. `rust` also skips raw strings and char
/// literals; otherwise `'` opens a string, as in TypeScript.
fn balanced_end(text: &str, start: usize, rust: bool) -> Option<usize> {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' => i = skip_string(b, i, b'"'),
            b'\'' if !rust => i = skip_string(b, i, b'\''),
            b'\'' => i = char_literal(b, i).unwrap_or(i + 1),
            b'r' if rust => match raw_string(b, i) {
                Some((_, _, end)) => i = end,
                None => i += 1,
            },
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            }
            _ => i += 1,
        }
    }
    None
}

/// Removes every comma that is followed, past whitespace, by `}` or `]`,
/// leaving string literals alone.
fn strip_trailing_commas(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        match b[i] {
            b'"' => i = skip_string(b, i, b'"'),
            b',' => {
                let next = b[i + 1..].iter().position(|c| !c.is_ascii_whitespace());
                if next.is_some_and(|n| matches!(b[i + 1 + n], b'}' | b']')) {
                    out.push_str(&text[last..i]);
                    last = i + 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[last..]);
    out
}

fn parse_literal(body: &str) -> Option<Value> {
    serde_json::from_str(&strip_trailing_commas(body)).ok()
}

/// A candidate JSON text found in a Rust source: its line, and its body.
struct Literal {
    line: usize,
    body: String,
}

fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset]
        .iter()
        .filter(|&&c| c == b'\n')
        .count()
        + 1
}

/// Every raw string and every `json!( … )` body in a Rust source.
fn rust_literals(text: &str) -> Vec<Literal> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            }
            b'"' => i = skip_string(b, i, b'"'),
            b'\'' => i = char_literal(b, i).unwrap_or(i + 1),
            b'r' => match raw_string(b, i) {
                Some((from, to, end)) => {
                    out.push(Literal {
                        line: line_of(text, i),
                        body: text[from..to].to_string(),
                    });
                    i = end;
                }
                None => i += 1,
            },
            b'j' if text[i..].starts_with("json!") && (i == 0 || !is_ident(b[i - 1])) => {
                let open = i + 5;
                let delim = b[open..]
                    .iter()
                    .position(|c| !c.is_ascii_whitespace())
                    .map(|n| open + n);
                match delim {
                    Some(d) if matches!(b[d], b'(' | b'[' | b'{') => {
                        match balanced_end(text, d, true) {
                            Some(end) => {
                                // `json!({ … })` and `json!([ … ])`: the body
                                // is what sits inside the macro's delimiters.
                                out.push(Literal {
                                    line: line_of(text, i),
                                    body: text[d + 1..end - 1].to_string(),
                                });
                                i = end;
                            }
                            None => i += 5,
                        }
                    }
                    _ => i += 5,
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// The values the pure-literal Rust literals of a source parse to.
fn rust_values(text: &str) -> Vec<(usize, Value)> {
    rust_literals(text)
        .into_iter()
        .filter_map(|l| parse_literal(&l.body).map(|v| (l.line, v)))
        .collect()
}

/// The bodies of the ```` ```json ```` fences of a markdown file, with the
/// line each starts on.
fn markdown_fences(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (n, line) in text.lines().enumerate() {
        let t = line.trim_start();
        match &mut open {
            Some((start, body)) => {
                if t.starts_with("```") {
                    out.push((*start, std::mem::take(body)));
                    open = None;
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
            None => {
                if let Some(info) = t.strip_prefix("```")
                    && info.split([',', ' ', '{']).next() == Some("json")
                {
                    open = Some((n + 2, String::new()));
                } else if t.starts_with("```") {
                    // A fence of another language: skip to its close.
                    open = Some((usize::MAX, String::new()));
                }
            }
        }
    }
    out.retain(|(start, _)| *start != usize::MAX);
    out
}

// ---------------------------------------------------------------------------
// TypeScript object literals
// ---------------------------------------------------------------------------

/// Rewrites a TypeScript object literal into JSON: unquoted identifier keys
/// are quoted, single-quoted strings become JSON strings, comments and
/// trailing commas are dropped. `Err` for anything else (a bare identifier
/// that is not a key, an unknown escape).
fn normalise_ts(text: &str) -> Result<String, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            '\'' | '"' => {
                let quote = c;
                let mut decoded = String::new();
                i += 1;
                loop {
                    let Some(&d) = chars.get(i) else {
                        return Err("unterminated string".into());
                    };
                    i += 1;
                    if d == quote {
                        break;
                    }
                    if d != '\\' {
                        decoded.push(d);
                        continue;
                    }
                    let e = *chars.get(i).ok_or("dangling escape")?;
                    i += 1;
                    match e {
                        'n' => decoded.push('\n'),
                        't' => decoded.push('\t'),
                        'r' => decoded.push('\r'),
                        'b' => decoded.push('\u{8}'),
                        'f' => decoded.push('\u{c}'),
                        '\\' | '\'' | '"' | '/' => decoded.push(e),
                        'u' => {
                            let hex: String =
                                chars.get(i..i + 4).ok_or("short \\u")?.iter().collect();
                            let code = u32::from_str_radix(&hex, 16).map_err(|e| e.to_string())?;
                            decoded.push(char::from_u32(code).ok_or("surrogate in \\u")?);
                            i += 4;
                        }
                        other => return Err(format!("unknown escape \\{other}")),
                    }
                }
                out.push_str(&serde_json::to_string(&decoded).map_err(|e| e.to_string())?);
            }
            '}' | ']' => {
                let kept = out.trim_end().len();
                out.truncate(kept);
                if out.ends_with(',') {
                    out.pop();
                }
                out.push(c);
                i += 1;
            }
            c if c.is_ascii_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let mut j = i;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if chars.get(j) == Some(&':') {
                    out.push_str(&serde_json::to_string(&word).map_err(|e| e.to_string())?);
                } else if matches!(word.as_str(), "true" | "false" | "null") {
                    out.push_str(&word);
                } else {
                    return Err(format!("bare identifier `{word}`"));
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// The values of every `workflows: [ … ]` array in a TypeScript source, and
/// a note for each that could not be normalised.
fn ts_workflow_arrays(text: &str) -> (Vec<(usize, Value)>, Vec<String>) {
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("workflows:") {
        let key = from + at;
        from = key + "workflows:".len();
        let Some(open) = text[from..]
            .find(|c: char| !c.is_whitespace())
            .map(|n| from + n)
            .filter(|&o| text.as_bytes()[o] == b'[')
        else {
            // `workflows: Workflow[]` in a type annotation.
            continue;
        };
        let line = line_of(text, key);
        let Some(end) = balanced_end(text, open, false) else {
            skipped.push(format!("line {line}: unbalanced"));
            continue;
        };
        match normalise_ts(&text[open..end]).and_then(|json| {
            serde_json::from_str::<Value>(&json)
                .map_err(|e| format!("not JSON after normalising: {e}"))
        }) {
            Ok(v) => found.push((line, v)),
            Err(e) => skipped.push(format!("line {line}: {e}")),
        }
        from = end;
    }
    (found, skipped)
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

struct Found {
    /// `path:line` (or just `path`).
    at: String,
    /// The file, relative to the repository root.
    file: String,
    /// What the count is grouped under.
    group: String,
    /// Held to "loads", not only to the typo rule.
    strict: bool,
    def: Value,
}

fn root() -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/..")).to_path_buf()
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) {
                walk(&path, ext, out);
            }
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

fn files_under(dirs: &[&str], ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in dirs {
        walk(&root().join(d), ext, &mut out);
    }
    out
}

fn rel(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn add(
    found: &mut Vec<Found>,
    file: &str,
    line: Option<usize>,
    group: &str,
    strict: bool,
    v: &Value,
) {
    let mut defs = Vec::new();
    find_definitions(v, &mut defs);
    for def in defs {
        found.push(Found {
            at: line.map_or_else(|| file.to_string(), |l| format!("{file}:{l}")),
            file: file.to_string(),
            group: group.to_string(),
            strict,
            def,
        });
    }
}

/// Everything the repository holds, and a note for each part not read.
fn collect() -> (Vec<Found>, Vec<String>, Vec<String>) {
    let mut problems = Vec::new();
    let mut found = Vec::new();
    let mut notes = vec![
        "excluded: docs/theme/dataflow-playground.js (samples built by string concatenation, not literals)"
            .to_string(),
    ];

    // JSON files.
    for path in files_under(
        &[
            "examples",
            "tests",
            "docs",
            "ui/src",
            "spike",
            "corpus/cases",
        ],
        "json",
    ) {
        let file = rel(&path);
        if file.ends_with(".v3.json") {
            continue;
        }
        let corpus = file.starts_with("corpus/cases/");
        let group = if corpus {
            "json: corpus/cases".to_string()
        } else {
            format!("json: {}", file.split('/').next().unwrap_or(""))
        };
        match fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            Some(v) => add(
                &mut found,
                &file,
                None,
                &group,
                !corpus && !file.starts_with("tests/"),
                &v,
            ),
            None => notes.push(format!("not parsed as JSON: {file}")),
        }
    }

    // Markdown.
    let mut markdown: Vec<PathBuf> = [
        "README.md",
        "CHANGELOG.md",
        "CONTRIBUTING.md",
        "CLAUDE.md",
        "ui/README.md",
        "wasm/README.md",
        "corpus/README.md",
    ]
    .iter()
    .map(|f| root().join(f))
    .collect();
    walk(&root().join("docs/src"), "md", &mut markdown);
    let mut elided = 0;
    for path in markdown {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let file = rel(&path);
        let group = if file.starts_with("docs/src") {
            "md: docs/src".to_string()
        } else {
            format!("md: {file}")
        };
        for (line, body) in markdown_fences(&text) {
            match serde_json::from_str::<Value>(&body) {
                Ok(v) => add(
                    &mut found,
                    &file,
                    Some(line),
                    &group,
                    file != "CLAUDE.md",
                    &v,
                ),
                Err(_) => {
                    // Fragments, `...` elisions and comments; counted only
                    // when one would have held a definition.
                    if body.contains("\"tasks\"") {
                        if body.contains("...") {
                            elided += 1;
                        } else if !SKIPPED_FENCES.iter().any(|(f, _)| *f == file) {
                            problems.push(format!(
                                "json fence with tasks that is not valid JSON: {file}:{line}"
                            ));
                        }
                    }
                }
            }
        }
    }
    notes.push(format!(
        "skipped {elided} json fences that elide with `...` (fragments, not definitions)"
    ));

    // Rust sources: raw strings and pure-literal `json!` bodies.
    for path in files_under(
        &["src", "tests", "examples", "docs-tests", "wasm", "corpus"],
        "rs",
    ) {
        let file = rel(&path);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let area = file.split('/').next().unwrap_or("");
        let strict = area == "examples";
        for (line, v) in rust_values(&text) {
            add(
                &mut found,
                &file,
                Some(line),
                &format!("rs: {area}"),
                strict,
                &v,
            );
        }
    }

    // The UI's samples.
    let ts = root().join("ui/src/data/sampleWorkflows.ts");
    if let Ok(text) = fs::read_to_string(&ts) {
        let file = rel(&ts);
        let (arrays, skipped) = ts_workflow_arrays(&text);
        for s in skipped {
            if EXPECTED_TS_SKIPS.iter().any(|e| s.contains(e)) {
                notes.push(format!("skipped in {file}: {s}"));
            } else {
                problems.push(format!("not normalised in {file}: {s}"));
            }
        }
        for (line, v) in arrays {
            add(&mut found, &file, Some(line), "ts: ui samples", true, &v);
        }
    }

    (found, notes, problems)
}

#[test]
fn every_definition_in_the_repository_parses_with_no_likely_typo() {
    let (found, notes, problems) = collect();

    let mut per_group: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &found {
        *per_group.entry(&f.group).or_default() += 1;
    }
    println!("definitions per source:");
    for (group, n) in &per_group {
        println!("  {n:4}  {group}");
    }
    println!("  {:4}  total", found.len());
    for note in &notes {
        println!("note: {note}");
    }

    assert!(
        problems.is_empty(),
        "parts of the repository could not be read:\n{}",
        problems.join("\n")
    );
    for (group, floor) in GROUP_FLOORS {
        let n = per_group.get(group).copied().unwrap_or(0);
        assert!(n >= *floor, "{group}: found {n} definitions, floor {floor}");
    }
    assert!(
        found.len() >= MINIMUM_DEFINITIONS,
        "the scan found {} definitions, fewer than {MINIMUM_DEFINITIONS}",
        found.len()
    );

    let mut typos = Vec::new();
    let mut rejected = Vec::new();
    let mut allowed = vec![false; DELIBERATELY_REJECTED.len()];
    let mut typo_used = vec![false; DELIBERATE_TYPOS.len()];
    for f in &found {
        let parsed = parse(&f.def, ParseOptions::default());
        for issue in &parsed.issues {
            if issue.code != IssueCode::LikelyTypo {
                continue;
            }
            let path = issue.path.to_string();
            let expected = DELIBERATE_TYPOS
                .iter()
                .enumerate()
                .position(|(n, (file, p))| !typo_used[n] && *file == f.file && *p == path);
            if let Some(n) = expected {
                typo_used[n] = true;
            } else {
                typos.push(format!(
                    "{}: {} at {}",
                    f.at,
                    issue.code.as_str(),
                    issue.path
                ));
            }
        }
        if f.strict && parsed.def.is_none() {
            let id = f.def["id"].as_str().unwrap_or_default();
            if let Some(n) = DELIBERATELY_REJECTED
                .iter()
                .position(|(file, aid, _)| *file == f.file && *aid == id)
            {
                allowed[n] = true;
                continue;
            }
            for issue in parsed
                .issues
                .iter()
                .filter(|i| i.severity == Severity::Rejected)
            {
                rejected.push(format!(
                    "{}: workflow `{id}`: {} at {}",
                    f.at,
                    issue.code.as_str(),
                    issue.path
                ));
            }
        }
    }

    assert!(
        typos.is_empty(),
        "LIKELY_TYPO in the repository:\n{}",
        typos.join("\n")
    );
    assert!(
        rejected.is_empty(),
        "Rejected docs, README, example or UI definitions:\n{}",
        rejected.join("\n")
    );
    for (n, used) in typo_used.iter().enumerate() {
        assert!(
            used,
            "expected typo {:?} was not reported",
            DELIBERATE_TYPOS[n]
        );
    }
    for (n, used) in allowed.iter().enumerate() {
        assert!(
            used,
            "allowlist entry {:?} matches no definition",
            DELIBERATELY_REJECTED[n]
        );
    }
}

// ---------------------------------------------------------------------------
// Every definition compiles
// ---------------------------------------------------------------------------

/// Compile findings that are expected, as `(file, workflow id, code, path,
/// reason)`. Each is a definition v3 refuses too, a break the design numbers,
/// or a secret the defining test declares in a store this compiler lacks.
/// Every row must be consumed.
const COMPILE_ALLOWLIST: &[(&str, &str, &str, &str, &str)] = &[
    (
        "corpus/cases/breaks/path_outside_roots.json",
        "w",
        "INVALID_PATH",
        "tasks[0].function.input.mappings[0].path",
        "break 15: a literal path outside the three roots is refused at compile; v3 wrote a new top-level key",
    ),
    (
        "corpus/cases/builtins/map_mode.json",
        "no_logic",
        "INVALID_MAPPING",
        "tasks[0].function.input.mappings[0].logic",
        "v3 refuses too: a mapping needs `logic` or `unset` (recorded as a parse error)",
    ),
    (
        "corpus/cases/builtins/map_mode.json",
        "root",
        "INVALID_MAPPING",
        "tasks[0].function.input.mappings[0].path",
        "v3 refuses too: a context root cannot be appended to",
    ),
    (
        "corpus/cases/builtins/parse_invalid.json",
        "no_target",
        "INPUT_PARSE",
        "tasks[0].function.input.target",
        "v3 refuses too: `parse_json` config is missing `target`",
    ),
    (
        "corpus/cases/expressions/dollar_escape_refused.json",
        "w",
        "DUPLICATE_TEMPLATE_KEY",
        "tasks[0].function.input.mappings[0].logic.a",
        "v3 refuses too: `$a` and `a` emit one key twice (build error)",
    ),
    (
        "corpus/cases/expressions/payload_not_in_context.json",
        "w",
        "PAYLOAD_VAR",
        "tasks[0].function.input.mappings[0].logic",
        "break 9: a literal `payload` path is a Defect; v3 read null",
    ),
    (
        "corpus/cases/expressions/secrets_refused.json",
        "w",
        "SECRET_IN_MESSAGE_WRITE",
        "tasks[0].function.input.mappings[0].logic",
        "v3 refuses too: a secret in a recorded expression (build error)",
    ),
    (
        "tests/for_each.rs",
        "w",
        "SECRET_IN_MESSAGE_WRITE",
        "tasks[0].for_each.over",
        "v3 refuses too: its test `check_workflow_refuses_a_secret_in_over`",
    ),
    (
        "tests/loop_over.rs",
        "w",
        "SECRET_IN_MESSAGE_WRITE",
        "loop.over",
        "v3 refuses too: its test pins a secret in `over` as refused",
    ),
    (
        "tests/secrets.rs",
        "w",
        "UNKNOWN_SECRET",
        "condition",
        "v3 refuses too: the undeclared `nope_wf`, reported by its test",
    ),
    (
        "tests/secrets.rs",
        "w",
        "UNKNOWN_SECRET",
        "tasks[0].condition",
        "v3 refuses too: the undeclared `nope_task`",
    ),
    (
        "tests/secrets.rs",
        "w",
        "UNKNOWN_SECRET",
        "tasks[0].function.input.rules[0].logic",
        "v3 refuses too: the undeclared `nope_rule`",
    ),
    (
        "tests/secrets.rs",
        "w",
        "UNKNOWN_SECRET",
        "tasks[1].condition",
        "v3 refuses too: the undeclared `nope_group`",
    ),
    (
        "tests/secrets.rs",
        "w",
        "UNKNOWN_SECRET",
        "tasks[2].function.input.condition",
        "v3 refuses too: the undeclared `nope_filter`",
    ),
    (
        "tests/secrets_isolation.rs",
        "failing",
        "UNKNOWN_SECRET",
        "tasks[0].function.input.rules[0].logic",
        "declared in that test's store; the compiler here holds no secrets",
    ),
    (
        "tests/secrets_isolation.rs",
        "w",
        "UNKNOWN_SECRET",
        "condition",
        "v3 refuses too: the undeclared `nope`",
    ),
    (
        "tests/secrets_isolation.rs",
        "w",
        "SECRET_IN_MESSAGE_WRITE",
        "tasks[0].function.input.mappings[0].logic",
        "v3 refuses too: a secret written by `map`",
    ),
    (
        "tests/secrets_isolation.rs",
        "leak",
        "SECRET_IN_MESSAGE_WRITE",
        "tasks[0].function.input.mappings[0].logic",
        "v3 refuses too: a reload cannot smuggle in a secret write",
    ),
    (
        "wasm/tests/web.rs",
        "gated",
        "UNKNOWN_SECRET",
        "condition",
        "declared by `with_secrets` in that test; `new` refuses it without a store, as here",
    ),
];

/// A config that accepts any input and visits nothing.
struct AcceptsAll;

impl FunctionConfig for AcceptsAll {
    fn compile(_input: &Value, _cx: &mut CompileCx<'_>) -> Option<Self> {
        Some(Self)
    }

    fn visit(&self, _v: &mut dyn ExprVisitor) {}

    fn fields() -> &'static [FieldSpec] {
        &[]
    }
}

/// A host function standing in for whatever a definition calls.
struct Stub;

impl Function for Stub {
    type Config = AcceptsAll;

    async fn call(&self, _ctx: &mut Ctx<'_>, _cfg: &AcceptsAll) -> dataflow_rs::Result<Outcome> {
        Ok(Outcome::Ok)
    }
}

#[test]
fn every_repository_definition_compiles() {
    let (found, _notes, problems) = collect();
    assert!(problems.is_empty(), "{}", problems.join("\n"));

    // The stubs' names come from the parsed definitions, where the compat
    // reader has rewritten v3's `validate` to `validation`: a raw name could
    // be one `register` reserves.
    let parsed: Vec<(&Found, Option<WorkflowDef>)> = found
        .iter()
        .map(|f| (f, parse(&f.def, ParseOptions::default()).def))
        .collect();
    let names: BTreeSet<&str> = parsed
        .iter()
        .filter_map(|(_, def)| def.as_ref())
        .flat_map(|def| def.walk())
        .filter_map(|(_, _, step)| match step {
            StepDef::Task(task) => Some(task.function.name.as_str()),
            StepDef::Group(_) => None,
        })
        .collect();
    let mut registry = Registry::default();
    let builtins: BTreeSet<String> = registry
        .catalogue()
        .filter(|i| i.builtin)
        .map(|i| i.name.to_string())
        .collect();
    for name in names.into_iter().filter(|name| !builtins.contains(*name)) {
        registry.register(name, Stub);
    }
    let compiler = registry.compiler(CompileOptions::default());

    let mut per_group: BTreeMap<&str, usize> = BTreeMap::new();
    let mut compiled = 0;
    // The one definition calling v3's `validate`: it compiles as `validation`.
    let alias = "examples/async_migration.rs";
    let mut alias_compiled = false;
    let mut failures = Vec::new();
    let mut used = vec![false; COMPILE_ALLOWLIST.len()];
    for (f, def) in &parsed {
        let Some(def) = def else { continue };
        let id = def.id.clone();
        *per_group.entry(&f.group).or_default() += 1;
        let issues = match compiler.compile_one(def) {
            Ok(c) => {
                compiled += 1;
                alias_compiled |= f.file == alias;
                c.issues
            }
            Err(issues) => issues,
        };
        for issue in issues
            .iter()
            .filter(|i| matches!(i.severity, Severity::Rejected | Severity::Defect))
        {
            let path = issue.path.to_string();
            let code = issue.code.as_str();
            let hit = COMPILE_ALLOWLIST.iter().position(|(file, wid, c, p, _)| {
                *file == f.file && *wid == id && *c == code && *p == path
            });
            match hit {
                Some(n) => used[n] = true,
                None => failures.push(format!("{}: workflow `{id}`: {code} at {path}", f.at)),
            }
        }
    }

    println!("compiled {compiled} definitions cleanly; checked per source:");
    for (group, n) in &per_group {
        println!("  {n:4}  {group}");
    }
    assert!(
        failures.is_empty(),
        "definitions that do not compile:\n{}",
        failures.join("\n")
    );
    for (group, floor) in GROUP_FLOORS {
        let n = per_group.get(group).copied().unwrap_or(0);
        assert!(n >= *floor, "{group}: checked {n}, floor {floor}");
    }
    assert!(compiled >= MINIMUM_DEFINITIONS);
    assert!(alias_compiled, "{alias}'s definition does not compile");
    for (n, u) in used.iter().enumerate() {
        assert!(
            u,
            "allowlist entry {:?} matches nothing",
            COMPILE_ALLOWLIST[n]
        );
    }
}

// ---------------------------------------------------------------------------
// The helpers' own tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod helpers {
    use super::*;
    use serde_json::json;

    fn ts(text: &str) -> Value {
        serde_json::from_str(&normalise_ts(text).unwrap()).unwrap()
    }

    #[test]
    fn identifier_keys_are_quoted() {
        assert_eq!(
            ts("{ a: 1, bB_2: [true, null] }"),
            json!({"a": 1, "bB_2": [true, null]})
        );
    }

    #[test]
    fn single_quoted_strings_are_re_escaped() {
        assert_eq!(
            ts(r#"{ k: 'it\'s "x"\nA', 'q': "d\"q" }"#),
            json!({"k": "it's \"x\"\nA", "q": "d\"q"})
        );
    }

    #[test]
    fn comments_and_trailing_commas_are_dropped() {
        let text = "{ // line\n a: [1, 2, /* in */ ], /* c */ b: { c: 3, }, }";
        assert_eq!(ts(text), json!({"a": [1, 2], "b": {"c": 3}}));
    }

    #[test]
    fn strings_keep_comment_lookalikes_and_colons() {
        assert_eq!(
            ts("{ a: 'http://x // y', b: 'k: v, ]' }"),
            json!({"a": "http://x // y", "b": "k: v, ]"})
        );
    }

    #[test]
    fn a_bare_identifier_value_is_refused() {
        assert!(normalise_ts("{ a: someVar }").is_err());
        assert!(normalise_ts("{ a: 'x\\q' }").is_err());
        assert!(normalise_ts("{ a: 'open }").is_err());
    }

    #[test]
    fn ts_arrays_are_extracted_and_type_annotations_ignored() {
        let text = "type T = { workflows: Workflow[]; }\nconst s = { workflows: [ { id: 'a', tasks: [], }, ], payload: {} };\nconst bad = { workflows: [ { id: f() } ] };";
        let (found, skipped) = ts_workflow_arrays(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 2);
        assert_eq!(skipped.len(), 1);
        let mut defs = Vec::new();
        find_definitions(&found[0].1, &mut defs);
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn json_macro_bodies_are_balanced_and_string_aware() {
        let src = r#"
            let a = json!({ "id": "w", "tasks": [], "n": ")}" , });
            let b = serde_json::json!([1, {"k": "}"},]);
            let c = json!({ "k": value });
        "#;
        let values = rust_values(src);
        assert_eq!(values.len(), 2, "the interpolating literal is skipped");
        assert_eq!(values[0].1["n"], json!(")}"));
        assert_eq!(values[1].1, json!([1, {"k": "}"}]));
        assert_eq!(values[0].0, 2);
    }

    #[test]
    fn json_macro_ignores_comments_chars_and_lifetimes() {
        let src = "fn f<'a>(x: &'a str) { let c = '}'; // json!({\"id\": 1})\n let v = json!({\"a\": 1}); }";
        let values = rust_values(src);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].1, json!({"a": 1}));
    }

    #[test]
    fn raw_strings_of_any_hash_count() {
        let src = "let a = r#\"{\"a\": \"q\\\"\"}\"#; let b = r\"[1,2]\"; let c = r##\"{\"h\": \"#\\\"#\"}\"##; let d = br#\"{}\"#;";
        let values: Vec<Value> = rust_values(src).into_iter().map(|(_, v)| v).collect();
        assert_eq!(
            values,
            vec![
                json!({"a": "q\""}),
                json!([1, 2]),
                json!({"h": "#\"#"}),
                json!({}),
            ]
        );
    }

    #[test]
    fn a_raw_string_that_is_not_json_is_skipped() {
        assert!(rust_values("let a = r#\"not json\"#;").is_empty());
    }

    #[test]
    fn trailing_commas_are_stripped_outside_strings_only() {
        assert_eq!(
            strip_trailing_commas(r#"{"a": ",}", "b": [1, ],}"#),
            r#"{"a": ",}", "b": [1 ]}"#
        );
    }

    #[test]
    fn markdown_json_fences_only() {
        let md =
            "x\n```json\n{\"a\": 1}\n```\n```rust\nlet x = 1;\n```\n  ```json,ignore\n[2]\n  ```\n";
        let f = markdown_fences(md);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].0, 3);
        assert_eq!(f[0].1.trim(), "{\"a\": 1}");
        assert_eq!(f[1].1.trim(), "[2]");
    }

    #[test]
    fn definitions_are_found_at_depth_and_not_inside_each_other() {
        let v = json!({"cases": [{"workflows": [
            {"id": "w", "tasks": [{"id": "g", "tasks": []}]},
            {"id": "x", "tasks": {}},
        ]}]});
        let mut out = Vec::new();
        find_definitions(&v, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], "w");
    }
}
