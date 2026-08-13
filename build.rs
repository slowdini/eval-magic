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
