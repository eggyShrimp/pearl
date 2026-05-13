use anyhow::Result;
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router, ServiceExt, transport::stdio};
use serde::Deserialize;

use crate::config::Config;
use crate::core::embedder;
use crate::core::vault;
use crate::indexer;
use crate::search;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchParams {
    /// Natural language search query
    query: String,
    /// Max results to return (default 10)
    limit: Option<usize>,
    /// Limit search to these folders, e.g. ["wiki/", "raw/"]
    folders: Option<Vec<String>>,
    /// Filter by tags (AND logic), e.g. ["ai", "tech"]
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct IndexParams {
    /// Force full reindex (ignore cached hashes)
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetNoteParams {
    /// Relative path from vault root, e.g. "wiki/topics/RAG.md"
    path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ListNotesParams {
    /// Directory to list (default: vault root)
    folder: Option<String>,
    /// List recursively (default: false)
    #[serde(default)]
    recursive: bool,
}

#[derive(Debug, Clone)]
struct VaultServer {
    vault_path: String,
}

#[tool_router(server_handler)]
impl VaultServer {
    /// Search vault using hybrid semantic + full-text search. Returns ranked chunks with context.
    #[tool(description = "Search vault using hybrid semantic + full-text search. Returns ranked chunks with context.")]
    async fn hybrid_search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        let limit = params.limit.unwrap_or(10);
        match search::hybrid_search(&self.vault_path, &params.query, limit, params.folders, params.tags).await {
            Ok(results) => serde_json::to_string_pretty(&results).unwrap_or_else(|e| format!("Serialization error: {}", e)),
            Err(e) => format!("Search error: {}", e),
        }
    }

    /// Index or reindex the vault for semantic search. Uses incremental hashing by default.
    #[tool(description = "Index or reindex the vault for semantic search. Uses incremental hashing by default.")]
    async fn index_vault(&self, Parameters(params): Parameters<IndexParams>) -> String {
        match indexer::index_vault(&self.vault_path, params.force).await {
            Ok(stats) => format!(
                "Indexing complete:\n  Total files: {}\n  Indexed: {}\n  Skipped (unchanged): {}\n  Deleted: {}\n  Total chunks: {}",
                stats.total_files, stats.indexed, stats.skipped, stats.deleted, stats.total_chunks
            ),
            Err(e) => format!("Indexing error: {}", e),
        }
    }

    /// Read a vault note by its relative path. Returns full markdown content.
    #[tool(description = "Read a vault note by its relative path. Returns full markdown content.")]
    fn get_note(&self, Parameters(params): Parameters<GetNoteParams>) -> String {
        let config = Config::new(&self.vault_path);
        match vault::read_vault_file(&config.vault_path, &params.path) {
            Ok(content) => content,
            Err(e) => format!("Error reading note: {}", e),
        }
    }

    /// List files and directories in the vault. Use for navigation and discovery.
    #[tool(description = "List files and directories in the vault. Use for navigation and discovery.")]
    fn list_notes(&self, Parameters(params): Parameters<ListNotesParams>) -> String {
        let config = Config::new(&self.vault_path);
        let folder = params.folder.unwrap_or_default();
        let entries = vault::list_directory(&config.vault_path, &folder, params.recursive);
        let formatted: Vec<String> = entries
            .iter()
            .map(|e| {
                if e.is_dir {
                    format!("[dir] {}", e.path)
                } else {
                    format!("      {}", e.path)
                }
            })
            .collect();
        format!("{} entries:\n{}", entries.len(), formatted.join("\n"))
    }

    /// Check vault-mcp health: embedding service connectivity, model availability, vault stats.
    #[tool(description = "Check vault-mcp health: embedding service connectivity, model availability, vault stats.")]
    fn vault_status(&self) -> String {
        let config = Config::new(&self.vault_path);
        let health = embedder::check_health(&config.embedding);
        let api_key_status = match config.embedding.resolve_api_key() {
            Some(k) if !k.is_empty() => format!("configured ({}...)", &k[..k.len().min(8)]),
            Some(_) => "empty".into(),
            None => "not required".into(),
        };
        format!(
            "Provider: {}\nEmbedding service: {}\nEndpoint: {}\nModel: {}\nAPI key: {}\nVault: {}\nConfig: {}",
            config.embedding.provider,
            health.message(),
            config.embedding.endpoint,
            config.embedding.model,
            api_key_status,
            config.vault_path.display(),
            if Config::config_exists(&self.vault_path) { "found" } else { "not found (using defaults)" }
        )
    }
}

/// Run the MCP server on stdio.
pub async fn run_server(vault_path: &str) -> Result<()> {
    let server = VaultServer {
        vault_path: vault_path.to_string(),
    };

    tracing::info!("Starting vault-mcp server for: {}", vault_path);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
