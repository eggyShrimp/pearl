use std::io::IsTerminal;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use console::style;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use serde::Deserialize;
use tokio::net::TcpListener;

use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::core::embedder;
use crate::core::vault;
use crate::indexer;
use crate::search;

/// Single unified tool parameter. The `command` field determines the operation.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct VaultSearchParams {
    /// Command to execute: "search", "index", "get", "list", "status"
    command: String,

    // ── search params ──
    /// Natural language search query (required for "search")
    query: Option<String>,
    /// Max results to return (default 10, for "search")
    limit: Option<usize>,
    /// Limit search to these folders, e.g. ["wiki/", "raw/"] (for "search")
    folders: Option<Vec<String>>,
    /// Filter by tags (AND logic), e.g. ["ai", "tech"] (for "search")
    tags: Option<Vec<String>>,
    /// Search mode: "hybrid" (default), "semantic", "fts" (for "search")
    mode: Option<String>,

    // ── index params ──
    /// Force full reindex, ignoring cached hashes (for "index")
    #[serde(default)]
    force: bool,

    // ── get params ──
    /// Relative path from vault root, e.g. "wiki/topics/RAG.md" (for "get")
    path: Option<String>,

    // ── list params ──
    /// Directory to list (for "list", default: vault root)
    folder: Option<String>,
    /// List recursively (for "list", default: false)
    #[serde(default)]
    recursive: bool,
}

#[derive(Debug, Clone)]
struct VaultServer {
    vault_path: String,
}

#[tool_router(server_handler)]
impl VaultServer {
    /// Unified vault search tool. Use the `command` parameter to specify the operation:
    ///
    /// - "search" — Hybrid semantic + full-text search. Params: query (required), mode, limit, folders, tags
    /// - "index" — Build/rebuild the search index. Params: force (bool)
    /// - "get" — Read a note by relative path. Params: path (required)
    /// - "list" — List files/directories. Params: folder, recursive
    /// - "status" — Check system health and configuration
    #[tool(
        description = "Unified vault search tool. Commands: 'search' (hybrid semantic+full-text, params: query, mode, limit, folders, tags), 'index' (rebuild index, params: force), 'get' (read note, params: path), 'list' (list files, params: folder, recursive), 'status' (health check)"
    )]
    async fn vault_search(&self, Parameters(params): Parameters<VaultSearchParams>) -> String {
        match params.command.as_str() {
            "search" => self.handle_search(params).await,
            "index" => self.handle_index(params).await,
            "get" => self.handle_get(params),
            "list" => self.handle_list(params),
            "status" => self.handle_status(),
            other => format!(
                "Unknown command: '{}'. Supported: search, index, get, list, status",
                other
            ),
        }
    }
}

impl VaultServer {
    async fn handle_search(&self, params: VaultSearchParams) -> String {
        let query = match params.query {
            Some(q) if !q.is_empty() => q,
            _ => return "Error: 'query' is required for search command".to_string(),
        };
        let limit = params.limit.unwrap_or(10);
        let mode = params.mode.as_deref().unwrap_or("hybrid");

        let result = match mode {
            "hybrid" => {
                search::hybrid_search(&self.vault_path, &query, limit, params.folders, params.tags)
                    .await
            }
            "semantic" => {
                search::vector_search_only(
                    &self.vault_path,
                    &query,
                    limit,
                    params.folders,
                    params.tags,
                )
                .await
            }
            "fts" => search::fts_search_only(&self.vault_path, &query, limit).await,
            _ => {
                return format!(
                    "Unknown search mode: '{}'. Supported: hybrid, semantic, fts",
                    mode
                )
            }
        };

        match result {
            Ok(results) => serde_json::to_string_pretty(&results)
                .unwrap_or_else(|e| format!("Serialization error: {}", e)),
            Err(e) => format!("Search error: {}", e),
        }
    }

    async fn handle_index(&self, params: VaultSearchParams) -> String {
        match indexer::index_vault(&self.vault_path, params.force).await {
            Ok(stats) => format!(
                "Indexing complete:\n  Total files: {}\n  Indexed: {}\n  Skipped (unchanged): {}\n  Deleted: {}\n  Total chunks: {}",
                stats.total_files, stats.indexed, stats.skipped, stats.deleted, stats.total_chunks
            ),
            Err(e) => format!("Indexing error: {}", e),
        }
    }

    fn handle_get(&self, params: VaultSearchParams) -> String {
        let path = match params.path {
            Some(p) if !p.is_empty() => p,
            _ => return "Error: 'path' is required for get command".to_string(),
        };
        let config = Config::new(&self.vault_path);
        match vault::read_vault_file(&config.vault_path, &path) {
            Ok(content) => content,
            Err(e) => format!("Error reading note: {}", e),
        }
    }

    fn handle_list(&self, params: VaultSearchParams) -> String {
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

    fn handle_status(&self) -> String {
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
            if Config::config_exists(&self.vault_path) {
                "found"
            } else {
                "not found (using defaults)"
            }
        )
    }
}

