mod config;
mod core;
mod indexer;
mod search;
mod server;

use std::io::IsTerminal;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "vault-search-mcp",
    version,
    about = "Semantic search for Obsidian vaults, exposed as an MCP server for AI agents.",
    long_about = "vault-search-mcp provides hybrid semantic + full-text search over your Obsidian vault.\n\n\
        It embeds your notes using a local (Ollama) or cloud (OpenAI) model, stores vectors\n\
        alongside your vault, and serves search results via the Model Context Protocol (MCP)\n\
        for AI coding agents like Claude Desktop, Cursor, or OpenCode.\n\n\
        Quick start:\n\
        \x20 1. vault-search-mcp init              # configure embedding provider\n\
        \x20 2. vault-search-mcp index             # build the search index\n\
        \x20 3. vault-search-mcp serve             # start MCP server for your agent\n\n\
        All index data is stored locally in {vault}/.vault-mcp/.",
    after_help = "Documentation: https://github.com/user/vault-search-mcp"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server for AI agents to connect to (stdio transport).
    ///
    /// The server exposes tools: hybrid_search, index_vault, get_note, list_notes, vault_status.
    /// AI agents communicate via JSON-RPC over stdin/stdout.
    Serve {
        /// Path to the Obsidian vault (resolved automatically if omitted)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
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
        /// Maximum number of results to return
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
        /// Output as JSON (machine-readable, pipe-friendly)
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Interactive setup wizard — configure embedding provider and generate config.
    ///
    /// Detects your vault location and Ollama endpoint automatically.
    /// Writes configuration to {vault}/.vault-mcp/config.toml.
    Init {
        /// Path to the Obsidian vault (auto-detected if omitted)
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { vault } => {
            let vault = resolve_vault(vault)?;
            // In serve mode, only log to stderr (stdout is MCP transport)
            tracing_subscriber::fmt()
                .with_env_filter("vault_search_mcp=info")
                .with_writer(std::io::stderr)
                .init();
            server::run_server(&vault).await
        }
        Commands::Index { vault, force } => {
            let vault = resolve_vault(vault)?;
            use console::style;
            use std::io::Write;

            tracing_subscriber::fmt()
                .with_env_filter("vault_search_mcp=info")
                .with_writer(std::io::stderr)
                .init();

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
            Ok(())
        }
        Commands::Search {
            vault,
            query,
            limit,
            json,
        } => {
            let vault = resolve_vault(vault)?;
            tracing_subscriber::fmt()
                .with_env_filter("vault_search_mcp=info")
                .with_writer(std::io::stderr)
                .init();
            let results = search::hybrid_search(&vault, &query, limit, None, None).await?;

            if json {
                // Machine-readable JSON output
                println!(
                    "{}",
                    serde_json::to_string_pretty(&results)
                        .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
                );
            } else {
                // Human-readable output
                for (i, r) in results.iter().enumerate() {
                    println!(
                        "{}. [{}] {} (score: {:.3})",
                        i + 1,
                        r.match_type,
                        r.path,
                        r.score
                    );
                    println!("   {}", r.chunk.chars().take(100).collect::<String>());
                    println!();
                }
                if results.is_empty() {
                    println!("  No results found.");
                }
            }
            Ok(())
        }
        Commands::Init { vault } => {
            // Guard: init requires an interactive terminal
            if !std::io::stdin().is_terminal() {
                anyhow::bail!(
                    "init requires an interactive terminal.\n\
                     Hint: create .vault-mcp/config.toml manually, or run in an interactive shell."
                );
            }
            let vault_path = match vault {
                Some(v) => v,
                None => detect_vault()?,
            };
            run_init(&vault_path)
        }
    }
}

