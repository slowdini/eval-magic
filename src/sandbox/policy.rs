//! Write-boundary primitives.
//!
//! Stateless classifiers shared by the armed guard ([`super::decide`]) and
//! `pipeline::detect-stray-writes`: which tools write, which Bash commands
//! mutate state outside a sandbox, and whether a path falls under an allowed
//! root. Tool names come from the adapters' cross-harness vocabulary union
//! ([`all_tool_vocabulary`]), so no harness's tool naming is hardcoded here.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::adapters::all_tool_vocabulary;

/// True for a tool name that writes the filesystem with a single target path
/// argument, in any harness's vocabulary.
pub fn is_write_tool(tool_name: &str) -> bool {
    all_tool_vocabulary()
        .write_tools
        .iter()
        .any(|t| t == tool_name)
}

/// True for an apply_patch-style tool whose payload carries patch targets
/// (extracted with [`apply_patch_paths`]), in any harness's vocabulary.
pub fn is_patch_tool(tool_name: &str) -> bool {
    all_tool_vocabulary()
        .patch_tools
        .iter()
        .any(|t| t == tool_name)
}

/// True for a shell-execution tool carrying a `command` argument, in any
/// harness's vocabulary.
pub fn is_shell_tool(tool_name: &str) -> bool {
    all_tool_vocabulary()
        .shell_tools
        .iter()
        .any(|t| t == tool_name)
}

/// Pull the target path from a write tool's arguments (`file_path` →
/// `notebook_path` → `path` → `filePath`, the last being OpenCode's camelCase
/// spelling). Returns `None` when the input is not an object or carries no
/// string path.
pub fn path_arg(args: &Value) -> Option<&str> {
    let obj = args.as_object()?;
    ["file_path", "notebook_path", "path", "filePath"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))
}

/// Extract file paths from an `apply_patch`-style tool payload. Codex exposes
/// patch targets as a structured `files` list or as freeform patch text
/// (`command`/`patch`/`input`/`content`), OpenCode as `patchText`; collect all
/// of them so the guard can deny unknown or out-of-bounds patches before they
/// run.
pub fn apply_patch_paths(args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = args.as_object() else {
        return out;
    };

    if let Some(files) = obj.get("files") {
        collect_file_values(files, &mut out);
    }

    for key in ["command", "patch", "input", "content", "patchText"] {
        if let Some(text) = obj.get(key).and_then(Value::as_str) {
            collect_patch_header_paths(text, &mut out);
        }
    }

    out.sort();
    out.dedup();
    out
}

fn collect_file_values(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(path) => out.push(path.to_string()),
        Value::Array(items) => {
            for item in items {
                collect_file_values(item, out);
            }
        }
        Value::Object(obj) => {
            for key in ["file_path", "path", "absolute_file_path", "move_path"] {
                if let Some(path) = obj.get(key).and_then(Value::as_str) {
                    out.push(path.to_string());
                }
            }
        }
        _ => {}
    }
}

fn collect_patch_header_paths(text: &str, out: &mut Vec<String>) {
    for line in text.lines() {
        for prefix in [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(path) = line.strip_prefix(prefix) {
                let path = path.trim();
                if !path.is_empty() {
                    out.push(path.to_string());
                }
            }
        }
    }
}

