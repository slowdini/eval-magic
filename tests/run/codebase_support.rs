use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed in {}:\n{}",
        args.join(" "),
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A repository usable as a codebase source: two commits on `branch`, with a
/// `.gitignore` that ignores `build/`, and an ignored file already present.
pub fn codebase_repo(root: &Path, name: &str, branch: &str) -> PathBuf {
    let repo = root.join(name);
    fs::create_dir_all(repo.join("src")).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch", branch, "."]);
    fs::write(repo.join(".gitignore"), "build/\n").unwrap();
    fs::write(repo.join("src/lib.rs"), "pub fn one() -> u32 { 1 }\n").unwrap();
    commit(&repo, "first");
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    commit(&repo, "second");
    fs::create_dir_all(repo.join("build")).unwrap();
    fs::write(repo.join("build/artifact.bin"), "not source\n").unwrap();
    repo
}

pub fn commit(cwd: &Path, message: &str) {
    git(cwd, &["add", "--all"]);
    git(
        cwd,
        &[
            "-c",
            "user.name=Codebase Author",
            "-c",
            "user.email=codebase@example.com",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    );
}

/// An evals config whose single eval overlays `TASK.md` onto `codebase`.
pub fn evals_with_codebase(codebase: &str) -> String {
    format!(
        r#"{{
          "skill_name": "mr-review",
          "codebase": {codebase},
          "evals": [
            {{
              "id": "e1",
              "prompt": "add a function",
              "expected_output": "a function",
              "files": ["TASK.md"]
            }}
          ]
        }}"#
    )
}

pub fn add_project_skill_roots(repo: &Path, roots: &[&str]) {
    for root in roots {
        fs::create_dir_all(repo.join(root).join("mr-review")).unwrap();
        fs::write(
            repo.join(root).join("mr-review/SKILL.md"),
            "---\nname: mr-review\ndescription: codebase copy\n---\n\nCODEBASE\n",
        )
        .unwrap();
        fs::create_dir_all(repo.join(root).join("slow-powers-eval-codebase-owned")).unwrap();
        fs::write(
            repo.join(root)
                .join("slow-powers-eval-codebase-owned/SKILL.md"),
            "CODEBASE PREFIXED SKILL\n",
        )
        .unwrap();
    }
    fs::write(repo.join("CLAUDE.md"), "claude instructions\n").unwrap();
    fs::write(repo.join("AGENTS.md"), "agent instructions\n").unwrap();
    fs::create_dir_all(repo.join(".opencode")).unwrap();
    fs::write(repo.join(".opencode/settings.json"), "{}\n").unwrap();
    commit(repo, "add project skill sources and instructions");
}

/// The number of hard links to `file` — the mechanism `git clone --local` uses
/// to share the cache's object store with an environment instead of copying it.
pub fn link_count(file: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(file).unwrap().nlink() as u32
}

/// A file from `repo`'s object store — a loose object or a pack — that a local
/// clone shares with its source by hard link. `objects/info` is skipped because
/// it holds per-repository metadata rather than objects.
pub fn an_object_file(repo: &Path) -> PathBuf {
    fn walk(dir: &Path) -> Option<PathBuf> {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "info") {
                    continue;
                }
                if let Some(found) = walk(&path) {
                    return Some(found);
                }
            } else {
                return Some(path);
            }
        }
        None
    }
    walk(&repo.join(".git/objects")).expect("a materialized repository has objects")
}