/// Print the startup banner to stderr.
fn print_banner(vault_path: &str, config: &Config, transport_info: &str) {
    if !std::io::stderr().is_terminal() {
        return;
    }
    eprintln!();
    eprintln!("  {} vault-search MCP server", style("●").green().bold());
    eprintln!();
    eprintln!("    {}  {}", style("vault").dim(), vault_path);
    eprintln!(
        "    {}  {} ({})",
        style("model").dim(),
        config.embedding.model,
        config.embedding.provider
    );
    eprintln!("    {}  {}", style("transport").dim(), transport_info);
    eprintln!();
}

fn print_stop() {
    if !std::io::stderr().is_terminal() {
        return;
    }
    eprintln!();
    eprintln!("  {} Server stopped", style("■").dim());
    eprintln!();
}

/// Run the MCP server on stdio transport.
pub async fn run_server_stdio(vault_path: &str) -> Result<()> {
    let server = VaultServer {
        vault_path: vault_path.to_string(),
    };
    let config = Config::new(vault_path);

    print_banner(vault_path, &config, "stdio (JSON-RPC)");

    if std::io::stderr().is_terminal() {
        eprintln!("  {} Waiting for client connection...", style("↺").dim());
        eprintln!(
            "  {} Press {} to stop",
            style("hint").dim(),
            style("Ctrl+C").bold()
        );
        eprintln!();
    }

    tracing::info!("Starting vault-mcp server (stdio) for: {}", vault_path);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    print_stop();
    Ok(())
}

/// Try to bind a TCP listener on 0.0.0.0, starting from `port` and incrementing on conflict.
/// Tries up to 16 consecutive ports before giving up.
async fn bind_with_fallback(port: u16) -> Result<TcpListener> {
    const MAX_ATTEMPTS: u16 = 16;
    for i in 0..MAX_ATTEMPTS {
        let attempt_port = port.saturating_add(i);
        match TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], attempt_port))).await {
            Ok(listener) => {
                if i > 0 && std::io::stderr().is_terminal() {
                    eprintln!(
                        "  {} Port {} in use, using {} instead",
                        style("!").yellow().bold(),
                        port,
                        attempt_port
                    );
                }
                return Ok(listener);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!(
        "Could not bind to any port in range {}–{}",
        port,
        port.saturating_add(MAX_ATTEMPTS - 1)
    );
}

/// Get the local LAN IP address (first non-loopback IPv4).
fn local_ip() -> Option<std::net::IpAddr> {
    use std::net::UdpSocket;
    // Connect to a public address to determine the outbound interface IP.
    // No actual traffic is sent (UDP, unbound).
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// Run the MCP server on HTTP (Streamable HTTP transport).
pub async fn run_server_http(vault_path: &str, port: u16) -> Result<()> {
    let vault_path_owned = vault_path.to_string();
    let config = Config::new(vault_path);

    let listener = bind_with_fallback(port).await?;
    let actual_addr = listener.local_addr()?;
    let actual_port = actual_addr.port();

    let lan_ip = local_ip();
    let transport_info = "Streamable HTTP (network)";
    print_banner(vault_path, &config, transport_info);

    if std::io::stderr().is_terminal() {
        eprintln!(
            "  {} Local:   {}",
            style("↺").dim(),
            style(format!("http://localhost:{}/mcp", actual_port)).underlined()
        );
        if let Some(ip) = lan_ip {
            eprintln!(
                "  {} Network: {}",
                style("↺").dim(),
                style(format!("http://{}:{}/mcp", ip, actual_port)).underlined()
            );
        }
        eprintln!();
        eprintln!(
            "  {} Press {} to stop",
            style("hint").dim(),
            style("Ctrl+C").bold()
        );
        eprintln!();
    }

    tracing::info!(
        "Starting vault-mcp server (HTTP) on {} for: {}",
        actual_addr,
        vault_path
    );

    let cancel_token = CancellationToken::new();

    let mut http_config = StreamableHttpServerConfig::default();
    http_config.stateful_mode = true;
    http_config.cancellation_token = cancel_token.clone();
    // Allow connections from any host (LAN access)
    http_config.allowed_hosts = vec![
        "localhost".into(),
        "127.0.0.1".into(),
        "::1".into(),
        "0.0.0.0".into(),
    ];
    if let Some(ip) = lan_ip {
        http_config.allowed_hosts.push(ip.to_string());
    }

    let session_manager = Arc::new(LocalSessionManager::default());

    let service = StreamableHttpService::new(
        move || {
            let server = VaultServer {
                vault_path: vault_path_owned.clone(),
            };
            Ok(server)
        },
        session_manager,
        http_config,
    );

    // Build hyper service from the StreamableHttpService
    let make_svc = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let svc = service.clone();
        async move { Ok::<_, std::convert::Infallible>(svc.handle(req).await) }
    });

    // Spawn Ctrl+C handler
    let cancel_for_signal = cancel_token.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_for_signal.cancel();
    });

    // Accept loop
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => break,
            accepted = listener.accept() => {
                let (stream, _addr) = accepted?;
                let svc = make_svc.clone();
                tokio::spawn(async move {
                    let io = hyper_util::rt::TokioIo::new(stream);
                    if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new()
                    )
                    .serve_connection(io, svc)
                    .await
                    {
                        tracing::debug!("Connection error: {}", e);
                    }
                });
            }
        }
    }

    print_stop();
    Ok(())
}
