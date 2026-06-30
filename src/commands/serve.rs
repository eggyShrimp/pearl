use anyhow::Result;

use crate::vault::resolve_vault;

pub async fn execute(vault: Option<String>, network: Option<u16>) -> Result<()> {
    let vault = resolve_vault(vault)?;
    tracing_subscriber::fmt()
        .with_env_filter("pearl=info")
        .with_writer(std::io::stderr)
        .init();
    crate::watch::ensure_watch_running(&vault);
    match network {
        Some(port) => crate::server::run_server_http(&vault, port).await,
        None => crate::server::run_server_stdio(&vault).await,
    }
}
