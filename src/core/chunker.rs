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
