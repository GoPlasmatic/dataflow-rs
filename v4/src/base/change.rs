//! The wire shape of one recorded write.

use datavalue::OwnedDataValue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// One write or removal, v3's `Change` on the wire.
///
/// A path that was absent before the write records `old_value` as `null`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Change {
    /// The dotted path written.
    pub path: Arc<str>,
    /// What the path held before; `null` when it was absent.
    pub old_value: OwnedDataValue,
    /// The value written; `null` for a removal (read [`Self::removed`] to tell
    /// a removal from a write of `null`).
    pub new_value: OwnedDataValue,
    /// The key at `path` was removed rather than written. Skipped when
    /// `false`, so a write's JSON is v3's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
}

impl Change {
    /// A write of `new_value` over `old_value` at `path`.
    pub fn write(
        path: impl Into<Arc<str>>,
        old_value: OwnedDataValue,
        new_value: OwnedDataValue,
    ) -> Self {
        Self {
            path: path.into(),
            old_value,
            new_value,
            removed: false,
        }
    }

    /// A removal of the key at `path`, which held `old_value`.
    pub fn removal(path: impl Into<Arc<str>>, old_value: OwnedDataValue) -> Self {
        Self {
            path: path.into(),
            old_value,
            new_value: OwnedDataValue::Null,
            removed: true,
        }
    }
}
