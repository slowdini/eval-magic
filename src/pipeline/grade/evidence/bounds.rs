//! UTF-8-safe, explicitly marked excerpts for untrusted Markdown evidence.

pub(super) struct Rendered {
    pub(super) content: String,
    pub(super) truncated: bool,
}

pub(super) fn bounded_fenced(
    language: &str,
    content: &str,
    limit: usize,
    source: &str,
) -> Rendered {
    let Some(fence) = safe_fence(content, limit.saturating_sub(language.len() + 3)) else {
        let marker = format!(
            "[eval-magic] content omitted because no collision-safe Markdown fence fits; full source: {source}"
        );
        return Rendered {
            content: clipped_prefix(&marker, limit),
            truncated: true,
        };
    };
    let overhead = fence.len() * 2 + language.len() + 3;
    let excerpt = bounded_excerpt(content, limit.saturating_sub(overhead), source);
    Rendered {
        content: format!("{fence}{language}\n{}\n{fence}", excerpt.content),
        truncated: excerpt.truncated,
    }
}

fn safe_fence(content: &str, fence_budget: usize) -> Option<String> {
    let backticks = longest_run(content, '`').saturating_add(1).max(3);
    if backticks.saturating_mul(2) <= fence_budget {
        return Some("`".repeat(backticks));
    }
    let tildes = longest_run(content, '~').saturating_add(1).max(3);
    (tildes.saturating_mul(2) <= fence_budget).then(|| "~".repeat(tildes))
}

fn longest_run(content: &str, target: char) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for character in content.chars() {
        if character == target {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

pub(super) fn bounded_excerpt(content: &str, limit: usize, source: &str) -> Rendered {
    if content.len() <= limit {
        return Rendered {
            content: content.to_string(),
            truncated: false,
        };
    }
    let detailed_marker = format!(
        "\n[eval-magic] content truncated from {} bytes; middle omitted; full source: {source}\n",
        content.len()
    );
    let marker = if detailed_marker.len() <= limit {
        detailed_marker
    } else {
        clipped_prefix(
            "\n[eval-magic] content truncated; full source is listed in the artifact manifest\n",
            limit,
        )
    };
    let available = limit.saturating_sub(marker.len());
    let (head, tail) = middle_parts(content, available);
    Rendered {
        content: format!("{head}{marker}{tail}"),
        truncated: true,
    }
}

fn clipped_prefix(content: &str, limit: usize) -> String {
    let mut end = limit.min(content.len());
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    content[..end].to_string()
}

fn middle_parts(content: &str, available: usize) -> (&str, &str) {
    let head_target = available / 2;
    let tail_target = available - head_target;
    let mut head_end = head_target.min(content.len());
    while head_end > 0 && !content.is_char_boundary(head_end) {
        head_end -= 1;
    }
    if let Some(newline) = content[..head_end].rfind('\n')
        && newline + 1 >= head_end / 2
    {
        head_end = newline + 1;
    }

    let mut tail_start = content.len().saturating_sub(tail_target);
    while tail_start < content.len() && !content.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    if let Some(newline) = content[tail_start..].find('\n') {
        let after = tail_start + newline + 1;
        if content.len().saturating_sub(after) >= tail_target / 2 {
            tail_start = after;
        }
    }
    if tail_start < head_end {
        tail_start = head_end;
    }
    (&content[..head_end], &content[tail_start..])
}
