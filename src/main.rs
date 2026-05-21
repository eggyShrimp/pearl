mod commands;
mod config;
mod core;
mod indexer;
mod search;
mod server;
mod vault;
mod watch;

use std::io::IsTerminal;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

use vault::resolve_vault;

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

const PEARL_LOGO_PLAIN: &str = r#"
                           __
    ____  ___  ____ ______/ /
   / __ \/ _ \/ __ `/ ___/ / 
  / /_/ /  __/ /_/ / /  / /  
 / .___/\___/\__,_/_/  /_/   
/_/                          
"#;

/// Render the logo with blue→purple gradient background fill on stroke characters.
pub fn pearl_logo_colored() -> String {
    const LINES: &[&str] = &[
        r"                           __",
        r"    ____  ___  ____ ______/ /",
        r"   / __ \/ _ \/ __ `/ ___/ / ",
        r"  / /_/ /  __/ /_/ / /  / /  ",
        r" / .___/\___/\__,_/_/  /_/   ",
        r"/_/                          ",
    ];
    let bg_colors: &[u8] = &[63, 69, 99, 105, 135, 129];

    let mut out = String::new();
    for (i, line) in LINES.iter().enumerate() {
        let bg = bg_colors[i % bg_colors.len()];
        for ch in line.chars() {
            if ch == ' ' {
                out.push(' ');
            } else {
                out.push_str(&format!("\x1b[97;48;5;{}m{}\x1b[0m", bg, ch));
            }
        }
        out.push('\n');
    }
    out
}

#[derive(Parser)]
#[command(
    name = "pearl",
    version,
    about = "Semantic search for Obsidian vaults, exposed as an MCP server for AI agents.",
    before_long_help = PEARL_LOGO_PLAIN,
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
    Serve {
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        #[arg(short, long, env = "MCP_PORT", default_missing_value = "8686", num_args = 0..=1)]
        network: Option<u16>,
    },

    /// Build or update the search index (embeddings + full-text).
    Index {
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        #[arg(short, long, default_value_t = false)]
        force: bool,
    },

    /// Run a search query from the terminal (for testing and scripting).
    Search {
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        query: String,
        #[arg(short = 'k', long = "top-k", alias = "limit", default_value_t = 10)]
        limit: usize,
        #[arg(short, long)]
        folder: Vec<String>,
        #[arg(short, long)]
        exclude: Vec<String>,
        #[arg(short, long)]
        tag: Vec<String>,
        #[arg(short, long, default_value = "hybrid")]
        mode: SearchMode,
        #[arg(long)]
        threshold: Option<f32>,
        #[arg(short = 'C', long, default_value_t = 0)]
        context: usize,
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Watch vault for file changes and auto-update the index.
    Watch {
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        #[arg(short, long, default_value_t = false)]
        daemon: bool,
        #[arg(long, default_value_t = false)]
        stop: bool,
        #[arg(long, default_value_t = false)]
        status: bool,
    },

    /// Interactive setup wizard — configure embedding provider and generate config.
    Init {
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        #[arg(short, long, default_value_t = false)]
        global: bool,
    },

    /// Install pearl as a tool/skill into AI coding agents.
    Install {
        #[arg(short, long, value_delimiter = ',')]
        target: Vec<String>,
    },

    /// Show the effective configuration (merged from all sources).
    Config {
        #[arg(short, long, env = "VAULT_PATH")]
        vault: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle --daemon BEFORE creating tokio runtime, because daemonize()
    // closes file descriptors which breaks an already-running tokio I/O driver.
    if matches!(
        cli.command,
        Commands::Watch {
            daemon: true,
            ..
        }
    ) {
        if let Commands::Watch { vault, .. } = &cli.command {
            let vault = resolve_vault(vault.clone())?;
            watch::daemonize_watch(&vault)?;
        }
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async { run(cli).await })
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Serve { vault, network } => {
            let vault = resolve_vault(vault)?;
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
                        eprintln!("  Run {} to reconfigure.", style("pearl init").bold());
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
                    eprint!(
                        "\r  [{}/{}] {}",
                        progress.current, progress.total, progress.path
                    );
                    eprint!("\x1b[K");
                    std::io::stderr().flush().ok();
                }
            })
            .await?;

            if is_tty {
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
            println!(
                "    {}  {} nodes, {} edges",
                style("graph").dim(),
                stats.graph_nodes,
                stats.graph_edges
            );
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

            let (mut results, linked_notes) = match mode {
                SearchMode::Hybrid => {
                    let response =
                        search::hybrid_search(&vault, &query, limit, folders, tags).await?;
                    (response.results, response.linked_notes)
                }
                SearchMode::Semantic => {
                    let r =
                        search::vector_search_only(&vault, &query, limit, folders, tags).await?;
                    (r, vec![])
                }
                SearchMode::Fts => {
                    let r = search::fts_search_only(&vault, &query, limit).await?;
                    (r, vec![])
                }
            };

            if let Some(ref excludes) = excludes {
                results.retain(|r| !excludes.iter().any(|ex| r.path.starts_with(ex.as_str())));
            }

            if let Some(min_score) = threshold {
                results.retain(|r| r.score >= min_score);
            }

            if let Some(ts) = since_ts {
                results.retain(|r| {
                    let full_path = std::path::Path::new(&vault).join(&r.path);
                    match std::fs::metadata(&full_path) {
                        Ok(meta) => match meta.modified() {
                            Ok(mtime) => {
                                let file_ts = mtime
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs() as i64)
                                    .unwrap_or(0);
                                file_ts >= ts
                            }
                            Err(_) => true,
                        },
                        Err(_) => false,
                    }
                });
            }

            if json {
                let output = serde_json::json!({
                    "results": results,
                    "linked_notes": linked_notes,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output)
                        .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
                );
            } else {
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

                if !linked_notes.is_empty() {
                    use console::style;
                    println!("  {} Linked notes:", style("⟡").dim());
                    for note in &linked_notes {
                        println!(
                            "    {} {} (via {})",
                            style("→").dim(),
                            style(&note.path).blue(),
                            style(&note.related_to).dim()
                        );
                    }
                    println!();
                }
            }
            Ok(())
        }
        Commands::Watch {
            vault,
            daemon: _,
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

            watch::run_watch(&vault).await
        }
        Commands::Init { vault, global } => {
            commands::init::check_interactive()?;
            if global {
                commands::init::run_init_global()
            } else {
                let vault_path = match vault {
                    Some(v) => v,
                    None => vault::detect_vault_interactive()?,
                };
                commands::init::run_init(&vault_path)
            }
        }
        Commands::Install { target } => commands::install::run_install(target),
        Commands::Config { vault, json } => {
            let config = if let Some(ref v) = vault {
                config::Config::new(v)
            } else if let Ok(v) = resolve_vault(None) {
                config::Config::new(&v)
            } else {
                config::Config::new(".")
            };
            if json {
                commands::config_cmd::print_config_json(&config);
            } else {
                commands::config_cmd::print_config_human(&config);
            }
            Ok(())
        }
    }
}
