//! `publish_json`'s and `publish_xml`'s config: what under `data` to
//! serialize, and where under `data` to write the text.

use super::paths::{DataPath, PublishSource};
use super::xml::{validation_error, value_to_xml};
use crate::base::Result;
use crate::expr::{Expr, Path};
use crate::function::{FunctionConfig, Outcome, SyncCtx};
use ::log::debug;
use datavalue::OwnedDataValue;
use std::borrow::Cow;

/// `publish_json`'s and `publish_xml`'s input.
#[derive(Debug, FunctionConfig)]
#[non_exhaustive]
pub struct PublishConfig {
    /// What to serialize, relative to `data`, with v3's fallback: a source
    /// beginning with `data.` is also read without that prefix when
    /// `data.<source>` is missing.
    #[config(records)]
    pub source: PublishSource,
    /// Where the text is written, relative to `data`.
    #[config(records)]
    pub target: DataPath,
    /// Indent the JSON (`publish_json` only).
    #[config(default = false)]
    pub pretty: bool,
    /// The XML document's root element (`publish_xml` only).
    #[config(records, default = "root")]
    pub root_element: Expr<String>,
}

impl PublishConfig {
    /// The paths this call reads and writes: the source with its fallback,
    /// then the target.
    #[allow(clippy::type_complexity, reason = "one call site")]
    fn paths<'p>(
        &'p self,
        cx: &mut SyncCtx<'_, '_, 'p>,
    ) -> Result<(Cow<'p, Path>, Option<Cow<'p, Path>>, Cow<'p, Path>)> {
        let (source, fallback) = self.source.paths(cx)?;
        let target = self.target.path(cx)?;
        Ok((source, fallback, target))
    }

    /// `publish_json`: the source serialized to JSON text, compact or, with
    /// `pretty`, indented, both in the source's key order.
    ///
    /// # Errors
    ///
    /// `VALIDATION_ERROR` when the source is missing or `null`.
    pub(crate) fn json<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let (source, fallback, target) = self.paths(cx)?;
        debug!("PublishJson: Serializing '{source}' to '{target}'");
        let text = {
            let found = find(cx, &source, fallback.as_deref(), "PublishJson")?;
            if self.pretty {
                serde_json::to_string_pretty(&*found).map_err(|e| {
                    validation_error(format_args!("Failed to serialize to JSON: {e}"))
                })?
            } else {
                found.to_json_string()
            }
        };
        cx.write(&target, OwnedDataValue::String(text), None);
        Ok(Outcome::Ok)
    }

    /// `publish_xml`: the source serialized to an XML document under
    /// `root_element`, its keys in the source's order.
    ///
    /// # Errors
    ///
    /// `VALIDATION_ERROR` when the source is missing or `null`.
    pub(crate) fn xml<'p>(&'p self, cx: &mut SyncCtx<'_, '_, 'p>) -> Result<Outcome> {
        let (source, fallback, target) = self.paths(cx)?;
        debug!("PublishXml: Serializing '{source}' to '{target}'");
        let xml = match self.root_element.constant() {
            // A constant root reads nothing and cannot fail: the source is
            // serialized where it is found.
            Some(root) => value_to_xml(
                &*find(cx, &source, fallback.as_deref(), "PublishXml")?,
                root,
            ),
            // Owned, as v3's bridge was: the root element is evaluated after
            // the source is found, as in v3, and the evaluation needs the
            // context.
            None => {
                let found = find(cx, &source, fallback.as_deref(), "PublishXml")?.into_owned();
                let root = cx.eval(&self.root_element)?;
                value_to_xml(&found, &root)
            }
        };
        cx.write(&target, OwnedDataValue::String(xml), None);
        Ok(Outcome::Ok)
    }
}

