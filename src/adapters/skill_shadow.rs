//! Harness-neutral skill-shadow report types and formatters.
//!
//! A *shadow candidate* is a staged skill name that is also present in the
//! operator's live environment. Loading that source would contaminate the
//! with/without comparison; a descriptor can separately assert that dispatches
//! isolate every detected source. Detection is harness-specific (each adapter's
//! [`detect_shadowed_skills`](crate::adapters::HarnessAdapter::detect_shadowed_skills)
//! decides what "discoverable" means); the report shape is shared. These
//! default renderers retain the original Claude wording for backward
//! compatibility; built-in adapters select harness-specific renderers through
//! [`HarnessAdapter`](crate::adapters::HarnessAdapter).

use serde::{Deserialize, Serialize};

const ISOLATION_DOC: &str = "docs/claude-notes.md → \"Isolating from installed plugins\"";

/// A staged skill that is also discoverable from the live environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ShadowSource {
    Plugin {
        plugin: String,
        skill_name: String,
        path: String,
    },
    GlobalSkill {
        skill_name: String,
        path: String,
    },
}

impl ShadowSource {
    pub(crate) fn skill_name(&self) -> &str {
        match self {
            ShadowSource::Plugin { skill_name, .. } => skill_name,
            ShadowSource::GlobalSkill { skill_name, .. } => skill_name,
        }
    }

    fn source_label(&self) -> String {
        match self {
            ShadowSource::Plugin { plugin, .. } => format!("enabled plugin '{plugin}'"),
            ShadowSource::GlobalSkill { .. } => "the global skills dir".to_string(),
        }
    }
}

/// The detector's findings for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginShadowReport {
    pub config_dir: String,
    pub shadowed: Vec<ShadowSource>,
}

/// The persisted `plugin-shadow.json` envelope. The flattened report preserves
/// the legacy artifact shape; the isolation assertion is additive and absent
/// from artifacts produced without it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PluginShadowArtifact {
    #[serde(flatten)]
    pub report: PluginShadowReport,
    #[serde(default, skip_serializing_if = "is_false")]
    pub isolates_live_sources: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Informational build-time notice for a report whose resolved descriptor
/// asserts that every detected live source is isolated from dispatches.
pub(crate) fn format_isolated_shadow_notice(report: &PluginShadowReport) -> String {
    let count = report.shadowed.len();
    let finding = if count == 1 { "finding" } else { "findings" };
    [
        String::new(),
        format!("ℹ Skill-shadow notice: preflight detected {count} live-source {finding}."),
        "  The resolved descriptor declares `[shadow] isolates_live_sources = true`, so"
            .to_string(),
        "  the findings remain in plugin-shadow.json as informational provenance and".to_string(),
        "  will not become benchmark.json validity_warnings. eval-magic does not verify"
            .to_string(),
        "  this assertion; it must cover every initial and resumed eval-agent dispatch."
            .to_string(),
    ]
    .join("\n")
}

/// One `validity_warnings` line per shadowed skill (for benchmark.json).
pub fn shadow_validity_warnings(report: &PluginShadowReport) -> Vec<String> {
    report
        .shadowed
        .iter()
        .map(|s| {
            format!(
                "staged skill '{}' is also provided by {} — each claude -p dispatch could discover \
                 both copies, so with/without results may be contaminated. Isolate each dispatch's \
                 Claude config (see {}).",
                s.skill_name(),
                s.source_label(),
                ISOLATION_DOC
            )
        })
        .collect()
}

