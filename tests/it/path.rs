// `Path` against v3's rules, as `dataflow_corpus::path` records them: random
// contexts, random paths below a random root, every result compared as text
// so key order counts.

use dataflow_corpus::{Json, path};
use dataflow_rs::Path;
use datavalue::OwnedDataValue;

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

const SEED: u64 = 0x00C0_FFEE_D00D_F00D;
/// Segments, `1048576` among them: one past the index cap, so a key even on
/// an array.
const SEGMENTS: &[&str] = &["a", "b", "0", "1", "#0", "01", "+1", "2", "1048576"];
const ROOTS: &[&str] = &["data", "metadata", "temp_data"];

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
    let root = ROOTS[rng.below(3)];
    let n = 1 + rng.below(4);
    let segs: Vec<&str> = (0..n)
        .map(|_| SEGMENTS[rng.below(SEGMENTS.len() as u64)])
        .collect();
    format!("{root}.{}", segs.join("."))
}

fn text(value: &OwnedDataValue) -> String {
    serde_json::to_string(value).unwrap()
}

#[test]
fn the_index_cap_is_the_models() {
    // The cap's two sides on an array: the last index pads it, one past is
    // a key, which an array cannot take.
    assert_eq!(path::MAX_INDEX, dataflow_rs::expr::MAX_ARRAY_INDEX);
    for raw in ["data.list.1048575", "data.list.1048576"] {
        let start = r#"{"data":{"list":[]},"metadata":{},"temp_data":{}}"#;
        let mut ours = Json::from_text(start).unwrap();
        let mut theirs: OwnedDataValue = serde_json::from_str(start).unwrap();
        path::set(&mut ours, raw, Json::from_text("1").unwrap());
        let parsed: Path = raw.parse().unwrap();
        parsed.set(&mut theirs, OwnedDataValue::from(1)).unwrap();
        assert_eq!(text(&theirs), ours.to_text(), "{raw}");
    }
}

#[test]
fn debug_prints_the_dotted_text() {
    let path: Path = "metadata.#errors.0".parse().unwrap();
    assert_eq!(format!("{path:?}"), r##"Path("metadata.#errors.0")"##);
}

#[test]
fn get_set_and_remove_agree_with_v3_on_random_contexts() {
    let mut rng = Rng(SEED);
    for round in 0..2000 {
        let start = format!(
            "{{\"data\":{},\"metadata\":{},\"temp_data\":{}}}",
            tree(&mut rng, 3),
            tree(&mut rng, 3),
            tree(&mut rng, 3)
        );
        let at = path_of(&mut rng);
        let value = tree(&mut rng, 1);
        let parsed: Path = at.parse().unwrap();
        let context = format!("seed {SEED:#x} round {round}: {start} at {at} with {value}");

        // get
        let ours = Json::from_text(&start).unwrap();
        let theirs: OwnedDataValue = serde_json::from_str(&start).unwrap();
        assert_eq!(
            parsed.get(&theirs).map(text),
            path::get(&ours, &at).map(Json::to_text),
            "get, {context}"
        );

        // set
        let mut ours = Json::from_text(&start).unwrap();
        let mut theirs: OwnedDataValue = serde_json::from_str(&start).unwrap();
        path::set(&mut ours, &at, Json::from_text(&value).unwrap());
        parsed
            .set(&mut theirs, serde_json::from_str(&value).unwrap())
            .unwrap();
        assert_eq!(text(&theirs), ours.to_text(), "set, {context}");

        // remove
        let mut ours = Json::from_text(&start).unwrap();
        let mut theirs: OwnedDataValue = serde_json::from_str(&start).unwrap();
        path::remove(&mut ours, &at);
        parsed.remove(&mut theirs).unwrap();
        assert_eq!(text(&theirs), ours.to_text(), "remove, {context}");
    }
}
