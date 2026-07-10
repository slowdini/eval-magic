//! OpenCode harness support.
//!
//! The declarative half of this harness lives in `harnesses/opencode.toml`
//! (including the stage-name rules, expressed as a regex + length cap); this
//! file keeps only the code-backed `opencode` slug capability — sanitization
//! and truncation the descriptor's format-string template cannot express.

/// True when `name` satisfies OpenCode's skill-name rules:
/// - 1–64 characters
/// - lowercase alphanumeric with single-hyphen separators
/// - no leading/trailing/consecutive hyphens
fn is_valid_opencode_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let mut prev = '-';
    for ch in name.chars() {
        if ch == '-' {
            if prev == '-' {
                return false;
            }
        } else if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() {
            return false;
        }
        prev = ch;
    }
    !name.starts_with('-') && !name.ends_with('-')
}

/// Sanitize an arbitrary identifier so it is a valid OpenCode skill name.
fn sanitize_opencode_name(name: &str) -> String {
    let mut out = String::new();
    let mut prev_hyphen = false;
    for ch in name.to_ascii_lowercase().chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            out.push(ch);
            prev_hyphen = false;
        } else if !prev_hyphen {
            out.push('-');
            prev_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    if out.is_empty() {
        out.push_str("skill");
    }
    if out.len() > 64 {
        out.truncate(64);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// Build a slug that is valid for OpenCode's skill directory + frontmatter name
/// constraints. `prefix` is the conspicuous staged-skill prefix, preserved so
/// cleanup prefix-scans still find it. `pub(crate)` because the `opencode`
/// named slug capability dispatches here.
pub(crate) fn opencode_slug(
    prefix: &str,
    iteration: u32,
    condition: &str,
    skill_name: &str,
) -> String {
    let condition = sanitize_opencode_name(condition);
    let skill = sanitize_opencode_name(skill_name);
    let base = format!("{prefix}{iteration}-{condition}-{skill}");
    if base.len() <= 64 && is_valid_opencode_name(&base) {
        return base;
    }
    // If the combined slug is too long, truncate the skill portion.
    let prefix = format!("{prefix}{iteration}-{condition}-");
    let budget = 64usize.saturating_sub(prefix.len());
    let mut truncated = skill.clone();
    truncated.truncate(budget);
    while truncated.ends_with('-') {
        truncated.pop();
    }
    if truncated.is_empty() {
        truncated.push_str("skill");
    }
    format!("{prefix}{truncated}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "slow-powers-eval-";

    #[test]
    fn opencode_slug_sanitizes_underscores_and_special_characters() {
        assert_eq!(
            opencode_slug(PREFIX, 1, "with_skill", "My_Skill!"),
            "slow-powers-eval-1-with-skill-my-skill"
        );
        assert_eq!(
            opencode_slug(PREFIX, 2, "without_skill", "snake_case"),
            "slow-powers-eval-2-without-skill-snake-case"
        );
    }

    #[test]
    fn opencode_slug_truncates_to_valid_max_length() {
        let very_long = "a".repeat(200);
        let slug = opencode_slug(PREFIX, 1, "with_skill", &very_long);
        assert!(slug.len() <= 64);
        assert!(is_valid_opencode_name(&slug));
        assert!(slug.starts_with("slow-powers-eval-1-with-skill-"));
    }
}
