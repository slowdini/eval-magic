//! Assemble the harness-neutral skill-shadow report across every comparison cell.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::adapters::skill_shadow::{
    PluginShadowArtifact, PluginShadowReport, ShadowAppearance, ShadowFindingClass,
    ShadowNamespace, ShadowRelation, ShadowResolution, ShadowRoot, ShadowRootScope, ShadowSource,
    format_isolated_shadow_notice, format_shadow_banner_with_verification,
};
use crate::adapters::{HarnessAdapter, adapter_for};
use crate::core::fs::artifact_path;
use crate::core::{Harness, RunContext};
use crate::pipeline::shadow_verification::write_verified;

use super::envs::EnvTarget;
use super::{Resolved, RunOptions, Staged};
use crate::cli::run::RunError;

/// Inventory evaluated skill names from every project root the selected harness
/// discovers. This runs immediately after codebase provisioning, before opt-in
/// exclusion or eval staging can remove/replace a source.
pub(super) fn scan_codebase_skill_sources(
    repo_root: &Path,
    harness: Harness,
    evaluated_names: &[&str],
) -> Vec<ShadowSource> {
    let adapter = adapter_for(harness);
    let native = adapter.skills_dir(repo_root);
    let evaluated = evaluated_names.iter().copied().collect::<BTreeSet<_>>();
    let mut sources = Vec::new();
    for root in adapter.project_skill_dirs(repo_root) {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !path.join("SKILL.md").is_file() {
                continue;
            }
            let folder_name = entry.file_name().to_string_lossy().into_owned();
            let frontmatter_name = frontmatter_name(&path.join("SKILL.md"));
            let Some(skill_name) = frontmatter_name
                .filter(|name| evaluated.contains(name.as_str()))
                .or_else(|| {
                    evaluated
                        .contains(folder_name.as_str())
                        .then_some(folder_name)
                })
            else {
                continue;
            };
            let namespace = project_namespace(&root);
            sources.push(ShadowSource::live_skill(
                skill_name,
                &path,
                ShadowRoot {
                    scope: ShadowRootScope::Project,
                    namespace,
                    plugin: None,
                    path: artifact_path(&root),
                    relation: if native.as_ref() == Some(&root) {
                        ShadowRelation::Native
                    } else {
                        ShadowRelation::CrossHarness
                    },
                },
                format!(
                    "Set `codebase.exclude_skill_sources = true` for this eval, or move or rename '{}'.",
                    path.display()
                ),
            ));
        }
    }
    sources.sort_by(|a, b| {
        (&a.skill_name, &a.discovery_path).cmp(&(&b.skill_name, &b.discovery_path))
    });
    sources
}

fn frontmatter_name(skill_md: &Path) -> Option<String> {
    let raw = fs::read_to_string(skill_md).ok()?;
    let mut lines = raw.lines();
    (lines.next()?.trim() == "---").then_some(())?;
    let mut found = None;
    for line in lines {
        if line.trim() == "---" {
            return found;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == "name" {
            let value = value.trim();
            let unquoted = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value[1..value.len() - 1].trim()
            } else {
                value
            };
            found = (!unquoted.is_empty()).then(|| unquoted.to_string());
        }
    }
    None
}