/// Lexically absolutize `path`, leaving a rooted-but-prefixless path
/// (`/work/env`) exactly as given.
///
/// Such a path is absolute on POSIX, but on Windows a root without a drive is
/// incomplete, so `std::path::absolute` grafts the process's current one. The
/// paths on both sides of these comparisons come from *agent* tool calls and
/// eval config, which spell them POSIX-style whatever the host is — grafting
/// `C:` would stop `/dev/null` reading as a device and would put a path that
/// never existed (`C:\etc\passwd`) into the evidence a denial records.
///
/// Every caller that compares or reports these paths has to apply this same
/// rule — [`resolve_path`] here and the stray-write scanner's live-directory
/// resolution — or one side gains a drive the other lacks and the comparison
/// silently stops matching.
pub(crate) fn lexically_absolute(path: &Path) -> PathBuf {
    if path.has_root() && !path.is_absolute() {
        return path.to_path_buf();
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Lexically absolutize a path: join onto `repo_root` if relative, then normalize.
/// Mirrors node's `resolve()` — no symlink resolution or existence requirement.
pub(crate) fn resolve_path(target: &str, repo_root: &Path) -> PathBuf {
    let path = Path::new(target);
    let joined = if path.has_root() {
        PathBuf::from(target)
    } else {
        repo_root.join(target)
    };
    // Applied to the *joined* path, so a relative target under a POSIX-rooted
    // `repo_root` resolves the same way its allowed roots do.
    let absolute = lexically_absolute(&joined);
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

/// True when `target` resolves to `dir` or a descendant of it. Relative `target`s
/// resolve against `repo_root`. `Path::starts_with` matches whole path
/// components, so `.eval-magic2` is correctly not under `.eval-magic`.
pub fn is_under(target: &str, dir: &str, repo_root: &Path) -> bool {
    let base = resolve_path(dir, repo_root);
    let abs = resolve_path(target, repo_root);
    abs.starts_with(&base)
}

/// True when `target` is under any of `dirs`.
pub fn is_under_any(target: &str, dirs: &[String], repo_root: &Path) -> bool {
    dirs.iter().any(|d| is_under(target, d, repo_root))
}

pub(super) const OUTPUT_REDIRECTION_REASON: &str = "output redirection to a file";

/// A cwd-aware Bash denial plus the literal targets the scanner resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BashClassification {
    pub reason: &'static str,
    pub resolved_targets: Vec<String>,
}

/// Cwd-aware/evidence-producing implementation behind [`classify_bash`].
pub(crate) fn classify_bash_with_cwd(
    command: &str,
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    classify_bash_with_policy(
        command,
        allowed_roots,
        invocation_cwd,
        &crate::core::GuardPolicyConfig::default(),
    )
}

/// Classify one shell tool call under its resolved eval command policy,
/// returning the first applicable denial.
pub(crate) fn classify_bash_with_policy(
    command: &str,
    allowed_roots: &[String],
    invocation_cwd: &Path,
    policy: &crate::core::GuardPolicyConfig,
) -> Option<BashClassification> {
    classify_bash_denials(command, allowed_roots, invocation_cwd, policy)
        .into_iter()
        .next()
}

/// Every layer that denies one shell tool call, containment checks first and
/// the eval command policy second (at most one denial each).
///
/// The stray-write audit wants only the first denial, but the live guard owes
/// the agent all of them in one verdict: a verdict that names just the
/// redirect it found sends the agent to fix a problem that cannot unblock the
/// command when the command policy was already denying it.
pub(crate) fn classify_bash_denials(
    command: &str,
    allowed_roots: &[String],
    invocation_cwd: &Path,
    policy: &crate::core::GuardPolicyConfig,
) -> Vec<BashClassification> {
    if command.is_empty() {
        return Vec::new();
    }
    let mut denials = Vec::new();
    if let Some(denial) = classify_fixed_containment(command, allowed_roots, invocation_cwd) {
        denials.push(denial);
    }
    if let Some(denial) = super::command_policy::classify_command_policy(command, policy) {
        denials.push(denial);
    }
    denials
}

fn classify_fixed_containment(
    command: &str,
    allowed_roots: &[String],
    invocation_cwd: &Path,
) -> Option<BashClassification> {
    if let Some(denial) =
        super::shell_targets::classify_output_targets(command, allowed_roots, invocation_cwd)
    {
        return Some(denial);
    }
    if let Some(denial) =
        super::git_command::classify_git_commands(command, allowed_roots, invocation_cwd)
    {
        return Some(denial);
    }
    if let Some(denial) =
        super::mutation_targets::classify_mutation_targets(command, allowed_roots, invocation_cwd)
    {
        return Some(denial);
    }
    for script in super::command_policy::literal_shell_scripts(command) {
        if let Some(denial) = classify_fixed_containment(&script, allowed_roots, invocation_cwd) {
            return Some(denial);
        }
    }
    None
}

/// Return the human reason when a Bash command has a recognized output,
/// repository, project, or mutation target that cannot be proven inside
/// `allowed_roots`; otherwise return `None`. Relative targets and commands with
/// an implicit destination resolve from the process cwd. Hook and audit callers
/// use [`classify_bash_with_cwd`] with the invocation cwd instead.
pub fn classify_bash(command: &str, allowed_roots: &[String]) -> Option<&'static str> {
    let cwd = std::env::current_dir().unwrap_or_default();
    classify_bash_with_cwd(command, allowed_roots, &cwd).map(|result| result.reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    mod command_policy;

    const ROOTS: [&str; 2] = ["/work/.eval-magic", "/work/.claude/skills"];

    fn roots() -> Vec<String> {
        ROOTS.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn is_write_tool_matches_every_harness_write_tool() {
        for t in ["Write", "Edit", "MultiEdit", "NotebookEdit", "file_change"] {
            assert!(is_write_tool(t), "{t} should be a write tool");
        }
        for t in ["Read", "Bash", "Grep", "apply_patch", ""] {
            assert!(!is_write_tool(t), "{t} should not be a write tool");
        }
    }

    #[test]
    fn is_patch_tool_matches_apply_patch_style_tools_only() {
        assert!(is_patch_tool("apply_patch"));
        for t in ["Write", "Bash", "file_change", ""] {
            assert!(!is_patch_tool(t), "{t} should not be a patch tool");
        }
    }

    #[test]
    fn is_shell_tool_matches_every_harness_shell_tool() {
        for t in ["Bash", "command_execution"] {
            assert!(is_shell_tool(t), "{t} should be a shell tool");
        }
        for t in ["Write", "apply_patch", ""] {
            assert!(!is_shell_tool(t), "{t} should not be a shell tool");
        }
    }

    #[test]
    fn path_arg_prefers_file_path_then_notebook_then_path() {
        assert_eq!(path_arg(&json!({ "file_path": "/a" })), Some("/a"));
        assert_eq!(path_arg(&json!({ "notebook_path": "/b" })), Some("/b"));
        assert_eq!(path_arg(&json!({ "path": "/c" })), Some("/c"));
        assert_eq!(
            path_arg(&json!({ "file_path": "/a", "path": "/c" })),
            Some("/a")
        );
        assert_eq!(path_arg(&json!({ "command": "ls" })), None);
        assert_eq!(path_arg(&json!("not an object")), None);
    }

    #[test]
    fn path_arg_recognizes_opencode_camel_case_file_path() {
        // OpenCode's edit/write tools take `filePath` (camelCase).
        assert_eq!(path_arg(&json!({ "filePath": "/op" })), Some("/op"));
        // snake_case still wins when both are present (claude/codex payloads).
        assert_eq!(
            path_arg(&json!({ "file_path": "/a", "filePath": "/op" })),
            Some("/a")
        );
    }

    #[test]
    fn apply_patch_paths_reads_opencode_patch_text() {
        // OpenCode's apply_patch tool takes the patch body as `patchText`.
        let paths = apply_patch_paths(&json!({
            "patchText": "*** Begin Patch\n*** Add File: docs/new.md\n*** End Patch\n"
        }));
        assert_eq!(paths, vec!["docs/new.md".to_string()]);
    }

    #[test]
    fn apply_patch_paths_reads_codex_command_and_every_patch_target() {
        let paths = apply_patch_paths(&json!({
            "command": "\
        *** Begin Patch\n\
        *** Add File: fixtures/new.txt\n\
        *** Update File: fixtures/source.txt\n\
        *** Move to: fixtures/moved.txt\n\
        *** Delete File: fixtures/old.txt\n\
        *** End Patch\n"
        }));
        assert_eq!(
            paths,
            vec![
                "fixtures/moved.txt".to_string(),
                "fixtures/new.txt".to_string(),
                "fixtures/old.txt".to_string(),
                "fixtures/source.txt".to_string(),
            ]
        );
    }

    #[test]
    fn apply_patch_paths_collects_structured_and_freeform_targets() {
        let paths = apply_patch_paths(&json!({
            "files": [
                "/tmp/out.md",
                { "path": "src/lib.rs" },
                { "move_path": "src/new.rs" }
            ],
            "patch": "*** Begin Patch\n*** Update File: docs/a.md\n*** Move to: docs/b.md\n*** End Patch\n"
        }));
        assert_eq!(
            paths,
            vec![
                "/tmp/out.md".to_string(),
                "docs/a.md".to_string(),
                "docs/b.md".to_string(),
                "src/lib.rs".to_string(),
                "src/new.rs".to_string(),
            ]
        );
    }

    /// The paths the guard classifies come from *agent* tool calls, which spell
    /// them POSIX-style whatever the host is. Windows has no root without a
    /// drive, so `std::path::absolute` grafts the process's current one —
    /// turning `/dev/null` into `C:\dev\null`, which stops reading as a device
    /// and starts reading as a file the guard must block.
    #[test]
    fn resolve_path_keeps_a_posix_rooted_target_rooted() {
        let repo = Path::new("/work");
        assert_eq!(resolve_path("/dev/null", repo), PathBuf::from("/dev/null"));
        assert_eq!(
            resolve_path("/etc/passwd", repo),
            PathBuf::from("/etc/passwd")
        );
    }

    /// Keeping the target rooted must not cost the lexical normalization that
    /// stops `/dev/..` from laundering an out-of-bounds write past the device
    /// check.
    #[test]
    fn resolve_path_normalizes_parent_segments_in_a_posix_rooted_target() {
        assert_eq!(
            resolve_path("/dev/../etc/passwd", Path::new("/work")),
            PathBuf::from("/etc/passwd")
        );
    }

    /// The containment verdict is what actually protects the sandbox: a
    /// POSIX-rooted target is still outside a drive-rooted allowed root.
    #[test]
    fn is_under_denies_a_posix_rooted_target_against_a_drive_rooted_root() {
        let env = r"C:\work\env";
        assert!(!is_under("/etc/passwd", env, Path::new(env)));
    }

    #[test]
    fn is_under_matches_dir_and_descendants() {
        let repo = Path::new("/work");
        assert!(is_under("/work/.eval-magic", "/work/.eval-magic", repo));
        assert!(is_under(
            "/work/.eval-magic/x/out.md",
            "/work/.eval-magic",
            repo
        ));
        assert!(!is_under("/work/runner/run.ts", "/work/.eval-magic", repo));
        // `.eval-magic2` is not under `.eval-magic` (separator boundary).
        assert!(!is_under("/work/.eval-magic2/x", "/work/.eval-magic", repo));
    }

    #[test]
    fn is_under_resolves_relative_targets_against_repo_root() {
        let repo = Path::new("/work");
        assert!(is_under(".eval-magic/x", "/work/.eval-magic", repo));
    }

    #[test]
    fn is_under_any_checks_every_root() {
        let repo = Path::new("/work");
        assert!(is_under_any("/work/.claude/skills/s", &roots(), repo));
        assert!(!is_under_any("/etc/passwd", &roots(), repo));
    }

    #[test]
    fn classify_bash_flags_targets_outside_allowed_roots() {
        let cwd = Path::new("/outside/project");
        assert_eq!(
            classify_bash_with_cwd("npm install left-pad", &roots(), cwd).map(|d| d.reason),
            Some("package install/add"),
        );
        assert_eq!(
            classify_bash_with_cwd("git worktree add ../wt -b scratch", &roots(), cwd)
                .map(|d| d.reason),
            Some("git worktree add (working tree outside the sandbox)"),
        );
        assert_eq!(
            classify_bash_with_cwd("echo hi > out.log", &roots(), cwd).map(|d| d.reason),
            Some("output redirection to a file"),
        );
    }

    #[test]
    fn classify_bash_denies_package_install_with_an_outside_destination() {
        let roots = vec!["/work/env".to_string()];

        let denial = classify_bash_with_cwd(
            "npm install left-pad --prefix /outside/project",
            &roots,
            Path::new("/work/env"),
        )
        .expect("outside package destination should be denied");

        assert_eq!(denial.reason, "package install/add");
        assert_eq!(denial.resolved_targets, vec!["/outside/project"]);

        let broad_policy = crate::core::GuardPolicyConfig {
            allow_tools: vec!["npm".to_string()],
            ..crate::core::GuardPolicyConfig::default()
        };
        assert_eq!(
            classify_bash_with_policy(
                "npm install left-pad --prefix /outside/project",
                &roots,
                Path::new("/work/env"),
                &broad_policy,
            )
            .map(|classification| classification.reason),
            Some("package install/add")
        );
    }

    #[test]
    fn classify_bash_allows_local_git_workflows_inside_the_task_repository() {
        let cwd = Path::new("/work/.eval-magic/task");
        for command in [
            "git status --short",
            "git add . && git commit -m baseline",
            "git switch -c scratch",
            "git checkout -- src/lib.rs",
            "git reset HEAD~1",
            "git branch topic",
            "git rev-parse --git-dir",
            "git -C /work/.eval-magic/task status",
            "GIT_WORK_TREE=/work/.eval-magic/task git status",
        ] {
            assert_eq!(
                classify_bash_with_cwd(command, &roots(), cwd),
                None,
                "{command}"
            );
        }
    }

    #[test]
    fn classify_bash_denies_git_repository_routing_escapes() {
        let cwd = Path::new("/work/.eval-magic/task");
        for command in [
            "git -C /outside status",
            "git --git-dir=/outside/repo.git status",
            "git --work-tree /outside status",
            "GIT_DIR=/outside/repo.git git status",
            "env GIT_WORK_TREE=../../outside git status",
            "git -C \"$TARGET\" status",
            "git --git-dir='unterminated status",
        ] {
            let denial = classify_bash_with_cwd(command, &roots(), cwd)
                .unwrap_or_else(|| panic!("expected Git routing denial for {command}"));
            assert_eq!(denial.reason, "git repository routing escape", "{command}");
        }
    }

    #[test]
    fn classify_bash_denies_remote_git_operations_before_allowed_root_shortcut() {
        let cwd = Path::new("/work/.eval-magic/task");
        for command in [
            "git clone https://example.com/repo.git",
            "git fetch origin",
            "git --no-pager push origin main",
            "git -c color.ui=false push origin main",
            "git pull",
            "git push /work/.eval-magic/task",
            "git ls-remote origin",
            "git submodule update --init",
            "git send-email patch",
            "git svn fetch",
            "git p4 sync",
            "git archive --remote=origin HEAD",
            "git remote add origin https://example.com/repo.git",
            "git remote remove origin",
            "git config remote.origin.url https://example.com/repo.git",
            "git config --add url.ssh://git@example.com/.insteadOf gh:",
            "git config set remote.origin.url https://example.com/repo.git",
            "git config unset remote.origin.url",
            "git config rename-section remote.origin remote.backup",
            "git config remove-section remote.origin",
            // Descriptor duplication no longer emits a separator, so it must
            // not end the Git segment early and hide the subcommand.
            "git push origin main 2>&1",
            "git fetch origin >/dev/null 2>&1",
            "git clone https://example.com/repo.git 2>&1 | tee log.txt",
        ] {
            let denial = classify_bash_with_cwd(command, &roots(), cwd)
                .unwrap_or_else(|| panic!("expected remote Git denial for {command}"));
            assert_eq!(denial.reason, "git remote operation", "{command}");
        }
    }

    #[test]
    fn classify_bash_allows_remote_inspection_without_mutation() {
        let cwd = Path::new("/work/.eval-magic/task");
        for command in [
            "git remote",
            "git remote -v",
            "git remote get-url origin",
            "git config remote.origin.url",
            "git config --get url.ssh://git@example.com/.insteadOf",
        ] {
            assert_eq!(
                classify_bash_with_cwd(command, &roots(), cwd),
                None,
                "{command}"
            );
        }
    }

    #[test]
    fn classify_bash_does_not_special_case_harness_config_dirs() {
        let cwd = Path::new("/work/.eval-magic/task");
        for dir in crate::adapters::all_config_dir_names() {
            assert_eq!(
                classify_bash_with_cwd(&format!("mkdir -p {dir}/x"), &roots(), cwd),
                None,
                "mkdir under {dir} should be allowed"
            );
            assert_eq!(
                classify_bash_with_cwd(&format!("cp hooks.json {dir}/hooks.json"), &roots(), cwd),
                None,
                "cp into {dir} should be allowed"
            );
            assert_eq!(
                classify_bash_with_cwd(&format!("cat {dir}/settings.json"), &roots(), cwd),
                None,
                "read of {dir} should stay allowed"
            );
            assert_eq!(
                classify_bash_with_cwd(&format!("ls {dir}"), &roots(), cwd),
                None
            );
        }
    }

    #[test]
    fn classify_bash_allows_in_bounds_outputs_and_readonly_commands() {
        // The redirect target resolves under an allowed root.
        assert_eq!(
            classify_bash("echo hi > /work/.eval-magic/x/log", &roots()),
            None
        );
        assert_eq!(classify_bash("ls -la /", &roots()), None);
        assert_eq!(classify_bash("", &roots()), None);
    }
}
