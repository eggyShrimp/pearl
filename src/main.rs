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

#[derive(Parser)]
#[command(
    name = "vault-search",
    version,
    about = "Semantic search for Obsidian vaults, exposed as an MCP server for AI agents.",
    long_about = "vault-search provides hybrid semantic + full-text search over your Obsidian vault.\n\n\
        It embeds your notes using a local (Ollama) or cloud (OpenAI) model, stores vectors\n\
        alongside your vault, and serves search results via the Model Context Protocol (MCP)\n\
        for AI coding agents like Claude Desktop, Cursor, or OpenCode.\n\n\
        Quick start:\n\
        \x20 1. vault-search init              # configure embedding provider\n\
        \x20 2. vault-search index             # build the search index\n\
        \x20 3. vault-search serve             # start MCP server for your agent\n\n\
        All index data is stored locally in {vault}/.vault-mcp/.",
    after_help = "Documentation: https://github.com/eggyShrimp/vault-search"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server for AI agents to connect to.
    ///
    /// Exposes a single `vault_search` tool with commands: search, index, get, list, status.
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
    /// Use --global to write to ~/.config/vault-search/config.toml instead,
    /// which serves as the default config for all vaults.
    Init {
        /// Path to the Obsidian vault (auto-detected if omitted)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        /// Write config to global path (~/.config/vault-search/config.toml) instead of vault-local
        #[arg(short, long, default_value_t = false)]
        global: bool,
    },

    /// Install vault-search as a tool/skill into AI coding agents.
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
                .with_env_filter("vault_search_mcp=info")
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
                .with_env_filter("vault_search_mcp=info")
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
                            style("vault-search init").bold()
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
                .with_env_filter("vault_search_mcp=info")
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
                .with_env_filter("vault_search_mcp=info")
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

/// Global state persisted at ~/.config/vault-search/state.json.
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
/// 4. Default vault from global state (~/.config/vault-search/state.json)
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

    // Try vault_path from global config (~/.config/vault-search/config.toml)
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
         \x20 • Run `vault-search init` first"
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

/// Path to global state: ~/.config/vault-search/state.json
fn global_state_path() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("vault-search").join("state.json"))
}

