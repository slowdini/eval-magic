//! Setup-time isolation grouping: decide which evals can share one environment
//! and which must be isolated, *before* a run dispatches anything.
//!
//! One env historically hosted every eval's fixtures, so two evals placing
//! different content at the same path were a hard error. Grouping turns that into
//! a decision: evals whose fixtures conflict (same env-relative dest from a
//! *different* source) are routed into separate groups, and an eval may opt into
//! its own singleton group via [`Isolation::Isolated`]. The realization differs by
//! dispatch mechanism (one env + reset barrier for in-session; one env per
//! `(group, condition)` for CLI), but the grouping decision here is shared.
//!
//! The conflict rule is identical to the per-env fixture-claim rule in
//! [`super::fixtures`]: same dest + same source is an idempotent share (evals may
//! co-group); same dest + different source is a clobber (they must not).

use std::collections::HashMap;

use crate::core::Isolation;

/// One eval's inputs to grouping.
pub struct GroupInput<'a> {
    pub eval_id: &'a str,
    pub isolation: Option<Isolation>,
    /// `(env-relative dest, source)` fixture pairs this eval declares.
    pub fixtures: &'a [(String, String)],
}

/// A computed isolation group: the evals that share one environment, plus a
/// human-readable reason the group exists (surfaced in `dispatch.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub eval_ids: Vec<String>,
    pub rationale: String,
}

