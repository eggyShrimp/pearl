use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorEntry {
    pub vector: Vec<f32>,
    pub metadata: ChunkMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMeta {
    pub path: String,
    pub title: String,
    pub tags: String,
    pub heading: String,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String, // truncated preview
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VectorIndex {
    pub entries: Vec<VectorEntry>,
}

impl VectorIndex {
    /// Load index from disk (bincode format, with JSON fallback for migration).
    pub fn load(path: &Path) -> Result<Self> {
        // Try bincode path first
        let bin_path = Self::bin_path(path);
        if bin_path.exists() {
            let data = fs::read(&bin_path).context("Failed to read vector index (bincode)")?;
            let index: VectorIndex =
                bincode::deserialize(&data).context("Failed to deserialize vector index")?;
            debug!(entries = index.entries.len(), "vector index loaded (bincode)");
            return Ok(index);
        }

        // Fallback: try legacy JSON path
        if path.exists() {
            let data = fs::read_to_string(path).context("Failed to read vector index (json)")?;
            let index: VectorIndex =
                serde_json::from_str(&data).context("Failed to parse vector index (json)")?;
            debug!(entries = index.entries.len(), "vector index loaded (json, migrating)");
            // Auto-migrate: save as bincode, remove old JSON
            if let Ok(()) = index.save(path) {
                // Remove legacy JSON file after successful migration
                let _ = fs::remove_file(path);
            }
            return Ok(index);
        }

        debug!("vector index not found, starting empty");
        Ok(Self::default())
    }

    /// Save index to disk in bincode format (atomic: write to temp, then rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        let bin_path = Self::bin_path(path);
        if let Some(parent) = bin_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_path = bin_path.with_extension("bin.tmp");
        let data = bincode::serialize(self).context("Failed to serialize vector index")?;
        fs::write(&tmp_path, data)?;
        fs::rename(&tmp_path, &bin_path)?;
        debug!(entries = self.entries.len(), "vector index saved");
        Ok(())
    }

    /// Remove all entries for a given file path.
    pub fn remove_file(&mut self, file_path: &str) {
        self.entries.retain(|e| e.metadata.path != file_path);
    }

    /// Add entries.
    pub fn add_entries(&mut self, entries: Vec<VectorEntry>) {
        let count = entries.len();
        self.entries.extend(entries);
        info!(count, total = self.entries.len(), "entries added to vector index");
    }

    /// Query the index with a vector, returning top-K results by cosine similarity.
    pub fn query(&self, query_vec: &[f32], limit: usize) -> Vec<SearchHit> {
        if self.entries.is_empty() {
            debug!("vector query on empty index, returning 0 results");
            return vec![];
        }

        let query_norm = vec_norm(query_vec);
        if query_norm == 0.0 {
            debug!("zero-norm query vector, returning 0 results");
            return vec![];
        }

        let mut scores: Vec<(usize, f32)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let score = cosine_similarity(query_vec, &entry.vector, query_norm);
                (i, score)
            })
            .collect();

        // Sort by score descending
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let results: Vec<SearchHit> = scores
            .into_iter()
            .take(limit)
            .map(|(i, score)| SearchHit {
                metadata: self.entries[i].metadata.clone(),
                score,
            })
            .collect();

        debug!(result_count = results.len(), total_entries = self.entries.len(), "vector query completed");
        results
    }

    /// Derive the bincode file path from the legacy JSON path.
    /// `vectors.json` → `vectors.bin`
    fn bin_path(json_path: &Path) -> std::path::PathBuf {
        json_path.with_extension("bin")
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub metadata: ChunkMeta,
    pub score: f32,
}

/// Compute cosine similarity between two vectors.
/// `a_norm` is pre-computed norm of vector `a`.
fn cosine_similarity(a: &[f32], b: &[f32], a_norm: f32) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let b_norm = vec_norm(b);
    if b_norm == 0.0 {
        return 0.0;
    }
    dot / (a_norm * b_norm)
}

