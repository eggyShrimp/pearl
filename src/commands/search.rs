use anyhow::Result;
use clap::ValueEnum;

use crate::vault::resolve_vault;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SearchMode {
    Hybrid,
    Semantic,
    Fts,
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    vault: Option<String>,
    query: String,
    limit: usize,
    folder: Vec<String>,
    exclude: Vec<String>,
    tag: Vec<String>,
    mode: SearchMode,
    threshold: Option<f32>,
    context: usize,
    since: Option<String>,
    json: bool,
) -> Result<()> {
    let vault = resolve_vault(vault)?;
    tracing_subscriber::fmt()
        .with_env_filter("pearl=info")
        .with_writer(std::io::stderr)
        .init();
    let folders = if folder.is_empty() {
        None
    } else {
        Some(folder)
    };
    let tags = if tag.is_empty() { None } else { Some(tag) };
    let excludes = if exclude.is_empty() {
        None
    } else {
        Some(exclude)
    };

    let since_ts = match &since {
        Some(date_str) => {
            let naive =
                chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").map_err(|e| {
                    anyhow::anyhow!(
                        "Invalid --since date '{}': {} (expected YYYY-MM-DD)",
                        date_str,
                        e
                    )
                })?;
            Some(naive.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
        }
        None => None,
    };

    let (mut results, linked_notes) = match mode {
        SearchMode::Hybrid => {
            let response =
                crate::search::hybrid_search(&vault, &query, limit, folders, tags).await?;
            (response.results, response.linked_notes)
        }
        SearchMode::Semantic => {
            let r =
                crate::search::vector_search_only(&vault, &query, limit, folders, tags).await?;
            (r, vec![])
        }
        SearchMode::Fts => {
            let r = crate::search::fts_search_only(&vault, &query, limit).await?;
            (r, vec![])
        }
    };

    if let Some(ref excludes) = excludes {
        results.retain(|r| !excludes.iter().any(|ex| r.path.starts_with(ex.as_str())));
    }

    if let Some(min_score) = threshold {
        results.retain(|r| r.score >= min_score);
    }

    if let Some(ts) = since_ts {
        results.retain(|r| {
            let full_path = std::path::Path::new(&vault).join(&r.path);
            match std::fs::metadata(&full_path) {
                Ok(meta) => match meta.modified() {
                    Ok(mtime) => {
                        let file_ts = mtime
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        file_ts >= ts
                    }
                    Err(_) => true,
                },
                Err(_) => false,
            }
        });
    }

    if json {
        let output = serde_json::json!({
            "results": results,
            "linked_notes": linked_notes,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output)
                .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
        );
    } else {
        use console::style;
        for (i, r) in results.iter().enumerate() {
            println!(
                "{}. [{}] {} (score: {:.3})",
                i + 1,
                r.match_type,
                style(&r.path).cyan(),
                r.score
            );
            if context > 0 && r.start_line > 0 {
                let full_path = std::path::Path::new(&vault).join(&r.path);
                if let Ok(content) = std::fs::read_to_string(&full_path) {
                    let lines: Vec<&str> = content.lines().collect();
                    let start = (r.start_line as usize)
                        .saturating_sub(1)
                        .saturating_sub(context);
                    let end = (r.end_line as usize)
                        .saturating_add(context)
                        .min(lines.len());
                    for (li, line) in lines[start..end].iter().enumerate() {
                        let line_num = start + li + 1;
                        let is_match_line = line_num >= r.start_line as usize
                            && line_num <= r.end_line as usize;
                        if is_match_line {
                            println!(
                                "   {} {}",
                                style(format!("{:>4}", line_num)).dim(),
                                line
                            );
                        } else {
                            println!(
                                "   {} {}",
                                style(format!("{:>4}", line_num)).dim(),
                                style(line).dim()
                            );
                        }
                    }
                } else {
                    println!("   {}", r.chunk.chars().take(120).collect::<String>());
                }
            } else {
                println!("   {}", r.chunk.chars().take(120).collect::<String>());
            }
            println!();
        }
        if results.is_empty() {
            println!("  No results found.");
        }

        if !linked_notes.is_empty() {
            use console::style;
            println!("  {} Linked notes:", style("⟡").dim());
            for note in &linked_notes {
                println!(
                    "    {} {} (via {})",
                    style("→").dim(),
                    style(&note.path).blue(),
                    style(&note.related_to).dim()
                );
            }
            println!();
        }
    }
    Ok(())
}
