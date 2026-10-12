//! Near-miss detection for unknown keys.

/// The nearest known key to `key`, if one is close enough to be a misspelling.
///
/// Distance is optimal string alignment (insert, delete, substitute and
/// adjacent transposition, each cost 1). The threshold is 1 for a key of up to
/// five characters, else 2. Ties go to the first in `known`. An exact match is
/// not a near miss.
pub(crate) fn near_miss<'k>(key: &str, known: &[&'k str]) -> Option<&'k str> {
    let key: Vec<char> = key.chars().collect();
    let threshold = if key.len() <= 5 { 1 } else { 2 };
    let mut best: Option<(usize, &'k str)> = None;
    for &candidate in known {
        let distance = osa(&key, &candidate.chars().collect::<Vec<_>>());
        if distance == 0 || distance > threshold {
            continue;
        }
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, k)| k)
}

fn osa(a: &[char], b: &[char]) -> usize {
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

    #[test]
    fn ties_go_to_the_first_and_exact_matches_are_not_near() {
        assert_eq!(near_miss("ab", &["ac", "ad"]), Some("ac"));
        assert_eq!(near_miss("id", WORKFLOW_KEYS), None);
    }
}
