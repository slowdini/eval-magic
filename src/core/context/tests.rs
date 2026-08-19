use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Build `<root>/skill-dir` containing one subdir per name, each with a
/// `SKILL.md`, and return the skill-dir path.
fn make_skill_dir(root: &Path, skills: &[&str]) -> PathBuf {
    let dir = root.join("skill-dir");
    fs::create_dir_all(&dir).unwrap();
    for name in skills {
        let sub = dir.join(name);
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\n\nbody\n"),
        )
        .unwrap();
    }
    dir
}

fn input(skill_dir: &Path, skill: &str) -> DetectInput {
    DetectInput {
        skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
        skill: Some(skill.to_string()),
        ..Default::default()
    }
}

fn input_from(cwd: &Path) -> DetectInput {
    DetectInput {
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    }
}

#[test]
fn cwd_skill_dir_is_the_default_single_skill() {
    let tmp = TempDir::new().unwrap();
    let skill_subdir = tmp.path().join("mr-review");
    fs::create_dir_all(&skill_subdir).unwrap();
    fs::write(
        skill_subdir.join("SKILL.md"),
        "---\nname: mr-review\n---\n\nbody\n",
    )
    .unwrap();

    let ctx = detect_run_context(input_from(&skill_subdir)).unwrap();

    assert_eq!(ctx.skill_name, "mr-review");
    assert_eq!(
        ctx.skill_subdir,
        crate::core::fs::real_path(&skill_subdir).unwrap()
    );
    assert!(ctx.sibling_skill_names.is_empty());
    assert!(!ctx.stage_siblings);
}

#[test]
fn skill_path_selects_one_skill_without_siblings() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["alpha", "beta"]);

    let ctx = detect_run_context(DetectInput {
        skill: Some(skill_dir.join("beta").to_string_lossy().into_owned()),
        cwd: Some(tmp.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(ctx.skill_name, "beta");
    assert_eq!(
        ctx.skill_subdir,
        crate::core::fs::real_path(&skill_dir.join("beta")).unwrap()
    );
    assert!(ctx.sibling_skill_names.is_empty());
    assert!(!ctx.stage_siblings);
}

#[test]
fn skill_dir_with_one_skill_infers_the_skill_name_and_stages_siblings_mode() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["only-skill"]);

    let ctx = detect_run_context(DetectInput {
        skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
        cwd: Some(tmp.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(ctx.skill_name, "only-skill");
    assert!(ctx.sibling_skill_names.is_empty());
    assert!(ctx.stage_siblings);
}

#[test]
fn skill_dir_with_multiple_skills_requires_a_skill_name() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["alpha", "beta"]);

    let err = detect_run_context(DetectInput {
        skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
        cwd: Some(tmp.path().to_path_buf()),
        ..Default::default()
    })
    .unwrap_err();

    assert!(matches!(err, ContextError::AmbiguousSkillSelection(_)));
    assert!(err.to_string().contains("alpha"));
    assert!(err.to_string().contains("beta"));
}

#[test]
fn missing_skill_errors_when_cwd_is_not_a_skill() {
    let tmp = TempDir::new().unwrap();
    let err = detect_run_context(input_from(tmp.path())).unwrap_err();
    assert!(matches!(err, ContextError::MissingSkill));
    assert!(err.to_string().contains("--skill"));
}

#[test]
fn empty_skill_dir_errors_when_skill_is_not_named() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join("skill-dir");
    fs::create_dir_all(&skill_dir).unwrap();
    let err = detect_run_context(DetectInput {
        skill_dir: Some(skill_dir.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .unwrap_err();
    assert!(matches!(err, ContextError::NoSkillsInSkillDir(_)));
    assert!(err.to_string().contains("no skills found"));
}

#[test]
fn skill_dir_not_directory_errors() {
    let err = detect_run_context(DetectInput {
        skill_dir: Some("/nonexistent/does-not-exist-12345".into()),
        skill: Some("foo".into()),
        ..Default::default()
    })
    .unwrap_err();
    assert!(matches!(err, ContextError::SkillDirNotDirectory(_)));
    assert!(err.to_string().contains("--skill-dir"));
}

#[test]
fn skill_subdir_missing_errors() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    let err = detect_run_context(input(&skill_dir, "bar")).unwrap_err();
    assert!(matches!(err, ContextError::SkillNotFound(_)));
    assert!(err.to_string().contains("skill not found"));
}

#[test]
fn bad_bootstrap_errors() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    let err = detect_run_context(DetectInput {
        bootstrap: Some("/nonexistent/no-bootstrap-12345.md".into()),
        ..input(&skill_dir, "foo")
    })
    .unwrap_err();
    assert!(matches!(err, ContextError::BootstrapNotFound(_)));
    assert!(err.to_string().contains("--bootstrap"));
}

#[test]
fn happy_path_absolute_paths() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["mr-review"]);
    let ctx = detect_run_context(input(&skill_dir, "mr-review")).unwrap();
    assert_eq!(
        ctx.skill_dir,
        crate::core::fs::real_path(&skill_dir).unwrap()
    );
    assert_eq!(ctx.skill_name, "mr-review");
    assert_eq!(
        ctx.skill_subdir,
        crate::core::fs::real_path(&skill_dir.join("mr-review")).unwrap()
    );
    assert!(ctx.sibling_skill_names.is_empty());
    assert!(ctx.bootstrap_path.is_none());
    assert_eq!(ctx.harness, Harness::resolve("claude-code").unwrap());
}

