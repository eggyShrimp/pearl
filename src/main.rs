mod config;
mod core;
mod indexer;
mod search;
mod server;
mod watch;

use std::io::IsTerminal;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

/// Search mode: which retrieval methods to combine.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchMode {
    /// Hybrid: combine vector similarity + full-text search (default)
    Hybrid,
    /// Semantic: vector similarity search only
    Semantic,
    /// FTS: full-text keyword search only
    Fts,
}

const PEARL_LOGO: &str = r#"
                           __
    ____  ___  ____ ______/ /
   / __ \/ _ \/ __ `/ ___/ / 
  / /_/ /  __/ /_/ / /  / /  
 / .___/\___/\__,_/_/  /_/   
/_/                          
"#;

#[derive(Parser)]
#[command(
    name = "pearl",
    version,
    about = "Semantic search for Obsidian vaults, exposed as an MCP server for AI agents.",
    before_long_help = PEARL_LOGO,
    long_about = "pearl provides hybrid semantic + full-text search over your Obsidian vault.\n\n\
        It embeds your notes using a local (Ollama) or cloud (OpenAI) model, stores vectors\n\
        alongside your vault, and serves search results via the Model Context Protocol (MCP)\n\
        for AI coding agents like Claude Desktop, Cursor, or OpenCode.\n\n\
        Quick start:\n\
        \x20 1. pearl init              # configure embedding provider\n\
        \x20 2. pearl index             # build the search index\n\
        \x20 3. pearl serve             # start MCP server for your agent\n\n\
        All index data is stored locally in {vault}/.vault-mcp/.",
    after_help = "Documentation: https://github.com/eggyShrimp/pearl"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server for AI agents to connect to.
    ///
    /// Exposes a single `pearl` tool with commands: search, index, get, list, status.
    /// By default uses stdio transport (JSON-RPC over stdin/stdout).
    /// Use --network to expose the server over HTTP (Streamable HTTP transport)
    /// for LAN access by other devices.
    Serve {
        /// Path to the Obsidian vault (resolved automatically if omitted)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,

        /// Expose server over the network via HTTP (Streamable HTTP transport).
        /// Optionally specify a port (default: 8686).
        /// If the port is occupied, the next available port will be used.
        #[arg(short, long, env = "MCP_PORT", default_missing_value = "8686", num_args = 0..=1)]
        network: Option<u16>,
    },

    /// Build or update the search index (embeddings + full-text).
    ///
    /// Only re-embeds files that changed since last run (incremental, based on SHA-256 hashes).
    /// Use --force to rebuild everything, e.g. after switching embedding models.
    Index {
        /// Path to the Obsidian vault (resolved automatically if omitted)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        /// Force full reindex (ignore cached hashes)
        #[arg(short, long, default_value_t = false)]
        force: bool,
    },

    /// Run a search query from the terminal (for testing and scripting).
    ///
    /// Combines vector similarity and full-text search, returns ranked results.
    /// Use --json for machine-readable output that can be piped to jq or other tools.
    Search {
        /// Path to the Obsidian vault (resolved automatically if omitted)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        /// Natural language search query
        query: String,
        /// Maximum number of results (top-k)
        #[arg(short = 'k', long = "top-k", alias = "limit", default_value_t = 10)]
        limit: usize,
        /// Restrict search to specific folders (can be repeated)
        #[arg(short, long)]
        folder: Vec<String>,
        /// Exclude folders from results (can be repeated)
        #[arg(short, long)]
        exclude: Vec<String>,
        /// Filter by tags (AND logic, can be repeated)
        #[arg(short, long)]
        tag: Vec<String>,
        /// Search mode: hybrid (default), semantic, fts
        #[arg(short, long, default_value = "hybrid")]
        mode: SearchMode,
        /// Minimum relevance score (0.0–1.0) to include in results
        #[arg(long)]
        threshold: Option<f32>,
        /// Lines of context to show around each match
        #[arg(short = 'C', long, default_value_t = 0)]
        context: usize,
        /// Only include notes modified after this date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,
        /// Output as JSON (machine-readable, pipe-friendly)
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Watch vault for file changes and auto-update the index.
    ///
    /// Monitors .md files for create/modify/delete events and runs incremental indexing.
    /// Uses a 2-second debounce to batch rapid changes.
    Watch {
        /// Path to the Obsidian vault (resolved automatically if omitted)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        /// Run as a background daemon
        #[arg(short, long, default_value_t = false)]
        daemon: bool,
        /// Stop a running watch daemon
        #[arg(long, default_value_t = false)]
        stop: bool,
        /// Show watcher status
        #[arg(long, default_value_t = false)]
        status: bool,
    },

    /// Interactive setup wizard — configure embedding provider and generate config.
    ///
    /// By default writes to {vault}/.vault-mcp/config.toml.
    /// Use --global to write to ~/.config/pearl/config.toml instead,
    /// which serves as the default config for all vaults.
    Init {
        /// Path to the Obsidian vault (auto-detected if omitted)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        /// Write config to global path (~/.config/pearl/config.toml) instead of vault-local
        #[arg(short, long, default_value_t = false)]
        global: bool,
    },

    /// Install pearl as a tool/skill into AI coding agents.
    ///
    /// Generates MCP server config and skill/rules files for the selected
    /// agent products (Cursor, Claude Code, Trae, Windsurf, OpenCode, Codex).
    Install {
        /// Target agent products (comma-separated). If omitted, shows interactive selection.
        /// Supported: cursor, claude-code, trae, windsurf, opencode, codex
        #[arg(short, long, value_delimiter = ',')]
        target: Vec<String>,
    },

    /// Show the effective configuration (merged from all sources).
    ///
    /// Useful for debugging which settings are active and where config files are located.
    Config {
        /// Path to the Obsidian vault (affects vault-local config lookup)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        /// Output as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { vault, network } => {
            let vault = resolve_vault(vault)?;
            // In serve mode, only log to stderr (stdout is MCP transport)
            tracing_subscriber::fmt()
                .with_env_filter("pearl=info")
                .with_writer(std::io::stderr)
                .init();
            watch::ensure_watch_running(&vault);
            match network {
                Some(port) => server::run_server_http(&vault, port).await,
                None => server::run_server_stdio(&vault).await,
            }
        }
        Commands::Index { vault, force } => {
            let vault = resolve_vault(vault)?;
            use console::style;
            use std::io::Write;

            tracing_subscriber::fmt()
                .with_env_filter("pearl=info")
                .with_writer(std::io::stderr)
                .init();

            // Pre-flight: check embedding service connectivity
            let config = config::Config::new(&vault);
            let health = core::embedder::check_health(&config.embedding);
            match &health {
                core::embedder::HealthStatus::Ok => {}
                core::embedder::HealthStatus::Unreachable(msg) => {
                    eprintln!();
                    eprintln!("  {} {}", style("✗").red().bold(), msg);
                    eprintln!();
                    if config.embedding.provider == config::EmbeddingProvider::Ollama {
                        eprintln!("  Make sure Ollama is running:");
                        eprintln!(
                            "    1. Install: {}",
                            style("https://ollama.com/download").underlined()
                        );
                        eprintln!("    2. Start:   {}", style("ollama serve").bold());
                        eprintln!(
                            "    3. Pull:    {}",
                            style(format!("ollama pull {}", config.embedding.model)).bold()
                        );
                    } else {
                        eprintln!("  Check your embedding endpoint and API key configuration.");
                        eprintln!(
                            "  Run {} to reconfigure.",
                            style("pearl init").bold()
                        );
                    }
                    eprintln!();
                    anyhow::bail!("Embedding service is not reachable. Cannot index.");
                }
                core::embedder::HealthStatus::ModelMissing(msg) => {
                    eprintln!();
                    eprintln!("  {} {}", style("✗").red().bold(), msg);
                    eprintln!();
                    anyhow::bail!("Required model is not available. Cannot index.");
                }
            }

            let is_tty = std::io::stdout().is_terminal();

            let stats = indexer::index_vault_with_progress(&vault, force, |progress| {
                if is_tty {
                    // Overwrite current line with progress
                    eprint!(
                        "\r  [{}/{}] {}",
                        progress.current, progress.total, progress.path
                    );
                    // Clear rest of line in case previous path was longer
                    eprint!("\x1b[K");
                    std::io::stderr().flush().ok();
                }
            })
            .await?;

            if is_tty {
                // Clear progress line
                eprint!("\r\x1b[K");
            }

            println!();
            println!("  {} Indexing complete", style("✓").green().bold());
            println!();
            println!("    {}  {}", style("files").dim(), stats.total_files);
            println!("    {}  {}", style("indexed").dim(), stats.indexed);
            println!("    {}  {}", style("skipped").dim(), stats.skipped);
            println!("    {}  {}", style("deleted").dim(), stats.deleted);
            println!("    {}  {}", style("chunks").dim(), stats.total_chunks);
            println!();
            watch::ensure_watch_running(&vault);
            Ok(())
        }
        Commands::Search {
            vault,
            query,
            limit,
            folder,
            exclude,
            tag,
            mode,
            threshold,
            context,
            since,
            json,
        } => {
            let vault = resolve_vault(vault)?;
            tracing_subscriber::fmt()
                .with_env_filter("pearl=info")
                .with_writer(std::io::stderr)
                .init();
            let folders = if folder.is_empty() {
                None
            } else {
                Some(folder)
            };
            let tags = if tag.is_empty() { None } else { Some(tag) };
            let excludes = if exclude.is_empty() {
                None
            } else {
                Some(exclude)
            };

            // Parse --since into a timestamp for file mtime filtering
            let since_ts = match &since {
                Some(date_str) => {
                    let naive =
                        chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").map_err(|e| {
                            anyhow::anyhow!(
                                "Invalid --since date '{}': {} (expected YYYY-MM-DD)",
                                date_str,
                                e
                            )
                        })?;
                    Some(naive.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
                }
                None => None,
            };

            let mut results = match mode {
                SearchMode::Hybrid => {
                    search::hybrid_search(&vault, &query, limit, folders, tags).await?
                }
                SearchMode::Semantic => {
                    search::vector_search_only(&vault, &query, limit, folders, tags).await?
                }
                SearchMode::Fts => search::fts_search_only(&vault, &query, limit).await?,
            };

            // Post-filter: --exclude
            if let Some(ref excludes) = excludes {
                results.retain(|r| !excludes.iter().any(|ex| r.path.starts_with(ex.as_str())));
            }

            // Post-filter: --threshold
            if let Some(min_score) = threshold {
                results.retain(|r| r.score >= min_score);
            }

            // Post-filter: --since (file modification time)
            if let Some(ts) = since_ts {
                results.retain(|r| {
                    let full_path = std::path::Path::new(&vault).join(&r.path);
                    match std::fs::metadata(&full_path) {
                        Ok(meta) => {
                            match meta.modified() {
                                Ok(mtime) => {
                                    let file_ts = mtime
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs() as i64)
                                        .unwrap_or(0);
                                    file_ts >= ts
                                }
                                Err(_) => true, // keep if mtime unavailable
                            }
                        }
                        Err(_) => false, // drop if file not accessible
                    }
                });
            }

            if json {
                // Machine-readable JSON output
                println!(
                    "{}",
                    serde_json::to_string_pretty(&results)
                        .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
                );
            } else {
                // Human-readable output
                use console::style;
                for (i, r) in results.iter().enumerate() {
                    println!(
                        "{}. [{}] {} (score: {:.3})",
                        i + 1,
                        r.match_type,
                        style(&r.path).cyan(),
                        r.score
                    );
                    if context > 0 && r.start_line > 0 {
                        // Show context lines from the source file
                        let full_path = std::path::Path::new(&vault).join(&r.path);
                        if let Ok(content) = std::fs::read_to_string(&full_path) {
                            let lines: Vec<&str> = content.lines().collect();
                            let start = (r.start_line as usize)
                                .saturating_sub(1)
                                .saturating_sub(context);
                            let end = (r.end_line as usize)
                                .saturating_add(context)
                                .min(lines.len());
                            for (li, line) in lines[start..end].iter().enumerate() {
                                let line_num = start + li + 1;
                                let is_match_line = line_num >= r.start_line as usize
                                    && line_num <= r.end_line as usize;
                                if is_match_line {
                                    println!(
                                        "   {} {}",
                                        style(format!("{:>4}", line_num)).dim(),
                                        line
                                    );
                                } else {
                                    println!(
                                        "   {} {}",
                                        style(format!("{:>4}", line_num)).dim(),
                                        style(line).dim()
                                    );
                                }
                            }
                        } else {
                            println!("   {}", r.chunk.chars().take(120).collect::<String>());
                        }
                    } else {
                        println!("   {}", r.chunk.chars().take(120).collect::<String>());
                    }
                    println!();
                }
                if results.is_empty() {
                    println!("  No results found.");
                }
            }
            Ok(())
        }
        Commands::Watch {
            vault,
            daemon,
            stop,
            status,
        } => {
            let vault = resolve_vault(vault)?;
            tracing_subscriber::fmt()
                .with_env_filter("pearl=info")
                .with_writer(std::io::stderr)
                .init();

            if status {
                if watch::is_watch_running(&vault) {
                    let pid =
                        std::fs::read_to_string(watch::pid_file_path(&vault)).unwrap_or_default();
                    println!("  Watcher is running (PID {})", pid.trim());
                } else {
                    println!("  Watcher is not running");
                }
                return Ok(());
            }

            if stop {
                return watch::stop_watch(&vault);
            }

            if daemon {
                watch::daemonize_watch(&vault)?;
                // After daemonize, we are in the child process — run the watcher
                watch::run_watch(&vault).await
            } else {
                watch::run_watch(&vault).await
            }
        }
        Commands::Init { vault, global } => {
            // Guard: init requires an interactive terminal
            if !std::io::stdin().is_terminal() {
                anyhow::bail!(
                    "init requires an interactive terminal.\n\
                     Hint: create config.toml manually, or run in an interactive shell."
                );
            }
            if global {
                run_init_global()
            } else {
                let vault_path = match vault {
                    Some(v) => v,
                    None => detect_vault()?,
                };
                run_init(&vault_path)
            }
        }
        Commands::Install { target } => run_install(target),
        Commands::Config { vault, json } => {
            let config = if let Some(ref v) = vault {
                config::Config::new(v)
            } else if let Ok(v) = resolve_vault(None) {
                config::Config::new(&v)
            } else {
                // No vault — build config from global only
                config::Config::new(".")
            };
            if json {
                print_config_json(&config);
            } else {
                print_config_human(&config);
            }
            Ok(())
        }
    }
}

// ─── Vault Resolution ────────────────────────────────────────────────────────

/// Global state persisted at ~/.config/pearl/state.json.
/// Supports multiple registered vaults with a default.
///
/// Format:
/// ```json
/// {
///   "default": "/Users/me/vault-a",
///   "vaults": [
///     "/Users/me/vault-a",
///     "/Users/me/vault-b"
///   ]
/// }
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct GlobalState {
    /// The default vault (used when no --vault is passed)
    #[serde(default)]
    default: Option<String>,
    /// All registered vaults (added on each `init`)
    #[serde(default)]
    vaults: Vec<String>,
}

