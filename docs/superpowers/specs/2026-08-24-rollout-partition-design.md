# Rollout partitioning and set validation (#48)

**Issue:** [GoPlasmatic/dataflow-rs#48](https://github.com/GoPlasmatic/dataflow-rs/issues/48)
**Target:** v3.7.0
**Date:** 2026-08-24

## Problem

The crate owns the rollout *model* — `Rollout { bucket_start, bucket_end }`
half-open, `Message::routing_bucket`, and `accepts(bucket)` gating in the
workflow executor. It owns none of the *invariants* that make the model safe:

- `Workflow::validate()` does not look at `rollout` at all. A set of workflow
  versions whose ranges overlap (two versions both match bucket 40) or leave a
  gap (bucket 40 matches nothing — traffic silently blackholed) is accepted
  without comment.
- Every host doing percentage-based rollout re-derives the same arithmetic:
  percentages → contiguous half-open ranges covering exactly 0–99.

Orion implements both by hand (`engine/loader.rs:234-309`), walking versions
newest-first accumulating an offset and refusing a set that does not sum to
exactly 100 — with the error distinguishing under-100 (*"traffic matches
nothing"*) from over-100 (*"later versions unreachable"*), because both failure
modes were real. That arithmetic has no host-specific content; it is the missing
second half of a model this crate already ships.

Bucket *derivation* — mapping a caller to 0–99 — is correctly documented as the
caller's policy and stays out of scope.

## API

```rust
impl Rollout {
    pub fn partition(percentages: &[u8]) -> Result<Vec<Rollout>, RolloutError>;
    pub fn validate_set<'a>(
        rollouts: impl IntoIterator<Item = &'a Rollout>,
    ) -> Result<(), RolloutError>;
}

pub enum RolloutError {
    /// Percentages sum to less than 100. The shortfall matches nothing.
    Under { total: u32 },
    /// Percentages sum to more than 100. The excess pushes later entries past
    /// the end of the bucket space, where they can never match.
    Over { total: u32 },
    /// No range in the set serves this bucket.
    Gap { bucket: u8 },
    /// More than one range serves this bucket.
    Overlap { bucket: u8 },
    /// A range is inverted or reaches past bucket 100.
    InvalidRange { rollout: Rollout },
}
```

`RolloutError` is its own type rather than a `DataflowError` variant: these are
pure arithmetic helpers with no engine involvement, and routing them through
`DataflowError` would drag in retryability classification that means nothing
here. It implements `Display` and `std::error::Error`.

## Design decisions

### `u32` accumulator — the one way this can be quietly wrong

Percentages are `u8`, so `partition(&[200, 200])` sums to 400. A `u8`
accumulator wraps to 144; worse, `partition(&[200, 56])` wraps to exactly 0 and
`partition(&[128, 128])` wraps to 0 too — a set that would **pass** a naive
`== 100` check. Accumulating in `u32` is what makes the check total.

### A 0% entry is allowed

`partition(&[100, 0])` yields `[0,100)` and `[100,100)`. The second is empty and
`accepts` nothing — which is exactly what 0% means, and is the natural way to
express "this version is staged but takes no traffic". It creates neither a gap
nor an overlap, so `validate_set` accepts it too. Documented rather than
rejected; the two functions agree.

### `validate_set` counts acceptors per bucket

For each of the 100 buckets, count how many ranges accept it; report the first
bucket with zero (`Gap`) or more than one (`Overlap`), scanning in bucket order
so the diagnosis is deterministic and points at the lowest affected traffic.

O(100·n) with n version-ranges is nothing at this scale, and the implementation
is obviously correct against the definition rather than correct-by-argument the
way a sort-and-sweep would be.

### Malformed ranges are diagnosed by their cause

Individual ranges are checked *before* the coverage scan, so a broken range gets
a precise diagnosis instead of surfacing as a confusing downstream symptom:

- An **inverted** range (`bucket_end < bucket_start`) accepts nothing. Without
  this check it appears only as `Gap { bucket }` somewhere else in the space,
  pointing at the wrong place.
- A range reaching **past 100** creates neither gap nor overlap — `[0, 200)`
  covers 0–99 exactly once — so it would pass silently despite `bucket_end`
  being nonsense against a model documented as `0..100`.

`bucket_end == bucket_start` is *not* invalid: that is the 0% case above.

### Declined: warning at `Engine::build()`

The issue asks whether `build()` should warn on a non-partitioning set for
same-`id` workflow groups. It should not. A `Workflow` does not know which
version-set it belongs to — that grouping is the host's concept, expressed in
whatever storage schema the host uses, and the crate would have to invent one to
check it. The helpers alone remove the per-host arithmetic, which is what the
issue is actually for.

## Placement

New module `src/engine/rollout.rs` holding `Rollout`, its helpers,
`RolloutError` and their tests. `workflow.rs` is already 935 lines carrying
`Rollout`, `LoopConfig`, `WorkflowStatus`, `Workflow` and `ConnectorRef`;
appending ~200 lines of traffic-splitting arithmetic would make a crowded file
worse. Re-exported from `workflow.rs` and `engine/mod.rs`, so every existing
import path keeps working.

Same call as `steps.rs` in #42: a concern with its own invariants gets its own
file.

## Testing

| Test | Pins |
|---|---|
| `partition_splits_the_bucket_space_contiguously` | `[100]`, `[90,10]`, `[34,33,33]` — the acceptance criterion. |
| `partition_rejects_a_shortfall_naming_the_direction` | 99 → `Under`, and the message says traffic matches nothing. |
| `partition_rejects_an_excess_naming_the_direction` | 101 → `Over`, and the message says later entries are unreachable. |
| `partition_does_not_wrap_on_a_large_sum` | `[128,128]` and `[200,56]` — sums that wrap a `u8` to exactly 0 and would otherwise pass. |
| `a_zero_percent_entry_is_an_empty_range_that_accepts_nothing` | The documented 0% reading, and that `validate_set` agrees. |
| `partition_output_always_validates` | Property over many splits: `validate_set(&partition(p)?)` is `Ok`. |
| `validate_set_accepts_an_exact_partition_in_any_order` | Order-independence, per the issue. |
| `validate_set_reports_the_first_gap` | Lowest uncovered bucket. |
| `validate_set_reports_the_first_overlap` | Lowest doubly-covered bucket. |
| `validate_set_rejects_an_inverted_range_by_its_cause` | `InvalidRange`, not a misleading `Gap` elsewhere. |
| `validate_set_rejects_a_range_past_the_bucket_space` | `[0,200)` — covers 0–99 cleanly, still nonsense. |
| `validate_set_rejects_an_empty_set` | Zero ranges is a gap at bucket 0. |

## Documentation

- Compiled rustdoc examples on both methods.
- A section in the rollout docs pointing hosts at the helpers, per the issue.

## Compatibility

Additive. `Rollout` moves module but is re-exported from its current paths, so
no import breaks. No behaviour change to matching.

## Verification

```
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy -p dataflow-rs --all-targets -- -D warnings
cargo test --workspace --all-features
cargo test -p dataflow-rs
cargo +1.85 check --workspace --all-targets --all-features --locked
```

MSRV 1.85: nested `if let`, never let-chains. Test counts in `CLAUDE.md`
(544 / 463) move and are updated against a measured baseline.