#[test]
fn enumerates_siblings_excluding_sut() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["alpha", "beta", "gamma"]);
    let ctx = detect_run_context(input(&skill_dir, "beta")).unwrap();
    assert_eq!(
        ctx.sibling_skill_names,
        vec!["alpha".to_string(), "gamma".to_string()]
    );
}

#[test]
fn ignores_non_skill_md_entries() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["real"]);
    fs::create_dir_all(skill_dir.join("node_modules")).unwrap();
    fs::create_dir_all(skill_dir.join("no-skill-md-here")).unwrap();
    fs::write(skill_dir.join("loose-file.txt"), "hello").unwrap();
    let ctx = detect_run_context(input(&skill_dir, "real")).unwrap();
    assert!(ctx.sibling_skill_names.is_empty());
}

/// The point of the relocation: eval artifacts stop landing inside whatever
/// repository the operator happened to be standing in.
#[test]
fn workspace_default_is_outside_the_cwd_and_the_skill_tree() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    let ctx = detect_run_context(input(&skill_dir, "foo")).unwrap();
    let cwd = crate::core::fs::real_path(&std::env::current_dir().unwrap()).unwrap();

    assert!(
        !ctx.workspace_root.starts_with(&cwd),
        "workspace {} is still under the cwd {}",
        ctx.workspace_root.display(),
        cwd.display()
    );
    assert!(
        !ctx.workspace_root.starts_with(&skill_dir),
        "workspace {} is still under the skill tree",
        ctx.workspace_root.display()
    );
}

/// `EVAL_MAGIC_WORKSPACE_DIR` sits between the flag and the derived default,
/// mirroring the `EVAL_MAGIC_CONFIG_DIR` ladder in `descriptor::layers`.
#[test]
fn workspace_root_env_override_is_taken_as_given() {
    let root = workspace_root_from(
        Some("/srv/evals"),
        Some("/xdg/data"),
        Some(Path::new("/home/u")),
        Path::new("/home/u/skills"),
    );
    assert_eq!(root, PathBuf::from("/srv/evals"));
}

#[test]
fn workspace_root_prefers_xdg_data_home_over_the_home_fallback() {
    let root = workspace_root_from(
        None,
        Some("/xdg/data"),
        Some(Path::new("/home/u")),
        Path::new("/home/u/skills"),
    );
    assert!(
        root.starts_with("/xdg/data/eval-magic"),
        "root was {}",
        root.display()
    );
}

