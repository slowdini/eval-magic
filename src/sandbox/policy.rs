//! Write-boundary primitives.
//!
//! Stateless classifiers shared by the armed guard ([`super::decide`]) and
//! `pipeline::detect-stray-writes`: which tools write, which Bash commands
//! mutate state outside a sandbox, and whether a path falls under an allowed
//! root. Tool names come from the adapters' cross-harness vocabulary union
//! ([`all_tool_vocabulary`]), so no harness's tool naming is hardcoded here.

use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
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

/// Bash command patterns that mutate state outside an eval's sandbox. Heuristics
/// — Bash is too flexible to parse exactly. `detect-stray-writes` surfaces these
/// as warnings; the opt-in guard denies them. Each is meaningful only when the
/// command does not reference an allowed root (see [`classify_bash`]).
///
/// Output redirects and `tee` are intentionally absent. The quote-aware target
/// scanner resolves relative paths from the tool invocation cwd instead of
/// relying on command-text containment.
///
/// Compiled once. The patterns are known-valid, so a compile failure here is a
/// programmer error and panics.
static BASH_MUTATION_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let config_dirs = crate::adapters::all_config_dir_names()
        .iter()
        .map(|d| regex::escape(d))
        .collect::<Vec<_>>()
        .join("|");
    [
        (
            r"\b(npm|pnpm|yarn|bun)\s+(install|add|ci|i)\b".to_string(),
            "package install/add",
        ),
        (r"\bpip3?\s+install\b".to_string(), "pip install"),
        (r"\bsed\s+-i\b".to_string(), "in-place file edit (sed -i)"),
        // A create/copy/move/link verb whose operand is a path under any
        // harness config dir (`adapters::all_config_dir_names`) — catches
        // stray writes to a config dir that aren't a `>` redirect (caught
        // below). Read-only verbs (`cat`, `ls`) aren't listed, so inspecting
        // the dirs stays allowed.
        (
            format!(r"\b(cp|mv|mkdir|touch|ln|rsync|install)\b[^|;&\n]*({config_dirs})(/|\b)"),
            "path under a harness config dir",
        ),
        // The same create verbs whose operand is a top-level `skills/` directory —
        // catches a bare `skills/` left in the cwd. `skills-data` and other
        // `skills`-prefixed names are excluded by the trailing `/`, whitespace, or
        // end-of-string boundary.
        (
            r#"\b(cp|mv|mkdir|touch|ln|rsync)\b[^|;&\n]*[\s'"=/]\.{0,2}/?skills(/|\s|$)"#
                .to_string(),
            "creates a bare skills/ dir",
        ),
    ]
    .into_iter()
    .map(|(re, reason)| {
        (
            Regex::new(&re)
                .unwrap_or_else(|e| panic!("bundled bash pattern {re:?} is invalid: {e}")),
            reason,
        )
    })
    .collect()
});

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

/// Lexically absolutize a path: join onto `repo_root` if relative, then normalize.
/// Mirrors node's `resolve()` — no symlink resolution or existence requirement.
pub(crate) fn resolve_path(target: &str, repo_root: &Path) -> PathBuf {
    let joined = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        repo_root.join(target)
    };
    let absolute = std::path::absolute(&joined).unwrap_or(joined);
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
    if command.is_empty() {
        return None;
    }
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
    if allowed_roots.iter().any(|r| command.contains(r)) {
        return None;
    }
    BASH_MUTATION_PATTERNS
        .iter()
        .find(|(re, _)| re.is_match(command))
        .map(|(_, reason)| BashClassification {
            reason,
            resolved_targets: Vec::new(),
        })
}

/// If a Bash command matches a mutation pattern and is not scoped to one of
/// `allowed_roots`, return the human reason; otherwise `None`. A command is
/// treated as scoped when it textually references an allowed root. Output-file
/// targets are the exception: they are resolved lexically from the process cwd
/// and every target must fall under an allowed root.
pub fn classify_bash(command: &str, allowed_roots: &[String]) -> Option<&'static str> {
    let cwd = std::env::current_dir().unwrap_or_default();
    classify_bash_with_cwd(command, allowed_roots, &cwd).map(|result| result.reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn classify_bash_flags_installs_and_git_worktree_escape() {
        assert_eq!(
            classify_bash("npm install left-pad", &roots()),
            Some("package install/add")
        );
        assert_eq!(
            classify_bash("git worktree add ../wt -b scratch", &roots()),
            Some("git worktree add (working tree outside the sandbox)")
        );
        assert_eq!(
            classify_bash("echo hi > out.log", &roots()),
            Some("output redirection to a file")
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
    fn classify_bash_flags_creates_under_every_harness_config_dir_but_allows_reads() {
        for dir in crate::adapters::all_config_dir_names() {
            assert_eq!(
                classify_bash(&format!("mkdir -p {dir}/x"), &[]),
                Some("path under a harness config dir"),
                "mkdir under {dir} should be flagged"
            );
            assert_eq!(
                classify_bash(&format!("cp evil.json {dir}/hooks.json"), &[]),
                Some("path under a harness config dir"),
                "cp into {dir} should be flagged"
            );
            assert_eq!(
                classify_bash(&format!("cat {dir}/settings.json"), &[]),
                None,
                "read of {dir} should stay allowed"
            );
            assert_eq!(classify_bash(&format!("ls {dir}"), &[]), None);
        }
    }

    #[test]
    fn classify_bash_allows_scoped_and_readonly_commands() {
        // Textually references an allowed root → scoped → allowed.
        assert_eq!(
            classify_bash("echo hi > /work/.eval-magic/x/log", &roots()),
            None
        );
        assert_eq!(classify_bash("ls -la /", &roots()), None);
        assert_eq!(classify_bash("", &roots()), None);
    }
}
