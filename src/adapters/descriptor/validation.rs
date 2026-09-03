//! Cross-field descriptor invariants — the load-time form of the old
//! cross-harness adapter tests, applied to every descriptor so user-supplied
//! files inherit the same checks as the built-ins.

use regex::Regex;

use crate::adapters::guard;
use crate::core::validate_agent_environment_entry;

use super::{
    DescriptorError, GuardEngine, GuardSection, HarnessDescriptor, render_staged_slug,
    stage_name_error,
};

mod conversation;
mod transcript;

/// The placeholders a slug template must carry to keep cleanup prefix-scans
/// and per-cell uniqueness working.
const SLUG_PLACEHOLDERS: [&str; 4] = ["{prefix}", "{iteration}", "{condition}", "{skill_name}"];

/// One cross-field invariant: it either passes or reports the violation as an
/// actionable message, which [`validate_descriptor`] pairs with the source path.
type Check = fn(&HarnessDescriptor) -> Result<(), String>;

/// Every cross-field invariant, in check order. Each check reports the first
/// violation it finds; the list order decides which message a descriptor that
/// breaks several invariants gets, so new checks go where their message reads
/// most usefully rather than at the end by default.
const CHECKS: &[Check] = &[
    check_dispatch_env,
    check_guard_lockstep,
    check_project_skill_dirs,
    check_skills_dir_requirements,
    check_slug_shape,
    check_config_dirs_cover_skills_dir,
    check_guard_engine_fields,
    check_guard_verdict_template,
    transcript::check_tool_vocabulary,
    transcript::check_skill_evidence,
    transcript::check_tiers,
    conversation::validate,
    check_tool_roles_disjoint,
    check_template_placeholder_backing,
    check_manifest_template_newline,
    check_skills_block_item,
];

/// Skill-root paths drive staging cleanup and opt-in codebase source moves, so
/// every one must stay beneath the task repository and name a single normalized
/// location.
fn check_project_skill_dirs(d: &HarnessDescriptor) -> Result<(), String> {
    let mut roots = Vec::new();
    if let Some(native) = &d.skills_dir {
        roots.push(("skills_dir", native));
    }
    roots.extend(
        d.additional_project_skill_dirs
            .iter()
            .map(|path| ("additional_project_skill_dirs", path)),
    );
    for (field, path) in roots {
        if path.starts_with('/')
            || path.contains('\\')
            || path
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        {
            return Err(format!(
                "{field} project skill path must be a relative `/`-separated path without empty, \
                 `.` or `..` segments (got \"{path}\")"
            ));
        }
    }
    if let Some(native) = &d.skills_dir
        && d.additional_project_skill_dirs.contains(native)
    {
        return Err(format!(
            "additional project skill dirs duplicate skills_dir \"{native}\""
        ));
    }
    Ok(())
}

/// Check every cross-field invariant, returning the first violation with an
/// actionable message.
pub(super) fn validate_descriptor(
    d: &HarnessDescriptor,
    source: &str,
) -> Result<(), DescriptorError> {
    for check in CHECKS {
        check(d).map_err(|message| DescriptorError::Invariant {
            path: source.to_string(),
            message,
        })?;
    }
    Ok(())
}

/// Dispatch env entries reach the agent process, so they answer to the same
/// name/value rules as any other agent environment entry.
fn check_dispatch_env(d: &HarnessDescriptor) -> Result<(), String> {
    for (name, value) in &d.dispatch.env {
        if let Err(message) = validate_agent_environment_entry(name, value) {
            return Err(format!("dispatch.env: {message}"));
        }
    }
    Ok(())
}

/// Guard capability and post-arm banner move in lockstep: `--guard` gates on
/// the capability, and an armed guard the user is never told about (or a
/// banner with no guard behind it) misleads the dispatch session.
fn check_guard_lockstep(d: &HarnessDescriptor) -> Result<(), String> {
    if d.run.supports_guard != d.guard.is_some() {
        return Err(format!(
            "run.supports_guard is {} but the [guard] table is {}; the guard capability and \
             the armed banner must move in lockstep — declare both or neither",
            d.run.supports_guard,
            if d.guard.is_some() {
                "present"
            } else {
                "absent"
            },
        ));
    }
    Ok(())
}