/// Resolve vault path from explicit argument, or auto-detect.
/// Priority:
/// 1. Explicit --vault argument (already handled by clap/env)
/// 2. Walk up from CWD looking for `.vault-mcp/config.toml` (already initialized vault)
/// 3. Walk up from CWD looking for `.obsidian` dir (vault without local config — uses global config)
/// 4. Default vault from global state (~/.config/pearl/state.json)
fn resolve_vault(explicit: Option<String>) -> Result<String> {
    if let Some(v) = explicit {
        return Ok(v);
    }

    // Walk up from CWD looking for an initialized vault (.vault-mcp/config.toml)
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd.as_path());
        while let Some(d) = dir {
            if d.join(".vault-mcp").join("config.toml").exists() {
                return Ok(d.display().to_string());
            }
            dir = d.parent();
        }
    }

    // Walk up from CWD looking for .obsidian directory (vault relying on global config)
    if config::Config::global_config_exists() {
        if let Ok(cwd) = std::env::current_dir() {
            let mut dir = Some(cwd.as_path());
            while let Some(d) = dir {
                if d.join(".obsidian").is_dir() {
                    return Ok(d.display().to_string());
                }
                dir = d.parent();
            }
        }
    }

    // Try vault_path from global config (~/.config/pearl/config.toml)
    if let Some(path) = config::Config::global_vault_path() {
        return Ok(path);
    }

    // Try global state (default vault)
    let state = load_global_state();
    if let Some(ref path) = state.default {
        return Ok(path.clone());
    }

    anyhow::bail!(
        "No vault specified. Either:\n\
         \x20 • Run from inside a vault directory\n\
         \x20 • Pass --vault <path>\n\
         \x20 • Set VAULT_PATH env var\n\
         \x20 • Run `pearl init` first"
    );
}

