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
        format!("{}...", &text[..text.char_indices().take_while(|(i, _)| *i < 250).last().map(|(i, c)| i + c.len_utf8()).unwrap_or(250)])
    } else {
        text
    })
}
