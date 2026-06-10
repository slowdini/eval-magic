//! Write-boundary primitives.
//!
//! Stateless classifiers shared by the armed guard ([`super::decide`]) and
//! `pipeline::detect-stray-writes`: which tools write, which Bash commands
//! mutate state outside a sandbox, and whether a path falls under an allowed
//! root.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// Tools that mutate the filesystem and carry a target path argument.
pub const WRITE_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// True for a tool name that writes the filesystem with a path argument.
pub fn is_write_tool(tool_name: &str) -> bool {
    WRITE_TOOLS.contains(&tool_name)
}

/// Bash command patterns that mutate state outside an eval's sandbox. Heuristics
/// — Bash is too flexible to parse exactly. `detect-stray-writes` surfaces these
/// as warnings; the opt-in guard denies them. Each is meaningful only when the
/// command does not reference an allowed root (see [`classify_bash`]).
///
/// Compiled once. The patterns are known-valid, so a compile failure here is a
/// programmer error and panics.
static BASH_MUTATION_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            r"\b(npm|pnpm|yarn|bun)\s+(install|add|ci|i)\b",
            "package install/add",
        ),
        (r"\bpip3?\s+install\b", "pip install"),
        (r"\bsed\s+-i\b", "in-place file edit (sed -i)"),
        (
            r"\bgit\s+(commit|add|push|checkout|reset|restore|merge|rebase)\b",
            "git mutation",
        ),
        (
            r"\bgit\s+worktree\s+add\b",
            "git worktree add (working tree outside the sandbox)",
        ),
        // A create/copy/move/link verb whose operand is a path under `.claude` —
        // catches stray writes to the harness config dir that aren't a `>`
        // redirect (caught below). Read-only verbs (`cat`, `ls`) aren't listed,
        // so inspecting `.claude` stays allowed.
        (
            r"\b(cp|mv|mkdir|touch|ln|rsync|install)\b[^|;&\n]*\.claude(/|\b)",
            "path under .claude",
        ),
        // The same create verbs whose operand is a top-level `skills/` directory —
        // catches a bare `skills/` left in the cwd. `skills-workspace` and other
        // `skills`-prefixed names are excluded by the trailing `/`, whitespace, or
        // end-of-string boundary.
        (
            r#"\b(cp|mv|mkdir|touch|ln|rsync)\b[^|;&\n]*[\s'"=/]\.{0,2}/?skills(/|\s|$)"#,
            "creates a bare skills/ dir",
        ),
        (r"(^|\s)(>>?|tee)\s", "output redirection to a file"),
    ]
    .into_iter()
    .map(|(re, reason)| {
        (
            Regex::new(re)
                .unwrap_or_else(|e| panic!("bundled bash pattern {re:?} is invalid: {e}")),
            reason,
        )
    })
    .collect()
});

/// Pull the target path from a write tool's arguments (`file_path` →
/// `notebook_path` → `path`). Returns `None` when the input is not an object or
/// carries no string path.
pub fn path_arg(args: &Value) -> Option<&str> {
    let obj = args.as_object()?;
    ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str))
}

/// Extract file paths from a Codex `apply_patch` hook payload. Codex can expose
/// patch targets as a structured `files` list or as freeform patch text; collect
/// both so the guard can deny unknown or out-of-bounds patches before they run.
pub fn apply_patch_paths(args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = args.as_object() else {
        return out;
    };

    if let Some(files) = obj.get("files") {
        collect_file_values(files, &mut out);
    }

    for key in ["patch", "input", "content"] {
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
fn absolutize(target: &str, repo_root: &Path) -> std::path::PathBuf {
    let joined = if Path::new(target).is_absolute() {
        std::path::PathBuf::from(target)
    } else {
        repo_root.join(target)
    };
    // `std::path::absolute` normalizes `.`/`..` lexically without touching disk.
    std::path::absolute(&joined).unwrap_or(joined)
}

/// True when `target` resolves to `dir` or a descendant of it. Relative `target`s
/// resolve against `repo_root`. `Path::starts_with` matches whole path
/// components, so `skills-workspace2` is correctly not under `skills-workspace`.
pub fn is_under(target: &str, dir: &str, repo_root: &Path) -> bool {
    let base = absolutize(dir, repo_root);
    let abs = absolutize(target, repo_root);
    abs.starts_with(&base)
}

/// True when `target` is under any of `dirs`.
pub fn is_under_any(target: &str, dirs: &[String], repo_root: &Path) -> bool {
    dirs.iter().any(|d| is_under(target, d, repo_root))
}

/// If a Bash command matches a mutation pattern and is not scoped to one of
/// `allowed_roots`, return the human reason; otherwise `None`. A command is
/// treated as scoped when it textually references an allowed root.
pub fn classify_bash(command: &str, allowed_roots: &[String]) -> Option<&'static str> {
    if command.is_empty() {
        return None;
    }
    if allowed_roots.iter().any(|r| command.contains(r)) {
        return None;
    }
    BASH_MUTATION_PATTERNS
        .iter()
        .find(|(re, _)| re.is_match(command))
        .map(|(_, reason)| *reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ROOTS: [&str; 2] = ["/work/skills-workspace", "/work/.claude/skills"];

    fn roots() -> Vec<String> {
        ROOTS.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn is_write_tool_matches_the_four_write_tools() {
        for t in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
            assert!(is_write_tool(t), "{t} should be a write tool");
        }
        for t in ["Read", "Bash", "Grep", ""] {
            assert!(!is_write_tool(t), "{t} should not be a write tool");
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
        assert!(is_under(
            "/work/skills-workspace",
            "/work/skills-workspace",
            repo
        ));
        assert!(is_under(
            "/work/skills-workspace/x/out.md",
            "/work/skills-workspace",
            repo
        ));
        assert!(!is_under(
            "/work/runner/run.ts",
            "/work/skills-workspace",
            repo
        ));
        // `skills-workspace2` is not under `skills-workspace` (separator boundary).
        assert!(!is_under(
            "/work/skills-workspace2/x",
            "/work/skills-workspace",
            repo
        ));
    }

    #[test]
    fn is_under_resolves_relative_targets_against_repo_root() {
        let repo = Path::new("/work");
        assert!(is_under(
            "skills-workspace/x",
            "/work/skills-workspace",
            repo
        ));
    }

    #[test]
    fn is_under_any_checks_every_root() {
        let repo = Path::new("/work");
        assert!(is_under_any("/work/.claude/skills/s", &roots(), repo));
        assert!(!is_under_any("/etc/passwd", &roots(), repo));
    }

    #[test]
    fn classify_bash_flags_install_and_git_mutations() {
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
    fn classify_bash_allows_scoped_and_readonly_commands() {
        // Textually references an allowed root → scoped → allowed.
        assert_eq!(
            classify_bash("echo hi > /work/skills-workspace/x/log", &roots()),
            None
        );
        assert_eq!(classify_bash("ls -la /", &roots()), None);
        assert_eq!(classify_bash("", &roots()), None);
    }
}