/// The source's value: at `source`, else at the fallback. A missing value and
/// an explicit `null` both fail.
fn find<'a>(
    cx: &'a SyncCtx<'_, '_, '_>,
    source: &Path,
    fallback: Option<&Path>,
    who: &str,
) -> Result<Cow<'a, OwnedDataValue>> {
    match cx
        .rc
        .get(source)
        .or_else(|| fallback.and_then(|f| cx.rc.get(f)))
    {
        Some(found) if !matches!(*found, OwnedDataValue::Null) => Ok(found),
        _ => Err(validation_error(format_args!(
            "{who}: Source '{source}' not found or is null"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use crate::function::builtins::differential::{
        Observed, same, v3_context_text, v4_context_text,
    };
    use datavalue::OwnedDataValue;
    use serde_json::{Value, json};

    fn context() -> Value {
        json!({
            "data": {
                "obj": {"a": [1, {"k": "v & <w>"}], "m": null, "t": true, "z": 1},
                "data": {"inner": 1},
                "num": 7, "text": "hi", "nothing": null,
                "kind": "obj", "slot": "out_slot", "tag": "doc"
            },
            "metadata": {"m": 1}
        })
    }

    async fn publish(function: &str, extra: Value) -> Observed {
        let mut input = json!({"source": "obj", "target": "out"});
        for (k, v) in extra.as_object().expect("an object") {
            input[k] = v.clone();
        }
        same(function, &input, &Value::Null, &context()).await
    }

    #[tokio::test]
    async fn publish_json_compact_and_pretty_agree_with_v3() {
        let compact = publish("publish_json", json!({})).await;
        assert_eq!(compact.status, Some(200));
        let pretty = publish("publish_json", json!({"pretty": true})).await;
        assert!(
            pretty.context.contains(r#""out":"{\n  \"a\": ["#),
            "{}",
            pretty.context
        );
    }

    #[tokio::test]
    async fn compact_pretty_and_xml_keep_the_sources_key_order() {
        // Built from JSON text so the keys stay out of order whatever
        // serde_json's map type is. All three write the source's order in
        // every build: break 23, where v3's default build sorted pretty's
        // and XML's keys. Compact is v3's text in either build.
        let text = r#"{"data":{"obj":{"z":1,"a":{"y":2,"b":[3,{"q":1,"c":2.0}]}}},"metadata":{},"temp_data":{}}"#;
        let context = || OwnedDataValue::from_json(text).expect("JSON");
        let compact = json!({"source": "obj", "target": "out"});
        assert_eq!(
            v4_context_text("publish_json", &compact, context()),
            v3_context_text("publish_json", &compact, context()).await
        );
        let out = |function: &str, pretty: bool| {
            let input = json!({"source": "obj", "target": "out", "pretty": pretty});
            let written: Value =
                serde_json::from_str(&v4_context_text(function, &input, context())).unwrap();
            written["data"]["out"]
                .as_str()
                .expect("a string")
                .to_owned()
        };
        assert_eq!(
            out("publish_json", false),
            r#"{"z":1,"a":{"y":2,"b":[3,{"q":1,"c":2.0}]}}"#
        );
        assert_eq!(
            out("publish_json", true),
            concat!(
                "{\n  \"z\": 1,\n  \"a\": {\n    \"y\": 2,\n    \"b\": [\n      3,\n",
                "      {\n        \"q\": 1,\n        \"c\": 2.0\n      }\n    ]\n  }\n}"
            )
        );
        assert_eq!(
            out("publish_xml", false),
            "<root><z>1</z><a><y>2</y><b><item>3</item><item><q>1</q><c>2.0</c></item></b></a></root>"
        );
    }

    #[tokio::test]
    async fn publish_json_reads_through_the_data_fallback() {
        // `data.obj`: `data.data.obj` is missing, so `data.obj` is read.
        let observed = publish("publish_json", json!({"source": "data.obj"})).await;
        assert_eq!(observed.status, Some(200));
        // `data.inner` finds `data.data.inner` first.
        publish("publish_json", json!({"source": "data.inner"})).await;
        publish("publish_json", json!({"source": "data"})).await;
        publish("publish_json", json!({"source": "obj.a.1"})).await;
        publish("publish_json", json!({"source": "num"})).await;
        publish("publish_json", json!({"source": "text"})).await;
    }

    #[tokio::test]
    async fn a_missing_or_null_source_fails_with_the_validation_error() {
        for (function, source, message) in [
            (
                "publish_json",
                "absent",
                "Validation error: PublishJson: Source 'data.absent' not found or is null",
            ),
            (
                "publish_json",
                "nothing",
                "Validation error: PublishJson: Source 'data.nothing' not found or is null",
            ),
            (
                "publish_json",
                "data.absent",
                "Validation error: PublishJson: Source 'data.data.absent' not found or is null",
            ),
            (
                "publish_xml",
                "absent",
                "Validation error: PublishXml: Source 'data.absent' not found or is null",
            ),
            (
                "publish_xml",
                "nothing",
                "Validation error: PublishXml: Source 'data.nothing' not found or is null",
            ),
        ] {
            let observed = publish(function, json!({"source": source})).await;
            assert_eq!(observed.status, Some(500), "{function} {source}");
            assert_eq!(observed.error_code.as_deref(), Some("VALIDATION_ERROR"));
            assert!(
                observed
                    .errors
                    .contains(&format!("Task t error: {message}")),
                "{}",
                observed.errors
            );
        }
    }

    #[tokio::test]
    async fn a_computed_source_and_target_resolve_per_call() {
        for function in ["publish_json", "publish_xml"] {
            publish(function, json!({"source": {"var": "data.kind"}})).await;
            publish(
                function,
                json!({"source": {"cat": ["data.", {"var": "data.kind"}]}}),
            )
            .await;
            publish(function, json!({"target": {"var": "data.slot"}})).await;
            // One that folds is resolved at compile; the answer is the same.
            publish(function, json!({"source": {"cat": ["o", "bj"]}})).await;
        }
    }

    #[tokio::test]
    async fn publish_xml_writes_the_document_under_the_root_element() {
        let observed = publish("publish_xml", json!({})).await;
        assert!(
            observed.context.contains("<root><a><item>1</item>"),
            "{}",
            observed.context
        );
        assert!(observed.context.contains("v &amp; &lt;w&gt;"));
        publish("publish_xml", json!({"root_element": "envelope"})).await;
        publish("publish_xml", json!({"root_element": {"var": "data.tag"}})).await;
        // Scalars, a list and a key that is no XML name.
        publish("publish_xml", json!({"source": "num"})).await;
        publish("publish_xml", json!({"source": "obj.a"})).await;
        publish("publish_xml", json!({"source": "text"})).await;
    }

    #[tokio::test]
    async fn a_source_that_exists_under_data_wins_over_the_fallback() {
        let context = json!({"data": {"data": {"x": 1}, "x": 2}});
        for function in ["publish_json", "publish_xml"] {
            let observed = same(
                function,
                &json!({"source": "data.x", "target": "out"}),
                &Value::Null,
                &context,
            )
            .await;
            assert!(observed.context.contains('1'), "{}", observed.context);
        }
    }

    #[tokio::test]
    async fn an_empty_remainder_publishes_the_whole_of_data_as_in_v3() {
        let context = json!({"data": {"e": "", "d": "data.", "k": 1, "o": {"x": 1}}});
        for function in ["publish_json", "publish_xml"] {
            for source in [
                json!(""),
                json!("data."),
                json!({"cat": ["data.", ""]}),
                json!({"var": "data.e"}),
                json!({"cat": ["data.", {"var": "data.e"}]}),
                json!({"var": "data.d"}),
            ] {
                let input = json!({"source": source, "target": "out"});
                let observed = same(function, &input, &Value::Null, &context).await;
                assert_eq!(observed.status, Some(200), "{function} {source}");
            }
        }
        // A key named `""` wins over the whole of `data`, as in v3's first step.
        let context = json!({"data": {"": {"only": 1}, "k": 2}});
        let input = json!({"source": "", "target": "out"});
        same("publish_json", &input, &Value::Null, &context).await;
    }

    #[tokio::test]
    async fn a_computed_parameter_that_fails_to_evaluate_records_v3s_text() {
        let context = json!({"data": {"word": "abc", "obj": {"a": 1}}});
        let broken = json!({"+": [{"var": "data.word"}, "x"]});
        for (function, input) in [
            ("publish_json", json!({"source": broken, "target": "out"})),
            ("publish_json", json!({"source": "obj", "target": broken})),
            ("publish_xml", json!({"source": broken, "target": "out"})),
            ("publish_xml", json!({"source": "obj", "target": broken})),
            (
                "publish_xml",
                json!({"source": "obj", "target": "out", "root_element": broken}),
            ),
        ] {
            let observed = same(function, &input, &Value::Null, &context).await;
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
