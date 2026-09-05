//! Deterministic private task-environment planning.
//!
//! Every eval has its own group, and every run within that group receives its
//! own environment per condition. Group ids remain stable artifact join keys.

/// One eval's inputs to grouping.
pub struct GroupInput<'a> {
    pub eval_id: &'a str,
    /// Effective run count, retained for per-run environment planning.
    pub runs: u32,
}

/// A private eval group surfaced in `dispatch.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub eval_ids: Vec<String>,
    pub rationale: String,
    /// Multi-run values fan the group out into one environment per run.
    pub runs: u32,
}

/// Assign one deterministic group to each eval in config order.
pub fn compute_groups(evals: &[GroupInput<'_>]) -> Vec<Group> {
    evals
        .iter()
        .enumerate()
        .map(|(index, eval)| Group {
            id: format!("g{}", index + 1),
            eval_ids: vec![eval.eval_id.to_string()],
            rationale: "private codebase".to_string(),
            runs: eval.runs,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_eval_gets_a_deterministic_private_group() {
        let evals = [
            GroupInput {
                eval_id: "first",
                runs: 1,
            },
            GroupInput {
                eval_id: "second",
                runs: 2,
            },
        ];

        let groups = compute_groups(&evals);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].id, "g1");
        assert_eq!(groups[0].eval_ids, vec!["first"]);
        assert_eq!(groups[0].rationale, "private codebase");
        assert_eq!(groups[0].runs, 1);
        assert_eq!(groups[1].id, "g2");
        assert_eq!(groups[1].eval_ids, vec!["second"]);
        assert_eq!(groups[1].runs, 2);
    }

    #[test]
    fn empty_selection_produces_no_groups() {
        assert!(compute_groups(&[]).is_empty());
    }
}
