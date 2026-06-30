use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::config::Config;
use crate::core::chunker::{Chunk, build_embedding_input, chunk_markdown};
use crate::core::embedder::{batch_size_for_provider, get_embeddings, get_embeddings_async};
use crate::core::frontmatter::parse_frontmatter;
use crate::core::fts::{FtsDocument, FtsEngine};
use crate::core::graph::{GraphStore, extract_links};
use crate::core::vault::{hash_content, scan_vault};
use crate::core::vector_store::{ChunkMeta, VectorEntry, VectorIndex};

#[derive(Debug, Default)]
pub struct IndexStats {
    pub total_files: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub deleted: usize,
    pub total_chunks: usize,
    pub graph_nodes: usize,
    pub graph_edges: usize,
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
    info!(total_files = files.len(), "index_vault started");

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

        // Collect FTS document for this changed file (before consuming chunks)
        fts_docs.push(FtsDocument {
            path: file_data.path.clone(),
            title: title.clone(),
            body: body.to_string(),
            tags: meta.tags.join(" "),
        });

        // Add chunks to the batch queue — consume chunks to avoid cloning text
        let tags_str = meta.tags.join(",");
        let chunk_count = chunks.len();
        for chunk in chunks {
            let embedding_input = build_embedding_input(&chunk);
            pending_batch.push(PendingChunk {
                file_path: file_data.path.clone(),
                title: title.clone(),
                tags: tags_str.clone(),
                chunk,
                embedding_input,
            });

            // Send batch when full
            if pending_batch.len() >= batch_size {
                let batch = EmbeddingBatch {
                    chunks: std::mem::replace(&mut pending_batch, Vec::with_capacity(batch_size)),
                };
                debug!(batch_size = batch.chunks.len(), "sending embedding batch");
                tx.send(batch).await.ok();
            }
        }

        stats.indexed += 1;

        on_progress(&IndexProgress {
            current: idx + 1,
            total: total_to_index,
            path: file_data.path.clone(),
            chunks: chunk_count,
        });

