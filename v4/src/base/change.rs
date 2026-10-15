//! The wire shape of one recorded write.

use datavalue::OwnedDataValue;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// One write or removal, v3's `Change` on the wire.
///
/// A path that was absent before the write records `old_value` as `null`.
#[derive(Clone, Serialize, Deserialize)]
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
    /// The write replaced the root at `path` instead of merging into it
    /// (I106): a host's `Ctx::set` at a root replaces it, and `map`'s root
    /// mapping, which records only the keys it wrote, merges. Skipped when
    /// `false`, so a merge's JSON, and every write v3 could record, is v3's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replaced: bool,
    /// The change is the trace's alone: a write the call made that v3's
    /// audit entry has no row for, which a replay needs, such as the array a
    /// `map` append creates. The recorder leaves it out of the audit entry.
    /// Never serialized, and neither compared nor shown.
    #[serde(skip)]
    pub(crate) trace_only: bool,
}

impl PartialEq for Change {
    /// The wire fields; not whether the change is the trace's alone.
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.old_value == other.old_value
            && self.new_value == other.new_value
            && self.removed == other.removed
            && self.replaced == other.replaced
    }
}

impl fmt::Debug for Change {
    /// The wire fields, as a derive would show them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Change")
            .field("path", &self.path)
            .field("old_value", &self.old_value)
            .field("new_value", &self.new_value)
            .field("removed", &self.removed)
            .field("replaced", &self.replaced)
            .finish()
    }
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
            replaced: false,
            trace_only: false,
        }
    }

    /// A removal of the key at `path`, which held `old_value`.
    pub fn removal(path: impl Into<Arc<str>>, old_value: OwnedDataValue) -> Self {
        Self {
            path: path.into(),
            old_value,
            new_value: OwnedDataValue::Null,
            removed: true,
            replaced: false,
            trace_only: false,
        }
    }

    /// A write only the trace keeps ([`trace_only`](Self::trace_only)).
    pub(crate) fn for_trace(
        path: impl Into<Arc<str>>,
        old_value: OwnedDataValue,
        new_value: OwnedDataValue,
    ) -> Self {
        Self {
            trace_only: true,
            ..Self::write(path, old_value, new_value)
        }
    }
}
