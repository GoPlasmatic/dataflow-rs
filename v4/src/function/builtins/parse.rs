//! `parse_json`'s and `parse_xml`'s config: where to read, and where under
//! `data` to write.

use super::paths::{DataPath, ParseFrom, ParseSource};
use super::xml::{validation_error, xml_to_json};
use crate::base::Result;
use crate::expr::path;
use crate::function::{FunctionConfig, Outcome, SyncCtx};
use crate::state::RunContext;
use ::log::debug;
use datavalue::OwnedDataValue;
use std::borrow::Cow;

/// `parse_json`'s and `parse_xml`'s input.
#[derive(Debug, FunctionConfig)]
#[non_exhaustive]
pub struct ParseConfig {
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
    /// when it is JSON and kept when it is not, written at the target.
    pub(crate) fn json<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let from = self.source.resolve(cx)?;
        let target = self.target.path(cx)?;
        debug!("ParseJson: Extracting from '{}' to '{target}'", from.text());
        let (value, hint) = match lookup(cx.payload, cx.rc, &from).as_deref() {
            None => (OwnedDataValue::Null, None),
            Some(OwnedDataValue::String(s)) => (
                OwnedDataValue::from_json(s).unwrap_or_else(|_| OwnedDataValue::String(s.clone())),
                Some(s.len()),
            ),
            Some(other) => (other.clone(), None),
        };
        write(cx, &target, value, hint);
        Ok(Outcome::Ok)
    }

    /// `parse_xml`: the source's string read as an XML document, written at
    /// the target.
    ///
    /// # Errors
    ///
    /// `VALIDATION_ERROR` when the source is not a string, or is not a
    /// document.
    pub(crate) fn xml<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let from = self.source.resolve(cx)?;
        debug!("ParseXml: Extracting from '{}'", from.text());
        let (parsed, len) = match lookup(cx.payload, cx.rc, &from).as_deref() {
            Some(OwnedDataValue::String(xml)) => (xml_to_json(xml)?, xml.len()),
            _ => {
                return Err(validation_error(format_args!(
                    "ParseXml: Source '{}' is not a string",
                    from.text()
                )));
            }
        };
        let target = self.target.path(cx)?;
        write(cx, &target, OwnedDataValue::from(&parsed), Some(len));
        Ok(Outcome::Ok)
    }
}

