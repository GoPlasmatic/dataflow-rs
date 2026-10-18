//! The wire shape of one recorded write.

use datavalue::OwnedDataValue;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// One write or removal, written as v3 wrote one.
///
/// A path that was absent before the write records `old_value` as `null`.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct Change {
    /// The dotted path written.
    pub path: Arc<str>,
    /// What the path held before; `null` when it was absent.
    #[serde(deserialize_with = "super::exact")]
    #[cfg_attr(feature = "ts", ts(type = "JsonValue"))]
    pub old_value: OwnedDataValue,
    /// The value written; `null` for a removal (read
    /// [`removed`](Self::removed) to tell a removal from a write of `null`).
    #[serde(deserialize_with = "super::exact")]
    #[cfg_attr(feature = "ts", ts(type = "JsonValue"))]
    pub new_value: OwnedDataValue,
    /// The key at `path` was removed rather than written. Absent when
    /// `false`, so a write's JSON is v3's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    /// The write replaced the root at `path` instead of merging into it: a
    /// host function's write at a root replaces it, and `map`'s root
    /// mapping, which records only the keys it wrote, merges. A trace's
    /// changes carry it, so a replay knows which; an audit entry's are
    /// always `false`, so the audit trail's JSON is v3's. Absent when
    /// `false`.
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

    /// An assignment of `new_value` over `old_value` at `path`, as a host
    /// function's `Ctx::set` and the engine's own writes make one: at a root
    /// it replaces the root, and says so ([`replaced`](Self::replaced)). A
    /// `map` root mapping merges instead, and records a plain
    /// [`write`](Self::write).
    pub(crate) fn assign(
        path: &crate::expr::Path,
        old_value: OwnedDataValue,
        new_value: OwnedDataValue,
    ) -> Self {
        Self {
            replaced: path.is_root(),
            ..Self::write(Arc::clone(path.dotted()), old_value, new_value)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Path;

    fn path(dotted: &str) -> Path {
        dotted.parse().unwrap()
    }

    #[test]
    fn each_kind_of_record_sets_its_own_flags() {
        let (old, new) = (OwnedDataValue::Null, OwnedDataValue::from(1));
        let flags = |c: &Change| (c.removed, c.replaced, c.trace_only);
        // An assignment replaces a root, and says so; below a root it is a
        // plain write.
        assert_eq!(
            flags(&Change::assign(&path("data"), old.clone(), new.clone())),
            (false, true, false)
        );
        assert_eq!(
            flags(&Change::assign(&path("data.a"), old.clone(), new.clone())),
            (false, false, false)
        );
        // `map`'s root mapping merges: a plain write, never `replaced`.
        assert_eq!(
            flags(&Change::write("data", old.clone(), new.clone())),
            (false, false, false)
        );
        assert_eq!(
            flags(&Change::removal("data.a", new.clone())),
            (true, false, false)
        );
        assert_eq!(
            flags(&Change::for_trace("data.a", old, new)),
            (false, false, true)
        );
    }
}
