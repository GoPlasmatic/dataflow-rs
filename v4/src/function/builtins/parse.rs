//! `parse_json`'s and `parse_xml`'s config: where to read, and where under
//! `data` to write.

use super::paths::{DataPath, ParseSource};
use crate::function::FunctionConfig;

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
