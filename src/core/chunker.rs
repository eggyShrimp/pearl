use pulldown_cmark::HeadingLevel;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub start_line: u32,
    pub end_line: u32,
    pub heading: Option<String>,
}

/// Estimate token count for mixed CJK/ASCII text.
/// CJK ≈ 1.5 chars/token, ASCII ≈ 4 chars/token.
fn estimate_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut ascii = 0usize;
    for ch in text.chars() {
        if ch as u32 > 0x2fff {
            cjk += 1;
        } else {
            ascii += 1;
        }
    }
    (cjk * 10 / 15) + (ascii / 4) + 1 // ceiling
}

/// Split markdown body into chunks using heading boundaries and token budget.
pub fn chunk_markdown(body: &str, title: &str, max_tokens: usize) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut buffer = String::new();
    let mut buffer_tokens = 0usize;
    let mut buffer_start_line = 1u32;
    let mut heading_stack: Vec<(HeadingLevel, String)> = Vec::new();
    let mut current_heading: Option<String> = None;

    // Process line by line for accurate line counting
    let lines: Vec<&str> = body.lines().collect();

    for (line_idx, line) in lines.iter().enumerate() {
        let line_num = (line_idx as u32) + 1;
        let line_with_newline = format!("{}\n", line);
        let line_tokens = estimate_tokens(line);

        // Detect ATX headings
        let heading_info = detect_heading(line);

        if let Some((level, text)) = heading_info {
            // Flush current buffer
            if !buffer.trim().is_empty() {
                chunks.push(Chunk {
                    text: buffer.trim().to_string(),
                    start_line: buffer_start_line,
                    end_line: line_num.saturating_sub(1),
                    heading: current_heading.clone(),
                });
            }
            buffer.clear();
            buffer_tokens = 0;
            buffer_start_line = line_num;

            // Update heading stack
            let level_num = heading_level_to_num(level);
            while heading_stack
                .last()
                .is_some_and(|(l, _)| heading_level_to_num(*l) >= level_num)
            {
                heading_stack.pop();
            }
            heading_stack.push((level, text.clone()));

            // Build breadcrumb
            let mut parts = vec![title.to_string()];
            parts.extend(heading_stack.iter().map(|(_, t)| t.clone()));
            current_heading = Some(parts.join(" > "));
        }

        // Token budget overflow
        if buffer_tokens + line_tokens > max_tokens && !buffer.trim().is_empty() {
            chunks.push(Chunk {
                text: buffer.trim().to_string(),
                start_line: buffer_start_line,
                end_line: line_num.saturating_sub(1),
                heading: current_heading.clone(),
            });
            buffer.clear();
            buffer_tokens = 0;
            buffer_start_line = line_num;
        }

        buffer.push_str(&line_with_newline);
        buffer_tokens += line_tokens;
    }

    // Flush remaining
    if !buffer.trim().is_empty() {
        chunks.push(Chunk {
            text: buffer.trim().to_string(),
            start_line: buffer_start_line,
            end_line: lines.len() as u32,
            heading: current_heading,
        });
    }

    chunks
}

fn detect_heading(line: &str) -> Option<(HeadingLevel, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }

    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }

    // Must have a space after hashes
    let rest = &trimmed[hashes..];
    if !rest.starts_with(' ') && !rest.is_empty() {
        return None;
    }

    let text = rest.trim().to_string();
    let level = match hashes {
        1 => HeadingLevel::H1,
        2 => HeadingLevel::H2,
        3 => HeadingLevel::H3,
        4 => HeadingLevel::H4,
        5 => HeadingLevel::H5,
        6 => HeadingLevel::H6,
        _ => return None,
    };

    Some((level, text))
}