/// Group `evals` (in config order) by fixture compatibility and explicit hints.
///
/// Deterministic greedy first-fit: each eval joins the first existing group it
/// does not conflict with; an [`Isolation::Isolated`] eval always gets a fresh,
/// sealed singleton; otherwise a new group is started. Group ids are `g1, g2, …`
/// in creation order. With no conflicts and no `isolated` hints this returns a
/// single `g1` containing every eval — the common case.
pub fn compute_groups(evals: &[GroupInput]) -> Vec<Group> {
    /// A group under construction: its accumulated `dest -> (source, eval_id)`
    /// claims plus whether it is sealed against new members (isolated singletons).
    struct Building {
        id: String,
        eval_ids: Vec<String>,
        claims: HashMap<String, (String, String)>,
        sealed: bool,
        rationale: String,
    }

    fn claims_of(ev: &GroupInput) -> HashMap<String, (String, String)> {
        ev.fixtures
            .iter()
            .map(|(dest, source)| (dest.clone(), (source.clone(), ev.eval_id.to_string())))
            .collect()
    }

    let mut groups: Vec<Building> = Vec::new();

    for ev in evals {
        // An `isolated` eval always gets a fresh, sealed singleton — nothing else
        // may join it, and it joins nothing else.
        if ev.isolation == Some(Isolation::Isolated) {
            let id = format!("g{}", groups.len() + 1);
            groups.push(Building {
                id,
                eval_ids: vec![ev.eval_id.to_string()],
                claims: claims_of(ev),
                sealed: true,
                rationale: "isolation: isolated".to_string(),
            });
            continue;
        }

        // Greedy first-fit over the non-sealed groups, in creation order.
        let mut joined = false;
        let mut conflict_note: Option<String> = None;
        for g in groups.iter_mut().filter(|g| !g.sealed) {
            // The eval conflicts with this group iff it claims a dest the group
            // already holds from a *different* source (an order-dependent clobber).
            // Same dest + same source is an idempotent share — not a conflict.
            let mut conflict: Option<(String, String)> = None;
            for (dest, source) in ev.fixtures {
                if let Some((prev_source, prev_eval)) = g.claims.get(dest)
                    && prev_source != source
                {
                    conflict = Some((dest.clone(), prev_eval.clone()));
                    break;
                }
            }
            match conflict {
                None => {
                    for (dest, source) in ev.fixtures {
                        g.claims
                            .entry(dest.clone())
                            .or_insert_with(|| (source.clone(), ev.eval_id.to_string()));
                    }
                    g.eval_ids.push(ev.eval_id.to_string());
                    joined = true;
                    break;
                }
                Some((dest, other_eval)) => {
                    // Record the first conflict as the new group's rationale, but
                    // keep scanning — a later group may still accept this eval.
                    conflict_note.get_or_insert_with(|| {
                        format!("fixture-conflict: {} vs {other_eval} at {dest}", ev.eval_id)
                    });
                }
            }
        }

        if !joined {
            let id = format!("g{}", groups.len() + 1);
            groups.push(Building {
                id,
                eval_ids: vec![ev.eval_id.to_string()],
                claims: claims_of(ev),
                sealed: false,
                rationale: conflict_note.unwrap_or_else(|| "default".to_string()),
            });
        }
    }

    groups
        .into_iter()
        .map(|b| Group {
            id: b.id,
            eval_ids: b.eval_ids,
            rationale: b.rationale,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(
        id: &'a str,
        isolation: Option<Isolation>,
        fixtures: &'a [(String, String)],
    ) -> GroupInput<'a> {
        GroupInput {
            eval_id: id,
            isolation,
            fixtures,
        }
    }

    fn pair(dest: &str, source: &str) -> (String, String) {
        (dest.to_string(), source.to_string())
    }

    #[test]
    fn single_group_when_no_conflicts_or_hints() {
        let f1 = [pair("a.txt", "/s/a.txt")];
        let f2 = [pair("b.txt", "/s/b.txt")];
        let evals = [
            input("e1", None, &f1),
            input("e2", None, &f2),
            input("e3", None, &[]),
        ];
        let groups = compute_groups(&evals);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "g1");
        assert_eq!(groups[0].eval_ids, vec!["e1", "e2", "e3"]);
        assert_eq!(groups[0].rationale, "default");
    }

    #[test]
    fn conflicting_fixtures_split_into_two_groups() {
        let f1 = [pair("config.json", "/a/config.json")];
        let f2 = [pair("config.json", "/b/config.json")];
        let evals = [input("e1", None, &f1), input("e2", None, &f2)];
        let groups = compute_groups(&evals);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].eval_ids, vec!["e1"]);
        assert_eq!(groups[1].id, "g2");
        assert_eq!(groups[1].eval_ids, vec!["e2"]);
        assert!(
            groups[1].rationale.contains("fixture-conflict"),
            "rationale: {}",
            groups[1].rationale
        );
        assert!(
            groups[1].rationale.contains("config.json"),
            "rationale: {}",
            groups[1].rationale
        );
    }

    #[test]
    fn idempotent_same_source_share_stays_one_group() {
        let f = [pair("config.json", "/a/config.json")];
        let evals = [input("e1", None, &f), input("e2", None, &f)];
        let groups = compute_groups(&evals);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].eval_ids, vec!["e1", "e2"]);
    }

    #[test]
    fn isolated_hint_forces_singleton_and_seals_it() {
        let evals = [
            input("e1", Some(Isolation::Isolated), &[]),
            input("e2", None, &[]),
            input("e3", None, &[]),
        ];
        let groups = compute_groups(&evals);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].eval_ids, vec!["e1"]);
        assert_eq!(groups[0].rationale, "isolation: isolated");
        // The shared evals never join the sealed singleton.
        assert_eq!(groups[1].eval_ids, vec!["e2", "e3"]);
        assert_eq!(groups[1].rationale, "default");
    }

    #[test]
    fn isolated_eval_with_fixtures_is_still_a_singleton() {
        let f = [pair("x.txt", "/s/x.txt")];
        let evals = [input("e1", Some(Isolation::Isolated), &f)];
        let groups = compute_groups(&evals);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].eval_ids, vec!["e1"]);
        assert_eq!(groups[0].rationale, "isolation: isolated");
    }

    #[test]
    fn ids_are_deterministic_in_creation_order() {
        let f1 = [pair("c.json", "/a/c.json")];
        let f2 = [pair("c.json", "/b/c.json")];
        let f3 = [pair("c.json", "/d/c.json")];
        let evals = [
            input("e1", None, &f1),
            input("e2", None, &f2),
            input("e3", None, &f3),
        ];
        let groups = compute_groups(&evals);
        assert_eq!(
            groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
            vec!["g1", "g2", "g3"]
        );
    }

    #[test]
    fn eval_joins_first_non_conflicting_group() {
        // e1 -> g1 (claims `a` from /s1). e2 conflicts on `a` -> g2. e3 shares `a`
        // from /s1 (same source as g1) -> rejoins g1, not g2.
        let f1 = [pair("a", "/s1/a")];
        let f2 = [pair("a", "/s2/a")];
        let f3 = [pair("a", "/s1/a")];
        let evals = [
            input("e1", None, &f1),
            input("e2", None, &f2),
            input("e3", None, &f3),
        ];
        let groups = compute_groups(&evals);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].eval_ids, vec!["e1", "e3"]);
        assert_eq!(groups[1].eval_ids, vec!["e2"]);
    }
}
