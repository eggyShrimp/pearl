use std::io::IsTerminal;

use anyhow::Result;

use crate::config;
use crate::vault::{register_vault, detect_vault_interactive};

/// Interactive onboarding: guide user to configure embedding provider (vault-local).
pub fn run_init(vault_path: &str) -> Result<()> {
    use config::{Config, ConfigFile};

    cliclack::clear_screen()?;
    cliclack::intro("pearl · Setup")?;

    cliclack::log::info(format!("Vault: {}", vault_path))?;

    if Config::config_exists(vault_path) {
        let overwrite: bool = cliclack::confirm("Config file already exists. Overwrite?")
            .initial_value(false)
            .interact()?;
        if !overwrite {
            cliclack::outro("Aborted.")?;
            return Ok(());
        }
    }

    let embedding_config = prompt_embedding_config()?;

    let config_file = ConfigFile {
        vault_path: None,
        embedding: embedding_config.clone(),
        search: None,
        index: None,
    };

    Config::save_config_file(vault_path, &config_file)?;
    register_vault(vault_path);

    cliclack::outro("Configuration saved!")?;

    let config = Config::new(vault_path);
    crate::commands::config_cmd::print_config_summary(&config);

    println!("  Next steps:");
    println!("    $ pearl index --vault {}", vault_path);
    println!("    $ pearl serve --vault {}", vault_path);
    println!();

    Ok(())
}

/// Interactive onboarding: write config to the global path (~/.config/pearl/config.toml).
pub fn run_init_global() -> Result<()> {
    use config::{Config, ConfigFile};

    let config_path = Config::global_config_path()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine global config directory"))?;

    cliclack::clear_screen()?;
    cliclack::intro("pearl · Global Setup")?;

    cliclack::log::info(format!(
        "Config: {}\nApplies to all vaults unless overridden locally.",
        config_path.display()
    ))?;

    if Config::global_config_exists() {
        let overwrite: bool = cliclack::confirm("Global config already exists. Overwrite?")
            .initial_value(false)
            .interact()?;
        if !overwrite {
            cliclack::outro("Aborted.")?;
            return Ok(());
        }
    }

    let default_vault = detect_vault_interactive().ok().unwrap_or_default();
    let vault_path: String = cliclack::input("Default vault path")
        .default_input(&default_vault)
        .interact()?;
    let vault_path = if vault_path.is_empty() {
        None
    } else {
        Some(vault_path)
    };

    let embedding_config = prompt_embedding_config()?;

    let config_file = ConfigFile {
        vault_path: vault_path.clone(),
        embedding: embedding_config.clone(),
        search: None,
        index: None,
    };

    Config::save_global_config_file(&config_file)?;

    cliclack::outro("Global configuration saved!")?;

    let effective_vault = vault_path.as_deref().unwrap_or(".");
    let config = Config::new(effective_vault);
    crate::commands::config_cmd::print_config_summary(&config);

    Ok(())
}

/// Guard: init requires an interactive terminal.
pub fn check_interactive() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "init requires an interactive terminal.\n\
             Hint: create config.toml manually, or run in an interactive shell."
        );
    }
    Ok(())
}

/// Interactively collect embedding provider configuration.
fn prompt_embedding_config() -> Result<config::EmbeddingConfig> {
    use config::EmbeddingProvider;

    let provider: &str = cliclack::select("Embedding provider")
        .item("ollama", "Ollama", "local, free, private")
        .item("openai", "OpenAI", "cloud API, high quality")
        .item("custom", "Custom", "any OpenAI-compatible endpoint")
        .interact()?;

    let provider = match provider {
        "ollama" => EmbeddingProvider::Ollama,
        "openai" => EmbeddingProvider::Openai,
        _ => EmbeddingProvider::Custom,
    };

    let embedding_config = match provider {
        EmbeddingProvider::Ollama => prompt_ollama_config()?,
        EmbeddingProvider::Openai => prompt_openai_config()?,
        EmbeddingProvider::Custom => prompt_custom_config()?,
    };

    Ok(embedding_config)
}