fn project_namespace(skills_dir: &Path) -> ShadowNamespace {
    match skills_dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
    {
        Some(".claude") => ShadowNamespace::Claude,
        Some(".agents") => ShadowNamespace::Agents,
        Some(".opencode") => ShadowNamespace::Opencode,
        Some(".cline") => ShadowNamespace::Cline,
        Some(".codex") => ShadowNamespace::Codex,
        _ => ShadowNamespace::Unknown,
    }
}

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
    let mut operator_shadowed_names = BTreeSet::new();
    let mut operator_scans = Vec::with_capacity(targets.len());
    let mut codebase_shadowed_names = BTreeSet::new();
    let mut codebase_scans = Vec::with_capacity(targets.len());
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
            operator_shadowed_names.insert(source.skill_name.clone());
            source.add_appearance(appearance.clone());
        }
        operator_scans.push((target, appearance.clone(), sources));

        let mut codebase_sources = staged
            .codebase_shadow_sources
            .get(&target.root)
            .cloned()
            .unwrap_or_default();
        omit_sources_displaced_by_staging(ctx, opts, r, staged, target, &mut codebase_sources);
        for source in &mut codebase_sources {
            codebase_shadowed_names.insert(source.skill_name.clone());
            source.add_appearance(appearance.clone());
        }
        codebase_scans.push((target, appearance, codebase_sources));
    }

    if operator_shadowed_names.is_empty() && codebase_shadowed_names.is_empty() {
        return Ok(());
    }

    let operator_sources = collect_observed_sources(
        ctx,
        opts,
        r,
        staged,
        adapter,
        operator_scans,
        &operator_shadowed_names,
    );
    let codebase_sources = collect_observed_sources(
        ctx,
        opts,
        r,
        staged,
        adapter,
        codebase_scans,
        &codebase_shadowed_names,
    );
    let operator_report = PluginShadowReport::from_observed_sources(
        config_dir.clone().unwrap_or_default(),
        operator_sources,
        &ctx.skill_name,
        &expected_cells,
    );
    let codebase_report = PluginShadowReport::from_observed_sources_with_class(
        config_dir.clone().unwrap_or_default(),
        codebase_sources,
        &ctx.skill_name,
        &expected_cells,
        ShadowFindingClass::CodebaseSourced,
    );
    let mut findings = operator_report.findings.clone();
    findings.extend(codebase_report.findings.clone());
    findings.sort_by(|a, b| {
        let class_key = |class| match class {
            ShadowFindingClass::OperatorEnvironment => 0,
            ShadowFindingClass::CodebaseSourced => 1,
        };
        (class_key(a.class), &a.skill_name).cmp(&(class_key(b.class), &b.skill_name))
    });
    let artifact = PluginShadowArtifact::new(
        PluginShadowReport {
            config_dir: config_dir.unwrap_or_default(),
            findings,
        },
        adapter.isolates_live_sources(),
    );
    let verifies = adapter.surfaces_session_surface();
    write_verified(&r.iteration_dir.join("plugin-shadow.json"), &artifact)
        .map_err(|e| RunError::Message(e.to_string()))?;
    if artifact.isolates_live_sources {
        if !operator_report.is_empty() {
            eprintln!(
                "{}",
                format_isolated_shadow_notice(&operator_report, verifies)
            );
        }
        if !codebase_report.is_empty() {
            eprintln!(
                "{}",
                format_shadow_banner_with_verification(&codebase_report, verifies)
            );
        }
    } else {
        eprintln!(
            "{}",
            format_shadow_banner_with_verification(&artifact.report, verifies)
        );
    }
    Ok(())
}

type Scan<'a> = (&'a EnvTarget, ShadowAppearance, Vec<ShadowSource>);

fn collect_observed_sources(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
    staged: &Staged,
    adapter: &dyn HarnessAdapter,
    scans: Vec<Scan<'_>>,
    shadowed_names: &BTreeSet<String>,
) -> Vec<ShadowSource> {
    let mut observed = Vec::new();
    for (target, appearance, mut sources) in scans {
        let Some(skills_dir) = adapter.skills_dir(&target.root) else {
            adapter.resolve_shadow_sources(&target.root, &mut sources);
            observed.extend(sources);
            continue;
        };
        if !opts.no_stage {
            let (condition, condition_skill_path) = &target.conditions[0];
            let condition_slug = condition_slug(r, staged, condition);
            if condition_skill_path.is_some()
                && shadowed_names.contains(&ctx.skill_name)
                && let Some(slug) = condition_slug
            {
                let mut source = ShadowSource::staged(
                    &ctx.skill_name,
                    slug,
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
        observed.extend(sources);
    }
    observed
}

fn condition_slug<'a>(r: &Resolved, staged: &'a Staged, condition: &str) -> Option<&'a str> {
    if condition == r.cond_a {
        staged.cond_a_slug.as_deref()
    } else if condition == r.cond_b {
        staged.cond_b_slug.as_deref()
    } else {
        None
    }
}

/// A source backed up because staging owns its exact discovery path cannot be
/// loaded during this run. Other project sources remain findings.
fn omit_sources_displaced_by_staging(
    ctx: &RunContext,
    opts: &RunOptions,
    r: &Resolved,
    staged: &Staged,
    target: &EnvTarget,
    sources: &mut Vec<ShadowSource>,
) {
    if opts.no_stage {
        return;
    }
    let adapter = adapter_for(ctx.harness);
    let Some(skills_dir) = adapter.skills_dir(&target.root) else {
        return;
    };
    let mut displaced = BTreeSet::new();
    let (condition, condition_skill_path) = &target.conditions[0];
    if condition_skill_path.is_some()
        && let Some(slug) = condition_slug(r, staged, condition)
    {
        displaced.insert(artifact_path(&skills_dir.join(slug)));
    }
    if ctx.stage_siblings {
        displaced.extend(
            ctx.sibling_skill_names
                .iter()
                .map(|name| artifact_path(&skills_dir.join(name))),
        );
    }
    sources.retain(|source| !displaced.contains(&source.discovery_path));
}
