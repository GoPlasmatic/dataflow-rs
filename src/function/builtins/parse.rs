//! `parse_json`'s and `parse_xml`'s config: where to read, and where under
//! `data` to write.

use super::paths::{DataPath, ParseFrom, ParseSource};
use super::xml::{validation_error, xml_to_value};
use crate::base::{Result, keep_last};
use crate::expr::path;
use crate::function::{FunctionConfig, Outcome, SyncCtx};
use crate::state::RunContext;
use ::log::debug;
use datavalue::OwnedDataValue;
use std::borrow::Cow;

/// `parse_json`'s and `parse_xml`'s input.
#[derive(Debug, FunctionConfig)]
pub(crate) struct ParseConfig {
    /// What to read: `payload`, `payload.<path>`, a context path, or
    /// JSONLogic computing one of those.
    #[config(records)]
    pub source: ParseSource,
    /// Where the parsed value is written, relative to `data`.
    #[config(records)]
    pub target: DataPath,
}

/// The value `from` names, if there is one: the payload, a path into it, or
/// a context path read as the writes so far left it.
fn lookup<'a>(
    payload: &'a OwnedDataValue,
    rc: &'a RunContext<'_, '_>,
    from: &ParseFrom,
) -> Option<Cow<'a, OwnedDataValue>> {
    match from {
        ParseFrom::Payload => Some(Cow::Borrowed(payload)),
        // The path's first segment is the `data` root standing for the payload.
        ParseFrom::PayloadPath(p) => path::get(payload, &p.segs()[1..]).map(Cow::Borrowed),
        ParseFrom::Context(p) => rc.get(p),
    }
}