/// Native staging and the write guard both live under the skills dir: staging
/// copies skills into it, and the guard keeps its marker/manifest there.
/// Without a skills_dir neither has anywhere to operate.
fn check_skills_dir_requirements(d: &HarnessDescriptor) -> Result<(), String> {
    if d.skills_dir.is_none() {
        if !d.additional_project_skill_dirs.is_empty() {
            return Err(
                "additional_project_skill_dirs is declared but skills_dir is not; exclusion and \
                 cleanup record project skill roots in the native skills_dir manifest — declare \
                 it, or drop additional_project_skill_dirs"
                    .into(),
            );
        }
        if d.staging.is_configured() {
            return Err(
                "[staging] is configured but skills_dir is not declared; native staging \
                 copies skills into skills_dir — declare it, or drop [staging] and let \
                 runs fall back to --no-stage"
                    .into(),
            );
        }
        if d.guard.is_some() {
            return Err(
                "[guard] is declared but skills_dir is not; the guard's marker and \
                 teardown manifest live under skills_dir — declare it, or drop the \
                 guard and rely on the detect-stray-writes audit"
                    .into(),
            );
        }
    }
    Ok(())
}

/// Slug shape: one source of truth, all four placeholders, and the generated
/// slug must satisfy the descriptor's own naming rules.
fn check_slug_shape(d: &HarnessDescriptor) -> Result<(), String> {
    if d.staging.slug_template.is_some() && d.staging.slug_capability.is_some() {
        return Err(
            "declare either staging.slug_template or staging.slug_capability, not both".into(),
        );
    }
    if let Some(template) = &d.staging.slug_template {
        for placeholder in SLUG_PLACEHOLDERS {
            if !template.contains(placeholder) {
                return Err(format!(
                    "staging.slug_template must contain {placeholder} — cleanup prefix-scans \
                     and per-cell uniqueness rely on all four placeholders"
                ));
            }
        }
    }
    let stage_regex = match &d.staging.stage_name_pattern {
        Some(pattern) => match Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(e) => {
                return Err(format!("staging.stage_name_pattern does not compile: {e}"));
            }
        },
        None => None,
    };
    let sample = render_staged_slug(&d.staging, "slow-powers-eval-", 2, "with_skill", "my-skill");
    if !sample.starts_with("slow-powers-eval-") {
        return Err(format!(
            "the staged slug must preserve the prefix (cleanup prefix-scans rely on it); \
             sample slug: \"{sample}\""
        ));
    }
    if let Some(message) = stage_name_error(&d.staging, stage_regex.as_ref(), &sample) {
        return Err(format!(
            "the staged slug \"{sample}\" fails its own stage-name rules ({message}); \
             align staging.slug_template/slug_capability with the naming rules"
        ));
    }
    Ok(())
}

/// The skills dir must live under a declared config dir, or staging's
/// sibling-asset filter would copy a checked-in copy into staged envs.
fn check_config_dirs_cover_skills_dir(d: &HarnessDescriptor) -> Result<(), String> {
    if let Some(skills_dir) = &d.skills_dir {
        let top = skills_dir.split('/').next().unwrap_or_default();
        if !d.config_dirs.iter().any(|dir| dir == top) {
            return Err(format!(
                "config_dirs {:?} misses \"{top}\", the parent of skills_dir — staging's \
                 sibling-asset filter keys off config_dirs",
                d.config_dirs
            ));
        }
    }
    for skills_dir in &d.additional_project_skill_dirs {
        let top = skills_dir.split('/').next().unwrap_or_default();
        if !d.config_dirs.iter().any(|dir| dir == top) {
            return Err(format!(
                "config_dirs {:?} misses \"{top}\", the parent of additional project skill dir \
                 \"{skills_dir}\" — discovery, sibling filtering, and task-repository baselining \
                 must use the same harness config surface",
                d.config_dirs
            ));
        }
    }
    Ok(())
}

