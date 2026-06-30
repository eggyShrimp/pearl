use std::io::IsTerminal;

use anyhow::Result;
use console::style;
use std::io::Write;

use crate::vault::resolve_vault;

pub async fn execute(vault: Option<String>, force: bool) -> Result<()> {
    let vault = resolve_vault(vault)?;

    tracing_subscriber::fmt()
        .with_env_filter("pearl=info")
        .with_writer(std::io::stderr)
        .init();

    let config = crate::config::Config::new(&vault);
    let health = crate::core::embedder::check_health(&config.embedding);
    match &health {
        crate::core::embedder::HealthStatus::Ok => {}
        crate::core::embedder::HealthStatus::Unreachable(msg) => {
            eprintln!();
            eprintln!("  {} {}", style("✗").red().bold(), msg);
            eprintln!();
            if config.embedding.provider == crate::config::EmbeddingProvider::Ollama {
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
        crate::core::embedder::HealthStatus::ModelMissing(msg) => {
            eprintln!();
            eprintln!("  {} {}", style("✗").red().bold(), msg);
            eprintln!();
            anyhow::bail!("Required model is not available. Cannot index.");
        }
    }

    let is_tty = std::io::stdout().is_terminal();

    let stats = crate::indexer::index_vault_with_progress(&vault, force, |progress| {
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
    crate::watch::ensure_watch_running(&vault);
    Ok(())
}
