use anyhow::Result;
use serde::Serialize;

use crate::config::Config;
use crate::core::embedder::get_query_embedding;
use crate::core::fts::FtsEngine;
use crate::core::vector_store::VectorIndex;

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub path: String,
    pub title: String,
    pub chunk: String,
    pub score: f32,
    pub start_line: u32,
    pub end_line: u32,
    pub tags: Vec<String>,
    pub heading: String,
    pub match_type: String,
}

/// Execute hybrid search combining vector similarity + FTS.
pub async fn hybrid_search(
    vault_path: &str,
    query: &str,
    limit: usize,
    folders: Option<Vec<String>>,
    tags: Option<Vec<String>>,
) -> Result<Vec<SearchResult>> {
    let config = Config::new(vault_path);

    // Load vector index
    let vector_index = VectorIndex::load(&config.vectors_path())?;

    // Run vector search
    let vector_results = vector_search(&config, &vector_index, query, limit * 2, &folders, &tags)?;

    // Run FTS search
    let fts_results = fts_search(&config, query, limit * 2)?;

    // Merge results
    let merged = merge_results(vector_results, fts_results, &config, limit);

    Ok(merged)
}

fn vector_search(
    config: &Config,
    index: &VectorIndex,
    query: &str,
    limit: usize,
    folders: &Option<Vec<String>>,
    tags: &Option<Vec<String>>,
) -> Result<Vec<SearchResult>> {
    // Get query embedding
    let query_vec = match get_query_embedding(&config.embedding, query) {
        Ok(v) => v,
        Err(_) => return Ok(vec![]), // Graceful degradation if embedding unavailable
    };

    let hits = index.query(&query_vec, limit);

    let results: Vec<SearchResult> = hits
        .into_iter()
        .filter(|hit| {
            // Folder filter
            if let Some(folders) = folders {
                if !folders.iter().any(|f| hit.metadata.path.starts_with(f)) {
                    return false;
                }
            }
            // Tag filter
            if let Some(tags) = tags {
                let item_tags: Vec<&str> = hit.metadata.tags.split(',').filter(|s| !s.is_empty()).collect();
                if !tags.iter().all(|t| item_tags.contains(&t.as_str())) {
                    return false;
                }
            }
            true
        })
        .map(|hit| SearchResult {
            path: hit.metadata.path,
            title: hit.metadata.title,
            chunk: hit.metadata.text,
            score: hit.score,
            start_line: hit.metadata.start_line,
            end_line: hit.metadata.end_line,
            tags: hit.metadata.tags.split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect(),
            heading: hit.metadata.heading,
            match_type: "semantic".to_string(),
        })
        .collect();

    Ok(results)
}

fn fts_search(config: &Config, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let fts = match FtsEngine::open(&config.tantivy_path()) {
        Ok(f) => f,
        Err(_) => return Ok(vec![]),
    };

    let hits = fts.search(query, limit)?;

    Ok(hits
        .into_iter()
        .map(|hit| SearchResult {
            path: hit.path,
            title: hit.title,
            chunk: String::new(),
            score: hit.score,
            start_line: 0,
            end_line: 0,
            tags: vec![],
            heading: String::new(),
            match_type: "fts".to_string(),
        })
        .collect())
}

fn merge_results(
    vector_results: Vec<SearchResult>,
    fts_results: Vec<SearchResult>,
    config: &Config,
    limit: usize,
) -> Vec<SearchResult> {
    let mut merged: std::collections::HashMap<String, SearchResult> = std::collections::HashMap::new();

    // Normalize FTS scores
    let max_fts_score = fts_results.iter().map(|r| r.score).fold(0.0f32, f32::max).max(1.0);

    // Add vector results
    for r in vector_results {
        let key = format!("{}:{}", r.path, r.start_line);
        merged.insert(key, r);
    }

    // Merge FTS results
    for fts in &fts_results {
        let normalized_fts = fts.score / max_fts_score;

        // Boost existing vector results with FTS signal
        let mut found_match = false;
        for (_, existing) in merged.iter_mut() {
            if existing.path == fts.path {
                existing.score =
                    config.search.vector_weight * existing.score + config.search.fts_weight * normalized_fts;
                existing.match_type = "hybrid".to_string();
                found_match = true;
            }
        }

        // Add FTS-only results
        if !found_match {
            let key = format!("{}:fts", fts.path);
            if !merged.contains_key(&key) {
                merged.insert(
                    key,
                    SearchResult {
                        path: fts.path.clone(),
                        title: fts.title.clone(),
                        chunk: "[FTS match]".to_string(),
                        score: config.search.fts_weight * normalized_fts,
                        start_line: 0,
                        end_line: 0,
                        tags: vec![],
                        heading: String::new(),
                        match_type: "fts".to_string(),
                    },
                );
            }
        }
    }

    // Sort and take top N
    let mut results: Vec<SearchResult> = merged.into_values().collect();
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
    results
}
