mod commands;
mod config;
mod core;
mod indexer;
mod search;
mod server;
mod vault;
mod watch;

use anyhow::Result;
use clap::{Parser, Subcommand};

use commands::search::SearchMode;
use vault::resolve_vault;

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
    if matches!(cli.command, Commands::Watch { daemon: true, .. }) {
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
        Commands::Serve { vault, network } => commands::serve::execute(vault, network).await,
        Commands::Index { vault, force } => commands::index::execute(vault, force).await,
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
            commands::search::execute(
                vault, query, limit, folder, exclude, tag, mode, threshold, context, since, json,
            )
            .await
        }
        Commands::Watch {
            vault,
            daemon,
            stop,
            status,
        } => commands::watch_cmd::execute(vault, daemon, stop, status).await,
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
        Commands::Config { vault, json } => commands::config_cmd::execute(vault, json),
    }
}