fn vec_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── cosine_similarity ────────────────────────────────────────────────────

    #[test]
    fn cosine_similarity_identical() {
        let a = [1.0, 0.0, 0.0];
        let score = cosine_similarity(&a, &a, vec_norm(&a));
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = [1.0, 0.0, 0.0];
        let b = [0.0, 1.0, 0.0];
        let score = cosine_similarity(&a, &b, vec_norm(&a));
        assert!((score - 0.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite() {
        let a = [1.0, 0.0];
        let b = [-1.0, 0.0];
        let score = cosine_similarity(&a, &b, vec_norm(&a));
        assert!((score - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = [0.0, 0.0];
        let b = [1.0, 0.0];
        let score = cosine_similarity(&a, &b, vec_norm(&a));
        // a_norm=0 → 0/0 = NaN; the caller (query) handles this by filtering
        assert!(score.is_nan());
    }

    // ── vec_norm ─────────────────────────────────────────────────────────────

    #[test]
    fn vec_norm_basic() {
        let v = [3.0, 4.0];
        assert!((vec_norm(&v) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn vec_norm_zero() {
        assert_eq!(vec_norm(&[0.0, 0.0]), 0.0);
    }

    // ── VectorIndex::query ───────────────────────────────────────────────────

    fn make_entry(path: &str, vector: Vec<f32>) -> VectorEntry {
        VectorEntry {
            vector,
            metadata: ChunkMeta {
                path: path.to_string(),
                title: path.to_string(),
                tags: String::new(),
                heading: String::new(),
                start_line: 1,
                end_line: 1,
                text: "test".to_string(),
            },
        }
    }

    #[test]
    fn query_top_k() {
        let mut idx = VectorIndex::default();
        idx.add_entries(vec![
            make_entry("a.md", vec![1.0, 0.0, 0.0]),
            make_entry("b.md", vec![0.0, 1.0, 0.0]),
            make_entry("c.md", vec![0.7, 0.7, 0.0]),
        ]);

        let query = vec![1.0, 0.0, 0.0];
        let results = idx.query(&query, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].metadata.path, "a.md"); // exact match
    }

    #[test]
    fn query_empty_index() {
        let idx = VectorIndex::default();
        let results = idx.query(&[1.0, 0.0], 5);
        assert!(results.is_empty());
    }

    #[test]
    fn query_respects_limit() {
        let mut idx = VectorIndex::default();
        idx.add_entries(vec![
            make_entry("a.md", vec![1.0, 0.0]),
            make_entry("b.md", vec![0.9, 0.1]),
            make_entry("c.md", vec![0.8, 0.2]),
        ]);
        let results = idx.query(&[1.0, 0.0], 1);
        assert_eq!(results.len(), 1);
    }

    // ── VectorIndex::remove_file ─────────────────────────────────────────────

    #[test]
    fn remove_file_entries() {
        let mut idx = VectorIndex::default();
        idx.add_entries(vec![
            make_entry("a.md", vec![1.0]),
            make_entry("b.md", vec![0.5]),
            make_entry("a.md", vec![0.8]),
        ]);
        assert_eq!(idx.entries.len(), 3);

        idx.remove_file("a.md");
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(idx.entries[0].metadata.path, "b.md");
    }

    // ── Save / Load ──────────────────────────────────────────────────────────

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vectors.json");

        let mut idx = VectorIndex::default();
        idx.add_entries(vec![
            make_entry("note.md", vec![0.1, 0.2, 0.3]),
            make_entry("other.md", vec![0.4, 0.5, 0.6]),
        ]);
        idx.save(&path).unwrap();

        let loaded = VectorIndex::load(&path).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].metadata.path, "note.md");
        assert_eq!(loaded.entries[0].vector, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let idx = VectorIndex::load(&path).unwrap();
        assert!(idx.entries.is_empty());
    }

    #[test]
    fn save_and_load_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vectors.json");
        let idx = VectorIndex::default();
        idx.save(&path).unwrap();

        let loaded = VectorIndex::load(&path).unwrap();
        assert!(loaded.entries.is_empty());
    }
}