fn heading_level_to_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Build embedding input: "{breadcrumb}\n\n{chunk text}"
pub fn build_embedding_input(chunk: &Chunk) -> String {
    match &chunk.heading {
        Some(h) => format!("{}\n\n{}", h, chunk.text),
        None => chunk.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── estimate_tokens ──────────────────────────────────────────────────────

    #[test]
    fn estimate_tokens_ascii() {
        // 4 ASCII chars ≈ 1 token, +1 ceiling
        assert_eq!(estimate_tokens("abcd"), 2); // 4/4 + 1 = 2
        assert_eq!(estimate_tokens("abcdefgh"), 3); // 8/4 + 1 = 3
    }

    #[test]
    fn estimate_tokens_cjk() {
        // CJK: 10/15 ≈ 0.67 tokens per char
        assert_eq!(estimate_tokens("你好"), 2); // 2*10/15 + 1 = 2
        assert_eq!(estimate_tokens("你好世界"), 3); // 4*10/15 + 1 = 3
    }

    #[test]
    fn estimate_tokens_mixed() {
        let tokens = estimate_tokens("hello 你好 world");
        assert!(tokens > 0);
        // "hello " = 6 ASCII, "你好" = 2 CJK, " world" = 6 ASCII
        // ASCII: 12/4 = 3, CJK: 2*10/15 = 1, +1 = 5
        assert_eq!(tokens, 5);
    }

    #[test]
    fn estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 1); // ceiling
    }

    // ── detect_heading ───────────────────────────────────────────────────────

    #[test]
    fn detect_heading_h1() {
        let result = detect_heading("# Title");
        assert!(result.is_some());
        let (level, text) = result.unwrap();
        assert_eq!(level, HeadingLevel::H1);
        assert_eq!(text, "Title");
    }

    #[test]
    fn detect_heading_h3() {
        let result = detect_heading("### Sub heading");
        assert!(result.is_some());
        let (level, text) = result.unwrap();
        assert_eq!(level, HeadingLevel::H3);
        assert_eq!(text, "Sub heading");
    }

    #[test]
    fn detect_heading_no_space() {
        // "#NoSpace" should NOT be a heading
        assert!(detect_heading("#NoSpace").is_none());
    }

    #[test]
    fn detect_heading_not_heading() {
        assert!(detect_heading("Just text").is_none());
        assert!(detect_heading("").is_none());
    }

    #[test]
    fn detect_heading_with_leading_whitespace() {
        let result = detect_heading("  ## Indented");
        assert!(result.is_some());
        let (level, text) = result.unwrap();
        assert_eq!(level, HeadingLevel::H2);
        assert_eq!(text, "Indented");
    }

    #[test]
    fn detect_heading_empty_text() {
        // "# " (hash + space, no text) is a valid heading with empty text
        let result = detect_heading("# ");
        assert!(result.is_some());
        let (_, text) = result.unwrap();
        assert_eq!(text, "");
    }

    // ── chunk_markdown ───────────────────────────────────────────────────────

    #[test]
    fn chunk_simple_text() {
        let body = "Hello world.\nThis is a test.";
        let chunks = chunk_markdown(body, "Test", 400);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert_eq!(chunks[0].end_line, 2);
        assert!(chunks[0].heading.is_none());
    }

    #[test]
    fn chunk_splits_on_heading() {
        let body = "Intro text\n## Section A\nContent A\n## Section B\nContent B";
        let chunks = chunk_markdown(body, "Doc", 400);
        assert!(chunks.len() >= 2);
        // First chunk should have no heading (intro)
        // Second chunk should have heading "Doc > Section A"
        let section_chunks: Vec<_> = chunks.iter().filter(|c| c.heading.is_some()).collect();
        assert!(section_chunks.len() >= 2);
    }

    #[test]
    fn chunk_heading_breadcrumb() {
        let body = "## Topic\nSome content";
        let chunks = chunk_markdown(body, "MyDoc", 400);
        let headed: Vec<_> = chunks.iter().filter(|c| c.heading.is_some()).collect();
        assert_eq!(headed.len(), 1);
        assert_eq!(headed[0].heading.as_ref().unwrap(), "MyDoc > Topic");
    }

    #[test]
    fn chunk_splits_on_token_budget() {
        // Create a long text that exceeds max_tokens
        let long_line = "word ".repeat(200); // ~1000 chars, ~200 tokens
        let body = format!("{}\n\n{}", long_line, long_line);
        let chunks = chunk_markdown(&body, "Doc", 100);
        assert!(chunks.len() >= 2, "Expected split into multiple chunks");
    }

    #[test]
    fn chunk_empty_body() {
        let chunks = chunk_markdown("", "Doc", 400);
        assert_eq!(chunks.len(), 0);
    }

    #[test]
    fn chunk_line_numbers_accurate() {
        let body = "Line 1\nLine 2\nLine 3\n## Heading\nLine 5\nLine 6";
        let chunks = chunk_markdown(body, "Doc", 400);
        // Should have at least 2 chunks: before heading and after
        assert!(chunks.len() >= 2);
        // First chunk starts at line 1
        assert_eq!(chunks[0].start_line, 1);
    }

    // ── build_embedding_input ────────────────────────────────────────────────

    #[test]
    fn build_embedding_input_with_heading() {
        let chunk = Chunk {
            text: "content".to_string(),
            start_line: 1,
            end_line: 1,
            heading: Some("Doc > Section".to_string()),
        };
        let input = build_embedding_input(&chunk);
        assert_eq!(input, "Doc > Section\n\ncontent");
    }

    #[test]
    fn build_embedding_input_no_heading() {
        let chunk = Chunk {
            text: "content".to_string(),
            start_line: 1,
            end_line: 1,
            heading: None,
        };
        let input = build_embedding_input(&chunk);
        assert_eq!(input, "content");
    }
}
