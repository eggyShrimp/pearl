use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use console::style;
use notify::RecursiveMode;
use notify_debouncer_full::new_debouncer;
use tokio::sync::mpsc;

use crate::config::{Config, EXCLUDE_DIRS};
use crate::indexer;

// ─── PID file management ────────────────────────────────────────────────────

/// Get the path to the watch pid file for a vault.
pub fn pid_file_path(vault_path: &str) -> PathBuf {
    Path::new(vault_path).join(".vault-mcp").join("watch.pid")
}

/// Write the current process PID to the pid file.
fn write_pid_file(vault_path: &str) -> Result<()> {
    let path = pid_file_path(vault_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, std::process::id().to_string())?;
    Ok(())
}

/// Remove the pid file.
fn remove_pid_file(vault_path: &str) {
    let _ = fs::remove_file(pid_file_path(vault_path));
}

/// Check if the watch process is alive for a given vault.
pub fn is_watch_running(vault_path: &str) -> bool {
    let path = pid_file_path(vault_path);
    match fs::read_to_string(&path) {
        Ok(contents) => {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                process_alive(pid)
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

/// Check if a process with the given PID is alive.
fn process_alive(pid: u32) -> bool {
    // On Unix, sending signal 0 checks if the process exists without killing it.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Ensure the watcher is running for a vault.
/// If not running, automatically spawn it as a background daemon.
/// Called from other commands (index, serve, search) to keep the index fresh.
pub fn ensure_watch_running(vault_path: &str) {
    use std::io::IsTerminal;

    if is_watch_running(vault_path) {
        return;
    }

    // Spawn ourselves as a daemon watcher
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return, // silently skip if we can't find our own binary
    };

    let result = std::process::Command::new(&exe)
        .args(["watch", "--daemon", "--vault", vault_path])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match result {
        Ok(_child) => {
            // Give the daemon a moment to write its pid file
            std::thread::sleep(Duration::from_millis(300));
            if std::io::stderr().is_terminal() {
                eprintln!("  {} Watcher started in background", style("⟳").green());
            }
        }
        Err(e) => {
            if std::io::stderr().is_terminal() {
                eprintln!("  {} Could not start watcher: {}", style("!").yellow(), e);
            }
        }
    }
}

// ─── Watch command implementation ────────────────────────────────────────────

/// Run the file watcher in foreground.
pub async fn run_watch(vault_path: &str) -> Result<()> {
    use std::io::IsTerminal;

    let config = Config::new(vault_path);

    // Check if already running
    if is_watch_running(vault_path) {
        let pid_content = fs::read_to_string(pid_file_path(vault_path)).unwrap_or_default();
        anyhow::bail!(
            "Watcher is already running (PID {}). Stop it first or use `pearl watch --stop`.",
            pid_content.trim()
        );
    }

    // Write pid file
    write_pid_file(vault_path)?;

    // Ensure cleanup on exit
    let vault_for_cleanup = vault_path.to_string();
    let _cleanup_guard = scopeguard::guard((), |_| {
        remove_pid_file(&vault_for_cleanup);
    });

    if std::io::stderr().is_terminal() {
        eprintln!();
        eprintln!("  {} pearl watcher", style("⟳").green().bold());
        eprintln!();
        eprintln!("    {}  {}", style("vault").dim(), vault_path);
        eprintln!(
            "    {}  {} ({})",
            style("model").dim(),
            config.embedding.model,
            config.embedding.provider
        );
        eprintln!("    {}  2s", style("debounce").dim());
        eprintln!();
    }

    // Initial incremental index
    if std::io::stderr().is_terminal() {
        eprintln!("  {} Running initial index...", style("↺").dim());
    }
    match indexer::index_vault(vault_path, false).await {
        Ok(stats) => {
            if std::io::stderr().is_terminal() {
                eprintln!(
                    "  {} Index ready ({} files, {} chunks)",
                    style("✓").green().bold(),
                    stats.total_files,
                    stats.total_chunks
                );
                eprintln!();
            }
        }
        Err(e) => {
            eprintln!("  {} Initial index failed: {}", style("✗").red().bold(), e);
            eprintln!("  Continuing with file watching...");
            eprintln!();
        }
    }

    if std::io::stderr().is_terminal() {
        eprintln!("  {} Watching for changes...", style("↺").dim());
        eprintln!(
            "  {} Press {} to stop",
            style("hint").dim(),
            style("Ctrl+C").bold()
        );
        eprintln!();
    }

    // Set up debounced file watcher
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<PathBuf>>();

    let vault_path_buf = PathBuf::from(vault_path);
    let exclude_dirs: HashSet<&str> = EXCLUDE_DIRS.iter().copied().collect();

    let mut debouncer = new_debouncer(
        Duration::from_secs(2),
        None,
        move |result: notify_debouncer_full::DebounceEventResult| {
            match result {
                Ok(events) => {
                    let changed_paths: Vec<PathBuf> = events
                        .into_iter()
                        .flat_map(|e| e.event.paths)
                        .filter(|p| {
                            // Only .md files
                            p.extension().is_some_and(|ext| ext == "md")
                        })
                        .filter(|p| {
                            // Exclude internal directories
                            !p.components().any(|c| {
                                if let std::path::Component::Normal(s) = c {
                                    exclude_dirs.contains(s.to_str().unwrap_or(""))
                                } else {
                                    false
                                }
                            })
                        })
                        .collect::<HashSet<_>>()
                        .into_iter()
                        .collect();

                    if !changed_paths.is_empty() {
                        let _ = tx.send(changed_paths);
                    }
                }
                Err(errors) => {
                    for e in errors {
                        tracing::warn!("Watch error: {:?}", e);
                    }
                }
            }
        },
    )?;

    debouncer.watch(&vault_path_buf, RecursiveMode::Recursive)?;

    // Set up Ctrl+C handler
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        let _ = shutdown_tx.send(()).await;
    });

    let vault_for_index = vault_path.to_string();

    // Event loop
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                break;
            }
            Some(changed) = rx.recv() => {
                let count = changed.len();
                // Make paths relative for display
                let relative: Vec<String> = changed.iter()
                    .filter_map(|p| p.strip_prefix(&vault_path_buf).ok())
                    .map(|p| p.display().to_string())
                    .collect();

                if std::io::stderr().is_terminal() {
                    if count <= 3 {
                        for p in &relative {
                            eprintln!("  {} {}", style("Δ").yellow(), p);
                        }
                    } else {
                        eprintln!("  {} {} files changed", style("Δ").yellow(), count);
                    }
                }

                // Run incremental index
                match indexer::index_vault(&vault_for_index, false).await {
                    Ok(stats) => {
                        if stats.indexed > 0 && std::io::stderr().is_terminal() {
                            eprintln!(
                                "  {} Indexed {} file{}",
                                style("✓").green(),
                                stats.indexed,
                                if stats.indexed == 1 { "" } else { "s" }
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Index error: {}", e);
                        if std::io::stderr().is_terminal() {
                            eprintln!("  {} Index error: {}", style("✗").red(), e);
                        }
                    }
                }
            }
        }
    }

    if std::io::stderr().is_terminal() {
        eprintln!();
        eprintln!("  {} Watcher stopped", style("■").dim());
        eprintln!();
    }

    Ok(())
}

