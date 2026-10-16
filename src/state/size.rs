//! The size test behind the invalidation rule: whether an owned value
//! is large enough that copying it into the view costs more than rebuilding
//! the view after a checkpoint.

use datavalue::OwnedDataValue;

/// Bytes a node costs in an arena: the size of a `DataValue`, or of an
/// object pair, which is what `to_arena` allocates per node.
const NODE: usize = 32;

/// Whether copying `v` into an arena would allocate more than `limit` bytes:
/// `NODE` bytes per node plus every string's length. The walk stops as soon as it
/// passes `limit`, so a small value costs a small walk and a large one costs
/// only `limit / NODE` steps.
pub(crate) fn exceeds(v: &OwnedDataValue, limit: usize) -> bool {
    let mut budget = limit;
    !fits(v, &mut budget)
}

/// Whether `v` fits in what is left of `budget`, which it spends.
fn fits(v: &OwnedDataValue, budget: &mut usize) -> bool {
    let cost = NODE
        + match v {
            OwnedDataValue::String(s) => s.len(),
            _ => 0,
        };
    if cost > *budget {
        return false;
    }
    *budget -= cost;
    match v {
        OwnedDataValue::Array(items) => items.iter().all(|item| fits(item, budget)),
        OwnedDataValue::Object(pairs) => pairs.iter().all(|(k, item)| {
            if k.len() > *budget {
                return false;
            }
            *budget -= k.len();
            fits(item, budget)
        }),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn small_values_fit_and_large_ones_do_not() {
        let small = OwnedDataValue::Object(vec![("k".into(), OwnedDataValue::from("v"))]);
        assert!(!exceeds(&small, 1024));
        let long = OwnedDataValue::String("x".repeat(2000));
        assert!(exceeds(&long, 1024));
        assert!(!exceeds(&long, 4096));
    }

    #[test]
    fn the_walk_stops_at_the_limit() {
        // A million nodes: a full walk takes milliseconds, a capped one
        // microseconds.
        let big = OwnedDataValue::Array((0..1_000_000).map(OwnedDataValue::from).collect());
        let start = Instant::now();
        for _ in 0..1000 {
            assert!(exceeds(&big, 1024));
        }
        let per = start.elapsed() / 1000;
        assert!(per.as_micros() < 50, "{per:?} per capped walk");
    }
}
