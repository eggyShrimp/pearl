use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Clone, Default)]
pub struct Frontmatter {
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub summary: Option<String>,
    pub r#type: Option<String>,
}

/// Parse YAML frontmatter from markdown content.
/// Returns the parsed metadata and the byte offset where the body starts.
pub fn parse_frontmatter(content: &str) -> (Frontmatter, usize) {
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() || lines[0].trim() != "---" {
        return (Frontmatter::default(), 0);
    }

    // Find closing ---
    let close_idx = lines[1..]
        .iter()
        .position(|l| l.trim() == "---")
        .map(|i| i + 1);

    let Some(close_idx) = close_idx else {
        return (Frontmatter::default(), 0);
    };

    let yaml_str = lines[1..close_idx].join("\n");

    // Calculate byte offset for body start
    let body_start: usize = lines[..=close_idx]
        .iter()
        .map(|l| l.len() + 1) // +1 for newline
        .sum();

    // Parse with serde_yml
    let fm = parse_yaml(&yaml_str).unwrap_or_default();

    // If no summary in frontmatter, extract first paragraph
    let meta = if fm.summary.is_none() {
        let summary = extract_first_paragraph(&lines[close_idx + 1..]);
        Frontmatter { summary, ..fm }
    } else {
        fm
    };

    (meta, body_start)
}

#[derive(Deserialize, Default)]
struct RawFrontmatter {
    title: Option<String>,
    #[serde(default)]
    tags: TagsField,
    #[serde(default)]
    aliases: Vec<String>,
    summary: Option<String>,
    r#type: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum TagsField {
    #[default]
    Empty,
    List(Vec<String>),
    Single(String),
}

fn parse_yaml(yaml: &str) -> Result<Frontmatter> {
    let raw: RawFrontmatter = serde_yml::from_str(yaml)?;
    let tags = match raw.tags {
        TagsField::Empty => vec![],
        TagsField::List(v) => v.into_iter().map(|s| s.to_lowercase()).collect(),
        TagsField::Single(s) => vec![s.to_lowercase()],
    };

    Ok(Frontmatter {
        title: raw.title,
        tags,
        aliases: raw.aliases,
        summary: raw.summary,
        r#type: raw.r#type,
    })
}

fn extract_first_paragraph(lines: &[&str]) -> Option<String> {
    let mut paragraph = Vec::new();
    let mut started = false;

    for line in lines {
        let trimmed = line.trim();
        if !started {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            started = true;
        }
        if started && (trimmed.is_empty() || trimmed.starts_with('#')) {
            break;
        }
        paragraph.push(trimmed);
    }

    if paragraph.is_empty() {
        return None;
    }

    let text = paragraph.join(" ");
    // Truncate to ~250 chars
    Some(if text.len() > 250 {
        format!(
            "{}...",
            &text[..text
                .char_indices()
                .take_while(|(i, _)| *i < 250)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(250)]
        )
    } else {
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_frontmatter ────────────────────────────────────────────────────

    #[test]
    fn parse_frontmatter_with_tags_list() {
        let content = "---\ntitle: Test\ntags:\n  - ai\n  - rust\n---\nBody here.";
        let (meta, body_start) = parse_frontmatter(content);
        assert_eq!(meta.title.as_deref(), Some("Test"));
        assert_eq!(meta.tags, vec!["ai", "rust"]);
        assert!(body_start > 0);
        assert!(content[body_start..].starts_with("Body here."));
    }

    #[test]
    fn parse_frontmatter_with_single_tag() {
        let content = "---\ntitle: Note\ntags: single\n---\nBody.";
        let (meta, _) = parse_frontmatter(content);
        // single "tags" field should be parsed as a single-element list
        assert_eq!(meta.tags, vec!["single"]);
    }

    #[test]
    fn parse_frontmatter_no_frontmatter() {
        let content = "Just plain markdown.\nNo frontmatter.";
        let (meta, body_start) = parse_frontmatter(content);
        assert_eq!(body_start, 0);
        assert!(meta.title.is_none());
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn parse_frontmatter_empty() {
        let content = "";
        let (meta, body_start) = parse_frontmatter(content);
        assert_eq!(body_start, 0);
        assert!(meta.title.is_none());
    }

    #[test]
    fn parse_frontmatter_with_summary() {
        let content = "---\ntitle: T\nsummary: Custom summary\n---\nBody.";
        let (meta, _) = parse_frontmatter(content);
        assert_eq!(meta.summary.as_deref(), Some("Custom summary"));
    }

    #[test]
    fn parse_frontmatter_auto_summary() {
        let content = "---\ntitle: T\n---\n\nFirst paragraph content here.\n\nSecond paragraph.";
        let (meta, _) = parse_frontmatter(content);
        assert!(meta.summary.is_some());
        assert!(meta.summary.unwrap().contains("First paragraph"));
    }

    #[test]
    fn parse_frontmatter_tags_lowercased() {
        let content = "---\ntags:\n  - AI\n  - Rust\n---\nBody.";
        let (meta, _) = parse_frontmatter(content);
        assert_eq!(meta.tags, vec!["ai", "rust"]);
    }

    #[test]
    fn parse_frontmatter_unclosed() {
        let content = "---\ntitle: T\nNo closing delimiter";
        let (_meta, body_start) = parse_frontmatter(content);
        // Unclosed frontmatter → treated as no frontmatter
        assert_eq!(body_start, 0);
    }

    // ── extract_first_paragraph ──────────────────────────────────────────────

    #[test]
    fn extract_first_paragraph_basic() {
        let lines = vec!["", "First paragraph.", "", "Second."];
        let result = extract_first_paragraph(&lines);
        assert_eq!(result.as_deref(), Some("First paragraph."));
    }

    #[test]
    fn extract_first_paragraph_skips_headings() {
        let lines = vec!["# Title", "", "Actual content."];
        let result = extract_first_paragraph(&lines);
        assert_eq!(result.as_deref(), Some("Actual content."));
    }

    #[test]
    fn extract_first_paragraph_empty() {
        let lines: Vec<&str> = vec![];
        assert!(extract_first_paragraph(&lines).is_none());
    }

    #[test]
    fn extract_first_paragraph_only_headings() {
        let lines = vec!["# H1", "## H2", "### H3"];
        assert!(extract_first_paragraph(&lines).is_none());
    }

    #[test]
    fn extract_first_paragraph_truncates_long() {
        let long_text = "a".repeat(300);
        let lines = vec![&long_text[..], "next paragraph"];
        let result = extract_first_paragraph(&lines);
        let text = result.unwrap();
        assert!(text.len() <= 253); // 250 + "..."
        assert!(text.ends_with("..."));
    }

    #[test]
    fn extract_first_paragraph_multiline() {
        let lines = vec!["Line one.", "Line two.", "", "Other."];
        let result = extract_first_paragraph(&lines);
        assert_eq!(result.as_deref(), Some("Line one. Line two."));
    }
}