#[test]
fn workspace_root_falls_back_to_the_home_data_directory() {
    let root = workspace_root_from(
        None,
        None,
        Some(Path::new("/home/u")),
        Path::new("/home/u/skills"),
    );
    assert!(
        root.starts_with("/home/u/.local/share/eval-magic"),
        "root was {}",
        root.display()
    );
}

/// One global root would collide two skills that share a name and come from
/// different repositories, silently interleaving their iterations. The slug
/// is what keeps them apart.
#[test]
fn workspace_root_keeps_same_named_skill_dirs_apart() {
    let home = Path::new("/home/u");
    let a = workspace_root_from(None, None, Some(home), Path::new("/work/one/skills"));
    let b = workspace_root_from(None, None, Some(home), Path::new("/work/two/skills"));
    assert_ne!(a, b);
}

/// The slug is part of a path the operator will re-type and that generated
/// commands embed, so it has to be the same on every run — which rules out
/// any hash without a cross-release stability guarantee.
#[test]
fn workspace_root_is_stable_for_one_skill_dir() {
    let home = Path::new("/home/u");
    let skills = Path::new("/work/one/skills");
    assert_eq!(
        workspace_root_from(None, None, Some(home), skills),
        workspace_root_from(None, None, Some(home), skills)
    );
}

#[test]
fn workspace_root_slug_survives_a_basename_that_is_not_path_safe() {
    let root = workspace_root_from(
        None,
        None,
        Some(Path::new("/home/u")),
        Path::new("/work/my skills:v2"),
    );
    let slug = root.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        slug.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
        "slug was {slug}"
    );
    assert!(slug.starts_with("my-skills-v2-"), "slug was {slug}");
}

/// An operator upgrading mid-campaign would otherwise find `ingest` unable to
/// see the iteration `run` had just built.
#[test]
fn a_legacy_workspace_in_the_cwd_is_reported() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    fs::create_dir_all(tmp.path().join(".eval-magic").join("foo")).unwrap();

    let ctx = detect_run_context(DetectInput {
        cwd: Some(tmp.path().to_path_buf()),
        ..input(&skill_dir, "foo")
    })
    .unwrap();

    assert!(
        ctx.warnings.iter().any(|w| w.contains(".eval-magic")),
        "warnings were: {:?}",
        ctx.warnings
    );
}

/// Advice to "pass --workspace-dir <x>" is worse than silence when the run is
/// already using `<x>`. Reachable whenever the resolved root lands on the old
/// path — an `EVAL_MAGIC_WORKSPACE_DIR` naming it, say.
#[test]
fn no_legacy_notice_when_the_resolved_workspace_is_that_directory() {
    let tmp = TempDir::new().unwrap();
    let legacy = tmp.path().join(".eval-magic");
    fs::create_dir_all(legacy.join("mr-review")).unwrap();

    assert_eq!(legacy_workspace_notice(tmp.path(), &legacy), None);
    assert!(legacy_workspace_notice(tmp.path(), Path::new("/elsewhere/eval-magic")).is_some());
}

/// `.eval-magic/harnesses/` is the project-local descriptor layer — a
/// deliberate, unrelated use of the same name that does not move and must
/// not be mistaken for an orphaned campaign.
#[test]
fn a_descriptor_layer_alone_is_not_reported_as_a_legacy_workspace() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    fs::create_dir_all(tmp.path().join(".eval-magic").join("harnesses")).unwrap();

    let ctx = detect_run_context(DetectInput {
        cwd: Some(tmp.path().to_path_buf()),
        ..input(&skill_dir, "foo")
    })
    .unwrap();

    assert!(ctx.warnings.is_empty(), "warnings were: {:?}", ctx.warnings);
}

#[test]
fn workspace_override_absolute() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    let custom = tmp.path().join("custom-ws");
    fs::create_dir_all(&custom).unwrap();
    let ctx = detect_run_context(DetectInput {
        workspace_dir: Some(custom.to_string_lossy().into_owned()),
        ..input(&skill_dir, "foo")
    })
    .unwrap();
    assert_eq!(
        ctx.workspace_root,
        crate::core::fs::real_path(&custom).unwrap()
    );
}

