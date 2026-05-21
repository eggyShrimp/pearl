use std::io::IsTerminal;
use std::path::Path;

use anyhow::Result;

use crate::vault::dirs_home;

const INSTALL_TARGETS: &[(&str, &str)] = &[
    ("cursor", "Cursor"),
    ("claude-code", "Claude Code"),
    ("trae", "Trae"),
    ("windsurf", "Windsurf"),
    ("opencode", "OpenCode"),
    ("codex", "Codex"),
];

const VAULT_SEARCH_SKILL_DESCRIPTION: &str = r#"When the user asks to search their notes, find related content, look up something
in their Obsidian vault, or needs context from their knowledge base, use the
pearl MCP server tools.

## pearl tools

- `pearl` — Single tool with a `command` parameter. Commands:
  - `search` — Hybrid semantic + full-text search
    Params: query (required), mode (hybrid|semantic|fts), limit, folders, tags
  - `index` — Build/rebuild the search index
    Params: force (bool, default false)
  - `get` — Read a note by relative path
    Params: path (required)
  - `list` — List files/directories in the vault
    Params: folder, recursive (bool)
  - `status` — Check system health and configuration
"#;

/// Main install orchestrator.
pub fn run_install(targets: Vec<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let already_installed = detect_installed_targets(&cwd);

    let selected: Vec<&str> = if targets.is_empty() {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(
                "install requires --target in non-interactive mode.\n\
                 Example: pearl install --target cursor,claude-code"
            );
        }

        cliclack::clear_screen()?;
        cliclack::intro("pearl · Install")?;

        let mut multi = cliclack::multiselect("Which agents to install into?");
        for (id, label) in INSTALL_TARGETS {
            let hint = if already_installed.contains(id) {
                "installed"
            } else {
                ""
            };
            multi = multi.item(*id, *label, hint);
        }
        multi = multi.initial_values(already_installed.clone());
        let selections: Vec<&str> = multi.interact()?;

        if selections.is_empty() {
            anyhow::bail!("No target selected.");
        }

        selections
    } else {
        for t in &targets {
            if !INSTALL_TARGETS.iter().any(|(id, _)| *id == t.as_str()) {
                anyhow::bail!(
                    "Unknown target: '{}'. Supported: cursor, claude-code, trae, windsurf, opencode, codex",
                    t
                );
            }
        }

        cliclack::intro("pearl · Install")?;

        targets.iter().map(|s| s.as_str()).collect()
    };

    let new_targets: Vec<&str> = selected
        .into_iter()
        .filter(|t| !already_installed.contains(t))
        .collect();

    if new_targets.is_empty() {
        cliclack::outro("All selected agents already have pearl installed.")?;
        return Ok(());
    }

    let bin_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "pearl".to_string());

    for target in &new_targets {
        match *target {
            "cursor" => install_cursor(&cwd, &bin_path)?,
            "claude-code" => install_claude_code(&cwd, &bin_path)?,
            "trae" => install_trae(&cwd, &bin_path)?,
            "windsurf" => install_windsurf(&cwd, &bin_path)?,
            "opencode" => install_opencode(&cwd, &bin_path)?,
            "codex" => install_codex(&cwd, &bin_path)?,
            _ => continue,
        }
    }

    let summary = new_targets
        .iter()
        .map(|t| format!("  {} installed", t))
        .collect::<Vec<_>>()
        .join("\n");
    cliclack::outro(format!("Done!\n{}", summary))?;

    Ok(())
}

fn file_contains(path: &Path, needle: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|content| content.contains(needle))
        .unwrap_or(false)
}

fn detect_installed_targets(cwd: &Path) -> Vec<&'static str> {
    let mut installed = Vec::new();

    let cursor_mcp = cwd.join(".cursor").join("mcp.json");
    if file_contains(&cursor_mcp, "pearl") {
        installed.push("cursor");
    }

    let claude_mcp = cwd.join(".mcp.json");
    if file_contains(&claude_mcp, "pearl") {
        installed.push("claude-code");
    }

    let trae_mcp = cwd.join(".trae").join("mcp.json");
    if file_contains(&trae_mcp, "pearl") {
        installed.push("trae");
    }

    if let Some(home) = dirs_home() {
        let windsurf_mcp = home
            .join(".codeium")
            .join("windsurf")
            .join("mcp_config.json");
        if file_contains(&windsurf_mcp, "pearl") {
            installed.push("windsurf");
        }
    }

    if let Some(home) = dirs_home() {
        let opencode_skill = home
            .join(".opencode")
            .join("skills")
            .join("pearl")
            .join("SKILL.md");
        if opencode_skill.exists() {
            installed.push("opencode");
        }
    }

    if let Some(home) = dirs_home() {
        let codex_config = home.join(".codex").join("config.toml");
        if file_contains(&codex_config, "mcp_servers.pearl") {
            installed.push("codex");
        }
    }

    installed
}