        info!(path = %file_data.path, chunks = chunk_count, "file indexed");
    }

    // Send remaining chunks
    if !pending_batch.is_empty() {
        let batch = EmbeddingBatch {
            chunks: pending_batch,
        };
        debug!(batch_size = batch.chunks.len(), "sending final embedding batch");
        tx.send(batch).await.ok();
    }

    // Close channel to signal consumers to finish
    drop(tx);

    // Wait for all embedding work to complete
    consumer_handle.await?;

    stats.total_chunks = chunk_counter.load(std::sync::atomic::Ordering::Relaxed);
    info!(
        total_chunks = stats.total_chunks,
        indexed = stats.indexed,
        skipped = stats.skipped,
        "embedding phase completed"
    );

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

    // ── Phase 3.5: Build/update link graph ────────────────────────────────────
    info!("FTS index updated, building link graph");
    let graph_store = GraphStore::open(&config.graph_db_path())?;

    if force {
        graph_store.clear()?;
    }

    // Build set of all known paths for wikilink resolution
    let known_paths: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();

    // Remove graph entries for deleted files
    for old_path in old_hashes.keys() {
        if !current_paths.contains(old_path.as_str()) {
            graph_store.remove_file(old_path)?;
        }
    }

    // Extract and store links for changed files, or all files if graph is empty/force
    let graph_needs_full_build = force
        || graph_store
            .stats()
            .map(|s| s.edge_count == 0 && !files.is_empty())
            .unwrap_or(true);
    let graph_files = if graph_needs_full_build {
        changed_files
            .iter()
            .chain(unchanged_files.iter())
            .collect::<Vec<_>>()
    } else {
        changed_files.iter().collect::<Vec<_>>()
    };

    for file_data in &graph_files {
        let links = extract_links(&file_data.content, &file_data.path, &known_paths);
        graph_store.update_file_links(&file_data.path, &links)?;
    }

    let graph_stats = graph_store.stats()?;
    stats.graph_nodes = graph_stats.node_count;
    stats.graph_edges = graph_stats.edge_count;
    info!(
        nodes = graph_stats.node_count,
        edges = graph_stats.edge_count,
        "graph updated"
    );

    // ── Phase 4: Persist ───────────────────────────────────────────────────────
    info!("persisting vector index and hashes");
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
    let tmp_path = path.with_extension("json.tmp");
    let data = serde_json::to_string_pretty(hashes)?;
    fs::write(&tmp_path, data)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigFile, EmbeddingConfig, EmbeddingProvider};

    // Helper: run async test in a separate thread to avoid blocking-in-runtime issues.
    fn run_async_test<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(f);
        })
        .join()
        .unwrap();
    }

    // Indexer tests require get_embeddings_async which delegates to blocking get_embeddings
    // for single batches. reqwest::blocking::Client cannot be created inside any tokio runtime.
    // These tests are ignored until the embedder is refactored to always use async client.
    // They can be run with: cargo test -- --ignored

    #[test]
    #[ignore]
    fn index_vault_full_pipeline() {
        run_async_test(async {
            // Set up temp vault with .md files
            let dir = tempfile::tempdir().unwrap();
            let vault = dir.path();

            fs::write(
                vault.join("note1.md"),
                "---\ntitle: Rust\n---\nRust is a systems language.",
            )
            .unwrap();
            fs::write(
                vault.join("note2.md"),
                "---\ntitle: Python\n---\nPython is great for scripting.",
            )
            .unwrap();
            fs::create_dir_all(vault.join("wiki")).unwrap();
            fs::write(
                vault.join("wiki/deep.md"),
                "---\ntitle: Deep Learning\n---\nNeural networks.",
            )
            .unwrap();

            // Set up mock embedding server
            let mut server = mockito::Server::new();

            // Return 3-dimensional vectors for any embedding request
            let response_body = serde_json::json!({
                "data": [
                    {"embedding": [0.1, 0.2, 0.3]},
                    {"embedding": [0.4, 0.5, 0.6]},
                    {"embedding": [0.7, 0.8, 0.9]},
                    {"embedding": [0.11, 0.22, 0.33]},
                    {"embedding": [0.44, 0.55, 0.66]},
                    {"embedding": [0.77, 0.88, 0.99]}
                ]
            });

            let _mock = server
                .mock("POST", "/v1/embeddings")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(response_body.to_string())
                .expect(1)
                .create();

            // Write config pointing to mock server
            let config = ConfigFile {
                vault_path: None,
                embedding: EmbeddingConfig {
                    provider: EmbeddingProvider::Custom,
                    endpoint: server.url(),
                    model: "test-model".into(),
                    api_key: None,
                    dimensions: None,
                },
                search: None,
                index: None,
            };
            Config::save_config_file(vault.to_str().unwrap(), &config).unwrap();

            // Run indexing
            let stats = index_vault(vault.to_str().unwrap(), false).await.unwrap();

            assert_eq!(stats.total_files, 3);
            assert_eq!(stats.indexed, 3);
            assert_eq!(stats.skipped, 0);
            assert!(stats.total_chunks > 0);

            // Verify vectors were persisted
            let config = Config::new(vault.to_str().unwrap());
            let vi = VectorIndex::load(&config.vectors_path()).unwrap();
            assert!(!vi.entries.is_empty());

            // Verify hashes were saved
            let hashes = load_hashes(&config.hashes_path());
            assert_eq!(hashes.len(), 3);
        });
    }

    #[test]
    #[ignore]
    fn index_vault_incremental_no_changes() {
        run_async_test(async {
            let dir = tempfile::tempdir().unwrap();
            let vault = dir.path();

            fs::write(vault.join("note.md"), "# Title\nContent here.").unwrap();

            let mut server = mockito::Server::new();
            let response_body = serde_json::json!({
                "data": [{"embedding": [0.1, 0.2, 0.3]}]
            });
            let _mock = server
                .mock("POST", "/v1/embeddings")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(response_body.to_string())
                .create();

            let config = ConfigFile {
                vault_path: None,
                embedding: EmbeddingConfig {
                    provider: EmbeddingProvider::Custom,
                    endpoint: server.url(),
                    model: "test-model".into(),
                    api_key: None,
                    dimensions: None,
                },
                search: None,
                index: None,
            };
            Config::save_config_file(vault.to_str().unwrap(), &config).unwrap();

            // First index
            let stats1 = index_vault(vault.to_str().unwrap(), false).await.unwrap();
            assert_eq!(stats1.indexed, 1);

            // Second index — should skip (unchanged)
            let stats2 = index_vault(vault.to_str().unwrap(), false).await.unwrap();
            assert_eq!(stats2.indexed, 0);
            assert_eq!(stats2.skipped, 1);
        });
    }

    #[test]
    #[ignore]
    fn index_vault_force_reindex() {
        run_async_test(async {
            let dir = tempfile::tempdir().unwrap();
            let vault = dir.path();

            fs::write(vault.join("note.md"), "# Title\nContent.").unwrap();

            let mut server = mockito::Server::new();
            let response_body = serde_json::json!({
                "data": [{"embedding": [0.1, 0.2, 0.3]}]
            });
            let _mock = server
                .mock("POST", "/v1/embeddings")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(response_body.to_string())
                .create();

            let config = ConfigFile {
                vault_path: None,
                embedding: EmbeddingConfig {
                    provider: EmbeddingProvider::Custom,
                    endpoint: server.url(),
                    model: "test-model".into(),
                    api_key: None,
                    dimensions: None,
                },
                search: None,
                index: None,
            };
            Config::save_config_file(vault.to_str().unwrap(), &config).unwrap();

            // First index
            index_vault(vault.to_str().unwrap(), false).await.unwrap();

            // Force reindex
            let stats = index_vault(vault.to_str().unwrap(), true).await.unwrap();
            assert_eq!(stats.indexed, 1); // re-indexed even though unchanged
            assert_eq!(stats.skipped, 0);
        });
    }

    #[test]
    #[ignore]
    fn index_vault_handles_deleted_files() {
        run_async_test(async {
            let dir = tempfile::tempdir().unwrap();
            let vault = dir.path();

            fs::write(vault.join("keep.md"), "keep this").unwrap();
            fs::write(vault.join("delete.md"), "delete this").unwrap();

            let mut server = mockito::Server::new();
            let response_body = serde_json::json!({
                "data": [
                    {"embedding": [0.1, 0.2, 0.3]},
                    {"embedding": [0.4, 0.5, 0.6]}
                ]
            });
            let _mock = server
                .mock("POST", "/v1/embeddings")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(response_body.to_string())
                .create();

            let config = ConfigFile {
                vault_path: None,
                embedding: EmbeddingConfig {
                    provider: EmbeddingProvider::Custom,
                    endpoint: server.url(),
                    model: "test-model".into(),
                    api_key: None,
                    dimensions: None,
                },
                search: None,
                index: None,
            };
            Config::save_config_file(vault.to_str().unwrap(), &config).unwrap();

            // First index with 2 files
            let stats1 = index_vault(vault.to_str().unwrap(), false).await.unwrap();
            assert_eq!(stats1.total_files, 2);

            // Delete one file
            fs::remove_file(vault.join("delete.md")).unwrap();

            // Second index should detect deletion
            let stats2 = index_vault(vault.to_str().unwrap(), false).await.unwrap();
            assert_eq!(stats2.total_files, 1);
            assert_eq!(stats2.deleted, 1);
        });
    }
}
