//! Percentage-based traffic splitting: the [`Rollout`] range, and the
//! invariants that make a *set* of them safe.
//!
//! A rollout gives one workflow a slice of the traffic on its channel. The
//! engine matches a single range against [`Message::routing_bucket`]; what
//! makes a deployment correct is a property of the whole set — the versions of
//! one logical workflow must partition `0..100` with no overlap and no gap.
//! [`Rollout::partition`] builds such a set from percentages, and
//! [`Rollout::validate_set`] checks one.
//!
//! Bucket *derivation* — how a caller maps a request to `0..=99`, whether by
//! sticky hash, per-message draw or round-robin — is deliberately the caller's
//! policy and stays outside this crate.
//!
//! [`Message::routing_bucket`]: crate::Message::routing_bucket

use serde::{Deserialize, Serialize};
use std::fmt;

/// The bucket space every rollout range divides up: `0..100`.
const BUCKETS: u16 = 100;

/// Half-open bucket range `[bucket_start, bucket_end)` over `0..100`, giving this
/// workflow a slice of the traffic on its channel.
///
/// Compared against [`crate::Message::routing_bucket`]. The engine does **not**
/// derive the bucket: how a caller maps to one — a sticky hash of some request
/// identity, a per-message random draw, round-robin — is entirely the caller's
/// policy and deliberately stays outside this crate.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Rollout {
    /// Inclusive lower bound.
    pub bucket_start: u8,
    /// Exclusive upper bound. `100` means "up to and including bucket 99".
    pub bucket_end: u8,
}

impl Rollout {
    /// Whether this range serves `bucket` (`0..=99`).
    ///
    /// `[0, 100)` accepts everything. An empty or inverted range
    /// (`bucket_end <= bucket_start`) accepts nothing.
    #[inline]
    pub fn accepts(&self, bucket: u8) -> bool {
        bucket >= self.bucket_start && bucket < self.bucket_end
    }

    /// Turn an ordered percentage split into contiguous half-open ranges
    /// covering exactly `0..100`.
    ///
    /// Entry `i` gets the range starting where entry `i-1` ended, so the input
    /// order is the traffic order. The percentages must sum to exactly 100:
    /// less leaves buckets that match nothing, more pushes later entries past
    /// the end of the bucket space where they can never match. The error names
    /// which.
    ///
    /// A `0` entry is allowed and yields an empty range, which accepts nothing
    /// — the natural way to express a version that is staged but takes no
    /// traffic.
    ///
    /// ```
    /// use dataflow_rs::{Rollout, RolloutError};
    ///
    /// let split = Rollout::partition(&[90, 10]).unwrap();
    /// assert_eq!(split[0], Rollout { bucket_start: 0, bucket_end: 90 });
    /// assert_eq!(split[1], Rollout { bucket_start: 90, bucket_end: 100 });
    ///
    /// // Anything the engine can route lands in exactly one range.
    /// for bucket in 0u8..=99 {
    ///     assert_eq!(split.iter().filter(|r| r.accepts(bucket)).count(), 1);
    /// }
    ///
    /// assert_eq!(Rollout::partition(&[90, 9]), Err(RolloutError::Under { total: 99 }));
    /// assert_eq!(Rollout::partition(&[90, 11]), Err(RolloutError::Over { total: 101 }));
    /// ```
    pub fn partition(percentages: &[u8]) -> Result<Vec<Self>, RolloutError> {
        // Accumulate wider than the input. Percentages are `u8`, so a `u8`
        // total wraps: [128, 128] and [200, 56] both wrap to exactly 0 and
        // would pass a naive `== 100` check while describing nonsense.
        let total: u32 = percentages.iter().map(|p| u32::from(*p)).sum();
        match total.cmp(&u32::from(BUCKETS)) {
            std::cmp::Ordering::Less => return Err(RolloutError::Under { total }),
            std::cmp::Ordering::Greater => return Err(RolloutError::Over { total }),
            std::cmp::Ordering::Equal => {}
        }

        // The sum is exactly 100, so every bound fits a u8.
        let mut offset = 0u8;
        let mut out = Vec::with_capacity(percentages.len());
        for pct in percentages {
            let end = offset + pct;
            out.push(Self {
                bucket_start: offset,
                bucket_end: end,
            });
            offset = end;
        }
        Ok(out)
    }