fn mcp_server_json(bin_path: &str) -> serde_json::Value {
    serde_json::json!({
        "command": bin_path,
        "args": ["serve"]
    })
}

fn mcp_server_json_typed(bin_path: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": bin_path,
        "args": ["serve"]
    })
}

fn write_mcp_config(path: &Path, bin_path: &str, typed: bool) -> Result<()> {
    let server_entry = if typed {
        mcp_server_json_typed(bin_path)
    } else {
        mcp_server_json(bin_path)
    };

    let mut config: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }
    config["mcpServers"]["pearl"] = server_entry;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&config)? + "\n")?;
    Ok(())
}

fn install_cursor(cwd: &Path, bin_path: &str) -> Result<()> {
    let mcp_path = cwd.join(".cursor").join("mcp.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".cursor").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("pearl.mdc");
    let content = format!(
        "---\ndescription: Use pearl for Obsidian knowledge base queries\nalwaysApply: false\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    cliclack::log::success(format!(
        "Cursor: {} + {}",
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        rules_path
            .strip_prefix(cwd)
            .unwrap_or(&rules_path)
            .display(),
    ))?;
    Ok(())
}

fn install_claude_code(cwd: &Path, bin_path: &str) -> Result<()> {
    let mcp_path = cwd.join(".mcp.json");
    write_mcp_config(&mcp_path, bin_path, true)?;

    let claude_md_path = cwd.join("CLAUDE.md");
    let section = format!(
        "\n## pearl — Obsidian Knowledge Base\n\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );

    if claude_md_path.exists() {
        let existing = std::fs::read_to_string(&claude_md_path)?;
        if !existing.contains("pearl") {
            std::fs::write(
                &claude_md_path,
                format!("{}\n{}", existing.trim_end(), section),
            )?;
        }
    } else {
        std::fs::write(
            &claude_md_path,
            format!("# Project Instructions\n{}", section),
        )?;
    }

    cliclack::log::success(format!(
        "Claude Code: {} + {}",
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        claude_md_path
            .strip_prefix(cwd)
            .unwrap_or(&claude_md_path)
            .display(),
    ))?;
    Ok(())
}

fn install_trae(cwd: &Path, bin_path: &str) -> Result<()> {
    let mcp_path = cwd.join(".trae").join("mcp.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".trae").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("pearl.md");
    let content = format!(
        "---\ndescription: Use pearl for Obsidian knowledge base queries\nalwaysApply: false\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    cliclack::log::success(format!(
        "Trae: {} + {}",
        mcp_path.strip_prefix(cwd).unwrap_or(&mcp_path).display(),
        rules_path
            .strip_prefix(cwd)
            .unwrap_or(&rules_path)
            .display(),
    ))?;
    Ok(())
}

fn install_windsurf(cwd: &Path, bin_path: &str) -> Result<()> {
    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let mcp_path = home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    write_mcp_config(&mcp_path, bin_path, false)?;

    let rules_dir = cwd.join(".windsurf").join("rules");
    std::fs::create_dir_all(&rules_dir)?;
    let rules_path = rules_dir.join("pearl.md");
    let content = format!(
        "---\ntrigger: model_decision\ndescription: Use pearl for Obsidian knowledge base queries\n---\n{}",
        VAULT_SEARCH_SKILL_DESCRIPTION
    );
    std::fs::write(&rules_path, content)?;

    cliclack::log::success(format!(
        "Windsurf: {} (global) + {}",
        mcp_path.display(),
        rules_path
            .strip_prefix(cwd)
            .unwrap_or(&rules_path)
            .display(),
    ))?;
    Ok(())
}

fn extract_skill_field<'a>(content: &'a str, field: &str) -> Option<&'a str> {
    let fm = content.strip_prefix("---")?;
    let end = fm.find("---")?;
    let frontmatter = &fm[..end];
    let prefix = format!("{}:", field);
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix(&prefix) {
            return Some(v.trim().trim_matches('"').trim_matches('\''));
        }
    }
    None
}