/// Writes `value` at `target`, recording the change under capture.
fn write(
    cx: &mut SyncCtx<'_, '_, '_>,
    target: &crate::expr::Path,
    value: OwnedDataValue,
    hint: Option<usize>,
) {
    if cx.capture {
        let old = cx
            .rc
            .get(target)
            .map_or(OwnedDataValue::Null, Cow::into_owned);
        cx.change(target.dotted().clone(), old, value.clone(), false);
    }
    cx.rc.set(target, value, hint);
    debug!("Parse: Successfully stored data to '{target}'");
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::differential::{Observed, same, v4_task};
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

    async fn json_parse(source: Value, target: Value) -> Observed {
        same(
            "parse_json",
            &json!({"source": source, "target": target}),
            &payload(),
            &context(),
        )
        .await
    }

    #[tokio::test]
    async fn parse_json_reads_the_payload_a_payload_path_or_a_context_path() {
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
            let observed = json_parse(json!(source), json!("out")).await;
            assert_eq!(observed.status, Some(200), "{source}");
        }
    }

    #[tokio::test]
    async fn a_string_source_is_parsed_when_it_is_json_and_kept_when_it_is_not() {
        let parsed = json_parse(json!("data.text"), json!("out")).await;
        assert!(
            parsed.context.contains(r#""out":{"k":[1,2]}"#),
            "{}",
            parsed.context
        );
        let kept = json_parse(json!("data.plain"), json!("out")).await;
        assert!(
            kept.context.contains(r#""out":"not json""#),
            "{}",
            kept.context
        );
        // The change records the parsed value.
        assert!(parsed.changes.contains(r#""new_value":{"k":[1,2]}"#));
    }

    #[tokio::test]
    async fn parse_json_computes_its_source_and_its_target() {
        // A computed source can name the payload, a payload path or a context path.
        for source in [
            json!({"cat": ["pay", "load"]}),
            json!({"cat": ["payload.", "x"]}),
            json!({"var": "data.name"}),
            json!({"cat": ["data.", "obj"]}),
            json!({"cat": ["payload.", {"var": "data.name"}]}),
        ] {
            json_parse(source, json!("out")).await;
        }
        // A computed target.
        json_parse(
            json!("payload.x"),
            json!({"cat": ["to_", {"var": "data.slot"}]}),
        )
        .await;
        json_parse(json!("payload.x"), json!({"var": "data.slot"})).await;
        json_parse(json!("payload.x"), json!("deep.er.out")).await;
    }

    #[tokio::test]
    async fn a_target_is_relative_to_data_so_data_t_lands_at_data_data_t() {
        let observed = json_parse(json!("payload.x"), json!("data.t")).await;
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

    #[tokio::test]
    async fn a_target_of_exactly_data_dot_does_what_v3_does() {
        // T2-f: the path `data.data.` ends in an empty segment. v4 writes
        // what v3 writes, whatever that is: the differential decides.
        let observed = json_parse(json!("payload.x"), json!("data.")).await;
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
        json_parse(json!("payload.x"), json!("")).await;
    }

    #[test]
    fn a_computed_source_that_is_no_source_fails_the_task() {
        // v3 read any name as a top-level key and found nothing; v4 refuses a
        // name outside the three roots (break 15): at compile when it folds,
        // as an error of the call when it does not.
        let folds = json!({"source": {"cat": ["extra", ".x"]}, "target": "out"});
        assert!(v4_task("parse_json", &folds).is_none());
        let input = json!({"source": {"cat": ["extra.", {"var": "data.slot"}]}, "target": "out"});
        let observed =
            v4_task("parse_json", &input)
                .expect("compiles")
                .run(&payload(), &context(), true);
        assert_eq!(observed.status, Some(500));
        assert_eq!(observed.error_code.as_deref(), Some("CONFIG_ERROR"));
    }

    #[tokio::test]
    async fn parse_xml_parses_a_valid_document() {
        let observed = same(
            "parse_xml",
            &json!({"source": "data.xml", "target": "doc"}),
            &Value::Null,
            &context(),
        )
        .await;
        assert_eq!(observed.status, Some(200));
        assert!(
            observed
                .context
                .contains(r#""doc":{"a":{"$text":"1"},"b":{"$text":"x"}}"#),
            "{}",
            observed.context
        );
        // The payload as the document, and a computed source.
        same(
            "parse_xml",
            &json!({"source": "payload", "target": "doc"}),
            &json!("<a><b>1</b></a>"),
            &context(),
        )
        .await;
        same(
            "parse_xml",
            &json!({"source": {"cat": ["data.", {"var": "data.slot"}]}, "target": "doc"}),
            &Value::Null,
            &json!({"data": {"slot": "xml", "xml": "<q>1</q>"}}),
        )
        .await;
    }

    #[tokio::test]
    async fn parse_xml_fails_on_a_source_that_is_not_a_string_or_not_a_document() {
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
            ("data.broken", "Validation error: Failed to parse XML: "),
        ] {
            let observed = same(
                "parse_xml",
                &json!({"source": source, "target": "doc"}),
                &payload(),
                &context(),
            )
            .await;
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

    #[tokio::test]
    async fn an_empty_remainder_names_its_root_as_in_v3() {
        // `get_nested_value(x, "")` is `x`: `payload.` is the payload, `data.` is `data`.
        let context = json!({"data": {"e": "", "p": "payload.", "d": "data.", "k": 1}});
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
            let observed = same("parse_json", &input, &payload(), &context).await;
            assert_eq!(observed.status, Some(200), "{source}");
        }
        let whole = same(
            "parse_json",
            &json!({"source": "data.", "target": "out"}),
            &payload(),
            &context,
        )
        .await;
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
            same("parse_xml", &input, &json!("<a>1</a>"), &context).await;
        }
    }

    #[tokio::test]
    async fn a_computed_parameter_that_fails_to_evaluate_records_v3s_text() {
        let context = json!({"data": {"word": "abc"}});
        let broken = json!({"+": [{"var": "data.word"}, "x"]});
        for input in [
            json!({"source": broken, "target": "out"}),
            json!({"source": "payload", "target": broken}),
        ] {
            for function in ["parse_json", "parse_xml"] {
                let observed = same(function, &input, &json!("<a>1</a>"), &context).await;
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