    /// Check that a set of ranges — the versions of one logical workflow —
    /// partitions `0..100`: every bucket served, none served twice.
    ///
    /// Both failures are silent in production otherwise. A gap blackholes a
    /// slice of traffic; an overlap makes which version answers depend on
    /// workflow ordering rather than on the rollout.
    ///
    /// Ranges are checked individually first, so an inverted range or one
    /// reaching past bucket 100 is reported as itself rather than as whatever
    /// downstream gap it happens to produce. Coverage is then reported at the
    /// **lowest** affected bucket, so the diagnosis is deterministic.
    ///
    /// ```
    /// use dataflow_rs::{Rollout, RolloutError};
    ///
    /// let good = Rollout::partition(&[50, 50]).unwrap();
    /// assert!(Rollout::validate_set(&good).is_ok());
    ///
    /// // Order does not matter — this is a property of the set.
    /// let reversed: Vec<_> = good.iter().rev().copied().collect();
    /// assert!(Rollout::validate_set(&reversed).is_ok());
    ///
    /// let gapped = [
    ///     Rollout { bucket_start: 0, bucket_end: 40 },
    ///     Rollout { bucket_start: 41, bucket_end: 100 },
    /// ];
    /// assert_eq!(
    ///     Rollout::validate_set(&gapped),
    ///     Err(RolloutError::Gap { bucket: 40 }),
    /// );
    /// ```
    pub fn validate_set<'a>(
        rollouts: impl IntoIterator<Item = &'a Self>,
    ) -> Result<(), RolloutError> {
        let ranges: Vec<&Self> = rollouts.into_iter().collect();

        // Diagnose a broken range by its cause, before it shows up as a
        // confusing symptom elsewhere in the space.
        for r in &ranges {
            if r.bucket_end < r.bucket_start || u16::from(r.bucket_end) > BUCKETS {
                return Err(RolloutError::InvalidRange { rollout: **r });
            }
        }

        // 100 buckets against a handful of ranges: counting directly is both
        // trivially fast and obviously correct against the definition, which a
        // sort-and-sweep would not be.
        for bucket in 0u8..(BUCKETS as u8) {
            match ranges.iter().filter(|r| r.accepts(bucket)).count() {
                1 => {}
                0 => return Err(RolloutError::Gap { bucket }),
                _ => return Err(RolloutError::Overlap { bucket }),
            }
        }
        Ok(())
    }
}

/// Why a rollout set is not a valid traffic split.
///
/// Its own type rather than a [`DataflowError`](crate::DataflowError) variant:
/// these are pure arithmetic checks with no engine involvement, and routing
/// them through the engine error would attach retryability classification that
/// means nothing here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RolloutError {
    /// Percentages sum to less than 100. The shortfall matches nothing, so
    /// that slice of traffic is silently dropped.
    Under {
        /// What the percentages actually summed to.
        total: u32,
    },
    /// Percentages sum to more than 100. The excess pushes later entries past
    /// the end of the bucket space, where they can never match.
    Over {
        /// What the percentages actually summed to.
        total: u32,
    },
    /// No range in the set serves this bucket — traffic mapping to it matches
    /// nothing. Reported at the lowest such bucket.
    Gap {
        /// The unserved bucket.
        bucket: u8,
    },
    /// More than one range serves this bucket, so which workflow answers
    /// depends on ordering rather than on the rollout. Reported at the lowest
    /// such bucket.
    Overlap {
        /// The doubly-served bucket.
        bucket: u8,
    },
    /// A range is inverted (`bucket_end < bucket_start`) or reaches past bucket
    /// 100. An empty range (`bucket_end == bucket_start`) is *not* this — that
    /// is a legitimate 0% entry.
    InvalidRange {
        /// The offending range.
        rollout: Rollout,
    },
}

impl fmt::Display for RolloutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Under { total } => write!(
                f,
                "rollout percentages sum to {total}, not 100 — \
                 the remaining {} buckets match nothing",
                u32::from(BUCKETS) - total
            ),
            Self::Over { total } => write!(
                f,
                "rollout percentages sum to {total}, not 100 — \
                 the excess {} pushes later entries past bucket 100, where they never match",
                total - u32::from(BUCKETS)
            ),
            Self::Gap { bucket } => write!(
                f,
                "bucket {bucket} is served by no rollout range — traffic mapping to it matches nothing"
            ),
            Self::Overlap { bucket } => write!(
                f,
                "bucket {bucket} is served by more than one rollout range — \
                 which workflow answers depends on ordering, not on the rollout"
            ),
            Self::InvalidRange { rollout } => write!(
                f,
                "rollout range [{}, {}) is not usable: {}",
                rollout.bucket_start,
                rollout.bucket_end,
                if rollout.bucket_end < rollout.bucket_start {
                    "the bounds are inverted"
                } else {
                    "bucket_end reaches past 100"
                }
            ),
        }
    }
}