/// Build-time banner for the runner. Empty string when nothing is shadowed.
pub fn format_shadow_banner(report: &PluginShadowReport) -> String {
    if report.shadowed.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        String::new(),
        "⚠ Plugin-shadow warning: skills staged for this eval are ALSO discoverable".to_string(),
        "  from your live environment:".to_string(),
    ];
    for s in &report.shadowed {
        lines.push(format!("  • {} — {}", s.skill_name(), s.source_label()));
    }
    lines.push(
        "  Each `claude -p` dispatch loads your user/global plugins and skills, so".to_string(),
    );
    lines.push("  both the staged copy and the installed copy are discoverable — the".to_string());
    lines.push(
        "  with/without comparison may be contaminated and the control arm is not truly"
            .to_string(),
    );
    lines.push(
        "  skill-absent. The runner cannot strip an installed plugin from the dispatch."
            .to_string(),
    );
    lines.push("  Isolate each dispatch's Claude config, one of these ways:".to_string());
    lines.push(
        "  1. Drop user-scope plugins, keep auth: add `--setting-sources project,local`"
            .to_string(),
    );
    lines.push("     to each dispatch.".to_string());
    lines.push(
        "  2. Disable the specific plugin: set `\"enabledPlugins\": { \"<plugin>@<marketplace>\":"
            .to_string(),
    );
    lines.push("     false }` in a settings source the dispatch loads.".to_string());
    lines.push("  3. Clean config dir (strips everything): run each dispatch under".to_string());
    lines.push(
        "     `CLAUDE_CONFIG_DIR=\"$(mktemp -d)\"` — auth may need `ANTHROPIC_API_KEY`."
            .to_string(),
    );
    lines.push(format!("  Full mechanics: {ISOLATION_DOC}."));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> PluginShadowReport {
        PluginShadowReport {
            config_dir: "/x".into(),
            shadowed: vec![ShadowSource::Plugin {
                plugin: "slow-powers@slowdini".into(),
                skill_name: "verification-before-completion".into(),
                path: "/p".into(),
            }],
        }
    }

    #[test]
    fn legacy_shadow_artifact_defaults_to_undeclared_isolation() {
        let artifact: PluginShadowArtifact = serde_json::from_value(serde_json::json!({
            "config_dir": "/x",
            "shadowed": [{
                "kind": "plugin",
                "plugin": "slow-powers@slowdini",
                "skill_name": "mr-review",
                "path": "/x/plugin/skills/mr-review"
            }]
        }))
        .unwrap();

        assert!(!artifact.isolates_live_sources);
        assert_eq!(artifact.report.shadowed.len(), 1);
        assert!(
            serde_json::to_value(&artifact)
                .unwrap()
                .get("isolates_live_sources")
                .is_none(),
            "false keeps the legacy artifact shape"
        );
    }

    #[test]
    fn declared_isolation_is_serialized_with_the_shadow_report() {
        let artifact = PluginShadowArtifact {
            report: sample_report(),
            isolates_live_sources: true,
        };
        let value = serde_json::to_value(&artifact).unwrap();
        assert_eq!(value["isolates_live_sources"], true);
        assert_eq!(
            value["shadowed"][0]["skill_name"],
            "verification-before-completion"
        );
    }

    #[test]
    fn validity_warnings_name_skill_plugin_and_contamination() {
        let warnings = shadow_validity_warnings(&sample_report());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("verification-before-completion"));
        assert!(warnings[0].contains("slow-powers@slowdini"));
        assert!(warnings[0].to_lowercase().contains("contaminat"));
    }

    #[test]
    fn banner_is_empty_when_nothing_shadowed() {
        let empty = PluginShadowReport {
            config_dir: "/x".into(),
            shadowed: vec![],
        };
        assert_eq!(format_shadow_banner(&empty), "");
    }

    #[test]
    fn banner_lists_shadowed_skills_and_points_at_isolation_docs() {
        let banner = format_shadow_banner(&sample_report());
        assert!(banner.contains("verification-before-completion"));
        assert!(banner.contains("slow-powers@slowdini"));
        assert!(banner.to_lowercase().contains("isolat"));
    }

    #[test]
    fn banner_carries_the_three_isolation_recipes_inline() {
        // The remediation options live in the banner itself — shown exactly when
        // the contamination is detected — not in a README section the operator
        // has to go find.
        let banner = format_shadow_banner(&sample_report());
        assert!(
            banner.contains("--setting-sources project,local"),
            "banner names the setting-sources option: {banner}"
        );
        assert!(
            banner.contains("enabledPlugins"),
            "banner names the per-plugin disable option: {banner}"
        );
        assert!(
            banner.contains("CLAUDE_CONFIG_DIR"),
            "banner names the clean-config-dir option: {banner}"
        );
        assert!(
            banner.contains("docs/claude-notes.md"),
            "banner points at the harness dev notes for the full mechanics: {banner}"
        );
    }

    #[test]
    fn validity_warnings_point_at_a_doc_that_exists() {
        let warnings = shadow_validity_warnings(&sample_report());
        assert!(
            warnings[0].contains("docs/claude-notes.md"),
            "warning points at the harness dev notes: {}",
            warnings[0]
        );
    }
}
