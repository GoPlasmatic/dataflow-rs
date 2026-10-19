//! The size test behind the invalidation rule: whether an owned value
//! is large enough that copying it into the view costs more than rebuilding
//! the view after a checkpoint.

use datavalue::{DataValue, OwnedDataValue};

/// Bytes a node costs in an arena: the size of a `DataValue`, or of an
/// object pair, which is what `to_arena` allocates per node.
const NODE: usize = size_of::<DataValue<'static>>();

/// Whether copying `v` into an arena would allocate more than `limit` bytes:
/// `NODE` bytes per node plus every string's length. The walk stops as soon as it
/// passes `limit`, so a small value costs a small walk and a large one costs
/// only `limit / NODE` steps.
pub(crate) fn exceeds(v: &OwnedDataValue, limit: usize) -> bool {
    let mut budget = limit;
    let mut visited = 0;
    !charge(v, NODE, &mut budget, &mut visited)
}

/// Charges `v`'s size (`node_cost` per node plus every string's and key's
/// byte length) against `budget`, returning `false` as soon as the remaining
/// budget cannot cover the current node. Shared by the run context's
/// invalidation check (`node_cost = size_of::<DataValue>()`) and the trace's
/// byte budget (`node_cost = size_of::<usize>()`).
pub(crate) fn charge(
    v: &OwnedDataValue,
    node_cost: usize,
    budget: &mut usize,
    visited: &mut usize,
) -> bool {
    *visited += 1;
    let cost = node_cost
        + match v {
            OwnedDataValue::String(s) => s.len(),
            _ => 0,
        };
    if cost > *budget {
        return false;
    }
    *budget -= cost;
    match v {
        OwnedDataValue::Array(items) => items
            .iter()
            .all(|item| charge(item, node_cost, budget, visited)),
        OwnedDataValue::Object(pairs) => pairs.iter().all(|(k, item)| {
            if k.len() > *budget {
                return false;
            }
            *budget -= k.len();
            charge(item, node_cost, budget, visited)
        }),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // A million nodes: a capped walk stops on the first node that crosses
        // `limit` (`1024 / NODE + 1` = 33 nodes) without visiting the rest.
        let big = OwnedDataValue::Array((0..1_000_000).map(OwnedDataValue::from).collect());
        let mut budget = 1024;
        let mut visited = 0;
        assert!(!charge(&big, NODE, &mut budget, &mut visited));
        assert_eq!(visited, 1024 / NODE + 1);
    }
}