/// Load global state from disk.
fn load_global_state() -> GlobalState {
    global_state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

/// Save global state to disk.
fn save_global_state(state: &GlobalState) {
    if let Some(state_path) = global_state_path() {
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(content) = serde_json::to_string_pretty(state) {
            std::fs::write(state_path, content).ok();
        }
    }
}

/// Register a vault in global state (called during `init`).
/// Sets it as default if it's the first, or if it's being re-initialized.
fn register_vault(vault_path: &str) {
    let mut state = load_global_state();
    let path = vault_path.to_string();

    // Add to list if not already present
    if !state.vaults.contains(&path) {
        state.vaults.push(path.clone());
    }

    // Set as default (most recently initialized vault wins)
    state.default = Some(path);

    save_global_state(&state);
}

/// Path to global state: ~/.config/pearl/state.json
fn global_state_path() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("pearl").join("state.json"))
}

/// Detect the Obsidian vault path automatically (interactive, for `init` only).
/// Strategy:
/// 1. Walk up from CWD looking for a `.obsidian` directory
/// 2. Recursively search common folders for `.obsidian` (max depth 4)
/// 3. If multiple found, let user choose; if none found, ask for manual input
fn detect_vault() -> Result<String> {
    use walkdir::WalkDir;

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();

    // Strategy 1: Walk up from CWD
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd.as_path());
        while let Some(d) = dir {
            if d.join(".obsidian").is_dir() {
                candidates.push(d.to_path_buf());
                break;
            }
            dir = d.parent();
        }
    }

    // Strategy 2: Recursively search common folders (max depth 4)
    if let Some(home) = dirs_home() {
        let search_roots = [
            home.join("Documents"),
            home.join("Obsidian"),
            home.join("vaults"),
            home.join("Desktop"),
            home.join("Library/Mobile Documents/iCloud~md~obsidian/Documents"), // iCloud sync
        ];

        // Also check home dir itself (depth 1 only)
        if home.join(".obsidian").is_dir() && !candidates.contains(&home) {
            candidates.push(home.clone());
        }

        for root in &search_roots {
            if !root.is_dir() {
                continue;
            }
            for entry in WalkDir::new(root)
                .max_depth(4)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_name() == ".obsidian" && entry.file_type().is_dir() {
                    if let Some(vault_path) = entry.path().parent() {
                        let path = vault_path.to_path_buf();
                        if !candidates.contains(&path) {
                            candidates.push(path);
                        }
                    }
                }
            }
        }
    }

    match candidates.len() {
        0 => {
            cliclack::log::warning("No Obsidian vaults found on this machine.")?;
            let path: String = cliclack::input("Vault / markdown folder path")
                .placeholder("/path/to/your/vault")
                .interact()?;
            Ok(path)
        }
        1 => {
            let path = candidates[0].display().to_string();
            let confirm: bool = cliclack::confirm(format!("Use vault: {}?", &path))
                .initial_value(true)
                .interact()?;
            if confirm {
                Ok(path)
            } else {
                let path: String = cliclack::input("Vault path")
                    .placeholder("/path/to/your/vault")
                    .interact()?;
                Ok(path)
            }
        }
        _ => {
            let mut select = cliclack::select("Select vault");
            for (i, c) in candidates.iter().enumerate() {
                let label = c.display().to_string();
                select = select.item(i, &label, "");
            }
            let selection: usize = select.interact()?;
            Ok(candidates[selection].display().to_string())
        }
    }
}

