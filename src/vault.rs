use std::path::PathBuf;

use anyhow::Result;

use crate::config;

/// Global state persisted at ~/.config/pearl/state.json.
/// Supports multiple registered vaults with a default.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct GlobalState {
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    vaults: Vec<String>,
}

/// Resolve vault path from explicit argument, or auto-detect.
/// Priority:
/// 1. Explicit --vault argument
/// 2. Walk up from CWD looking for `.vault-mcp/config.toml`
/// 3. Walk up from CWD looking for `.obsidian` dir (if global config exists)
/// 4. Default vault from global state
pub fn resolve_vault(explicit: Option<String>) -> Result<String> {
    if let Some(v) = explicit {
        return Ok(v);
    }

    // Walk up from CWD looking for an initialized vault (.vault-mcp/config.toml)
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = Some(cwd.as_path());
        while let Some(d) = dir {
            if d.join(".vault-mcp").join("config.toml").exists() {
                return Ok(d.display().to_string());
            }
            dir = d.parent();
        }
    }

    // Walk up from CWD looking for .obsidian directory (vault relying on global config)
    if config::Config::global_config_exists() {
        if let Ok(cwd) = std::env::current_dir() {
            let mut dir = Some(cwd.as_path());
            while let Some(d) = dir {
                if d.join(".obsidian").is_dir() {
                    return Ok(d.display().to_string());
                }
                dir = d.parent();
            }
        }
    }

    // Try vault_path from global config (~/.config/pearl/config.toml)
    if let Some(path) = config::Config::global_vault_path() {
        return Ok(path);
    }

    // Try global state (default vault)
    let state = load_global_state();
    if let Some(ref path) = state.default {
        return Ok(path.clone());
    }

    anyhow::bail!(
        "No vault specified. Either:\n\
         \x20 • Run from inside a vault directory\n\
         \x20 • Pass --vault <path>\n\
         \x20 • Set VAULT_PATH env var\n\
         \x20 • Run `pearl init` first"
    );
}

/// Load global state from disk.
pub fn load_global_state() -> GlobalState {
    global_state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

/// Save global state to disk.
pub fn save_global_state(state: &GlobalState) {
    if let Some(state_path) = global_state_path() {
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(content) = serde_json::to_string_pretty(state) {
            std::fs::write(state_path, content).ok();
        }
    }
}

/// Register a vault in global state (called during `init`).
pub fn register_vault(vault_path: &str) {
    let mut state = load_global_state();
    let path = vault_path.to_string();

    if !state.vaults.contains(&path) {
        state.vaults.push(path.clone());
    }
    state.default = Some(path);

    save_global_state(&state);
}

/// Path to global state: ~/.config/pearl/state.json
fn global_state_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("pearl").join("state.json"))
}

/// Get user's home directory.
pub fn dirs_home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Detect the Obsidian vault path automatically (interactive, for `init` only).
pub fn detect_vault_interactive() -> Result<String> {
    use walkdir::WalkDir;

    let mut candidates: Vec<PathBuf> = Vec::new();

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

    if let Some(home) = dirs_home() {
        let search_roots = [
            home.join("Documents"),
            home.join("Obsidian"),
            home.join("vaults"),
            home.join("Desktop"),
            home.join("Library/Mobile Documents/iCloud~md~obsidian/Documents"),
        ];

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
            cliclack::log::warning("No Obsidian vaults found on this machine.")?;
            let path: String = cliclack::input("Vault / markdown folder path")
                .placeholder("/path/to/your/vault")
                .interact()?;
            Ok(path)
        }
        1 => {
            let path = candidates[0].display().to_string();
            let confirm: bool = cliclack::confirm(format!("Use vault: {}?", &path))
                .initial_value(true)
                .interact()?;
            if confirm {
                Ok(path)
            } else {
                let path: String = cliclack::input("Vault path")
                    .placeholder("/path/to/your/vault")
                    .interact()?;
                Ok(path)
            }
        }
        _ => {
            let mut select = cliclack::select("Select vault");
            for (i, c) in candidates.iter().enumerate() {
                let label = c.display().to_string();
                select = select.item(i, &label, "");
            }
            let selection: usize = select.interact()?;
            Ok(candidates[selection].display().to_string())
        }
    }
}