fn install_opencode(_cwd: &Path, bin_path: &str) -> Result<()> {
    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let skill_dir = home.join(".opencode").join("skills").join("pearl");
    std::fs::create_dir_all(&skill_dir)?;

    let skill_path = skill_dir.join("SKILL.md");
    const SKILL_UPDATED_AT: &str = "2025-05-15";

    if skill_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&skill_path) {
            if let Some(old_date) = extract_skill_field(&existing, "updated_at") {
                if old_date == SKILL_UPDATED_AT {
                    cliclack::log::info(format!(
                        "OpenCode: skill already up-to-date ({})",
                        SKILL_UPDATED_AT
                    ))?;
                    return Ok(());
                }
                cliclack::log::step(format!(
                    "OpenCode: updating skill {} -> {}",
                    old_date, SKILL_UPDATED_AT
                ))?;
            }
        }
    }

    let content = format!(
        r#"---
name: pearl
updated_at: "{updated_at}"
description: Semantic search over Obsidian vaults using pearl CLI. Use when the user asks to search their notes, find related content, look up something in their vault, or needs context from their knowledge base. Supports hybrid (vector + keyword), semantic-only, and full-text search with folder/tag filtering.
---

# pearl

Local-first semantic search for Obsidian vaults. Provides hybrid (vector + full-text) search over markdown notes.

Binary: `{bin_path}` (must be installed and configured via `pearl init`).

## Command reference

```bash
pearl search <QUERY> [--mode hybrid|semantic|fts] [--top-k N] [--json]
pearl index [--force] [--vault PATH]
pearl serve [--vault PATH]
pearl config [--json]
```

## Search modes

| Mode | Description |
|------|-------------|
| `hybrid` | Semantic + full-text combined, unified ranking (default) |
| `semantic` | Vector similarity only |
| `fts` | Keyword matching only |

## Common patterns

```bash
# Search vault (most common)
pearl search --json "your question here"

# More results
pearl search -k 20 --json "error handling patterns"

# Restrict to a folder
pearl search -f wiki/ --json "architecture"

# Filter by tag
pearl search -t project --json "status update"

# Check health
pearl config
```

## Tips

- Default mode is `hybrid` — combines semantic understanding with keyword matching
- Index auto-updates via file watcher when running `serve` or `index`
- Results include file path, relevance score, and matched text chunk
- Use `--json` for machine-readable output
"#,
        updated_at = SKILL_UPDATED_AT,
        bin_path = bin_path,
    );
    std::fs::write(&skill_path, &content)?;

    cliclack::log::success(format!("OpenCode: {}", skill_path.display()))?;
    Ok(())
}

fn install_codex(_cwd: &Path, bin_path: &str) -> Result<()> {
    let home = dirs_home().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let codex_dir = home.join(".codex");

    let config_path = codex_dir.join("config.toml");
    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        if !content.contains("mcp_servers.pearl") {
            let mcp_block = format!(
                "\n[mcp_servers.pearl]\ncommand = \"{}\"\nargs = [\"serve\"]\nenabled = true\n",
                bin_path
            );
            std::fs::write(&config_path, format!("{}{}", content, mcp_block))?;
        }
    } else {
        std::fs::create_dir_all(&codex_dir)?;
        let content = format!(
            "[mcp_servers.pearl]\ncommand = \"{}\"\nargs = [\"serve\"]\nenabled = true\n",
            bin_path
        );
        std::fs::write(&config_path, content)?;
    }

    let skill_dir = codex_dir.join("skills").join("pearl");
    std::fs::create_dir_all(&skill_dir)?;

    let skill_path = skill_dir.join("SKILL.md");
    const SKILL_UPDATED_AT: &str = "2025-05-15";

    if skill_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&skill_path) {
            if let Some(old_date) = extract_skill_field(&existing, "updated_at") {
                if old_date == SKILL_UPDATED_AT {
                    cliclack::log::info(format!(
                        "Codex: skill already up-to-date ({})",
                        SKILL_UPDATED_AT
                    ))?;
                    return Ok(());
                }
            }
        }
    }

    let content = format!(
        r#"---
name: pearl
updated_at: "{updated_at}"
description: Semantic search over Obsidian vaults using pearl CLI.
---

# pearl

Local-first semantic search for Obsidian vaults.

Binary: `{bin_path}`

## Usage

```bash
pearl search --json "query"
pearl search -m semantic -k 20 --json "query"
pearl search -f folder/ --json "query"
pearl config
```
"#,
        updated_at = SKILL_UPDATED_AT,
        bin_path = bin_path,
    );
    std::fs::write(&skill_path, &content)?;

    cliclack::log::success(format!(
        "Codex: {} + {}",
        config_path.display(),
        skill_path.display(),
    ))?;
    Ok(())
}