/// Get user's home directory.
fn dirs_home() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Interactively collect embedding provider configuration.
fn prompt_embedding_config() -> Result<config::EmbeddingConfig> {
    use config::{EmbeddingConfig, EmbeddingProvider};

    // ── Step 1: Provider ────────────────────────────────────────────────
    let provider: &str = cliclack::select("Embedding provider")
        .item("ollama", "Ollama", "local, free, private")
        .item("openai", "OpenAI", "cloud API, high quality")
        .item("custom", "Custom", "any OpenAI-compatible endpoint")
        .interact()?;

    let provider = match provider {
        "ollama" => EmbeddingProvider::Ollama,
        "openai" => EmbeddingProvider::Openai,
        _ => EmbeddingProvider::Custom,
    };

    // ── Step 2: Provider-specific config ────────────────────────────────
    let embedding_config = match provider {
        EmbeddingProvider::Ollama => {
            // Auto-detect Ollama endpoint
            let detected = crate::core::embedder::detect_ollama_endpoint();
            let default_endpoint = detected
                .as_ref()
                .cloned()
                .unwrap_or_else(|| "http://localhost:11434".into());

            if detected.is_some() {
                cliclack::log::success(format!(
                    "Auto-detected Ollama at {}",
                    &default_endpoint
                ))?;
            } else {
                cliclack::log::warning(
                    "Ollama is not running. Install: https://ollama.com/download",
                )?;
            }

            let endpoint: String = cliclack::input("Endpoint")
                .default_input(&default_endpoint)
                .interact()?;

            let model: String = cliclack::input("Model")
                .default_input("bge-m3")
                .interact()?;

            // Verify model availability if Ollama is reachable
            let tmp_config = EmbeddingConfig {
                provider: EmbeddingProvider::Ollama,
                endpoint: endpoint.clone(),
                model: model.clone(),
                api_key: None,
                dimensions: None,
            };
            let health = crate::core::embedder::check_health(&tmp_config);
            match &health {
                crate::core::embedder::HealthStatus::Ok => {
                    cliclack::log::success(format!(
                        "Ollama is running, model '{}' is available",
                        &model
                    ))?;
                }
                crate::core::embedder::HealthStatus::ModelMissing(_) => {
                    cliclack::log::warning(format!(
                        "Model '{}' not pulled yet. Run: ollama pull {}",
                        &model, &model
                    ))?;
                }
                crate::core::embedder::HealthStatus::Unreachable(_) => {
                    // Already warned above
                }
            }

            tmp_config
        }
        EmbeddingProvider::Openai => {
            let model: String = cliclack::input("Model")
                .default_input("text-embedding-3-small")
                .interact()?;

            let key_source: &str = cliclack::select("API key source")
                .item("env", "Read from $OPENAI_API_KEY env var", "")
                .item("direct", "Enter key now", "")
                .interact()?;

            let api_key = if key_source == "env" {
                "$OPENAI_API_KEY".to_string()
            } else {
                cliclack::input("API key").interact()?
            };

            let dimensions: String = cliclack::input("Dimensions (empty to skip)")
                .default_input("")
                .interact()?;

            let tmp_config = EmbeddingConfig {
                provider: EmbeddingProvider::Openai,
                endpoint: "https://api.openai.com".into(),
                model,
                api_key: Some(api_key),
                dimensions: if dimensions.is_empty() {
                    None
                } else {
                    dimensions.parse().ok()
                },
            };

            // Verify API key connectivity
            let health = crate::core::embedder::check_health(&tmp_config);
            match &health {
                crate::core::embedder::HealthStatus::Ok => {
                    cliclack::log::success(format!(
                        "API key verified, model '{}' is accessible",
                        &tmp_config.model
                    ))?;
                }
                crate::core::embedder::HealthStatus::Unreachable(msg) => {
                    cliclack::log::warning(format!("{}\n  Check your API key and network.", msg))?;
                }
                crate::core::embedder::HealthStatus::ModelMissing(msg) => {
                    cliclack::log::warning(msg)?;
                }
            }

            tmp_config
        }
        EmbeddingProvider::Custom => {
            let endpoint: String = cliclack::input("Endpoint (must serve /v1/embeddings)")
                .placeholder("http://localhost:8080")
                .interact()?;

            let model: String = cliclack::input("Model").interact()?;

            let needs_key: bool = cliclack::confirm("Requires API key?")
                .initial_value(true)
                .interact()?;

            let api_key = if needs_key {
                let key_source: &str = cliclack::select("API key source")
                    .item("env", "Read from env var", "")
                    .item("direct", "Enter key now", "")
                    .interact()?;

                if key_source == "env" {
                    let env_name: String = cliclack::input("Env var name")
                        .default_input("EMBEDDING_API_KEY")
                        .interact()?;
                    Some(format!("${}", env_name))
                } else {
                    let key: String = cliclack::input("API key").interact()?;
                    Some(key)
                }
            } else {
                None
            };

            let dimensions: String = cliclack::input("Dimensions (empty to skip)")
                .default_input("")
                .interact()?;

            let tmp_config = EmbeddingConfig {
                provider: EmbeddingProvider::Custom,
                endpoint,
                model,
                api_key,
                dimensions: if dimensions.is_empty() {
                    None
                } else {
                    dimensions.parse().ok()
                },
            };

            // Verify connectivity
            let health = crate::core::embedder::check_health(&tmp_config);
            match &health {
                crate::core::embedder::HealthStatus::Ok => {
                    cliclack::log::success(format!(
                        "Endpoint verified, model '{}' is accessible",
                        &tmp_config.model
                    ))?;
                }
                crate::core::embedder::HealthStatus::Unreachable(msg) => {
                    cliclack::log::warning(format!(
                        "{}\n  Check your endpoint and API key.",
                        msg
                    ))?;
                }
                crate::core::embedder::HealthStatus::ModelMissing(msg) => {
                    cliclack::log::warning(msg)?;
                }
            }

            tmp_config
        }
    };

    Ok(embedding_config)
}

