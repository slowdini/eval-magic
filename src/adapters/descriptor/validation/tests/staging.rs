use super::{MINIMAL, err_of};

#[test]
fn rejects_slug_template_missing_a_placeholder() {
    let err = err_of(&format!(
        "{MINIMAL}\n[staging]\nslug_template = \"{{prefix}}{{iteration}}-{{condition}}\"\n"
    ));
    assert!(err.contains("staging.slug_template"), "{err}");
    assert!(err.contains("{skill_name}"), "{err}");
}

#[test]
fn rejects_slug_template_and_capability_together() {
    let err = err_of(&format!(
        "{MINIMAL}\n[staging]\nslug_template = \"{{prefix}}{{iteration}}-{{condition}}__{{skill_name}}\"\nslug_capability = \"opencode\"\n"
    ));
    assert!(err.contains("not both"), "{err}");
}

#[test]
fn rejects_slug_that_fails_its_own_stage_name_rules() {
    // The default slug shape emits `__`, which the single-hyphen pattern
    // rejects — the staged-slug↔naming-rules invariant.
    let err = err_of(&format!(
        "{MINIMAL}\n[staging]\nslug_template = \"{{prefix}}{{iteration}}-{{condition}}__{{skill_name}}\"\nstage_name_pattern = \"^[a-z0-9]+(-[a-z0-9]+)*$\"\nstage_name_max_len = 64\n"
    ));
    assert!(err.contains("stage-name rules"), "{err}");
}

#[test]
fn rejects_config_dirs_missing_skills_dir_parent() {
    let err = err_of(&MINIMAL.replace("[\".demo\"]", "[\".other\"]"));
    assert!(err.contains("parent of skills_dir"), "{err}");
    assert!(err.contains(".demo"), "{err}");
}

#[test]
fn rejects_stage_name_pattern_that_does_not_compile() {
    let err = err_of(&format!(
        "{MINIMAL}\n[staging]\nstage_name_pattern = \"[unclosed\"\n"
    ));
    assert!(err.contains("does not compile"), "{err}");
}
