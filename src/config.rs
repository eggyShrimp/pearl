use std::fmt;
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

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

// ─── MCP Runtime State ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRuntimeState {
    pub transport: String,
    pub vault_path: String,
    pub pid: u32,
    pub local_url: Option<String>,
    pub network_url: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub updated_at: String,
}

impl McpRuntimeState {
    pub fn streamable_http(vault_path: &str, port: u16, network_host: Option<String>) -> Self {
        Self {
            transport: "streamable_http".into(),
            vault_path: vault_path.into(),
            pid: std::process::id(),
            local_url: Some(format!("http://localhost:{}/mcp", port)),
            network_url: network_host.map(|host| format!("http://{}:{}/mcp", host, port)),
            host: Some("0.0.0.0".into()),
            port: Some(port),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn stdio(vault_path: &str) -> Self {
        Self {
            transport: "stdio".into(),
            vault_path: vault_path.into(),
            pid: std::process::id(),
            local_url: None,
            network_url: None,
            host: None,
            port: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn is_reachable(&self) -> bool {
        let Some(port) = self.port else {
            return false;
        };
        can_connect("127.0.0.1", port) || can_connect("localhost", port)
    }
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

/// The structure of config.toml (both global and vault-local).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    /// Default vault path (primarily useful in global config).
    #[serde(default)]
    pub vault_path: Option<String>,
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub search: Option<SearchConfigFile>,
    #[serde(default)]
    pub index: Option<IndexConfigFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfigFile {
    pub vector_weight: Option<f32>,
    pub fts_weight: Option<f32>,
    pub default_limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfigFile {
    pub max_chunk_tokens: Option<usize>,
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
    /// Build config by loading from multiple sources (highest priority first):
    /// 1. Environment variables
    /// 2. Vault-local config: `{vault}/.vault-mcp/config.toml`
    /// 3. Global user config: `~/.config/pearl/config.toml`
    /// 4. Hardcoded defaults
    pub fn new(vault_path: &str) -> Self {
        let vault = PathBuf::from(vault_path);
        let data_dir = vault.join(".vault-mcp");
        let vault_config_path = data_dir.join("config.toml");

        // Try loading vault-local config, then global config, then merge
        let vault_config = Self::load_config_file(&vault_config_path).ok();
        let global_config =
            Self::global_config_path().and_then(|p| Self::load_config_file(&p).ok());

        // Merge: vault-local overrides global
        let file_config = Self::merge_config_files(global_config, vault_config);

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

        let max_chunk_tokens = file_config
            .as_ref()
            .and_then(|fc| fc.index.as_ref())
            .and_then(|i| i.max_chunk_tokens)
            .unwrap_or(400);

        Config {
            vault_path: vault,
            embedding,
            index: IndexConfig {
                data_dir,
                max_chunk_tokens,
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

    /// Merge two config files: vault-local overrides global.
    /// If both are None, returns None.
    fn merge_config_files(
        global: Option<ConfigFile>,
        vault_local: Option<ConfigFile>,
    ) -> Option<ConfigFile> {
        match (global, vault_local) {
            (None, None) => None,
            (Some(g), None) => Some(g),
            (None, Some(v)) => Some(v),
            (Some(g), Some(v)) => {
                // Vault-local fully overrides global embedding config
                // For search config, vault-local fields override global fields
                let search = match (g.search, v.search) {
                    (None, None) => None,
                    (Some(gs), None) => Some(gs),
                    (None, Some(vs)) => Some(vs),
                    (Some(gs), Some(vs)) => Some(SearchConfigFile {
                        vector_weight: vs.vector_weight.or(gs.vector_weight),
                        fts_weight: vs.fts_weight.or(gs.fts_weight),
                        default_limit: vs.default_limit.or(gs.default_limit),
                    }),
                };
                let index = match (g.index, v.index) {
                    (None, None) => None,
                    (Some(gi), None) => Some(gi),
                    (None, Some(vi)) => Some(vi),
                    (Some(gi), Some(vi)) => Some(IndexConfigFile {
                        max_chunk_tokens: vi.max_chunk_tokens.or(gi.max_chunk_tokens),
                    }),
                };
                Some(ConfigFile {
                    vault_path: v.vault_path.or(g.vault_path),
                    embedding: v.embedding,
                    search,
                    index,
                })
            }
        }
    }

    /// Read vault_path from the global config file (if set).
    pub fn global_vault_path() -> Option<String> {
        Self::global_config_path()
            .and_then(|p| Self::load_config_file(&p).ok())
            .and_then(|c| c.vault_path)
    }

    /// Path to the global user config: `~/.config/pearl/config.toml`
    pub fn global_config_path() -> Option<PathBuf> {
        directories::BaseDirs::new().map(|d| d.config_dir().join("pearl").join("config.toml"))
    }

    /// Check if a global config file exists.
    pub fn global_config_exists() -> bool {
        Self::global_config_path()
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    /// Save a ConfigFile to the global config path.
    pub fn save_global_config_file(config: &ConfigFile) -> Result<()> {
        let config_path =
            Self::global_config_path().context("Cannot determine global config directory")?;
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(config).context("Failed to serialize config")?;
        fs::write(&config_path, &content)
            .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        Ok(())
    }

    /// Save a ConfigFile to the vault-local config path.
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

    /// Check if any usable config exists (vault-local or global).
    pub fn any_config_exists(vault_path: &str) -> bool {
        Self::config_exists(vault_path) || Self::global_config_exists()
    }

    /// Path to the vault-local config file.
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

    pub fn graph_db_path(&self) -> PathBuf {
        self.index.data_dir.join("graph.db")
    }

    pub fn mcp_state_path(vault_path: &str) -> PathBuf {
        PathBuf::from(vault_path)
            .join(".vault-mcp")
            .join("mcp-state.json")
    }

    pub fn save_mcp_state(vault_path: &str, state: &McpRuntimeState) -> Result<()> {
        let state_path = Self::mcp_state_path(vault_path);
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content =
            serde_json::to_string_pretty(state).context("Failed to serialize MCP state")?;
        fs::write(&state_path, content)
            .with_context(|| format!("Failed to write MCP state to {}", state_path.display()))?;
        Ok(())
    }

    pub fn load_mcp_state(vault_path: &str) -> Option<McpRuntimeState> {
        let state_path = Self::mcp_state_path(vault_path);
        fs::read_to_string(state_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
    }

    pub fn clear_mcp_state(vault_path: &str) -> Result<()> {
        let state_path = Self::mcp_state_path(vault_path);
        if state_path.exists() {
            fs::remove_file(&state_path).with_context(|| {
                format!("Failed to remove MCP state at {}", state_path.display())
            })?;
        }
        Ok(())
    }
}

fn can_connect(host: &str, port: u16) -> bool {
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok() {
            return true;
        }
    }
    false
}