/// Interactive onboarding: guide user to configure embedding provider (vault-local).
fn run_init(vault_path: &str) -> Result<()> {
    use config::{Config, ConfigFile};

    cliclack::clear_screen()?;
    eprintln!("{}", PEARL_LOGO);
    cliclack::intro("pearl · Setup")?;

    cliclack::log::info(format!("Vault: {}", vault_path))?;

    // Check if config already exists
    if Config::config_exists(vault_path) {
        let overwrite: bool = cliclack::confirm("Config file already exists. Overwrite?")
            .initial_value(false)
            .interact()?;
        if !overwrite {
            cliclack::outro("Aborted.")?;
            return Ok(());
        }
    }

    let embedding_config = prompt_embedding_config()?;

    // ── Save & Summary ──────────────────────────────────────────────────
    let config_file = ConfigFile {
        vault_path: None, // not needed in vault-local config
        embedding: embedding_config.clone(),
        search: None,
        index: None,
    };

    Config::save_config_file(vault_path, &config_file)?;
    register_vault(vault_path);

    cliclack::outro("Configuration saved!")?;

    // Show key config summary (outside cliclack TUI boundary)
    let config = Config::new(vault_path);
    print_config_summary(&config);

    println!("  Next steps:");
    println!("    $ pearl index --vault {}", vault_path);
    println!("    $ pearl serve --vault {}", vault_path);
    println!();

    Ok(())
}

// ─── Install Command ─────────────────────────────────────────────────────────

const INSTALL_TARGETS: &[(&str, &str)] = &[
    ("cursor", "Cursor"),
    ("claude-code", "Claude Code"),
    ("trae", "Trae"),
    ("windsurf", "Windsurf"),
    ("opencode", "OpenCode"),
    ("codex", "Codex"),
];

/// Description of pearl's MCP capabilities (used in rules/skill files).
const VAULT_SEARCH_SKILL_DESCRIPTION: &str = r#"When the user asks to search their notes, find related content, look up something
in their Obsidian vault, or needs context from their knowledge base, use the
pearl MCP server tools.

## pearl tools

- `pearl` — Single tool with a `command` parameter. Commands:
  - `search` — Hybrid semantic + full-text search
    Params: query (required), mode (hybrid|semantic|fts), limit, folders, tags
  - `index` — Build/rebuild the search index
    Params: force (bool, default false)
  - `get` — Read a note by relative path
    Params: path (required)
  - `list` — List files/directories in the vault
    Params: folder, recursive (bool)
  - `status` — Check system health and configuration
"#;

/// Check if a file contains a needle string.
fn file_contains(path: &std::path::Path, needle: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|content| content.contains(needle))
        .unwrap_or(false)
}

