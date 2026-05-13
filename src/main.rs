mod config;
mod core;
mod indexer;
mod search;
mod server;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "obsidian-mcp", version, about = "Local-first semantic search MCP server for Obsidian vaults")]
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { vault } => {
            // In serve mode, only log to stderr (stdout is MCP transport)
            tracing_subscriber::fmt()
                .with_env_filter("obsidian_mcp=info")
                .with_writer(std::io::stderr)
                .init();
            server::run_server(&vault).await
        }
        Commands::Index { vault, force } => {
            tracing_subscriber::fmt()
                .with_env_filter("obsidian_mcp=info")
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
        Commands::Search { vault, query, limit } => {
            tracing_subscriber::fmt()
                .with_env_filter("obsidian_mcp=info")
                .with_writer(std::io::stderr)
                .init();
            let results = search::hybrid_search(&vault, &query, limit, None, None).await?;
            for (i, r) in results.iter().enumerate() {
                println!("{}. [{}] {} (score: {:.3})", i + 1, r.match_type, r.path, r.score);
                println!("   {}", r.chunk.chars().take(100).collect::<String>());
                println!();
            }
            Ok(())
        }
    }
}