/// Detect the Obsidian vault path automatically (interactive, for `init` only).
/// Strategy:
/// 1. Walk up from CWD looking for a `.obsidian` directory
/// 2. Recursively search common folders for `.obsidian` (max depth 4)
/// 3. If multiple found, let user choose; if none found, ask for manual input
fn detect_vault() -> Result<String> {
    use console::style;
    use dialoguer::{Input, Select, theme::ColorfulTheme};
    use walkdir::WalkDir;

    let theme = ColorfulTheme::default();
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
            eprintln!(
                "  {} No Obsidian vaults found on this machine.",
                style("⚠").yellow().bold()
            );
            eprintln!();
            eprintln!("  vault-search works with any folder of markdown files.");
            eprintln!("  If you don't have Obsidian yet:");
            eprintln!();
            eprintln!(
                "    • Download: {}",
                style("https://obsidian.md/download").underlined()
            );
            eprintln!("    • Or point to any folder containing .md files");
            eprintln!();
            let path: String = Input::with_theme(&theme)
                .with_prompt("  Vault / markdown folder path")
                .interact_text()?;
            Ok(path)
        }
        1 => {
            let path = candidates[0].display().to_string();
            eprintln!(
                "  {}  Found vault: {}",
                style("✓").green().bold(),
                style(&path).underlined()
            );
            println!();
            let confirm = dialoguer::Confirm::with_theme(&theme)
                .with_prompt("  Use this vault?")
                .default(true)
                .interact()?;
            if confirm {
                Ok(path)
            } else {
                let path: String = Input::with_theme(&theme)
                    .with_prompt("  Vault path")
                    .interact_text()?;
                Ok(path)
            }
        }
        _ => {
            eprintln!(
                "  {}  Found {} vaults:",
                style("✓").green().bold(),
                candidates.len()
            );
            println!();
            let items: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
            let selection = Select::with_theme(&theme)
                .with_prompt("  Select vault")
                .items(&items)
                .default(0)
                .interact()?;
            Ok(items[selection].clone())
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
    use console::style;
    use dialoguer::{Input, Select, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();

    // ── Step 1: Provider ────────────────────────────────────────────────
    println!(
        "  {} {}",
        style("[1/2]").dim(),
        style("Embedding Provider").bold()
    );
    println!();

    let providers = &[
        "Ollama          local, free, private",
        "OpenAI          cloud API, high quality",
        "Custom          any OpenAI-compatible endpoint",
    ];
    let selection = Select::with_theme(&theme)
        .with_prompt("  Provider")
        .items(providers)
        .default(0)
        .interact()?;

    let provider = match selection {
        0 => EmbeddingProvider::Ollama,
        1 => EmbeddingProvider::Openai,
        _ => EmbeddingProvider::Custom,
    };

    // ── Step 2: Provider-specific config ────────────────────────────────
    println!();
    println!(
        "  {} {}",
        style("[2/2]").dim(),
        style("Connection Details").bold()
    );
    println!();

    let embedding_config = match provider {
        EmbeddingProvider::Ollama => {
            // Auto-detect Ollama endpoint
            let detected = crate::core::embedder::detect_ollama_endpoint();
            let default_endpoint = detected
                .as_ref()
                .cloned()
                .unwrap_or_else(|| "http://localhost:11434".into());

            if detected.is_some() {
                eprintln!(
                    "  {}  Auto-detected Ollama at {}",
                    style("✓").green().bold(),
                    style(&default_endpoint).underlined()
                );
            } else {
                eprintln!();
                eprintln!(
                    "  {} Ollama is not running or not installed.",
                    style("⚠").yellow().bold()
                );
                eprintln!("  To use local embeddings, you need Ollama:");
                eprintln!();
                eprintln!(
                    "    1. Install:  {}",
                    style("https://ollama.com/download").underlined()
                );
                eprintln!("    2. Start:    {}", style("ollama serve").bold());
                eprintln!("    3. Pull model: {}", style("ollama pull bge-m3").bold());
                eprintln!();
                eprintln!(
                    "  You can continue setup now and start Ollama before running {}.",
                    style("index").bold()
                );
                eprintln!();
            }

            let endpoint: String = Input::with_theme(&theme)
                .with_prompt("  Endpoint")
                .default(default_endpoint)
                .interact_text()?;

            let model: String = Input::with_theme(&theme)
                .with_prompt("  Model")
                .default("bge-m3".into())
                .interact_text()?;

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
                    eprintln!(
                        "  {}  Ollama is running, model '{}' is available.",
                        style("✓").green().bold(),
                        &model
                    );
                }
                crate::core::embedder::HealthStatus::ModelMissing(_) => {
                    eprintln!();
                    eprintln!(
                        "  {} Model '{}' is not pulled yet.",
                        style("⚠").yellow().bold(),
                        &model
                    );
                    eprintln!(
                        "    Run: {}",
                        style(format!("ollama pull {}", &model)).bold()
                    );
                    eprintln!();
                }
                crate::core::embedder::HealthStatus::Unreachable(_) => {
                    // Already warned above
                }
            }

            tmp_config
        }
        EmbeddingProvider::Openai => {
            let model: String = Input::with_theme(&theme)
                .with_prompt("  Model")
                .default("text-embedding-3-small".into())
                .interact_text()?;

            let api_key_source = Select::with_theme(&theme)
                .with_prompt("  API key source")
                .items(&["Read from $OPENAI_API_KEY env var", "Enter key now"])
                .default(0)
                .interact()?;

            let api_key = if api_key_source == 0 {
                "$OPENAI_API_KEY".to_string()
            } else {
                Input::with_theme(&theme)
                    .with_prompt("  API key")
                    .interact_text()?
            };

            let dimensions: String = Input::with_theme(&theme)
                .with_prompt("  Dimensions (enter to skip)")
                .default("".into())
                .allow_empty(true)
                .interact_text()?;

            let tmp_config = EmbeddingConfig {
                provider: EmbeddingProvider::Openai,
                endpoint: "https://api.openai.com".into(),
                model,
                api_key: Some(api_key),
                dimensions: dimensions.parse().ok(),
            };

            // Verify API key connectivity
            let health = crate::core::embedder::check_health(&tmp_config);
            match &health {
                crate::core::embedder::HealthStatus::Ok => {
                    eprintln!(
                        "  {}  API key verified, model '{}' is accessible.",
                        style("✓").green().bold(),
                        &tmp_config.model
                    );
                }
                crate::core::embedder::HealthStatus::Unreachable(msg) => {
                    eprintln!();
                    eprintln!("  {} {}", style("⚠").yellow().bold(), msg);
                    eprintln!("    Check your API key and network connectivity.");
                    eprintln!();
                }
                crate::core::embedder::HealthStatus::ModelMissing(msg) => {
                    eprintln!();
                    eprintln!("  {} {}", style("⚠").yellow().bold(), msg);
                    eprintln!();
                }
            }

            tmp_config
        }
        EmbeddingProvider::Custom => {
            let endpoint: String = Input::with_theme(&theme)
                .with_prompt("  Endpoint (must serve /v1/embeddings)")
                .interact_text()?;

            let model: String = Input::with_theme(&theme)
                .with_prompt("  Model")
                .interact_text()?;

            let needs_key = dialoguer::Confirm::with_theme(&theme)
                .with_prompt("  Requires API key?")
                .default(true)
                .interact()?;

            let api_key = if needs_key {
                let key_source = Select::with_theme(&theme)
                    .with_prompt("  API key source")
                    .items(&["Read from env var", "Enter key now"])
                    .default(0)
                    .interact()?;

                if key_source == 0 {
                    let env_name: String = Input::with_theme(&theme)
                        .with_prompt("  Env var name")
                        .default("EMBEDDING_API_KEY".into())
                        .interact_text()?;
                    Some(format!("${}", env_name))
                } else {
                    let key: String = Input::with_theme(&theme)
                        .with_prompt("  API key")
                        .interact_text()?;
                    Some(key)
                }
            } else {
                None
            };

            let dimensions: String = Input::with_theme(&theme)
                .with_prompt("  Dimensions (enter to skip)")
                .default("".into())
                .allow_empty(true)
                .interact_text()?;

            let tmp_config = EmbeddingConfig {
                provider: EmbeddingProvider::Custom,
                endpoint,
                model,
                api_key,
                dimensions: dimensions.parse().ok(),
            };

            // Verify connectivity
            let health = crate::core::embedder::check_health(&tmp_config);
            match &health {
                crate::core::embedder::HealthStatus::Ok => {
                    eprintln!(
                        "  {}  Endpoint verified, model '{}' is accessible.",
                        style("✓").green().bold(),
                        &tmp_config.model
                    );
                }
                crate::core::embedder::HealthStatus::Unreachable(msg) => {
                    eprintln!();
                    eprintln!("  {} {}", style("⚠").yellow().bold(), msg);
                    eprintln!("    Check your endpoint and API key configuration.");
                    eprintln!();
                }
                crate::core::embedder::HealthStatus::ModelMissing(msg) => {
                    eprintln!();
                    eprintln!("  {} {}", style("⚠").yellow().bold(), msg);
                    eprintln!();
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
    use console::style;
    use dialoguer::theme::ColorfulTheme;

    let theme = ColorfulTheme::default();

    println!();
    println!("  {}", style("vault-search · Setup").bold());
    println!("  {}", style("─".repeat(40)).dim());
    println!("  Vault: {}", style(vault_path).cyan().underlined());
    println!();

    // Check if config already exists
    if Config::config_exists(vault_path) {
        eprintln!(
            "  {} Config file already exists.",
            style("!").yellow().bold()
        );
        let overwrite = dialoguer::Confirm::with_theme(&theme)
            .with_prompt("  Overwrite existing config?")
            .default(false)
            .interact()?;
        if !overwrite {
            println!("  Aborted.");
            return Ok(());
        }
        println!();
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

    let config_path = Config::config_path(vault_path);
    println!();
    println!("  {}", style("─".repeat(40)).dim());
    println!("  {} Configuration saved!", style("✓").green().bold());
    println!();
    println!(
        "  {}  {}",
        style("provider").dim(),
        embedding_config.provider
    );
    println!(
        "  {}  {}",
        style("endpoint").dim(),
        embedding_config.endpoint
    );
    println!("  {}     {}", style("model").dim(), embedding_config.model);
    println!(
        "  {}    {}",
        style("config").dim(),
        style(config_path.display()).underlined()
    );

    // Show global config info if it exists
    if Config::global_config_exists() {
        if let Some(global_path) = Config::global_config_path() {
            println!(
                "  {}    {}",
                style("global").dim(),
                style(global_path.display()).underlined()
            );
            println!(
                "           {}",
                style("(vault-local config overrides global)").dim()
            );
        }
    }

    println!();
    println!("  {}", style("Next steps:").bold());
    println!();
    println!(
        "    {}  vault-search index --vault {}",
        style("$").dim(),
        vault_path
    );
    println!(
        "    {}  vault-search serve --vault {}",
        style("$").dim(),
        vault_path
    );
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

/// Description of vault-search's MCP capabilities (used in rules/skill files).
const VAULT_SEARCH_SKILL_DESCRIPTION: &str = r#"When the user asks to search their notes, find related content, look up something
in their Obsidian vault, or needs context from their knowledge base, use the
vault-search MCP server tools.

## vault-search tools

- `vault_search` — Single tool with a `command` parameter. Commands:
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

/// Detect which editors already have vault-search installed.
fn detect_installed_targets(cwd: &std::path::Path) -> Vec<&'static str> {
    let mut installed = Vec::new();

    // Cursor: .cursor/mcp.json
    let cursor_mcp = cwd.join(".cursor").join("mcp.json");
    if file_contains(&cursor_mcp, "vault-search") {
        installed.push("cursor");
    }

    // Claude Code: .mcp.json
    let claude_mcp = cwd.join(".mcp.json");
    if file_contains(&claude_mcp, "vault-search") {
        installed.push("claude-code");
    }

    // Trae: .trae/mcp.json
    let trae_mcp = cwd.join(".trae").join("mcp.json");
    if file_contains(&trae_mcp, "vault-search") {
        installed.push("trae");
    }

    // Windsurf: global mcp_config.json
    if let Some(home) = dirs_home() {
        let windsurf_mcp = home
            .join(".codeium")
            .join("windsurf")
            .join("mcp_config.json");
        if file_contains(&windsurf_mcp, "vault-search") {
            installed.push("windsurf");
        }
    }

    // OpenCode: global skill
    if let Some(home) = dirs_home() {
        let opencode_skill = home
            .join(".opencode")
            .join("skills")
            .join("vault-search")
            .join("SKILL.md");
        if opencode_skill.exists() {
            installed.push("opencode");
        }
    }

    // Codex: ~/.codex/config.toml
    if let Some(home) = dirs_home() {
        let codex_config = home.join(".codex").join("config.toml");
        if file_contains(&codex_config, "mcp_servers.vault-search") {
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
    config["mcpServers"]["vault-search"] = server_entry;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&config)? + "\n")?;
    Ok(())
}

/// Install into Cursor: .cursor/mcp.json + .cursor/rules/vault-search.mdc
fn install_cursor(cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    use console::style;

    let mcp_path = cwd.join(".cursor").join("mcp.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".cursor").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("vault-search.mdc");
    let content = format!(
        "---\ndescription: Use vault-search for Obsidian knowledge base queries\nalwaysApply: false\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    eprintln!(
        "  {} Cursor: {} + {}",
        style("✓").green().bold(),
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        rules_path
            .strip_prefix(cwd)
            .unwrap_or(&rules_path)
            .display(),
    );
    Ok(())
}

/// Install into Claude Code: .mcp.json + CLAUDE.md
fn install_claude_code(cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    use console::style;

    let mcp_path = cwd.join(".mcp.json");
    write_mcp_config(&mcp_path, bin_path, true)?;

    let claude_md_path = cwd.join("CLAUDE.md");
    let section = format!(
        "\n## vault-search — Obsidian Knowledge Base\n\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );

    if claude_md_path.exists() {
        let existing = std::fs::read_to_string(&claude_md_path)?;
        if !existing.contains("vault-search") {
            std::fs::write(&claude_md_path, format!("{}\n{}", existing.trim_end(), section))?;
        }
    } else {
        std::fs::write(
            &claude_md_path,
            format!("# Project Instructions\n{}", section),
        )?;
    }

    eprintln!(
        "  {} Claude Code: {} + {}",
        style("✓").green().bold(),
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        claude_md_path
            .strip_prefix(cwd)
            .unwrap_or(&claude_md_path)
            .display(),
    );
    Ok(())
}

/// Install into Trae: .trae/mcp.json + .trae/rules/vault-search.md
fn install_trae(cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    use console::style;

    let mcp_path = cwd.join(".trae").join("mcp.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".trae").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("vault-search.md");
    let content = format!(
        "---\ndescription: Use vault-search for Obsidian knowledge base queries\nalwaysApply: false\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    eprintln!(
        "  {} Trae: {} + {}",
        style("✓").green().bold(),
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        rules_path
            .strip_prefix(cwd)
            .unwrap_or(&rules_path)
            .display(),
    );
    Ok(())
}

/// Install into Windsurf: global MCP config + .windsurf/rules/vault-search.md
fn install_windsurf(cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    use console::style;

    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let mcp_path = home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".windsurf").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("vault-search.md");
    let content = format!(
        "---\ntrigger: model_decision\ndescription: Use vault-search for Obsidian knowledge base queries\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    eprintln!(
        "  {} Windsurf: {} (global) + {}",
        style("✓").green().bold(),
        mcp_path.display(),
        rules_path
            .strip_prefix(cwd)
            .unwrap_or(&rules_path)
            .display(),
    );
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

/// Install into OpenCode: global skill at ~/.opencode/skills/vault-search/SKILL.md
fn install_opencode(_cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    use console::style;

    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let skill_dir = home.join(".opencode").join("skills").join("vault-search");
    std::fs::create_dir_all(&skill_dir)?;

    let skill_path = skill_dir.join("SKILL.md");
    const SKILL_UPDATED_AT: &str = "2025-05-15";

    // Check existing — skip if already current
    if skill_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&skill_path) {
            if let Some(old_date) = extract_skill_field(&existing, "updated_at") {
                if old_date == SKILL_UPDATED_AT {
                    eprintln!(
                        "  {} OpenCode: skill already up-to-date ({})",
                        style("·").dim(),
                        SKILL_UPDATED_AT
                    );
                    return Ok(());
                }
                eprintln!(
                    "  {} OpenCode: updating skill {} -> {}",
                    style("↑").cyan(),
                    old_date,
                    SKILL_UPDATED_AT
                );
            }
        }
    }

    let content = format!(
        r#"---
name: vault-search
updated_at: "{updated_at}"
description: Semantic search over Obsidian vaults using vault-search CLI. Use when the user asks to search their notes, find related content, look up something in their vault, or needs context from their knowledge base. Supports hybrid (vector + keyword), semantic-only, and full-text search with folder/tag filtering.
---

# vault-search

Local-first semantic search for Obsidian vaults. Provides hybrid (vector + full-text) search over markdown notes.

Binary: `{bin_path}` (must be installed and configured via `vault-search init`).

## Command reference

```bash
vault-search search <QUERY> [--mode hybrid|semantic|fts] [--top-k N] [--json]
vault-search index [--force] [--vault PATH]
vault-search serve [--vault PATH]
vault-search config [--json]
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
vault-search search --json "your question here"

# More results
vault-search search -k 20 --json "error handling patterns"

# Restrict to a folder
vault-search search -f wiki/ --json "architecture"

# Filter by tag
vault-search search -t project --json "status update"

# Check health
vault-search config
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

    eprintln!(
        "  {} OpenCode: {}",
        style("✓").green().bold(),
        skill_path.display(),
    );
    Ok(())
}

/// Install into Codex: ~/.codex/config.toml + ~/.codex/skills/vault-search/SKILL.md
fn install_codex(_cwd: &std::path::Path, bin_path: &str) -> Result<()> {
    use console::style;

    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let codex_dir = home.join(".codex");

    // MCP server config in config.toml
    let config_path = codex_dir.join("config.toml");
    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        if !content.contains("mcp_servers.vault-search") {
            let mcp_block = format!(
                "\n[mcp_servers.vault-search]\ncommand = \"{}\"\nargs = [\"serve\"]\nenabled = true\n",
                bin_path
            );
            std::fs::write(&config_path, format!("{}{}", content, mcp_block))?;
        }
    } else {
        std::fs::create_dir_all(&codex_dir)?;
        let content = format!(
            "[mcp_servers.vault-search]\ncommand = \"{}\"\nargs = [\"serve\"]\nenabled = true\n",
            bin_path
        );
        std::fs::write(&config_path, content)?;
    }

    // Skill file (reuse same content as OpenCode)
    let skill_dir = codex_dir.join("skills").join("vault-search");
    std::fs::create_dir_all(&skill_dir)?;

    let skill_path = skill_dir.join("SKILL.md");
    const SKILL_UPDATED_AT: &str = "2025-05-15";

    if skill_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&skill_path) {
            if let Some(old_date) = extract_skill_field(&existing, "updated_at") {
                if old_date == SKILL_UPDATED_AT {
                    eprintln!(
                        "  {} Codex: skill already up-to-date ({})",
                        style("·").dim(),
                        SKILL_UPDATED_AT
                    );
                    return Ok(());
                }
            }
        }
    }

    let content = format!(
        r#"---
name: vault-search
updated_at: "{updated_at}"
description: Semantic search over Obsidian vaults using vault-search CLI.
---

# vault-search

Local-first semantic search for Obsidian vaults.

Binary: `{bin_path}`

## Usage

```bash
vault-search search --json "query"
vault-search search -m semantic -k 20 --json "query"
vault-search search -f folder/ --json "query"
vault-search config
```
"#,
        updated_at = SKILL_UPDATED_AT,
        bin_path = bin_path,
    );
    std::fs::write(&skill_path, &content)?;

    eprintln!(
        "  {} Codex: {} + {}",
        style("✓").green().bold(),
        config_path.display(),
        skill_path.display(),
    );
    Ok(())
}

/// Main install orchestrator.
fn run_install(targets: Vec<String>) -> Result<()> {
    use console::style;
    use dialoguer::MultiSelect;
    use dialoguer::theme::ColorfulTheme;

    let cwd = std::env::current_dir()?;
    let already_installed = detect_installed_targets(&cwd);

    let selected: Vec<&str> = if targets.is_empty() {
        // Interactive mode
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(
                "install requires --target in non-interactive mode.\n\
                 Example: vault-search install --target cursor,claude-code"
            );
        }

        let theme = ColorfulTheme::default();
        println!();
        println!("  {}", style("vault-search · Install").bold());
        println!("  {}", style("─".repeat(40)).dim());
        println!();

        let items: Vec<String> = INSTALL_TARGETS
            .iter()
            .map(|(id, label)| {
                if already_installed.contains(id) {
                    format!("{} (installed)", label)
                } else {
                    label.to_string()
                }
            })
            .collect();

        // Pre-select already-installed items
        let defaults: Vec<bool> = INSTALL_TARGETS
            .iter()
            .map(|(id, _)| already_installed.contains(id))
            .collect();

        let selections = MultiSelect::with_theme(&theme)
            .with_prompt("  Which agents to install into?")
            .items(&items)
            .defaults(&defaults)
            .interact()?;

        if selections.is_empty() {
            anyhow::bail!("No target selected.");
        }

        selections
            .into_iter()
            .map(|i| INSTALL_TARGETS[i].0)
            .collect()
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

        println!();
        println!("  {}", style("vault-search · Install").bold());
        println!("  {}", style("─".repeat(40)).dim());
        println!();

        targets.iter().map(|s| s.as_str()).collect()
    };

    // Only install targets not already configured
    let new_targets: Vec<&str> = selected
        .into_iter()
        .filter(|t| !already_installed.contains(t))
        .collect();

    if new_targets.is_empty() {
        println!(
            "  {} All selected agents already have vault-search installed.",
            style("✓").green().bold()
        );
        println!();
        return Ok(());
    }

    // Resolve binary path
    let bin_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "vault-search".to_string());

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

    println!();
    println!("  {} Done!", style("✓").green().bold());
    println!();

    Ok(())
}

// ─── Config Command ──────────────────────────────────────────────────────────

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

/// Interactive onboarding: write config to the global path (~/.config/vault-search/config.toml).
fn run_init_global() -> Result<()> {
    use config::{Config, ConfigFile};
    use console::style;
    use dialoguer::{Input, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();

    let config_path = Config::global_config_path()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine global config directory"))?;

    println!();
    println!("  {}", style("vault-search · Global Setup").bold());
    println!("  {}", style("─".repeat(40)).dim());
    println!(
        "  Config: {}",
        style(config_path.display()).cyan().underlined()
    );
    println!();
    println!(
        "  {}",
        style("This config applies to all vaults unless overridden locally.").dim()
    );
    println!();

    // Check if global config already exists
    if Config::global_config_exists() {
        eprintln!(
            "  {} Global config already exists.",
            style("!").yellow().bold()
        );
        let overwrite = dialoguer::Confirm::with_theme(&theme)
            .with_prompt("  Overwrite existing global config?")
            .default(false)
            .interact()?;
        if !overwrite {
            println!("  Aborted.");
            return Ok(());
        }
        println!();
    }

    // ── Vault path ──────────────────────────────────────────────────────
    // Try to auto-detect a default vault path for the prompt
    let default_vault = detect_vault().ok().unwrap_or_default();
    let vault_path: String = Input::with_theme(&theme)
        .with_prompt("  Default vault path")
        .default(default_vault)
        .interact_text()?;
    let vault_path = if vault_path.is_empty() {
        None
    } else {
        Some(vault_path)
    };
    println!();

    let embedding_config = prompt_embedding_config()?;

    // ── Save & Summary ──────────────────────────────────────────────────
    let config_file = ConfigFile {
        vault_path: vault_path.clone(),
        embedding: embedding_config.clone(),
        search: None,
        index: None,
    };

    Config::save_global_config_file(&config_file)?;

    println!();
    println!("  {}", style("─".repeat(40)).dim());
    println!(
        "  {} Global configuration saved!",
        style("✓").green().bold()
    );
    println!();
    if let Some(ref vp) = vault_path {
        println!("  {}     {}", style("vault").dim(), style(vp).underlined());
    }
    println!(
        "  {}  {}",
        style("provider").dim(),
        embedding_config.provider
    );
    println!(
        "  {}  {}",
        style("endpoint").dim(),
        embedding_config.endpoint
    );
    println!("  {}     {}", style("model").dim(), embedding_config.model);
    println!(
        "  {}    {}",
        style("config").dim(),
        style(config_path.display()).underlined()
    );
    println!();
    println!(
        "  {}",
        style("All vaults will use this config unless they have a local .vault-mcp/config.toml.")
            .dim()
    );
    println!();

    Ok(())
}
