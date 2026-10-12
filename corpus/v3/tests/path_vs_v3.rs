//! `dataflow_corpus::path` is v3's path rules on the corpus's own JSON value.
//! This checks it against v3 itself: random trees, random paths, every result
//! compared as text, so key order is compared too.

use dataflow_corpus::{Json, path};
use dataflow_rs::datavalue::OwnedDataValue;
use dataflow_rs::engine::utils::{remove_nested_value, set_nested_value};

/// xorshift64*, seeded: no dependency, and every failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> usize {
        (self.next() % n) as usize
    }
}

const SEGMENTS: &[&str] = &["a", "b", "0", "1", "#0", "01", "+1", "2"];

fn tree(rng: &mut Rng, depth: u32) -> String {
    match rng.below(if depth == 0 { 3 } else { 6 }) {
        0 => "null".to_owned(),
        1 => rng.below(10).to_string(),
        2 => "\"s\"".to_owned(),
        3 | 4 => {
            let n = rng.below(4);
            let mut keys: Vec<&str> = Vec::new();
            for _ in 0..n {
                let key = ["a", "b", "0", "1", "#0", "c"][rng.below(6)];
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            let body: Vec<String> = keys
                .iter()
                .map(|k| format!("\"{k}\":{}", tree(rng, depth - 1)))
                .collect();
            format!("{{{}}}", body.join(","))
        }
        _ => {
            let n = rng.below(4);
            let body: Vec<String> = (0..n).map(|_| tree(rng, depth - 1)).collect();
            format!("[{}]", body.join(","))
        }
    }
}

fn path_of(rng: &mut Rng) -> String {
    let n = 1 + rng.below(4);
    (0..n)
        .map(|_| SEGMENTS[rng.below(SEGMENTS.len() as u64)])
        .collect::<Vec<_>>()
        .join(".")
}

fn v3_text(value: &OwnedDataValue) -> String {
    serde_json::to_string(value).unwrap()
}

#[test]
fn set_and_remove_agree_with_v3_on_random_trees() {
    let mut rng = Rng(0x00C0_FFEE_D00D_F00D);
    for round in 0..2000 {
        let start = format!("{{\"root\":{}}}", tree(&mut rng, 3));
        let at = format!("root.{}", path_of(&mut rng));
        let value = tree(&mut rng, 1);

        let mut ours = Json::from_text(&start).unwrap();
        let mut theirs: OwnedDataValue = serde_json::from_str(&start).unwrap();
        if rng.below(3) == 0 {
            path::remove(&mut ours, &at);
            remove_nested_value(&mut theirs, &at);
        } else {
            path::set(&mut ours, &at, Json::from_text(&value).unwrap());
            set_nested_value(&mut theirs, &at, serde_json::from_str(&value).unwrap());
        }
        assert_eq!(
            ours.to_text(),
            v3_text(&theirs),
            "round {round}: {start} at {at} with {value}"
        );
    }
}
