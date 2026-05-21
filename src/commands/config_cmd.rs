use crate::config;

/// Print a concise config summary (used after init).
pub fn print_config_summary(config: &config::Config) {
    use console::style;

    println!();
    println!(
        "    {}    {}",
        style("vault").dim(),
        config.vault_path.display()
    );
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
    println!();
}

/// Print effective config in human-readable format.
pub fn print_config_human(config: &config::Config) {
    use console::style;

    let mcp = mcp_status(config);

    println!();
    println!("  {}", style("Effective Configuration").bold());
    println!("  {}", style("─".repeat(40)).dim());
    println!();

    println!("  {}", style("[Embedding]").bold());
    println!(
        "    {}    {}",
        style("vault").dim(),
        config.vault_path.display()
    );
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
    println!("  {}", style("[MCP]").bold());
    println!("    {}  {}", style("status").dim(), mcp.status);
    println!("    {} {}", style("command").dim(), mcp.command);
    println!("    {} {}", style("network").dim(), mcp.network_command);
    if let Some(address) = &mcp.address {
        println!("    {} {}", style("address").dim(), address);
    }
    if let Some(network_url) = &mcp.network_url {
        println!("    {} {}", style("lan").dim(), network_url);
    }

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
pub fn print_config_json(config: &config::Config) {
    let mcp = mcp_status(config);
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
        "mcp": {
            "status": mcp.status,
            "transport": mcp.transport,
            "address": mcp.address,
            "local_url": mcp.local_url,
            "network_url": mcp.network_url,
            "pid": mcp.pid,
            "updated_at": mcp.updated_at,
            "command": mcp.command,
            "network_command": mcp.network_command,
            "state_file": mcp.state_file,
        },
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&output)
            .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
    );
}

struct McpConfigStatus {
    status: String,
    transport: Option<String>,
    address: Option<String>,
    local_url: Option<String>,
    network_url: Option<String>,
    pid: Option<u32>,
    updated_at: Option<String>,
    command: String,
    network_command: String,
    state_file: String,
}

fn mcp_status(config: &config::Config) -> McpConfigStatus {
    let vault_path = config.vault_path.display().to_string();
    let state = config::Config::load_mcp_state(&vault_path);
    let status = match &state {
        Some(s) if s.transport == "streamable_http" && s.is_reachable() => "running",
        Some(s) if s.transport == "streamable_http" => "stale",
        Some(s) if s.transport == "stdio" => "stdio",
        Some(_) => "unknown",
        None => "not_running",
    };
    let address = state
        .as_ref()
        .filter(|s| s.transport == "streamable_http" && s.is_reachable())
        .and_then(|s| s.local_url.clone());

    McpConfigStatus {
        status: status.into(),
        transport: state.as_ref().map(|s| s.transport.clone()),
        address,
        local_url: state.as_ref().and_then(|s| s.local_url.clone()),
        network_url: state.as_ref().and_then(|s| s.network_url.clone()),
        pid: state.as_ref().map(|s| s.pid),
        updated_at: state.as_ref().map(|s| s.updated_at.clone()),
        command: format!("pearl serve --vault {}", vault_path),
        network_command: format!("pearl serve --vault {} --network", vault_path),
        state_file: config::Config::mcp_state_path(&vault_path)
            .display()
            .to_string(),
    }
}
