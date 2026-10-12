//! Near-miss detection for unknown keys, and what an unknown key is reported
//! as.

use super::IssueCode;

/// What an unknown `key` is: a `LIKELY_TYPO` naming the nearest of
/// `candidates` when it is a near miss of one, otherwise an `UNKNOWN_KEY`,
/// each with its message. The one classification behind every unknown key,
/// in a definition and in a function's config; the caller decides the
/// severity, where the issue goes, and what it adds to the message.
pub(crate) fn unknown_key(key: &str, candidates: &[&str]) -> (IssueCode, String) {
    match near_miss(key, candidates) {
        Some(near) => (
            IssueCode::LikelyTypo,
            format!("unknown key `{key}`, did you mean `{near}`?"),
        ),
        None => (IssueCode::UnknownKey, format!("unknown key `{key}`")),
    }
}

/// The nearest known key to `key`, if one is close enough to be a misspelling.
///
/// Distance is optimal string alignment (insert, delete, substitute and
/// adjacent transposition, each cost 1). The threshold is 1 for a key of up to
/// five characters, else 2. Ties go to the first in `known`. An exact match is
/// not a near miss.
///
/// The key is untrusted. A candidate whose length differs from the key's by
/// more than the threshold is skipped before any distance is computed (the
/// distance is at least the length difference), so a key far longer than
/// every known key costs one pass to count its characters and nothing more.
pub(crate) fn near_miss<'k>(key: &str, known: &[&'k str]) -> Option<&'k str> {
    let len = key.chars().count();
    let threshold = if len <= 5 { 1 } else { 2 };
    let mut chars: Option<Vec<char>> = None;
    let mut best: Option<(usize, &'k str)> = None;
    for &candidate in known {
        if len.abs_diff(candidate.chars().count()) > threshold {
            continue;
        }
        let key = chars.get_or_insert_with(|| key.chars().collect());
        let distance = osa(key, &candidate.chars().collect::<Vec<_>>());
        if distance == 0 || distance > threshold {
            continue;
        }
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, k)| k)
}

/// The optimal string alignment distance, in O(`b.len()`) memory: the rows are
/// rolled, keeping the two before the current one for the transposition.
fn osa(a: &[char], b: &[char]) -> usize {
    let m = b.len();
    let mut two_back = vec![0usize; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut row = vec![0usize; m + 1];
    for i in 1..=a.len() {
        row[0] = i;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(row[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(two_back[j - 2] + 1);
            }
            row[j] = v;
        }
        // Roll: the current row becomes `prev`, `prev` becomes `two_back`,
        // and the old `two_back` is overwritten next.
        std::mem::swap(&mut two_back, &mut prev);
        std::mem::swap(&mut prev, &mut row);
    }
    prev[m]
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW_KEYS: &[&str] = &[
        "id",
        "name",
        "description",
        "priority",
        "condition",
        "tasks",
        "on_failure",
        "channel",
        "status",
        "version",
        "rollout",
        "loop",
        "tags",
        "created_at",
        "updated_at",
        "ext",
    ];
    const TASK_KEYS: &[&str] = &[
        "id",
        "name",
        "description",
        "condition",
        "function",
        "on_failure",
        "on_rejection",
        "terminal",
        "for_each",
        "ext",
    ];

    #[test]
    fn a_missing_letter_is_found() {
        assert_eq!(near_miss("conditon", TASK_KEYS), Some("condition"));
    }

    #[test]
    fn the_threshold_scales_with_the_key_length() {
        // "ui" is distance 2 from "id"; a key of up to five characters allows 1.
        assert_eq!(near_miss("ui", WORKFLOW_KEYS), None);
        assert_eq!(near_miss("path", WORKFLOW_KEYS), None);
    }

    #[test]
    fn a_transposition_costs_one() {
        assert_eq!(near_miss("tpoic", &["topic"]), Some("topic"));
    }

    /// The full-matrix OSA the rolled one replaced.
    fn osa_matrix(a: &[char], b: &[char]) -> usize {
        let (n, m) = (a.len(), b.len());
        let mut d = vec![vec![0usize; m + 1]; n + 1];
        for (i, row) in d.iter_mut().enumerate() {
            row[0] = i;
        }
        for (j, cell) in d[0].iter_mut().enumerate() {
            *cell = j;
        }
        for i in 1..=n {
            for j in 1..=m {
                let cost = usize::from(a[i - 1] != b[j - 1]);
                let mut v = (d[i - 1][j] + 1)
                    .min(d[i][j - 1] + 1)
                    .min(d[i - 1][j - 1] + cost);
                if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                    v = v.min(d[i - 2][j - 2] + 1);
                }
                d[i][j] = v;
            }
        }
        d[n][m]
    }

    #[test]
    fn the_rolled_distance_equals_the_full_matrix() {
        let words: Vec<&str> = WORKFLOW_KEYS
            .iter()
            .chain(TASK_KEYS)
            .copied()
            .chain([
                "",
                "a",
                "ab",
                "ba",
                "abc",
                "acb",
                "ca",
                "abcd",
                "badc",
                "conditon",
                "maxx",
                "ass",
                "tpoic",
                "bucket_strat",
                "inptu",
                "ébc",
                "cbé",
            ])
            .collect();
        for a in &words {
            for b in &words {
                let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
                assert_eq!(osa(&a, &b), osa_matrix(&a, &b), "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn a_key_far_longer_than_every_candidate_is_no_near_miss() {
        // Every candidate is skipped on length alone: no distance is computed.
        let key = "x".repeat(1_000_000);
        assert_eq!(near_miss(&key, WORKFLOW_KEYS), None);
        // A key one longer than the threshold allows is skipped too.
        assert_eq!(near_miss("conditionxyz", TASK_KEYS), None);
        assert_eq!(near_miss("conditionx", TASK_KEYS), Some("condition"));
    }

    #[test]
    fn ties_go_to_the_first_and_exact_matches_are_not_near() {
        assert_eq!(near_miss("ab", &["ac", "ad"]), Some("ac"));
        assert_eq!(near_miss("id", WORKFLOW_KEYS), None);
    }

    #[test]
    fn an_unknown_key_is_a_typo_naming_its_near_key_or_unknown() {
        assert_eq!(
            unknown_key("conditon", TASK_KEYS),
            (
                IssueCode::LikelyTypo,
                "unknown key `conditon`, did you mean `condition`?".to_owned()
            )
        );
        assert_eq!(
            unknown_key("owner", TASK_KEYS),
            (IssueCode::UnknownKey, "unknown key `owner`".to_owned())
        );
    }
}
