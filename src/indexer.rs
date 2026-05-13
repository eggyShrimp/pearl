use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use tracing::info;

use crate::config::Config;
use crate::core::chunker::{build_embedding_input, chunk_markdown};
use crate::core::embedder::get_embeddings;
use crate::core::frontmatter::parse_frontmatter;
use crate::core::fts::{FtsDocument, FtsEngine};
use crate::core::vault::{hash_content, scan_vault};
use crate::core::vector_store::{ChunkMeta, VectorEntry, VectorIndex};

#[derive(Debug, Default)]
pub struct IndexStats {
    pub total_files: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub deleted: usize,
    pub total_chunks: usize,
}

/// Progress information emitted during indexing.
pub struct IndexProgress {
    pub current: usize,
    pub total: usize,
    pub path: String,
    pub chunks: usize,
}

/// Run incremental indexing of the vault.
pub async fn index_vault(vault_path: &str, force: bool) -> Result<IndexStats> {
    index_vault_with_progress(vault_path, force, |_| {}).await
}

/// Run incremental indexing with a progress callback.
pub async fn index_vault_with_progress(
    vault_path: &str,
    force: bool,
    on_progress: impl Fn(&IndexProgress),
) -> Result<IndexStats> {
    let config = Config::new(vault_path);
    fs::create_dir_all(&config.index.data_dir)?;

    let files = scan_vault(&config.vault_path);

    // Detect dimension mismatch: if existing vectors have a different dimension
    // than the configured model, force a full reindex.
    let force = force || detect_dimension_mismatch(&config);

    let old_hashes = if force {
        HashMap::new()
    } else {
        load_hashes(&config.hashes_path())
    };

    let mut new_hashes: HashMap<String, String> = HashMap::new();
    let mut stats = IndexStats {
        total_files: files.len(),
        ..Default::default()
    };

    // Load existing vector index
    let mut vector_index = VectorIndex::load(&config.vectors_path())?;

    // Determine which files need indexing
    let mut to_index = Vec::new();
    for file in &files {
        let content = fs::read_to_string(&file.abs_path).unwrap_or_default();
        let hash = hash_content(&content);
        new_hashes.insert(file.path.clone(), hash.clone());

        if !force && old_hashes.get(&file.path) == Some(&hash) {
            stats.skipped += 1;
        } else {
            to_index.push((file.path.clone(), content));
        }
    }

    // Remove vectors for deleted files
    let current_paths: std::collections::HashSet<&str> =
        files.iter().map(|f| f.path.as_str()).collect();
    for old_path in old_hashes.keys() {
        if !current_paths.contains(old_path.as_str()) {
            vector_index.remove_file(old_path);
            stats.deleted += 1;
        }
    }

    // Process files that need indexing
    let mut fts_docs = Vec::new();
    let total_to_index = to_index.len();

    for (idx, (file_path, content)) in to_index.iter().enumerate() {
        // Remove old vectors for this file
        vector_index.remove_file(file_path);

        // Parse frontmatter
        let (meta, body_start) = parse_frontmatter(content);
        let body = if body_start < content.len() {
            &content[body_start..]
        } else {
            ""
        };
        let title = meta
            .title
            .unwrap_or_else(|| file_path.trim_end_matches(".md").split('/').last().unwrap_or("").to_string());

        // Chunk
        let chunks = chunk_markdown(body, &title, config.index.max_chunk_tokens);
        if chunks.is_empty() {
            continue;
        }

        // Build embedding inputs
        let embedding_inputs: Vec<String> = chunks.iter().map(|c| build_embedding_input(c)).collect();

        // Get embeddings
        let vectors = match get_embeddings(&config.embedding, &embedding_inputs) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Failed to embed {}: {}", file_path, e);
                continue;
            }
        };

        // Store vectors
        let entries: Vec<VectorEntry> = chunks
            .iter()
            .zip(vectors.into_iter())
            .map(|(chunk, vector)| VectorEntry {
                vector,
                metadata: ChunkMeta {
                    path: file_path.clone(),
                    title: title.clone(),
                    tags: meta.tags.join(","),
                    heading: chunk.heading.clone().unwrap_or_default(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    text: chunk.text.chars().take(500).collect(),
                },
            })
            .collect();

        stats.total_chunks += entries.len();
        vector_index.add_entries(entries);

        // Collect for FTS
        fts_docs.push(FtsDocument {
            path: file_path.clone(),
            title: title.clone(),
            body: body.to_string(),
            tags: meta.tags.join(" "),
        });

        stats.indexed += 1;

        on_progress(&IndexProgress {
            current: idx + 1,
            total: total_to_index,
            path: file_path.clone(),
            chunks: chunks.len(),
        });

        info!("Indexed: {} ({} chunks)", file_path, chunks.len());
    }

    // Also add unchanged files to FTS rebuild
    for file in &files {
        if !to_index.iter().any(|(p, _)| p == &file.path) {
            let content = fs::read_to_string(&file.abs_path).unwrap_or_default();
            let (meta, body_start) = parse_frontmatter(&content);
            let body = if body_start < content.len() {
                &content[body_start..]
            } else {
                ""
            };
            let title = meta.title.unwrap_or_else(|| {
                file.path.trim_end_matches(".md").split('/').last().unwrap_or("").to_string()
            });
            fts_docs.push(FtsDocument {
                path: file.path.clone(),
                title,
                body: body.to_string(),
                tags: meta.tags.join(" "),
            });
        }
    }

    // Save vector index
    vector_index.save(&config.vectors_path())?;

    // Rebuild FTS index
    let fts = FtsEngine::open(&config.tantivy_path())?;
    fts.rebuild(fts_docs)?;

    // Save hashes
    save_hashes(&config.hashes_path(), &new_hashes)?;

    Ok(stats)
}

/// Detect if existing vector dimensions don't match what we'd get from the current model.
/// If `dimensions` is configured, compare against that. Otherwise, probe with a test embedding.
fn detect_dimension_mismatch(config: &Config) -> bool {
    let vector_index = match VectorIndex::load(&config.vectors_path()) {
        Ok(idx) => idx,
        Err(_) => return false, // No existing index, no mismatch
    };

    // Get dimension from existing vectors
    let existing_dim = match vector_index.entries.first() {
        Some(entry) => entry.vector.len(),
        None => return false, // Empty index, no mismatch
    };

    // If dimensions is explicitly configured, use that
    if let Some(configured_dim) = config.embedding.dimensions {
        if configured_dim != existing_dim {
            info!(
                "Dimension mismatch: existing index has {}, config specifies {}. Forcing full reindex.",
                existing_dim, configured_dim
            );
            return true;
        }
        return false;
    }

    // Otherwise, do a probe embedding to check actual dimensions
    match get_embeddings(&config.embedding, &["dimension probe".to_string()]) {
        Ok(vecs) if !vecs.is_empty() => {
            let actual_dim = vecs[0].len();
            if actual_dim != existing_dim {
                info!(
                    "Dimension mismatch: existing index has {}, current model produces {}. Forcing full reindex.",
                    existing_dim, actual_dim
                );
                true
            } else {
                false
            }
        }
        _ => false, // Can't probe, assume OK
    }
}

fn load_hashes(path: &Path) -> HashMap<String, String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_hashes(path: &Path, hashes: &HashMap<String, String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(hashes)?;
    fs::write(path, data)?;
    Ok(())
}
