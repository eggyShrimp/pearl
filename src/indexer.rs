use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::info;

use crate::config::Config;
use crate::core::chunker::{Chunk, build_embedding_input, chunk_markdown};
use crate::core::embedder::{batch_size_for_provider, get_embeddings, get_embeddings_async};
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

// ─── Internal types ──────────────────────────────────────────────────────────

/// A chunk waiting in the batch queue, carrying enough context to build a VectorEntry after embedding.
struct PendingChunk {
    file_path: String,
    title: String,
    tags: String,
    chunk: Chunk,
    embedding_input: String,
}

/// A batch of chunks ready to be sent to the embedding API.
struct EmbeddingBatch {
    chunks: Vec<PendingChunk>,
}

/// Run incremental indexing with a progress callback.
///
/// Pipeline architecture:
///   1. Main thread: read files, compute hashes, chunk changed files, push batches to channel
///   2. Background tasks: consume batches from channel, call embedding API concurrently
///   3. After all embedding completes: persist vectors, update FTS
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
    let vector_index = Arc::new(Mutex::new(VectorIndex::load(&config.vectors_path())?));

    // ── Phase 1: Read all files, compute hashes, partition into changed/unchanged ──
    struct FileData {
        path: String,
        content: String,
    }

    let mut changed_files: Vec<FileData> = Vec::new();
    let mut unchanged_files: Vec<FileData> = Vec::new();

    for file in &files {
        let content = fs::read_to_string(&file.abs_path).unwrap_or_default();
        let hash = hash_content(&content);
        new_hashes.insert(file.path.clone(), hash.clone());

        if !force && old_hashes.get(&file.path) == Some(&hash) {
            stats.skipped += 1;
            unchanged_files.push(FileData {
                path: file.path.clone(),
                content,
            });
        } else {
            changed_files.push(FileData {
                path: file.path.clone(),
                content,
            });
        }
    }

    // Remove vectors for deleted files
    let current_paths: HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    {
        let mut idx = vector_index.lock().await;
        for old_path in old_hashes.keys() {
            if !current_paths.contains(old_path.as_str()) {
                idx.remove_file(old_path);
                stats.deleted += 1;
            }
        }
    }

    // ── Phase 2: Chunk changed files → produce embedding batches → consume concurrently ──
    let batch_size = batch_size_for_provider(&config.embedding.provider);
    let mut fts_docs: Vec<FtsDocument> = Vec::new();
    let total_to_index = changed_files.len();

    // Channel: producer (chunking) → consumer (embedding)
    // Buffer up to 8 batches ahead to keep the pipeline full
    let (tx, rx) = tokio::sync::mpsc::channel::<EmbeddingBatch>(8);
    let rx = Arc::new(Mutex::new(rx));

    // Spawn embedding consumer tasks (up to 4 concurrent workers)
    let num_workers = 4usize;
    let embedding_config = config.embedding.clone();
    let vector_index_clone = vector_index.clone();
    let chunk_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let chunk_counter_clone = chunk_counter.clone();

    let consumer_handle = {
        let rx = rx.clone();
        tokio::spawn(async move {
            let mut handles = Vec::new();

            // Spawn worker tasks
            for _ in 0..num_workers {
                let rx = rx.clone();
                let emb_config = embedding_config.clone();
                let vi = vector_index_clone.clone();
                let counter = chunk_counter_clone.clone();

                let handle = tokio::spawn(async move {
                    loop {
                        // Take the next batch from the channel
                        let batch = {
                            let mut rx_guard = rx.lock().await;
                            rx_guard.recv().await
                        };

                        let batch = match batch {
                            Some(b) => b,
                            None => break, // Channel closed, no more work
                        };

                        let inputs: Vec<String> = batch
                            .chunks
                            .iter()
                            .map(|p| p.embedding_input.clone())
                            .collect();

                        let vectors = match get_embeddings_async(&emb_config, &inputs).await {
                            Ok(v) => v,
                            Err(e) => {
                                let paths: HashSet<&str> =
                                    batch.chunks.iter().map(|p| p.file_path.as_str()).collect();
                                tracing::warn!(
                                    "Failed to embed batch of {} chunks (files: {:?}): {}",
                                    batch.chunks.len(),
                                    paths,
                                    e
                                );
                                continue;
                            }
                        };

                        // Store vectors
                        let entries: Vec<VectorEntry> = batch
                            .chunks
                            .into_iter()
                            .zip(vectors.into_iter())
                            .map(|(pending, vector)| VectorEntry {
                                vector,
                                metadata: ChunkMeta {
                                    path: pending.file_path,
                                    title: pending.title,
                                    tags: pending.tags,
                                    heading: pending.chunk.heading.unwrap_or_default(),
                                    start_line: pending.chunk.start_line,
                                    end_line: pending.chunk.end_line,
                                    text: pending.chunk.text,
                                },
                            })
                            .collect();

                        let count = entries.len();
                        counter.fetch_add(count, std::sync::atomic::Ordering::Relaxed);

                        let mut idx = vi.lock().await;
                        idx.add_entries(entries);
                    }
                });

                handles.push(handle);
            }

            // Wait for all workers to finish
            for handle in handles {
                let _ = handle.await;
            }
        })
    };

    // ── Producer: chunk files and send batches ──────────────────────────────────
    let mut pending_batch: Vec<PendingChunk> = Vec::with_capacity(batch_size);

    for (idx, file_data) in changed_files.iter().enumerate() {
        // Remove old vectors for this file
        {
            let mut vi = vector_index.lock().await;
            vi.remove_file(&file_data.path);
        }

        // Parse frontmatter
        let (meta, body_start) = parse_frontmatter(&file_data.content);
        let body = if body_start < file_data.content.len() {
            &file_data.content[body_start..]
        } else {
            ""
        };
        let title = meta.title.unwrap_or_else(|| {
            file_data
                .path
                .trim_end_matches(".md")
                .split('/')
                .last()
                .unwrap_or("")
                .to_string()
        });

        // Chunk
        let chunks = chunk_markdown(body, &title, config.index.max_chunk_tokens);

        // Add chunks to the batch queue
        let tags_str = meta.tags.join(",");
        for chunk in &chunks {
            let embedding_input = build_embedding_input(chunk);
            pending_batch.push(PendingChunk {
                file_path: file_data.path.clone(),
                title: title.clone(),
                tags: tags_str.clone(),
                chunk: chunk.clone(),
                embedding_input,
            });

            // Send batch when full
            if pending_batch.len() >= batch_size {
                let batch = EmbeddingBatch {
                    chunks: std::mem::replace(&mut pending_batch, Vec::with_capacity(batch_size)),
                };
                tx.send(batch).await.ok();
            }
        }

        // Collect FTS document for this changed file
        fts_docs.push(FtsDocument {
            path: file_data.path.clone(),
            title: title.clone(),
            body: body.to_string(),
            tags: meta.tags.join(" "),
        });

        stats.indexed += 1;

        on_progress(&IndexProgress {
            current: idx + 1,
            total: total_to_index,
            path: file_data.path.clone(),
            chunks: chunks.len(),
        });

        info!("Indexed: {} ({} chunks)", file_data.path, chunks.len());
    }

    // Send remaining chunks
    if !pending_batch.is_empty() {
        let batch = EmbeddingBatch {
            chunks: pending_batch,
        };
        tx.send(batch).await.ok();
    }

    // Close channel to signal consumers to finish
    drop(tx);

    // Wait for all embedding work to complete
    consumer_handle.await?;

    stats.total_chunks = chunk_counter.load(std::sync::atomic::Ordering::Relaxed);

    // ── Phase 3: Incremental FTS update ────────────────────────────────────────
    let fts = FtsEngine::open(&config.tantivy_path())?;

    if force || !fts.has_documents()? {
        // Full rebuild on force or empty index: include unchanged files too
        for file_data in &unchanged_files {
            let (meta, body_start) = parse_frontmatter(&file_data.content);
            let body = if body_start < file_data.content.len() {
                &file_data.content[body_start..]
            } else {
                ""
            };
            let title = meta.title.unwrap_or_else(|| {
                file_data
                    .path
                    .trim_end_matches(".md")
                    .split('/')
                    .last()
                    .unwrap_or("")
                    .to_string()
            });
            fts_docs.push(FtsDocument {
                path: file_data.path.clone(),
                title,
                body: body.to_string(),
                tags: meta.tags.join(" "),
            });
        }
        fts.rebuild(fts_docs)?;
    } else {
        // Incremental: only update changed + deleted files
        let deleted_paths: Vec<&str> = old_hashes
            .keys()
            .filter(|p| !current_paths.contains(p.as_str()))
            .map(|p| p.as_str())
            .collect();
        fts.update(fts_docs, &deleted_paths)?;
    }

    // ── Phase 4: Persist ───────────────────────────────────────────────────────
    let vi = vector_index.lock().await;
    vi.save(&config.vectors_path())?;
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