/// Detect which editors already have pearl installed.
fn detect_installed_targets(cwd: &std::path::Path) -> Vec<&'static str> {
    let mut installed = Vec::new();

    // Cursor: .cursor/mcp.json
    let cursor_mcp = cwd.join(".cursor").join("mcp.json");
    if file_contains(&cursor_mcp, "pearl") {
        installed.push("cursor");
    }

    // Claude Code: .mcp.json
    let claude_mcp = cwd.join(".mcp.json");
    if file_contains(&claude_mcp, "pearl") {
        installed.push("claude-code");
    }

    // Trae: .trae/mcp.json
    let trae_mcp = cwd.join(".trae").join("mcp.json");
    if file_contains(&trae_mcp, "pearl") {
        installed.push("trae");
    }

    // Windsurf: global mcp_config.json
    if let Some(home) = dirs_home() {
        let windsurf_mcp = home
            .join(".codeium")
            .join("windsurf")
            .join("mcp_config.json");
        if file_contains(&windsurf_mcp, "pearl") {
            installed.push("windsurf");
        }
    }

    // OpenCode: global skill
    if let Some(home) = dirs_home() {
        let opencode_skill = home
            .join(".opencode")
            .join("skills")
            .join("pearl")
            .join("SKILL.md");
        if opencode_skill.exists() {
            installed.push("opencode");
        }
    }

    // Codex: ~/.codex/config.toml
    if let Some(home) = dirs_home() {
        let codex_config = home.join(".codex").join("config.toml");
        if file_contains(&codex_config, "mcp_servers.pearl") {
            installed.push("codex");
        }
    }

    installed
}

/// Generate MCP server JSON block.
fn mcp_server_json(bin_path: &str) -> serde_json::Value {
    serde_json::json!({
        "command": bin_path,
        "args": ["serve"]
    })
}

/// Generate MCP server JSON block with explicit type (for Claude Code).
fn mcp_server_json_typed(bin_path: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": bin_path,
        "args": ["serve"]
    })
}

/// Write/merge MCP config JSON file.
fn write_mcp_config(path: &std::path::Path, bin_path: &str, typed: bool) -> Result<()> {
    let server_entry = if typed {
        mcp_server_json_typed(bin_path)
    } else {
        mcp_server_json(bin_path)
    };

    let mut config: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers object exists
    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }
    config["mcpServers"]["pearl"] = server_entry;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&config)? + "\n")?;
    Ok(())
}

/// Install into Cursor: .cursor/mcp.json + .cursor/rules/pearl.mdc
fn install_cursor(cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    let mcp_path = cwd.join(".cursor").join("mcp.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".cursor").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("pearl.mdc");
    let content = format!(
        "---\ndescription: Use pearl for Obsidian knowledge base queries\nalwaysApply: false\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    cliclack::log::success(format!(
        "Cursor: {} + {}",
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        rules_path.strip_prefix(cwd).unwrap_or(&rules_path).display(),
    ))?;
    Ok(())
}

/// Install into Claude Code: .mcp.json + CLAUDE.md
fn install_claude_code(cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    let mcp_path = cwd.join(".mcp.json");
    write_mcp_config(&mcp_path, bin_path, true)?;

    let claude_md_path = cwd.join("CLAUDE.md");
    let section = format!(
        "\n## pearl — Obsidian Knowledge Base\n\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );

    if claude_md_path.exists() {
        let existing = std::fs::read_to_string(&claude_md_path)?;
        if !existing.contains("pearl") {
            std::fs::write(&claude_md_path, format!("{}\n{}", existing.trim_end(), section))?;
        }
    } else {
        std::fs::write(
            &claude_md_path,
            format!("# Project Instructions\n{}", section),
        )?;
    }

    cliclack::log::success(format!(
        "Claude Code: {} + {}",
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        claude_md_path.strip_prefix(cwd).unwrap_or(&claude_md_path).display(),
    ))?;
    Ok(())
}

/// Install into Trae: .trae/mcp.json + .trae/rules/pearl.md
fn install_trae(cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    let mcp_path = cwd.join(".trae").join("mcp.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".trae").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("pearl.md");
    let content = format!(
        "---\ndescription: Use pearl for Obsidian knowledge base queries\nalwaysApply: false\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    cliclack::log::success(format!(
        "Trae: {} + {}",
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        rules_path.strip_prefix(cwd).unwrap_or(&rules_path).display(),
    ))?;
    Ok(())
}

/// Install into Windsurf: global MCP config + .windsurf/rules/pearl.md
fn install_windsurf(cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let mcp_path = home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".windsurf").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("pearl.md");
    let content = format!(
        "---\ntrigger: model_decision\ndescription: Use pearl for Obsidian knowledge base queries\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    cliclack::log::success(format!(
        "Windsurf: {} (global) + {}",
        mcp_path.display(),
        rules_path.strip_prefix(cwd).unwrap_or(&rules_path).display(),
    ))?;
    Ok(())
}

/// Extract a frontmatter field value from a SKILL.md file.
fn extract_skill_field<'a>(content: &'a str, field: &str) -> Option<&'a str> {
    let fm = content.strip_prefix("---")?;
    let end = fm.find("---")?;
    let frontmatter = &fm[..end];
    let prefix = format!("{}:", field);
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix(&prefix) {
            return Some(v.trim().trim_matches('"').trim_matches('\''));
        }
    }
    None
}