/// The guard block is rendered by the engine at arm/verdict time, and the
/// guard fails open — so every data contract is proven here, before any run
/// arms the hook. Which fields apply depends on the engine: the schema proves
/// per-engine requiredness, and the per-engine checks below prove
/// applicability and content.
fn check_guard_engine_fields(d: &HarnessDescriptor) -> Result<(), String> {
    let Some(guard) = &d.guard else {
        return Ok(());
    };
    match guard.engine {
        GuardEngine::JsonHooks => check_json_hooks_guard(d, guard),
        GuardEngine::OpencodePlugin => check_plugin_guard(guard, "opencode-plugin"),
        GuardEngine::ClinePlugin => check_plugin_guard(guard, "cline-plugin"),
    }
}

/// Guard paths are joined onto the staged env root, so a leading `/` or a
/// `.`/`..` segment would install outside the env it is meant to guard.
fn check_guard_relative_path(field: &str, path: &str) -> Result<(), String> {
    if path.starts_with('/')
        || path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(format!(
            "guard.{field} must be a relative `/`-separated path without \".\" or \
             \"..\" segments (got \"{path}\") — it resolves under the staged env root"
        ));
    }
    Ok(())
}

/// The json-hooks engine merges a rendered hook entry into the harness's hook
/// config, so its four fields must be present, the matcher must stay inside
/// the tool vocabulary, and both templates must carry their placeholders.
fn check_json_hooks_guard(d: &HarnessDescriptor, guard: &GuardSection) -> Result<(), String> {
    if guard.plugin_file.is_some() {
        return Err(
            "guard.plugin_file is only valid with engine = \"opencode-plugin\"; \
             the json-hooks engine merges a hook entry into guard.hooks_file"
                .into(),
        );
    }
    let (hooks_file, matcher, command_template, hook_entry) = match (
        &guard.hooks_file,
        &guard.matcher,
        &guard.command_template,
        &guard.hook_entry,
    ) {
        (Some(h), Some(m), Some(c), Some(e)) => (h, m, c, e),
        _ => {
            return Err("the json-hooks engine requires hooks_file, matcher, \
                 command_template, and hook_entry (proven by the schema gate)"
                .into());
        }
    };

    // Every tool the guard hooks must be declared in the vocabulary, or
    // the write-guard arbiter would silently wave it through.
    let vocabulary: Vec<&str> = d
        .tools
        .write
        .iter()
        .chain(&d.tools.patch)
        .chain(&d.tools.shell)
        .map(String::as_str)
        .collect();
    for token in matcher.split('|') {
        let token = token.trim_matches(['^', '$']);
        if !vocabulary.contains(&token) {
            return Err(format!(
                "the guard matcher hooks tool \"{token}\" but [tools] does not \
                 declare it in write/patch/shell — the write-guard arbiter would \
                 not recognize it"
            ));
        }
    }

    check_guard_relative_path("hooks_file", hooks_file)?;

    for placeholder in ["{exe}", "{marker}"] {
        if !command_template.contains(placeholder) {
            return Err(format!(
                "guard.command_template must reference {placeholder} — the armed \
                 hook invokes this binary with the marker path"
            ));
        }
    }

    match serde_json::from_str::<serde_json::Value>(hook_entry) {
        Err(e) => Err(format!(
            "guard.hook_entry does not parse as JSON ({e}); it is the hook \
             object appended to the harness's hook config"
        )),
        Ok(entry) => {
            if !entry.is_object() {
                return Err(
                    "guard.hook_entry must be a JSON object — it is appended to \
                     the hook config's hooks.PreToolUse array"
                        .into(),
                );
            }
            for placeholder in ["{matcher}", "{command}"] {
                if !guard::any_string_value_contains(&entry, placeholder) {
                    return Err(format!(
                        "guard.hook_entry must reference the {placeholder} \
                         placeholder in a string value — placeholders substitute \
                         into string values only, so anywhere else would render \
                         an inert hook"
                    ));
                }
            }
            Ok(())
        }
    }
}

