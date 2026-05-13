mod config;
mod core;
mod indexer;
mod search;
mod server;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "vault-search-mcp", version, about = "Local-first semantic search MCP server for Obsidian vaults")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server (stdio transport)
    Serve {
        /// Path to the Obsidian vault
        #[arg(short, long, env = "VAULT_PATH")]
        vault: String,
    },
    /// Index the vault (build or update search index)
    Index {
        /// Path to the Obsidian vault
        #[arg(short, long, env = "VAULT_PATH")]
        vault: String,
        /// Force full reindex (ignore cached hashes)
        #[arg(short, long, default_value_t = false)]
        force: bool,
    },
    /// Search the vault (CLI mode, for testing)
    Search {
        /// Path to the Obsidian vault
        #[arg(short, long, env = "VAULT_PATH")]
        vault: String,
        /// Search query
        query: String,
        /// Max results
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },
    /// Initialize vault configuration (interactive setup)
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
            // In serve mode, only log to stderr (stdout is MCP transport)
            tracing_subscriber::fmt()
                .with_env_filter("vault_search_mcp=info")
                .with_writer(std::io::stderr)
                .init();
            server::run_server(&vault).await
        }
        Commands::Index { vault, force } => {
            tracing_subscriber::fmt()
                .with_env_filter("vault_search_mcp=info")
                .init();
            let stats = indexer::index_vault(&vault, force).await?;
            println!("Indexing complete:");
            println!("  Total files: {}", stats.total_files);
            println!("  Indexed: {}", stats.indexed);
            println!("  Skipped (unchanged): {}", stats.skipped);
            println!("  Deleted: {}", stats.deleted);
            println!("  Total chunks: {}", stats.total_chunks);
            Ok(())
        }
        Commands::Search {
            vault,
            query,
            limit,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter("vault_search_mcp=info")
                .with_writer(std::io::stderr)
                .init();
            let results = search::hybrid_search(&vault, &query, limit, None, None).await?;
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
            Ok(())
        }
        Commands::Init { vault } => {
            let vault_path = match vault {
                Some(v) => v,
                None => detect_vault()?,
            };
            run_init(&vault_path)
        }
    }
}

/// Detect the Obsidian vault path automatically.
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

    eprintln!(
        "  {} Scanning for Obsidian vaults...",
        style("⟳").cyan().bold()
    );

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
    println!(
        "  {}",
        style("vault-search-mcp · Setup").bold()
    );
    println!(
        "  {}",
        style("─".repeat(40)).dim()
    );
    println!(
        "  Vault: {}",
        style(vault_path).cyan().underlined()
    );
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
                .items(&[
                    "Read from $OPENAI_API_KEY env var",
                    "Enter key now",
                ])
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

    let config_path = Config::config_path(vault_path);
    println!();
    println!(
        "  {}",
        style("─".repeat(40)).dim()
    );
    println!(
        "  {} Configuration saved!",
        style("✓").green().bold()
    );
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
    println!(
        "  {}     {}",
        style("model").dim(),
        embedding_config.model
    );
    println!(
        "  {}    {}",
        style("config").dim(),
        style(config_path.display()).underlined()
    );
    println!();
    println!(
        "  {}",
        style("Next steps:").bold()
    );
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
