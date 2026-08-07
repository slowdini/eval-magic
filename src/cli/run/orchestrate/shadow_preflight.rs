//! Assemble the harness-neutral skill-shadow report across every comparison cell.

use std::collections::BTreeSet;

use crate::adapters::adapter_for;
use crate::adapters::skill_shadow::{
    PluginShadowArtifact, PluginShadowReport, ShadowAppearance, ShadowResolution, ShadowRoot,
    ShadowSource, format_isolated_shadow_notice, format_shadow_banner_with_verification,
};
use crate::core::RunContext;
use crate::pipeline::shadow_verification::write_verified;

use super::envs::EnvTarget;
use super::{Resolved, RunOptions, Staged};
use crate::cli::run::RunError;

pub(super) fn run(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
    staged: &Staged,
    targets: &[EnvTarget],
) -> Result<(), RunError> {
    let mut names: Vec<&str> = vec![ctx.skill_name.as_str()];
    names.extend(ctx.sibling_skill_names.iter().map(String::as_str));
    let adapter = adapter_for(ctx.harness);
    let expected_cells = targets
        .iter()
        .filter_map(|target| {
            target
                .conditions
                .first()
                .map(|(condition, _)| (target.group_id.clone(), (*condition).to_string()))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut config_dir = None;
    let mut shadowed_names = BTreeSet::new();
    let mut scans = Vec::with_capacity(targets.len());
    for target in targets {
        let Some((condition, _)) = target.conditions.first() else {
            continue;
        };
        let appearance = ShadowAppearance {
            group: target.group_id.clone(),
            condition: (*condition).to_string(),
            eval_ids: target.eval_ids.clone(),
            resolution: ShadowResolution::Unknown,
            precedence_rank: None,
        };
        let mut sources = adapter
            .detect_shadowed_skills(&target.root, &names)
            .map(|report| {
                config_dir.get_or_insert(report.config_dir.clone());
                report.into_sources()
            })
            .unwrap_or_default();
        for source in &mut sources {
            shadowed_names.insert(source.skill_name.clone());
            source.add_appearance(appearance.clone());
        }
        scans.push((target, appearance, sources));
    }

    if shadowed_names.is_empty() {
        return Ok(());
    }

    let mut observed_sources = Vec::new();
    for (target, appearance, mut sources) in scans {
        let Some(skills_dir) = adapter.skills_dir(&target.root) else {
            adapter.resolve_shadow_sources(&target.root, &mut sources);
            observed_sources.extend(sources);
            continue;
        };
        if !opts.no_stage {
            let (condition, condition_skill_path) = &target.conditions[0];
            let condition_slug = if *condition == r.cond_a {
                staged.cond_a_slug.as_deref()
            } else if *condition == r.cond_b {
                staged.cond_b_slug.as_deref()
            } else {
                None
            };
            if condition_skill_path.is_some()
                && shadowed_names.contains(&ctx.skill_name)
                && let Some(slug) = condition_slug
            {
                let runtime_id = if adapter.advertises_staged_slug_name() {
                    slug
                } else {
                    &ctx.skill_name
                };
                let mut source = ShadowSource::staged(
                    &ctx.skill_name,
                    runtime_id,
                    &skills_dir.join(slug),
                    ShadowRoot::staged(&skills_dir),
                );
                source.add_appearance(appearance.clone());
                sources.push(source);
            }
            if ctx.stage_siblings {
                for sibling in &ctx.sibling_skill_names {
                    if !shadowed_names.contains(sibling) {
                        continue;
                    }
                    let mut source = ShadowSource::staged(
                        sibling,
                        sibling,
                        &skills_dir.join(sibling),
                        ShadowRoot::staged(&skills_dir),
                    );
                    source.add_appearance(appearance.clone());
                    sources.push(source);
                }
            }
        }
        adapter.resolve_shadow_sources(&target.root, &mut sources);
        observed_sources.extend(sources);
    }

    let report = PluginShadowReport::from_observed_sources(
        config_dir.unwrap_or_default(),
        observed_sources,
        &ctx.skill_name,
        &expected_cells,
    );
    let artifact = PluginShadowArtifact::new(report, adapter.isolates_live_sources());
    let verifies = adapter.surfaces_session_surface();
    write_verified(&r.iteration_dir.join("plugin-shadow.json"), &artifact)
        .map_err(|e| RunError::Message(e.to_string()))?;
    if artifact.isolates_live_sources {
        eprintln!(
            "{}",
            format_isolated_shadow_notice(&artifact.report, verifies)
        );
    } else {
        eprintln!(
            "{}",
            format_shadow_banner_with_verification(&artifact.report, verifies)
        );
    }
    Ok(())
}