/// Install into OpenCode: global skill at ~/.opencode/skills/pearl/SKILL.md
fn install_opencode(_cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let skill_dir = home.join(".opencode").join("skills").join("pearl");
    std::fs::create_dir_all(&skill_dir)?;

    let skill_path = skill_dir.join("SKILL.md");
    const SKILL_UPDATED_AT: &str = "2025-05-15";

    // Check existing — skip if already current
    if skill_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&skill_path) {
            if let Some(old_date) = extract_skill_field(&existing, "updated_at") {
                if old_date == SKILL_UPDATED_AT {
                    cliclack::log::info(format!(
                        "OpenCode: skill already up-to-date ({})",
                        SKILL_UPDATED_AT
                    ))?;
                    return Ok(());
                }
                cliclack::log::step(format!(
                    "OpenCode: updating skill {} -> {}",
                    old_date, SKILL_UPDATED_AT
                ))?;
            }
        }
    }

    let content = format!(
        r#"---
name: pearl
updated_at: "{updated_at}"
description: Semantic search over Obsidian vaults using pearl CLI. Use when the user asks to search their notes, find related content, look up something in their vault, or needs context from their knowledge base. Supports hybrid (vector + keyword), semantic-only, and full-text search with folder/tag filtering.
---

# pearl

Local-first semantic search for Obsidian vaults. Provides hybrid (vector + full-text) search over markdown notes.

Binary: `{bin_path}` (must be installed and configured via `pearl init`).

## Command reference

```bash
pearl search <QUERY> [--mode hybrid|semantic|fts] [--top-k N] [--json]
pearl index [--force] [--vault PATH]
pearl serve [--vault PATH]
pearl config [--json]
```

## Search modes

| Mode | Description |
|------|-------------|
| `hybrid` | Semantic + full-text combined, unified ranking (default) |
| `semantic` | Vector similarity only |
| `fts` | Keyword matching only |

## Common patterns

```bash
# Search vault (most common)
pearl search --json "your question here"

# More results
pearl search -k 20 --json "error handling patterns"

# Restrict to a folder
pearl search -f wiki/ --json "architecture"

# Filter by tag
pearl search -t project --json "status update"

# Check health
pearl config
```

## Tips

- Default mode is `hybrid` — combines semantic understanding with keyword matching
- Index auto-updates via file watcher when running `serve` or `index`
- Results include file path, relevance score, and matched text chunk
- Use `--json` for machine-readable output
"#,
        updated_at = SKILL_UPDATED_AT,
        bin_path = bin_path,
    );
    std::fs::write(&skill_path, &content)?;

    cliclack::log::success(format!("OpenCode: {}", skill_path.display()))?;
    Ok(())
}

/// Install into Codex: ~/.codex/config.toml + ~/.codex/skills/pearl/SKILL.md
fn install_codex(_cwd: &std::path::Path, bin_path: &str) -> Result<()> {

    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let codex_dir = home.join(".codex");

    // MCP server config in config.toml
    let config_path = codex_dir.join("config.toml");
    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        if !content.contains("mcp_servers.pearl") {
            let mcp_block = format!(
                "\n[mcp_servers.pearl]\ncommand = \"{}\"\nargs = [\"serve\"]\nenabled = true\n",
                bin_path
            );
            std::fs::write(&config_path, format!("{}{}", content, mcp_block))?;
        }
    } else {
        std::fs::create_dir_all(&codex_dir)?;
        let content = format!(
            "[mcp_servers.pearl]\ncommand = \"{}\"\nargs = [\"serve\"]\nenabled = true\n",
            bin_path
        );
        std::fs::write(&config_path, content)?;
    }

    // Skill file (reuse same content as OpenCode)
    let skill_dir = codex_dir.join("skills").join("pearl");
    std::fs::create_dir_all(&skill_dir)?;

    let skill_path = skill_dir.join("SKILL.md");
    const SKILL_UPDATED_AT: &str = "2025-05-15";

    if skill_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&skill_path) {
            if let Some(old_date) = extract_skill_field(&existing, "updated_at") {
                if old_date == SKILL_UPDATED_AT {
                    cliclack::log::info(format!(
                        "Codex: skill already up-to-date ({})",
                        SKILL_UPDATED_AT
                    ))?;
                    return Ok(());
                }
            }
        }
    }

    let content = format!(
        r#"---
name: pearl
updated_at: "{updated_at}"
description: Semantic search over Obsidian vaults using pearl CLI.
---

# pearl

Local-first semantic search for Obsidian vaults.

Binary: `{bin_path}`

## Usage

```bash
pearl search --json "query"
pearl search -m semantic -k 20 --json "query"
pearl search -f folder/ --json "query"
pearl config
```
"#,
        updated_at = SKILL_UPDATED_AT,
        bin_path = bin_path,
    );
    std::fs::write(&skill_path, &content)?;

    cliclack::log::success(format!(
        "Codex: {} + {}",
        config_path.display(),
        skill_path.display(),
    ))?;
    Ok(())
}

/// Main install orchestrator.
fn run_install(targets: Vec<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let already_installed = detect_installed_targets(&cwd);

    let selected: Vec<&str> = if targets.is_empty() {
        // Interactive mode
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(
                "install requires --target in non-interactive mode.\n\
                 Example: pearl install --target cursor,claude-code"
            );
        }

        cliclack::clear_screen()?;
        cliclack::intro("pearl · Install")?;

        let mut multi = cliclack::multiselect("Which agents to install into?");
        for (id, label) in INSTALL_TARGETS {
            let hint = if already_installed.contains(id) {
                "installed"
            } else {
                ""
            };
            multi = multi.item(*id, *label, hint);
        }
        multi = multi.initial_values(already_installed.clone());
        let selections: Vec<&str> = multi.interact()?;

        if selections.is_empty() {
            anyhow::bail!("No target selected.");
        }

        selections
    } else {
        // Validate targets
        for t in &targets {
            if !INSTALL_TARGETS.iter().any(|(id, _)| *id == t.as_str()) {
                anyhow::bail!(
                    "Unknown target: '{}'. Supported: cursor, claude-code, trae, windsurf, opencode, codex",
                    t
                );
            }
        }

        cliclack::intro("pearl · Install")?;

        targets.iter().map(|s| s.as_str()).collect()
    };

    // Only install targets not already configured
    let new_targets: Vec<&str> = selected
        .into_iter()
        .filter(|t| !already_installed.contains(t))
        .collect();

    if new_targets.is_empty() {
        cliclack::outro("All selected agents already have pearl installed.")?;
        return Ok(());
    }

    // Resolve binary path
    let bin_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "pearl".to_string());

    for target in &new_targets {
        match *target {
            "cursor" => install_cursor(&cwd, &bin_path)?,
            "claude-code" => install_claude_code(&cwd, &bin_path)?,
            "trae" => install_trae(&cwd, &bin_path)?,
            "windsurf" => install_windsurf(&cwd, &bin_path)?,
            "opencode" => install_opencode(&cwd, &bin_path)?,
            "codex" => install_codex(&cwd, &bin_path)?,
            _ => continue,
        }
    }

    let summary = new_targets
        .iter()
        .map(|t| format!("  {} installed", t))
        .collect::<Vec<_>>()
        .join("\n");
    cliclack::outro(format!("Done!\n{}", summary))?;

    Ok(())
}

