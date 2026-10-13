//! `publish_json`'s and `publish_xml`'s config: what under `data` to
//! serialize, and where under `data` to write the text.

use super::paths::{DataPath, PublishSource};
use crate::expr::Expr;
use crate::function::FunctionConfig;

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