impl std::error::Error for RolloutError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_is_a_half_open_range() {
        let all = Rollout {
            bucket_start: 0,
            bucket_end: 100,
        };
        assert!(all.accepts(0));
        assert!(all.accepts(99));

        let lower = Rollout {
            bucket_start: 0,
            bucket_end: 50,
        };
        assert!(lower.accepts(0));
        assert!(lower.accepts(49));
        assert!(!lower.accepts(50), "bucket_end is exclusive");
        assert!(!lower.accepts(99));

        // `start` inclusive, `end` exclusive — boundary exactness.
        let upper = Rollout {
            bucket_start: 50,
            bucket_end: 100,
        };
        assert!(upper.accepts(50), "bucket_start is inclusive");
        assert!(upper.accepts(99));
        assert!(!upper.accepts(49));

        // The two halves partition 0..=99 exactly.
        for b in 0u8..=99 {
            assert_ne!(
                lower.accepts(b),
                upper.accepts(b),
                "bucket {b} must be served by exactly one half"
            );
        }
    }

    #[test]
    fn empty_and_inverted_ranges_accept_nothing() {
        let empty = Rollout {
            bucket_start: 50,
            bucket_end: 50,
        };
        let inverted = Rollout {
            bucket_start: 60,
            bucket_end: 20,
        };
        for b in 0u8..=99 {
            assert!(!empty.accepts(b), "empty range accepted {b}");
            assert!(!inverted.accepts(b), "inverted range accepted {b}");
        }
    }

    #[test]
    fn end_of_100_is_representable_without_overflow() {
        // `bucket_end = 100` fits a u8 and `accepts` does no arithmetic on it.
        let r = Rollout {
            bucket_start: 99,
            bucket_end: 100,
        };
        assert!(r.accepts(99));
        assert!(!r.accepts(98));
    }

    // -----------------------------------------------------------------
    // partition
    // -----------------------------------------------------------------

    fn bounds(rollouts: &[Rollout]) -> Vec<(u8, u8)> {
        rollouts
            .iter()
            .map(|r| (r.bucket_start, r.bucket_end))
            .collect()
    }

    #[test]
    fn partition_splits_the_bucket_space_contiguously() {
        assert_eq!(bounds(&Rollout::partition(&[100]).unwrap()), [(0, 100)]);
        assert_eq!(
            bounds(&Rollout::partition(&[90, 10]).unwrap()),
            [(0, 90), (90, 100)]
        );
        assert_eq!(
            bounds(&Rollout::partition(&[34, 33, 33]).unwrap()),
            [(0, 34), (34, 67), (67, 100)],
            "input order is traffic order"
        );
    }

    #[test]
    fn partition_rejects_a_shortfall_naming_the_direction() {
        let err = Rollout::partition(&[90, 9]).unwrap_err();
        assert_eq!(err, RolloutError::Under { total: 99 });
        let msg = err.to_string();
        assert!(msg.contains("match nothing"), "got: {msg}");
    }

    #[test]
    fn partition_rejects_an_excess_naming_the_direction() {
        let err = Rollout::partition(&[90, 11]).unwrap_err();
        assert_eq!(err, RolloutError::Over { total: 101 });
        let msg = err.to_string();
        assert!(msg.contains("never match"), "got: {msg}");
    }

    #[test]
    fn partition_does_not_wrap_on_a_large_sum() {
        // Both of these wrap a u8 accumulator to exactly 0, and a naive
        // `sum == 100` check would reject them for the wrong reason — or a
        // `sum as u8 == 100` check would accept [128, 228].
        for input in [vec![128u8, 128], vec![200, 56], vec![255, 255, 255]] {
            let total: u32 = input.iter().map(|p| u32::from(*p)).sum();
            assert_eq!(
                Rollout::partition(&input),
                Err(RolloutError::Over { total }),
                "{input:?} sums to {total} and must be rejected as an excess"
            );
        }
    }

    #[test]
    fn an_empty_input_is_a_shortfall_not_an_empty_partition() {
        assert_eq!(
            Rollout::partition(&[]),
            Err(RolloutError::Under { total: 0 })
        );
    }

    #[test]
    fn a_zero_percent_entry_is_an_empty_range_that_accepts_nothing() {
        let split = Rollout::partition(&[100, 0]).unwrap();
        assert_eq!(bounds(&split), [(0, 100), (100, 100)]);
        for b in 0u8..=99 {
            assert!(!split[1].accepts(b), "a 0% entry serves no traffic");
        }
        // The two functions agree: a 0% entry is neither gap nor overlap.
        assert!(Rollout::validate_set(&split).is_ok());
    }

    #[test]
    fn partition_output_always_validates() {
        let splits: &[&[u8]] = &[
            &[100],
            &[50, 50],
            &[90, 10],
            &[34, 33, 33],
            &[1, 99],
            &[100, 0],
            &[0, 100],
            &[25, 25, 25, 25],
            &[1, 1, 98],
        ];
        for pcts in splits {
            let split = Rollout::partition(pcts).expect("sums to 100");
            assert!(
                Rollout::validate_set(&split).is_ok(),
                "partition({pcts:?}) produced a set that does not validate"
            );
        }
    }

    // -----------------------------------------------------------------
    // validate_set
    // -----------------------------------------------------------------

    #[test]
    fn validate_set_accepts_an_exact_partition_in_any_order() {
        let split = Rollout::partition(&[20, 30, 50]).unwrap();
        assert!(Rollout::validate_set(&split).is_ok());

        let reversed: Vec<Rollout> = split.iter().rev().copied().collect();
        assert!(
            Rollout::validate_set(&reversed).is_ok(),
            "partitioning is a property of the set, not of its order"
        );
    }

    #[test]
    fn validate_set_reports_the_first_gap() {
        let gapped = [
            Rollout {
                bucket_start: 0,
                bucket_end: 40,
            },
            Rollout {
                bucket_start: 41,
                bucket_end: 100,
            },
        ];
        assert_eq!(
            Rollout::validate_set(&gapped),
            Err(RolloutError::Gap { bucket: 40 })
        );
    }

    #[test]
    fn validate_set_reports_the_first_overlap() {
        let overlapping = [
            Rollout {
                bucket_start: 0,
                bucket_end: 60,
            },
            Rollout {
                bucket_start: 40,
                bucket_end: 100,
            },
        ];
        assert_eq!(
            Rollout::validate_set(&overlapping),
            Err(RolloutError::Overlap { bucket: 40 }),
            "the lowest affected bucket, so the diagnosis is deterministic"
        );
    }

    #[test]
    fn validate_set_rejects_an_inverted_range_by_its_cause() {
        // Without the per-range check this surfaces as a gap somewhere else,
        // pointing at the wrong thing.
        let inverted = Rollout {
            bucket_start: 60,
            bucket_end: 20,
        };
        let set = [
            Rollout {
                bucket_start: 0,
                bucket_end: 60,
            },
            inverted,
        ];
        assert_eq!(
            Rollout::validate_set(&set),
            Err(RolloutError::InvalidRange { rollout: inverted })
        );
        assert!(
            Rollout::validate_set(&set)
                .unwrap_err()
                .to_string()
                .contains("inverted")
        );
    }

    #[test]
    fn validate_set_rejects_a_range_past_the_bucket_space() {
        // Covers 0..=99 exactly once, so it produces neither gap nor overlap —
        // it would pass a coverage-only check while being nonsense.
        let over = Rollout {
            bucket_start: 0,
            bucket_end: 200,
        };
        assert_eq!(
            Rollout::validate_set(&[over]),
            Err(RolloutError::InvalidRange { rollout: over })
        );
        assert!(
            Rollout::validate_set(&[over])
                .unwrap_err()
                .to_string()
                .contains("past 100")
        );
    }

    #[test]
    fn validate_set_rejects_an_empty_set() {
        let none: [Rollout; 0] = [];
        assert_eq!(
            Rollout::validate_set(&none),
            Err(RolloutError::Gap { bucket: 0 }),
            "no ranges means every bucket is unserved"
        );
    }

    #[test]
    fn a_zero_percent_range_does_not_count_as_covering_its_bucket() {
        // [40,40) is legal but serves nothing, so it cannot fill a gap.
        let set = [
            Rollout {
                bucket_start: 0,
                bucket_end: 40,
            },
            Rollout {
                bucket_start: 40,
                bucket_end: 40,
            },
            Rollout {
                bucket_start: 41,
                bucket_end: 100,
            },
        ];
        assert_eq!(
            Rollout::validate_set(&set),
            Err(RolloutError::Gap { bucket: 40 })
        );
    }
}