/// Resolve vault path from explicit argument, or auto-detect.
/// Priority:
/// 1. Explicit --vault argument (already handled by clap/env)
/// 2. Walk up from CWD looking for `.vault-mcp/config.toml` (already initialized vault)
/// 3. Global last-used vault from ~/.config/vault-search-mcp/state.json
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

    // Try global state (last vault used during init)
    if let Some(path) = load_last_vault() {
        return Ok(path);
    }

    anyhow::bail!(
        "No vault specified. Either:\n\
         \x20 • Run from inside a vault directory\n\
         \x20 • Pass --vault <path>\n\
         \x20 • Set VAULT_PATH env var\n\
         \x20 • Run `vault-search-mcp init` first"
    );
}

/// Load the last-used vault path from global state file.
fn load_last_vault() -> Option<String> {
    let state_path = global_state_path()?;
    let content = std::fs::read_to_string(state_path).ok()?;
    let state: serde_json::Value = serde_json::from_str(&content).ok()?;
    state.get("vault_path")?.as_str().map(|s| s.to_string())
}

/// Save vault path to global state so subsequent commands auto-resolve it.
fn save_last_vault(vault_path: &str) {
    if let Some(state_path) = global_state_path() {
        let state = serde_json::json!({ "vault_path": vault_path });
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(state_path, state.to_string()).ok();
    }
}

/// Path to global state: ~/.config/vault-search-mcp/state.json
fn global_state_path() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new()
        .map(|d| d.config_dir().join("vault-search-mcp").join("state.json"))
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
                "  {} No vaults found automatically.",
                style("!").yellow().bold()
            );
            println!();
            let path: String = Input::with_theme(&theme)
                .with_prompt("  Vault path")
                .interact_text()?;
            Ok(path)
        }
        1 => {
            let path = candidates[0].display().to_string();
            eprintln!(
                "  {} Found vault: {}",
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
                "  {} Found {} vaults:",
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

/// Interactive onboarding: guide user to configure embedding provider.
fn run_init(vault_path: &str) -> Result<()> {
    use config::{Config, ConfigFile, EmbeddingConfig, EmbeddingProvider};
    use console::style;
    use dialoguer::{Input, Select, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();

    println!();
    println!("  {}", style("vault-search-mcp · Setup").bold());
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
            let default_endpoint = detected.unwrap_or_else(|| "http://localhost:11434".into());

            if default_endpoint != "http://localhost:11434" {
                eprintln!(
                    "  {} Auto-detected Ollama at {}",
                    style("✓").green().bold(),
                    style(&default_endpoint).underlined()
                );
            }

            let endpoint: String = Input::with_theme(&theme)
                .with_prompt("  Endpoint")
                .default(default_endpoint)
                .interact_text()?;

            let model: String = Input::with_theme(&theme)
                .with_prompt("  Model")
                .default("bge-m3".into())
                .interact_text()?;

            EmbeddingConfig {
                provider: EmbeddingProvider::Ollama,
                endpoint,
                model,
                api_key: None,
                dimensions: None,
            }
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

            EmbeddingConfig {
                provider: EmbeddingProvider::Openai,
                endpoint: "https://api.openai.com".into(),
                model,
                api_key: Some(api_key),
                dimensions: dimensions.parse().ok(),
            }
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

            EmbeddingConfig {
                provider: EmbeddingProvider::Custom,
                endpoint,
                model,
                api_key,
                dimensions: dimensions.parse().ok(),
            }
        }
    };

    // ── Save & Summary ──────────────────────────────────────────────────
    let config_file = ConfigFile {
        embedding: embedding_config.clone(),
        search: None,
    };

    Config::save_config_file(vault_path, &config_file)?;
    save_last_vault(vault_path);

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
    println!();
    println!("  {}", style("Next steps:").bold());
    println!();
    println!(
        "    {}  vault-search-mcp index --vault {}",
        style("$").dim(),
        vault_path
    );
    println!(
        "    {}  vault-search-mcp serve --vault {}",
        style("$").dim(),
        vault_path
    );
    println!();

    Ok(())
}