/// Daemonize the watch process (run in background).
pub fn daemonize_watch(vault_path: &str) -> Result<()> {
    // Check if already running
    if is_watch_running(vault_path) {
        let pid_content = fs::read_to_string(pid_file_path(vault_path)).unwrap_or_default();
        anyhow::bail!("Watcher already running (PID {})", pid_content.trim());
    }

    let log_path = Path::new(vault_path).join(".vault-mcp").join("watch.log");

    // Ensure .vault-mcp dir exists
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let stdout = fs::File::create(&log_path)?;
    let stderr = stdout.try_clone()?;

    let daemon = daemonize::Daemonize::new()
        .pid_file(pid_file_path(vault_path))
        .working_directory(vault_path)
        .stdout(stdout)
        .stderr(stderr);

    match daemon.start() {
        Ok(_) => {
            // We are now in the child process — start the async runtime
            // This will be called from main after daemonize
            Ok(())
        }
        Err(e) => {
            anyhow::bail!("Failed to daemonize: {}", e);
        }
    }
}

/// Stop a running watch daemon.
pub fn stop_watch(vault_path: &str) -> Result<()> {
    let path = pid_file_path(vault_path);
    match fs::read_to_string(&path) {
        Ok(contents) => {
            if let Ok(pid) = contents.trim().parse::<i32>() {
                if process_alive(pid as u32) {
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                    // Wait briefly for process to exit
                    std::thread::sleep(Duration::from_millis(500));
                    if process_alive(pid as u32) {
                        unsafe {
                            libc::kill(pid, libc::SIGKILL);
                        }
                    }
                    remove_pid_file(vault_path);
                    eprintln!("  {} Watcher stopped (PID {})", style("■").dim(), pid);
                    Ok(())
                } else {
                    remove_pid_file(vault_path);
                    anyhow::bail!("Watcher was not running (stale pid file cleaned up)");
                }
            } else {
                remove_pid_file(vault_path);
                anyhow::bail!("Invalid pid file");
            }
        }
        Err(_) => {
            anyhow::bail!("Watcher is not running (no pid file found)");
        }
    }
}
