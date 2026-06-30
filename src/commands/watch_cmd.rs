use anyhow::Result;

use crate::vault::resolve_vault;

pub async fn execute(
    vault: Option<String>,
    _daemon: bool,
    stop: bool,
    status: bool,
) -> Result<()> {
    let vault = resolve_vault(vault)?;
    tracing_subscriber::fmt()
        .with_env_filter("pearl=info")
        .with_writer(std::io::stderr)
        .init();

    if status {
        if crate::watch::is_watch_running(&vault) {
            let pid = std::fs::read_to_string(crate::watch::pid_file_path(&vault))
                .unwrap_or_default();
            println!("  Watcher is running (PID {})", pid.trim());
        } else {
            println!("  Watcher is not running");
        }
        return Ok(());
    }

    if stop {
        return crate::watch::stop_watch(&vault);
    }

    crate::watch::run_watch(&vault).await
}
