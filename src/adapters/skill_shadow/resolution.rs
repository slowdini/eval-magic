//! Shared runtime-id resolution policies.

use super::*;

fn sources_by_runtime_id(sources: &[ShadowSource]) -> BTreeMap<String, Vec<usize>> {
    let mut grouped = BTreeMap::<String, Vec<usize>>::new();
    for (index, source) in sources.iter().enumerate() {
        grouped
            .entry(source.runtime_id.clone())
            .or_default()
            .push(index);
    }
    grouped
}

/// Resolve duplicate runtime ids using an ordered root-scope policy.
pub(crate) fn resolve_by_precedence(sources: &mut [ShadowSource]) {
    let grouped = sources_by_runtime_id(sources);
    for indices in grouped.values() {
        let ranks = indices
            .iter()
            .map(|index| match sources[*index].root.scope {
                ShadowRootScope::Admin => 0,
                ShadowRootScope::Global => 1,
                ShadowRootScope::Project => 2,
                ShadowRootScope::Unknown => 3,
            })
            .collect::<Vec<_>>();
        let selected_rank = ranks.iter().copied().min().unwrap_or_default();
        for (index, rank) in indices.iter().zip(ranks) {
            let resolution = if rank == selected_rank {
                ShadowResolution::Selected
            } else {
                ShadowResolution::Shadowed
            };
            for appearance in &mut sources[*index].appearances {
                appearance.resolution = resolution;
                appearance.precedence_rank = Some(rank);
            }
        }
    }
}

/// Resolve duplicate runtime ids as simultaneously discoverable.
pub(crate) fn resolve_as_coexisting(sources: &mut [ShadowSource]) {
    let grouped = sources_by_runtime_id(sources);
    for indices in grouped.values() {
        let resolution = if indices.len() > 1 {
            ShadowResolution::Coexisting
        } else {
            ShadowResolution::Selected
        };
        for index in indices {
            for appearance in &mut sources[*index].appearances {
                appearance.resolution = resolution;
                appearance.precedence_rank = None;
            }
        }
    }
}

fn normalize_skill_location(path: &str) -> String {
    let path = Path::new(path);
    let dir = if path.file_name().is_some_and(|name| name == "SKILL.md") {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    dir.canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Resolve duplicate runtime ids from a harness probe's selected locations.
pub(crate) fn resolve_from_selected_paths(
    sources: &mut [ShadowSource],
    selected_paths: &BTreeMap<String, String>,
) {
    let grouped = sources_by_runtime_id(sources);
    for (runtime_id, indices) in grouped {
        if indices.len() == 1 {
            for appearance in &mut sources[indices[0]].appearances {
                appearance.resolution = ShadowResolution::Selected;
                appearance.precedence_rank = None;
            }
            continue;
        }
        let selected = selected_paths
            .get(&runtime_id)
            .map(|path| normalize_skill_location(path));
        for index in indices {
            let source_path = sources[index]
                .canonical_path
                .as_deref()
                .unwrap_or(&sources[index].discovery_path);
            let resolution = selected
                .as_ref()
                .map_or(ShadowResolution::Unknown, |selected| {
                    if normalize_skill_location(source_path) == *selected {
                        ShadowResolution::Selected
                    } else {
                        ShadowResolution::Shadowed
                    }
                });
            for appearance in &mut sources[index].appearances {
                appearance.resolution = resolution;
                appearance.precedence_rank = None;
            }
        }
    }
}
