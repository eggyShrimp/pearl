use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::core::embedder::detect_ollama_endpoint;

// ─── Top-level Config ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Config {
    pub vault_path: PathBuf,
    pub embedding: EmbeddingConfig,
    pub index: IndexConfig,
    pub search: SearchConfig,
}

// ─── Embedding Config ────────────────────────────────────────────────────────

/// Supported embedding providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    Ollama,
    Openai,
    Custom,
}

impl fmt::Display for EmbeddingProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbeddingProvider::Ollama => write!(f, "ollama"),
            EmbeddingProvider::Openai => write!(f, "openai"),
            EmbeddingProvider::Custom => write!(f, "custom"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: EmbeddingProvider,
    pub endpoint: String,
    pub model: String,
    /// API key for online providers. Can be a literal value or an env var name
    /// prefixed with `$`, e.g. `$OPENAI_API_KEY`.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Embedding dimensions (for providers that support it, e.g. OpenAI text-embedding-3-*).
    #[serde(default)]
    pub dimensions: Option<usize>,
}

impl EmbeddingConfig {
    /// Resolve the actual API key value.
    /// If `api_key` starts with `$`, treat it as an env var name.
    pub fn resolve_api_key(&self) -> Option<String> {
        self.api_key.as_ref().and_then(|key| {
            if let Some(env_name) = key.strip_prefix('$') {
                std::env::var(env_name).ok()
            } else {
                Some(key.clone())
            }
        })
    }

    /// Default config for Ollama (local).
    pub fn default_ollama() -> Self {
        EmbeddingConfig {
            provider: EmbeddingProvider::Ollama,
            endpoint: "http://localhost:11434".into(),
            model: "bge-m3".into(),
            api_key: None,
            dimensions: None,
        }
    }

    /// Default config for OpenAI.
    pub fn default_openai() -> Self {
        EmbeddingConfig {
            provider: EmbeddingProvider::Openai,
            endpoint: "https://api.openai.com".into(),
            model: "text-embedding-3-small".into(),
            api_key: Some("$OPENAI_API_KEY".into()),
            dimensions: None,
        }
    }
}

// ─── Index / Search Config ───────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexConfig {
    pub data_dir: PathBuf,
    pub max_chunk_tokens: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SearchConfig {
    pub vector_weight: f32,
    pub fts_weight: f32,
    pub default_limit: usize,
}

// ─── On-disk TOML representation ─────────────────────────────────────────────

/// The structure of `{vault}/.vault-mcp/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub search: Option<SearchConfigFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfigFile {
    pub vector_weight: Option<f32>,
    pub fts_weight: Option<f32>,
    pub default_limit: Option<usize>,
}

// ─── Directories to exclude from vault scanning ──────────────────────────────

pub const EXCLUDE_DIRS: &[&str] = &[
    ".obsidian",
    ".git",
    ".trash",
    "node_modules",
    ".smart-connections",
    ".vault-mcp",
];

// ─── Implementation ──────────────────────────────────────────────────────────