// ─── Config Command ──────────────────────────────────────────────────────────

/// Print a concise config summary (used after init).
fn print_config_summary(config: &config::Config) {
    use console::style;

    println!();
    println!("    {}    {}", style("vault").dim(), config.vault_path.display());
    println!("    {} {}", style("provider").dim(), config.embedding.provider);
    println!("    {} {}", style("endpoint").dim(), config.embedding.endpoint);
    println!("    {}    {}", style("model").dim(), config.embedding.model);
    println!();
}

/// Print effective config in human-readable format.
fn print_config_human(config: &config::Config) {
    use console::style;

    println!();
    println!("  {}", style("Effective Configuration").bold());
    println!("  {}", style("─".repeat(40)).dim());
    println!();

    // Local search
    println!("  {}", style("[Embedding]").bold());
    println!("    {}    {}", style("vault").dim(), config.vault_path.display());
    println!(
        "    {} {}",
        style("provider").dim(),
        config.embedding.provider
    );
    println!(
        "    {} {}",
        style("endpoint").dim(),
        config.embedding.endpoint
    );
    println!("    {}    {}", style("model").dim(), config.embedding.model);
    if let Some(dim) = config.embedding.dimensions {
        println!("    {}     {}", style("dims").dim(), dim);
    }
    let key_status = if config.embedding.resolve_api_key().is_some() {
        style("configured").green().to_string()
    } else if config.embedding.api_key.is_some() {
        style("set but unresolvable").yellow().to_string()
    } else {
        style("none").dim().to_string()
    };
    println!("    {}  {}", style("api_key").dim(), key_status);

    println!();
    println!("  {}", style("[Search]").bold());
    println!(
        "    {}  vector={:.1}, fts={:.1}",
        style("weights").dim(),
        config.search.vector_weight,
        config.search.fts_weight
    );
    println!(
        "    {}    {}",
        style("limit").dim(),
        config.search.default_limit
    );

    println!();
    println!("  {}", style("[Index]").bold());
    println!(
        "    {} {}",
        style("data_dir").dim(),
        config.index.data_dir.display()
    );
    println!(
        "    {}   {} tokens",
        style("chunks").dim(),
        config.index.max_chunk_tokens
    );

    println!();
    println!("  {}", style("[Files]").bold());
    if let Some(global_path) = config::Config::global_config_path() {
        let exists = global_path.exists();
        println!(
            "    {}   {} {}",
            style("global").dim(),
            global_path.display(),
            if exists { "" } else { "(not found)" }
        );
    }
    let local_path = config.vault_path.join(".vault-mcp").join("config.toml");
    let local_exists = local_path.exists();
    println!(
        "    {}    {} {}",
        style("local").dim(),
        local_path.display(),
        if local_exists { "" } else { "(not found)" }
    );
    println!();
}

/// Print effective config as JSON.
fn print_config_json(config: &config::Config) {
    let output = serde_json::json!({
        "vault_path": config.vault_path.display().to_string(),
        "embedding": {
            "provider": config.embedding.provider.to_string(),
            "endpoint": config.embedding.endpoint,
            "model": config.embedding.model,
            "dimensions": config.embedding.dimensions,
            "api_key_configured": config.embedding.resolve_api_key().is_some(),
        },
        "search": {
            "vector_weight": config.search.vector_weight,
            "fts_weight": config.search.fts_weight,
            "default_limit": config.search.default_limit,
        },
        "index": {
            "data_dir": config.index.data_dir.display().to_string(),
            "max_chunk_tokens": config.index.max_chunk_tokens,
        },
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&output).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
    );
}

/// Interactive onboarding: write config to the global path (~/.config/pearl/config.toml).
fn run_init_global() -> Result<()> {
    use config::{Config, ConfigFile};

    let config_path = Config::global_config_path()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine global config directory"))?;

    cliclack::clear_screen()?;
    cliclack::intro("pearl · Global Setup")?;

    cliclack::log::info(format!(
        "Config: {}\nApplies to all vaults unless overridden locally.",
        config_path.display()
    ))?;

    // Check if global config already exists
    if Config::global_config_exists() {
        let overwrite: bool = cliclack::confirm("Global config already exists. Overwrite?")
            .initial_value(false)
            .interact()?;
        if !overwrite {
            cliclack::outro("Aborted.")?;
            return Ok(());
        }
    }

    // ── Vault path ──────────────────────────────────────────────────────
    let default_vault = detect_vault().ok().unwrap_or_default();
    let vault_path: String = cliclack::input("Default vault path")
        .default_input(&default_vault)
        .interact()?;
    let vault_path = if vault_path.is_empty() {
        None
    } else {
        Some(vault_path)
    };

    let embedding_config = prompt_embedding_config()?;

    // ── Save & Summary ──────────────────────────────────────────────────
    let config_file = ConfigFile {
        vault_path: vault_path.clone(),
        embedding: embedding_config.clone(),
        search: None,
        index: None,
    };

    Config::save_global_config_file(&config_file)?;

    cliclack::outro("Global configuration saved!")?;

    // Show key config summary (outside cliclack TUI boundary)
    let effective_vault = vault_path.as_deref().unwrap_or(".");
    let config = Config::new(effective_vault);
    print_config_summary(&config);

    Ok(())
}
