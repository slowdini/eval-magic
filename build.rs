use std::env;
use std::fs;
use std::path::{Path, PathBuf};

struct Guide {
    topic: String,
    title: String,
    body: String,
    path: PathBuf,
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let guide_dir = manifest_dir.join("docs/guides");
    println!("cargo:rerun-if-changed={}", guide_dir.display());

    let mut guides = discover_guides(&guide_dir);
    guides.sort_by(|left, right| left.topic.cmp(&right.topic));
    validate_guides(&guides);

    let generated = render_topics(&guides);
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out_dir.join("guide_topics.rs"), generated)
        .expect("failed to write generated guide topic table");

    embed_profiles(
        &manifest_dir.join("guard-profiles"),
        &out_dir.join("guard_profiles.rs"),
        "PACKAGED_GUARD_PROFILES",
    );
    embed_profiles(
        &manifest_dir.join("ignore-profiles"),
        &out_dir.join("ignore_profiles.rs"),
        "PACKAGED_IGNORE_PROFILES",
    );
}

/// Render one directory of TOML profiles as a `&[(filename, body)]` table the
/// crate `include!`s. Both packaged profile families — guard command policy and
/// tool-ignore targets — ship this way, so a new profile is a new file and
/// nothing else.
fn embed_profiles(profile_dir: &Path, out_file: &Path, table_name: &str) {
    println!("cargo:rerun-if-changed={}", profile_dir.display());
    let mut profiles: Vec<PathBuf> = fs::read_dir(profile_dir)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", profile_dir.display()))
        .map(|entry| entry.expect("failed to read profile entry").path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("toml"))
        .collect();
    profiles.sort();
    assert!(
        !profiles.is_empty(),
        "{} must contain a TOML profile",
        profile_dir.display()
    );

    let mut generated = format!("const {table_name}: &[(&str, &str)] = &[\n");
    for path in profiles {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_else(|| panic!("profile path is not UTF-8: {}", path.display()));
        let body = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        generated.push_str(&format!("    ({name:?}, {body:?}),\n"));
    }
    generated.push_str("];\n");
    fs::write(out_file, generated)
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", out_file.display()));
}

fn discover_guides(guide_dir: &Path) -> Vec<Guide> {
    fs::read_dir(guide_dir)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", guide_dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("failed to read guide directory entry").path();
            (path.extension().and_then(|value| value.to_str()) == Some("md")).then_some(path)
        })
        .map(|path| {
            let topic = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_else(|| panic!("guide path is not UTF-8: {}", path.display()))
                .to_string();
            let body = fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
            let title = body
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("# "))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| panic!("{} must start with a non-empty H1", path.display()))
                .to_string();
            Guide {
                topic,
                title,
                body,
                path,
            }
        })
        .collect()
}

fn validate_guides(guides: &[Guide]) {
    assert!(
        !guides.is_empty(),
        "docs/guides must contain a Markdown guide"
    );
    for guide in guides {
        assert!(
            is_kebab_case(&guide.topic),
            "guide filename stem must be ASCII kebab-case: {}",
            guide.path.display()
        );
    }
    for pair in guides.windows(2) {
        assert_ne!(
            pair[0].topic, pair[1].topic,
            "duplicate guide topic {}",
            pair[0].topic
        );
    }

    let width = guides.iter().map(|guide| guide.topic.len()).max().unwrap();
    for guide in guides {
        let row_len = 2 + width + 1 + guide.title.chars().count();
        assert!(
            row_len <= 80,
            "guide listing row exceeds 80 columns for topic {}",
            guide.topic
        );
    }
}

fn is_kebab_case(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn render_topics(guides: &[Guide]) -> String {
    let mut generated = String::from("const TOPICS: &[Topic] = &[\n");
    for guide in guides {
        generated.push_str("    Topic {\n");
        generated.push_str(&format!("        name: {:?},\n", guide.topic));
        generated.push_str(&format!("        summary: {:?},\n", guide.title));
        generated.push_str(&format!("        body: {:?},\n", guide.body));
        generated.push_str("    },\n");
    }
    generated.push_str("];\n");
    generated
}
