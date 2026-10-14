//! Seeded random trees, paths and values for the state property tests. No
//! dependency: a xorshift64* generator, so every failure reproduces from its
//! seed and case number.

use crate::expr::Seg;
use datavalue::OwnedDataValue;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// True with probability `num / den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next() % den < num
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Object keys: plain words, keys that look like indices, a key that needs
/// the `#` escape to be named, and the empty key that `a..b` names.
pub const KEYS: &[&str] = &["a", "b", "c", "0", "1", "20", "#h", ""];

/// Segments that are not taken from the tree: indices past the end,
/// escaped numbers (never an index), and the forms `usize::from_str` accepts
/// that a reader might not expect.
pub const WILD: &[&str] = &[
    "a", "x", "0", "1", "2", "5", "20", "#20", "#0", "##h", "#h", "#a", "01", "+1", "",
];

pub fn scalar(rng: &mut Rng) -> OwnedDataValue {
    match rng.below(5) {
        0 => OwnedDataValue::Null,
        1 => OwnedDataValue::Bool(rng.chance(1, 2)),
        2 => OwnedDataValue::from(rng.below(100) as i64),
        3 => OwnedDataValue::from(rng.below(1000) as f64 / 8.0),
        _ => OwnedDataValue::String(format!("s{}", rng.below(50))),
    }
}

/// A random value at most `depth` containers deep.
pub fn value_at(rng: &mut Rng, depth: usize) -> OwnedDataValue {
    if depth == 0 || rng.chance(2, 5) {
        return scalar(rng);
    }
    if rng.chance(1, 2) {
        let n = rng.below(5);
        OwnedDataValue::Array((0..n).map(|_| value_at(rng, depth - 1)).collect())
    } else {
        object(rng, depth)
    }
}

fn object(rng: &mut Rng, depth: usize) -> OwnedDataValue {
    let mut pairs: Vec<(String, OwnedDataValue)> = Vec::new();
    for _ in 0..rng.below(5) {
        let key = rng.pick(KEYS).to_string();
        if !pairs.iter().any(|(k, _)| *k == key) {
            pairs.push((key, value_at(rng, depth - 1)));
        }
    }
    OwnedDataValue::Object(pairs)
}

/// A random tree whose root is an object, as a message context always is.
pub fn tree(rng: &mut Rng, depth: usize) -> OwnedDataValue {
    object(rng, depth.max(1))
}

/// A small value to write.
pub fn value(rng: &mut Rng) -> OwnedDataValue {
    value_at(rng, 2)
}

/// A dotted path of one to five segments, each taken from the tree where
/// the walk is still inside it (an existing key, spelled plain or escaped,
/// or an index), and otherwise from [`WILD`]. Rarely the empty path.
pub fn path(rng: &mut Rng, tree: &OwnedDataValue) -> String {
    if rng.chance(1, 200) {
        return String::new();
    }
    walk(rng, tree)
}

fn walk(rng: &mut Rng, tree: &OwnedDataValue) -> String {
    let len = 1 + rng.below(5);
    let mut segs: Vec<String> = Vec::with_capacity(len);
    let mut at = Some(tree);
    for _ in 0..len {
        let from_tree = rng.chance(3, 4);
        let seg = match (at, from_tree) {
            (Some(OwnedDataValue::Object(pairs)), true) if !pairs.is_empty() => {
                let (key, child) = &pairs[rng.below(pairs.len())];
                at = Some(child);
                // A key starting with `#` can only be named escaped; any
                // other key either way.
                if key.starts_with('#') || rng.chance(1, 4) {
                    format!("#{key}")
                } else {
                    key.clone()
                }
            }
            (Some(OwnedDataValue::Array(items)), true) if !items.is_empty() => {
                let i = rng.below(items.len() + 1);
                at = items.get(i);
                i.to_string()
            }
            _ => {
                at = None;
                rng.pick(WILD).to_string()
            }
        };
        segs.push(seg);
    }
    segs.join(".")
}

/// `v` as compact JSON. `OwnedDataValue`'s `==` ignores object key order, but
/// the wire does not, so every comparison of trees goes through this.
pub fn exact(v: &OwnedDataValue) -> String {
    serde_json::to_string(v).expect("a value serializes")
}