fn prompt_ollama_config() -> Result<config::EmbeddingConfig> {
    use config::{EmbeddingConfig, EmbeddingProvider};

    let detected = crate::core::embedder::detect_ollama_endpoint();
    let default_endpoint = detected
        .as_ref()
        .cloned()
        .unwrap_or_else(|| "http://localhost:11434".into());

    if detected.is_some() {
        cliclack::log::success(format!("Auto-detected Ollama at {}", &default_endpoint))?;
    } else {
        cliclack::log::warning("Ollama is not running. Install: https://ollama.com/download")?;
    }

    let endpoint: String = cliclack::input("Endpoint")
        .default_input(&default_endpoint)
        .interact()?;

    let model: String = cliclack::input("Model")
        .default_input("bge-m3")
        .interact()?;

    let tmp_config = EmbeddingConfig {
        provider: EmbeddingProvider::Ollama,
        endpoint: endpoint.clone(),
        model: model.clone(),
        api_key: None,
        dimensions: None,
    };

    verify_embedding_health(&tmp_config, "Ollama", &model);

    Ok(tmp_config)
}

fn prompt_openai_config() -> Result<config::EmbeddingConfig> {
    use config::{EmbeddingConfig, EmbeddingProvider};

    let model: String = cliclack::input("Model")
        .default_input("text-embedding-3-small")
        .interact()?;

    let key_source: &str = cliclack::select("API key source")
        .item("env", "Read from $OPENAI_API_KEY env var", "")
        .item("direct", "Enter key now", "")
        .interact()?;

    let api_key = if key_source == "env" {
        "$OPENAI_API_KEY".to_string()
    } else {
        cliclack::input("API key").interact()?
    };

    let dimensions: String = cliclack::input("Dimensions (empty to skip)")
        .default_input("")
        .interact()?;

    let tmp_config = EmbeddingConfig {
        provider: EmbeddingProvider::Openai,
        endpoint: "https://api.openai.com".into(),
        model: model.clone(),
        api_key: Some(api_key),
        dimensions: if dimensions.is_empty() {
            None
        } else {
            dimensions.parse().ok()
        },
    };

    verify_embedding_health(&tmp_config, "API", &model);

    Ok(tmp_config)
}

fn prompt_custom_config() -> Result<config::EmbeddingConfig> {
    use config::{EmbeddingConfig, EmbeddingProvider};

    let endpoint: String = cliclack::input("Endpoint (must serve /v1/embeddings)")
        .placeholder("http://localhost:8080")
        .interact()?;

    let model: String = cliclack::input("Model").interact()?;

    let needs_key: bool = cliclack::confirm("Requires API key?")
        .initial_value(true)
        .interact()?;

    let api_key = if needs_key {
        let key_source: &str = cliclack::select("API key source")
            .item("env", "Read from env var", "")
            .item("direct", "Enter key now", "")
            .interact()?;

        if key_source == "env" {
            let env_name: String = cliclack::input("Env var name")
                .default_input("EMBEDDING_API_KEY")
                .interact()?;
            Some(format!("${}", env_name))
        } else {
            let key: String = cliclack::input("API key").interact()?;
            Some(key)
        }
    } else {
        None
    };

    let dimensions: String = cliclack::input("Dimensions (empty to skip)")
        .default_input("")
        .interact()?;

    let tmp_config = EmbeddingConfig {
        provider: EmbeddingProvider::Custom,
        endpoint,
        model: model.clone(),
        api_key,
        dimensions: if dimensions.is_empty() {
            None
        } else {
            dimensions.parse().ok()
        },
    };

    verify_embedding_health(&tmp_config, "endpoint", &model);

    Ok(tmp_config)
}

fn verify_embedding_health(config: &config::EmbeddingConfig, label: &str, model: &str) {
    let health = crate::core::embedder::check_health(config);
    match &health {
        crate::core::embedder::HealthStatus::Ok => {
            let _ = cliclack::log::success(format!(
                "{} is running, model '{}' is accessible",
                label, model
            ));
        }
        crate::core::embedder::HealthStatus::ModelMissing(msg) => {
            let _ = cliclack::log::warning(format!(
                "{}\n  Run: ollama pull {}",
                msg, model
            ));
        }
        crate::core::embedder::HealthStatus::Unreachable(msg) => {
            let _ = cliclack::log::warning(format!("{}\n  Check your endpoint and API key.", msg));
        }
    }
}