impl ParseConfig {
    /// `parse_json`: the source's value, which a string is parsed as JSON
    /// when it is JSON and kept when it is not, written at the target. A
    /// parsed object holding a key twice keeps its first position and its
    /// last value, as 3.x's `serde_json::Value` read it.
    pub(crate) fn json<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let from = self.source.resolve(cx)?;
        let target = self.target.path(cx)?;
        debug!("ParseJson: Extracting from '{}' to '{target}'", from.text());
        // A value converted from the view is moved, and only a borrowed one
        // is copied.
        let (value, hint) = match lookup(cx.payload, cx.rc, &from) {
            None => (OwnedDataValue::Null, None),
            Some(found) => {
                let parsed = match &*found {
                    OwnedDataValue::String(s) => {
                        let mut json = OwnedDataValue::from_json(s).ok();
                        json.iter_mut().for_each(keep_last);
                        Some((json, s.len()))
                    }
                    _ => None,
                };
                match parsed {
                    Some((Some(json), len)) => (json, Some(len)),
                    // Not JSON: the string as it is.
                    Some((None, len)) => (found.into_owned(), Some(len)),
                    None => (found.into_owned(), None),
                }
            }
        };
        cx.write(&target, value, hint);
        debug!("Parse: Successfully stored data to '{target}'");
        Ok(Outcome::Ok)
    }

    /// `parse_xml`: the source's string read as an XML document, its
    /// elements in document order, written at the target.
    ///
    /// # Errors
    ///
    /// `VALIDATION_ERROR` when the source is not a string, or is not a
    /// document.
    pub(crate) fn xml<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let from = self.source.resolve(cx)?;
        debug!("ParseXml: Extracting from '{}'", from.text());
        let (parsed, len) = match lookup(cx.payload, cx.rc, &from).as_deref() {
            Some(OwnedDataValue::String(xml)) => (xml_to_value(xml)?, xml.len()),
            _ => {
                return Err(validation_error(format_args!(
                    "ParseXml: Source '{}' is not a string",
                    from.text()
                )));
            }
        };
        let target = self.target.path(cx)?;
        cx.write(&target, parsed, Some(len));
        debug!("Parse: Successfully stored data to '{target}'");
        Ok(Outcome::Ok)
    }
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::one_task::{Golden, Observed, context_text_after, task};
    use serde_json::{Value, json};

    fn context() -> Value {
        json!({
            "data": {
                "text": "{\"k\": [1, 2]}", "plain": "not json", "num": 7, "obj": {"a": 1},
                "xml": "<r><a>1</a><b>x</b></r>", "name": "payload", "slot": "dest",
                "payloads": {"x": "{\"z\": 1}"}, "broken": "<a><b></a>"
            },
            "metadata": {"m": "[true]"},
            "temp_data": {"t": {"deep": 2}}
        })
    }

    fn payload() -> Value {
        json!({"x": {"y": [10, 20]}, "s": "{\"inner\": true}", "n": null, "list": [1, 2, 3]})
    }

    /// `parse_json` from `source` to `target`, its whole observation held
    /// by `golden`.
    fn json_parse(golden: &mut Golden, source: Value, target: Value) -> Observed {
        golden.observe(
            "parse_json",
            &json!({"source": source, "target": target}),
            &payload(),
            &context(),
        )
    }

    #[test]
    fn parse_json_reads_the_payload_a_payload_path_or_a_context_path() {
        let mut golden = Golden::new(
            "parse",
            "parse_json_reads_the_payload_a_payload_path_or_a_context_path",
        );
        for source in [
            "payload",
            "payload.x",
            "payload.x.y",
            "payload.list.1",
            "payload.s",
            "payload.n",
            "payload.missing",
            "data.obj",
            "data.num",
            "data.missing",
            "metadata.m",
            "temp_data.t.deep",
        ] {
            let observed = json_parse(&mut golden, json!(source), json!("out"));
            assert_eq!(observed.status, Some(200), "{source}");
        }
    }

    #[test]
    fn a_string_source_is_parsed_when_it_is_json_and_kept_when_it_is_not() {
        let mut golden = Golden::new(
            "parse",
            "a_string_source_is_parsed_when_it_is_json_and_kept_when_it_is_not",
        );
        let parsed = json_parse(&mut golden, json!("data.text"), json!("out"));
        assert!(
            parsed.context.contains(r#""out":{"k":[1,2]}"#),
            "{}",
            parsed.context
        );
        let kept = json_parse(&mut golden, json!("data.plain"), json!("out"));
        assert!(
            kept.context.contains(r#""out":"not json""#),
            "{}",
            kept.context
        );
        // The change records the parsed value.
        assert!(parsed.changes.contains(r#""new_value":{"k":[1,2]}"#));
    }

    #[test]
    fn a_parsed_key_held_twice_keeps_its_first_position_and_its_last_value() {
        // As 3.x's `serde_json::Value` read it, and as `parse_xml` reads a
        // repeated element: a context never holds a key twice.
        let input = json!({"source": "data.dup", "target": "out"});
        let context = json!({"data": {"dup": r#"{"a":1,"b":[{"c":1,"c":2}],"a":3}"#}});
        let observed =
            task("parse_json", &input)
                .expect("compiles")
                .run(&Value::Null, &context, true);
        assert_eq!(observed.status, Some(200));
        assert!(
            observed.context.contains(r#""out":{"a":3,"b":[{"c":2}]}"#),
            "{}",
            observed.context
        );
    }

    #[test]
    fn parse_json_computes_its_source_and_its_target() {
        let mut golden = Golden::new("parse", "parse_json_computes_its_source_and_its_target");
        // A computed source can name the payload, a payload path or a context path.
        for source in [
            json!({"cat": ["pay", "load"]}),
            json!({"cat": ["payload.", "x"]}),
            json!({"var": "data.name"}),
            json!({"cat": ["data.", "obj"]}),
            json!({"cat": ["payload.", {"var": "data.name"}]}),
        ] {
            json_parse(&mut golden, source, json!("out"));
        }
        // A computed target.
        json_parse(
            &mut golden,
            json!("payload.x"),
            json!({"cat": ["to_", {"var": "data.slot"}]}),
        );
        json_parse(&mut golden, json!("payload.x"), json!({"var": "data.slot"}));
        json_parse(&mut golden, json!("payload.x"), json!("deep.er.out"));
    }

    #[test]
    fn a_target_is_relative_to_data_so_data_t_lands_at_data_data_t() {
        let mut golden = Golden::new(
            "parse",
            "a_target_is_relative_to_data_so_data_t_lands_at_data_data_t",
        );
        let observed = json_parse(&mut golden, json!("payload.x"), json!("data.t"));
        assert!(
            observed.context.contains(r#""data":{"t":"#),
            "{}",
            observed.context
        );
        assert_eq!(
            observed.changes.matches(r#""path":"data.data.t""#).count(),
            1
        );
    }

    #[test]
    fn a_target_of_exactly_data_dot_writes_the_empty_key() {
        let mut golden = Golden::new("parse", "a_target_of_exactly_data_dot_writes_the_empty_key");
        // T2-f: the path `data.data.` ends in an empty segment, which is a
        // key, as in 3.x.
        let observed = json_parse(&mut golden, json!("payload.x"), json!("data."));
        assert_eq!(observed.status, Some(200));
        // It is the key `""` under `data.data`, and the change names it so.
        assert!(
            observed.context.contains(r#""data":{"":{"y":[10,20]}}"#),
            "{}",
            observed.context
        );
        assert_eq!(
            observed.changes.matches(r#""path":"data.data.""#).count(),
            1
        );
        // And so does a bare empty target.
        json_parse(&mut golden, json!("payload.x"), json!(""));
    }

    #[test]
    fn a_computed_source_that_is_no_source_fails_the_task() {
        // v3 read any name as a top-level key and found nothing; v4 refuses a
        // name outside the three roots: at compile when it folds,
        // as an error of the call when it does not.
        let folds = json!({"source": {"cat": ["extra", ".x"]}, "target": "out"});
        assert!(task("parse_json", &folds).is_none());
        let input = json!({"source": {"cat": ["extra.", {"var": "data.slot"}]}, "target": "out"});
        let observed =
            task("parse_json", &input)
                .expect("compiles")
                .run(&payload(), &context(), true);
        assert_eq!(observed.status, Some(500));
        assert_eq!(observed.error_code.as_deref(), Some("CONFIG_ERROR"));
    }

    #[test]
    fn parse_xml_parses_a_valid_document() {
        let mut golden = Golden::new("parse", "parse_xml_parses_a_valid_document");
        let observed = golden.observe(
            "parse_xml",
            &json!({"source": "data.xml", "target": "doc"}),
            &Value::Null,
            &context(),
        );
        assert_eq!(observed.status, Some(200));
        assert!(
            observed
                .context
                .contains(r#""doc":{"a":{"$text":"1"},"b":{"$text":"x"}}"#),
            "{}",
            observed.context
        );
        // The payload as the document, and a computed source.
        golden.observe(
            "parse_xml",
            &json!({"source": "payload", "target": "doc"}),
            &json!("<a><b>1</b></a>"),
            &context(),
        );
        golden.observe(
            "parse_xml",
            &json!({"source": {"cat": ["data.", {"var": "data.slot"}]}, "target": "doc"}),
            &Value::Null,
            &json!({"data": {"slot": "xml", "xml": "<q>1</q>"}}),
        );
    }

    #[test]
    fn parse_xml_keeps_the_documents_element_order() {
        let mut golden = Golden::new("parse", "parse_xml_keeps_the_documents_element_order");
        // In every build, where v3's default build sorted the keys (break
        // 23). A repeated element keeps its first place and its last value,
        // as 3.x read it with serde_json's `preserve_order` on.
        let repeated = golden.observe(
            "parse_xml",
            &json!({"source": "data.xml", "target": "doc"}),
            &Value::Null,
            &json!({"data": {"xml": "<r><a>1</a><b>2</b><a>3</a></r>"}}),
        );
        assert!(
            repeated
                .context
                .contains(r#""doc":{"a":{"$text":"3"},"b":{"$text":"2"}}"#),
            "{}",
            repeated.context
        );
        let context = datavalue::OwnedDataValue::from_json(concat!(
            r#"{"data":{"xml":"<r><z>1</z><a><y>2</y><b>3</b></a><m>x</m><z>4</z></r>"},"#,
            r#""metadata":{},"temp_data":{}}"#
        ))
        .expect("JSON");
        let input = json!({"source": "data.xml", "target": "doc"});
        let written = context_text_after("parse_xml", &input, context);
        assert!(
            written.contains(concat!(
                r#""doc":{"z":{"$text":"4"},"a":{"y":{"$text":"2"},"b":{"$text":"3"}},"#,
                r#""m":{"$text":"x"}}"#
            )),
            "{written}"
        );
    }

    #[test]
    fn parse_xml_fails_on_a_source_that_is_not_a_string_or_not_a_document() {
        let mut golden = Golden::new(
            "parse",
            "parse_xml_fails_on_a_source_that_is_not_a_string_or_not_a_document",
        );
        for (source, message) in [
            (
                "data.num",
                "Validation error: ParseXml: Source 'data.num' is not a string",
            ),
            (
                "data.missing",
                "Validation error: ParseXml: Source 'data.missing' is not a string",
            ),
            (
                "payload.x",
                "Validation error: ParseXml: Source 'payload.x' is not a string",
            ),
            (
                "data.broken",
                "Validation error: Failed to parse XML: <dependency message>",
            ),
        ] {
            let observed = golden.observe(
                "parse_xml",
                &json!({"source": source, "target": "doc"}),
                &payload(),
                &context(),
            );
            assert_eq!(observed.status, Some(500), "{source}");
            assert_eq!(observed.error_code.as_deref(), Some("VALIDATION_ERROR"));
            assert!(
                observed
                    .errors
                    .contains(&format!("Task t error: {message}")),
                "{source}: {}",
                observed.errors
            );
        }
    }

    #[test]
    fn an_empty_remainder_names_its_root() {
        let mut golden = Golden::new("parse", "an_empty_remainder_names_its_root");
        // An empty remainder reads its root whole: `payload.` is the
        // payload, `data.` is `data`.
        let context = json!({"data": {"d": "data.", "e": "", "k": 1, "p": "payload."}});
        for source in [
            json!("payload."),
            json!("data."),
            json!({"cat": ["payload.", ""]}),
            json!({"cat": ["data.", ""]}),
            json!({"cat": ["payload.", {"var": "data.e"}]}),
            json!({"cat": ["data.", {"var": "data.e"}]}),
            json!({"var": "data.p"}),
            json!({"var": "data.d"}),
        ] {
            let input = json!({"source": source, "target": "out"});
            let observed = golden.observe("parse_json", &input, &payload(), &context);
            assert_eq!(observed.status, Some(200), "{source}");
        }
        let whole = golden.observe(
            "parse_json",
            &json!({"source": "data.", "target": "out"}),
            &payload(),
            &context,
        );
        assert!(
            whole.context.contains(r#""out":{"d":"data.""#),
            "{}",
            whole.context
        );
        // And for XML.
        let context = json!({"data": {"e": ""}});
        for source in [
            json!("payload."),
            json!({"cat": ["payload.", {"var": "data.e"}]}),
        ] {
            let input = json!({"source": source, "target": "out"});
            golden.observe("parse_xml", &input, &json!("<a>1</a>"), &context);
        }
    }

    #[test]
    fn a_computed_parameter_that_fails_to_evaluate_records_a_logic_error() {
        let mut golden = Golden::new(
            "parse",
            "a_computed_parameter_that_fails_to_evaluate_records_a_logic_error",
        );
        let context = json!({"data": {"word": "abc"}});
        let broken = json!({"+": [{"var": "data.word"}, "x"]});
        for input in [
            json!({"source": broken, "target": "out"}),
            json!({"source": "payload", "target": broken}),
        ] {
            for function in ["parse_json", "parse_xml"] {
                let observed = golden.observe(function, &input, &json!("<a>1</a>"), &context);
                assert_eq!(observed.status, Some(500), "{function} {input}");
                assert_eq!(observed.error_code.as_deref(), Some("LOGIC_ERROR"));
                assert!(
                    observed
                        .errors
                        .contains("Task t error: Logic evaluation error: "),
                    "{}",
                    observed.errors
                );
            }
        }
    }
}