impl Config {
    /// Build config by loading from `{vault}/.vault-mcp/config.toml` if it exists,
    /// falling back to environment variables, then hardcoded defaults.
    pub fn new(vault_path: &str) -> Self {
        let vault = PathBuf::from(vault_path);
        let data_dir = vault.join(".vault-mcp");
        let config_path = data_dir.join("config.toml");

        // Try loading config file
        let file_config = Self::load_config_file(&config_path).ok();

        let embedding = if let Some(ref fc) = file_config {
            let mut emb = fc.embedding.clone();
            // Allow env vars to override config file values
            if let Ok(endpoint) = std::env::var("EMBEDDING_ENDPOINT") {
                emb.endpoint = endpoint;
            }
            if let Ok(model) = std::env::var("EMBEDDING_MODEL") {
                emb.model = model;
            }
            // For Ollama provider, auto-detect endpoint if still using default
            if emb.provider == EmbeddingProvider::Ollama && emb.endpoint == "http://localhost:11434"
            {
                if let Some(detected) = detect_ollama_endpoint() {
                    emb.endpoint = detected;
                }
            }
            emb
        } else {
            // Legacy: pure env-var driven config
            let provider = match std::env::var("EMBEDDING_PROVIDER").as_deref() {
                Ok("openai") => EmbeddingProvider::Openai,
                Ok("custom") => EmbeddingProvider::Custom,
                _ => EmbeddingProvider::Ollama,
            };
            let default_endpoint = match provider {
                EmbeddingProvider::Ollama => "http://localhost:11434",
                EmbeddingProvider::Openai => "https://api.openai.com",
                EmbeddingProvider::Custom => "http://localhost:8080",
            };
            let default_model = match provider {
                EmbeddingProvider::Ollama => "bge-m3",
                EmbeddingProvider::Openai => "text-embedding-3-small",
                EmbeddingProvider::Custom => "custom-model",
            };

            let endpoint = std::env::var("EMBEDDING_ENDPOINT").unwrap_or_else(|_| {
                // For Ollama, try auto-detection before falling back to default
                if provider == EmbeddingProvider::Ollama {
                    detect_ollama_endpoint().unwrap_or_else(|| default_endpoint.into())
                } else {
                    default_endpoint.into()
                }
            });

            EmbeddingConfig {
                provider: provider.clone(),
                endpoint,
                model: std::env::var("EMBEDDING_MODEL").unwrap_or_else(|_| default_model.into()),
                api_key: std::env::var("EMBEDDING_API_KEY").ok().or_else(|| {
                    // For OpenAI provider, default to $OPENAI_API_KEY env
                    if provider == EmbeddingProvider::Openai {
                        Some("$OPENAI_API_KEY".into())
                    } else {
                        None
                    }
                }),
                dimensions: std::env::var("EMBEDDING_DIMENSIONS")
                    .ok()
                    .and_then(|d| d.parse().ok()),
            }
        };

        let search = if let Some(ref fc) = file_config {
            let s = fc.search.as_ref();
            SearchConfig {
                vector_weight: s.and_then(|x| x.vector_weight).unwrap_or(0.7),
                fts_weight: s.and_then(|x| x.fts_weight).unwrap_or(0.3),
                default_limit: s.and_then(|x| x.default_limit).unwrap_or(10),
            }
        } else {
            SearchConfig {
                vector_weight: 0.7,
                fts_weight: 0.3,
                default_limit: 10,
            }
        };

        Config {
            vault_path: vault,
            embedding,
            index: IndexConfig {
                data_dir,
                max_chunk_tokens: 400,
            },
            search,
        }
    }

    /// Load and parse a config file from disk.
    fn load_config_file(path: &Path) -> Result<ConfigFile> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Cannot read config file: {}", path.display()))?;
        let config: ConfigFile = toml::from_str(&content).context("Failed to parse config.toml")?;
        Ok(config)
    }

    /// Save a ConfigFile to disk.
    pub fn save_config_file(vault_path: &str, config: &ConfigFile) -> Result<()> {
        let data_dir = PathBuf::from(vault_path).join(".vault-mcp");
        fs::create_dir_all(&data_dir)?;
        let config_path = data_dir.join("config.toml");
        let content = toml::to_string_pretty(config).context("Failed to serialize config")?;
        fs::write(&config_path, content)
            .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        Ok(())
    }

    /// Check if a config file exists for this vault.
    pub fn config_exists(vault_path: &str) -> bool {
        let config_path = PathBuf::from(vault_path)
            .join(".vault-mcp")
            .join("config.toml");
        config_path.exists()
    }

    /// Path to the config file.
    pub fn config_path(vault_path: &str) -> PathBuf {
        PathBuf::from(vault_path)
            .join(".vault-mcp")
            .join("config.toml")
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