#[test]
fn stage_root_default() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    let ctx = detect_run_context(input(&skill_dir, "foo")).unwrap();
    assert_eq!(
        ctx.stage_root,
        crate::core::fs::real_path(&std::env::current_dir().unwrap()).unwrap()
    );
}

/// Every root derives from the cwd, and the guard later compares those roots
/// against paths the agent's own tools report — so an alias of the cwd has to
/// collapse here, once, or the two sides disagree forever after.
///
/// Windows spells one directory several ways (8.3 short names, junctions,
/// `subst` drives, redirected profiles); each is one `canonicalize` apart
/// from the real path, so exercising one exercises the mechanism.
#[test]
fn a_cwd_alias_collapses_so_every_derived_root_shares_one_spelling() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real-workspace");
    fs::create_dir_all(&real).unwrap();
    let alias = tmp.path().join("alias-workspace");
    crate::core::fs::create_directory_alias(&real, &alias).unwrap();
    make_skill_dir(&real, &["foo"]);

    // Enter through the alias, exactly as a user whose workspace sits under a
    // junction or a redirected profile directory does.
    let ctx = detect_run_context(DetectInput {
        skill: Some("foo".to_string()),
        ..input_from(&alias.join("skill-dir"))
    })
    .unwrap();

    let expected = crate::core::fs::real_path(&real).unwrap();
    assert_eq!(ctx.stage_root, expected.join("skill-dir"));
    assert_eq!(ctx.skill_dir, expected.join("skill-dir"));

    // The workspace root now derives from the skill dir rather than the cwd,
    // so the alias has to collapse there too: entering through the alias and
    // entering directly must name one workspace, not two.
    let direct = detect_run_context(DetectInput {
        skill: Some("foo".to_string()),
        ..input_from(&expected.join("skill-dir"))
    })
    .unwrap();
    assert_eq!(ctx.workspace_root, direct.workspace_root);
}

/// `--workspace-dir` is the second way into the same tree: the guard's roots
/// descend from it, so an alias passed here would reintroduce the split the
/// cwd resolution just closed.
#[test]
fn an_aliased_workspace_dir_flag_resolves_to_the_same_spelling() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real-workspace");
    fs::create_dir_all(&real).unwrap();
    let alias = tmp.path().join("alias-workspace");
    crate::core::fs::create_directory_alias(&real, &alias).unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);

    let ctx = detect_run_context(DetectInput {
        workspace_dir: Some(alias.join("nested-ws").to_string_lossy().into_owned()),
        ..input(&skill_dir, "foo")
    })
    .unwrap();

    assert_eq!(
        ctx.workspace_root,
        crate::core::fs::real_path(&real).unwrap().join("nested-ws")
    );
}

#[test]
fn bootstrap_resolved_absolute() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    let bootstrap = tmp.path().join("my-bootstrap.md");
    fs::write(&bootstrap, "BOOT").unwrap();
    let ctx = detect_run_context(DetectInput {
        bootstrap: Some(bootstrap.to_string_lossy().into_owned()),
        ..input(&skill_dir, "foo")
    })
    .unwrap();
    assert_eq!(
        ctx.bootstrap_path,
        Some(crate::core::fs::real_path(&bootstrap).unwrap())
    );
}

#[test]
fn harness_codex_accepted() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    let ctx = detect_run_context(DetectInput {
        harness: Some(Harness::resolve("codex").unwrap()),
        ..input(&skill_dir, "foo")
    })
    .unwrap();
    assert_eq!(ctx.harness, Harness::resolve("codex").unwrap());
}

#[test]
fn harness_opencode_accepted() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = make_skill_dir(tmp.path(), &["foo"]);
    let ctx = detect_run_context(DetectInput {
        harness: Some(Harness::resolve("opencode").unwrap()),
        ..input(&skill_dir, "foo")
    })
    .unwrap();
    assert_eq!(ctx.harness, Harness::resolve("opencode").unwrap());
}
