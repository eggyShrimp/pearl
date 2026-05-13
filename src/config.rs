use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub vault_path: PathBuf,
    pub embedding: EmbeddingConfig,
    pub index: IndexConfig,
    pub search: SearchConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    pub endpoint: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexConfig {
    pub data_dir: PathBuf,
    pub max_chunk_tokens: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    pub vector_weight: f32,
    pub fts_weight: f32,
    pub default_limit: usize,
}

/// Directories to exclude from vault scanning
pub const EXCLUDE_DIRS: &[&str] = &[
    ".obsidian",
    ".git",
    ".trash",
    "node_modules",
    ".smart-connections",
    ".vault-mcp",
];

impl Config {
    pub fn new(vault_path: &str) -> Self {
        let vault = PathBuf::from(vault_path);
        let data_dir = vault.join(".vault-mcp");

        Config {
            vault_path: vault,
            embedding: EmbeddingConfig {
                endpoint: std::env::var("EMBEDDING_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:11434".into()),
                model: std::env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "bge-m3".into()),
            },
            index: IndexConfig {
                data_dir,
                max_chunk_tokens: 400,
            },
            search: SearchConfig {
                vector_weight: 0.7,
                fts_weight: 0.3,
                default_limit: 10,
            },
        }
    }

    pub fn vectors_path(&self) -> PathBuf {
        self.index.data_dir.join("vectors.json")
    }

    pub fn hashes_path(&self) -> PathBuf {
        self.index.data_dir.join("hashes.json")
    }

    pub fn tantivy_path(&self) -> PathBuf {
        self.index.data_dir.join("tantivy")
    }
}