/// The plugin engine stages the plugin file whole — there is no hook-config
/// merge, tool matcher, or shell command to render.
fn check_plugin_guard(guard: &GuardSection, engine: &str) -> Result<(), String> {
    for (field, present) in [
        ("hooks_file", guard.hooks_file.is_some()),
        ("matcher", guard.matcher.is_some()),
        ("command_template", guard.command_template.is_some()),
        ("hook_entry", guard.hook_entry.is_some()),
    ] {
        if present {
            return Err(format!(
                "guard.{field} is only valid with the json-hooks engine; the \
                 {engine} engine stages the plugin file whole — declare \
                 plugin_file instead"
            ));
        }
    }
    let Some(plugin_file) = &guard.plugin_file else {
        return Err(format!(
            "engine = \"{engine}\" requires plugin_file (proven by the \
             schema gate)"
        ));
    };
    check_guard_relative_path("plugin_file", plugin_file)
}

/// The deny verdict is printed verbatim by the guard hook, so it must be JSON
/// and must carry the reason through to the agent that tripped it.
fn check_guard_verdict_template(d: &HarnessDescriptor) -> Result<(), String> {
    let Some(guard) = &d.guard else {
        return Ok(());
    };
    match serde_json::from_str::<serde_json::Value>(&guard.verdict_template) {
        Err(e) => Err(format!(
            "guard.verdict_template does not parse as JSON ({e}); it is printed \
             verbatim as the deny verdict"
        )),
        Ok(verdict) => {
            if !guard::any_string_value_contains(&verdict, "{reason}") {
                return Err(
                    "guard.verdict_template must reference the {reason} placeholder in a \
                     string value — a deny verdict that hides the reason is undebuggable"
                        .into(),
                );
            }
            Ok(())
        }
    }
}

/// Tool roles are disjoint: one name in two roles would double-classify
/// invocations in the stray-writes audit.
fn check_tool_roles_disjoint(d: &HarnessDescriptor) -> Result<(), String> {
    let mut seen: Vec<&str> = Vec::new();
    for name in d
        .tools
        .write
        .iter()
        .chain(&d.tools.patch)
        .chain(&d.tools.shell)
        .chain(&d.tools.read)
    {
        if seen.contains(&name.as_str()) {
            return Err(format!(
                "tool \"{name}\" appears in more than one [tools] role — \
                 write/patch/shell/read must be disjoint"
            ));
        }
        seen.push(name);
    }
    Ok(())
}

/// Placeholders must have a backing field, or the template renders with the
/// token left in (the artifact tests' `!contains("{{")` rule, at load time).
fn check_template_placeholder_backing(d: &HarnessDescriptor) -> Result<(), String> {
    let dispatch = &d.dispatch;
    let pairings: [(&Option<String>, &str, &str, bool); 4] = [
        (
            &dispatch.next_steps_template,
            "next_steps_template",
            "{exec_command}",
            dispatch.exec_template.is_some(),
        ),
        (
            &dispatch.next_steps_template,
            "next_steps_template",
            "{model_note}",
            dispatch.model_note.is_some(),
        ),
        (
            &dispatch.manifest_template,
            "manifest_template",
            "{exec_command}",
            dispatch.exec_template.is_some(),
        ),
        (
            &dispatch.exec_template,
            "exec_template",
            "{guard_args}",
            dispatch.guard_args.is_some(),
        ),
    ];
    for (template, template_name, placeholder, backed) in pairings {
        if template.as_deref().is_some_and(|t| t.contains(placeholder)) && !backed {
            return Err(format!(
                "dispatch.{template_name} references {placeholder} but the field that fills \
                 it is not set"
            ));
        }
    }
    Ok(())
}

/// The manifest template is spliced as `split('\n')` lines; exactly one
/// trailing newline reproduces the section's closing blank line.
fn check_manifest_template_newline(d: &HarnessDescriptor) -> Result<(), String> {
    if let Some(manifest) = &d.dispatch.manifest_template
        && (!manifest.ends_with('\n') || manifest.ends_with("\n\n"))
    {
        return Err(
            "dispatch.manifest_template must end with exactly one trailing newline — it \
             becomes the manifest section's closing blank line"
                .into(),
        );
    }
    Ok(())
}

/// A skills-block item that never names the skill renders an unusable list.
fn check_skills_block_item(d: &HarnessDescriptor) -> Result<(), String> {
    if let Some(block) = &d.skills_block
        && !block.item.contains("{name}")
    {
        return Err("skills_block.item must contain {name}".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