/// The two strings around their first difference, for a readable failure.
pub fn diff(a: &str, b: &str) -> String {
    let at = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let from = at.saturating_sub(120);
    let clip = |s: &str| {
        s.get(from..(at + 120).min(s.len()))
            .unwrap_or("")
            .to_string()
    };
    format!("at byte {at}:\n  got  …{}…\n  want …{}…", clip(a), clip(b))
}

/// The context roots, in the order a context holds them.
pub const ROOTS: [&str; 3] = ["data", "metadata", "temp_data"];

/// A context: an object holding the three roots, each a random object.
pub fn rooted_tree(rng: &mut Rng, depth: usize) -> OwnedDataValue {
    OwnedDataValue::Object(
        ROOTS
            .iter()
            .map(|root| ((*root).to_string(), object(rng, depth.max(2))))
            .collect(),
    )
}

/// A dotted path a `Path` parses: a root, and below it what [`path`] would
/// walk in that root's subtree. Rarely the root alone.
pub fn rooted_path(rng: &mut Rng, ctx: &OwnedDataValue) -> String {
    let root = *rng.pick(&ROOTS);
    if rng.chance(1, 50) {
        return root.to_string();
    }
    let below = match ctx {
        OwnedDataValue::Object(pairs) => pairs.iter().find(|(k, _)| k == root).map(|(_, v)| v),
        _ => None,
    };
    let rest = match below {
        Some(subtree) => walk(rng, subtree),
        None => walk(rng, &OwnedDataValue::Null),
    };
    format!("{root}.{rest}")
}

/// A dotted path as segments, with no root required: the empty string is no
/// segments, as v3's helpers read it.
pub fn segs(dotted: &str) -> Vec<Seg> {
    if dotted.is_empty() {
        Vec::new()
    } else {
        dotted.split('.').map(Seg::parse).collect()
    }
}

/// v3's `merge_root_field` (`src/engine/functions/map.rs`), verbatim: the
/// reference for `map`'s write to a root slot.
pub fn v3_merge_root(context: &mut OwnedDataValue, path: &str, new_value: OwnedDataValue) {
    let OwnedDataValue::Object(ctx_pairs) = context else {
        *context = OwnedDataValue::Object(vec![(path.to_string(), new_value)]);
        return;
    };
    let slot_idx = ctx_pairs.iter().position(|(k, _)| k == path);
    match slot_idx {
        Some(idx) => {
            let slot = &mut ctx_pairs[idx].1;
            match (slot, new_value) {
                (OwnedDataValue::Object(existing), OwnedDataValue::Object(new_pairs)) => {
                    for (k, v) in new_pairs {
                        if let Some(s) = existing.iter_mut().find(|(ek, _)| ek == &k) {
                            s.1 = v;
                        } else {
                            existing.push((k, v));
                        }
                    }
                }
                (slot, new) => *slot = new,
            }
        }
        None => ctx_pairs.push((path.to_string(), new_value)),
    }
}

/// `map`'s write to a root slot as the run's owned side does it (the spike's
/// `path::merge_root`): two objects merge, existing keys replaced in place and
/// new keys appended in order; anything else replaces the slot. Checked
/// against [`v3_merge_root`] in `splice_property`.
pub fn owned_merge_root(context: &mut OwnedDataValue, key: &str, value: OwnedDataValue) {
    let OwnedDataValue::Object(slots) = context else {
        *context = OwnedDataValue::Object(vec![(key.to_string(), value)]);
        return;
    };
    let Some(slot) = slots.iter_mut().find(|(k, _)| k == key) else {
        slots.push((key.to_string(), value));
        return;
    };
    match (&mut slot.1, value) {
        (OwnedDataValue::Object(existing), OwnedDataValue::Object(new_pairs)) => {
            for (k, v) in new_pairs {
                match existing.iter_mut().find(|(ek, _)| *ek == k) {
                    Some(s) => s.1 = v,
                    None => existing.push((k, v)),
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

/// Whether `DATAFLOW_V4_DEEP` is set (to anything): the model test then runs
/// its full sequence count.
pub fn deep() -> bool {
    std::env::var_os("DATAFLOW_V4_DEEP").is_some()
}
