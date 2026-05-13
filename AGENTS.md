# AGENTS.md

## Project Overview

**vault-search-mcp** is a local-first semantic search MCP server for Obsidian vaults, written in Rust. It provides hybrid search (vector + full-text) over markdown notes, exposed via the Model Context Protocol (stdio transport) for use by AI agents and code editors.

## Architecture

```
src/
├── main.rs              CLI entry point (clap): serve | index | search | init
├── config.rs            Config loading (TOML file + env vars + defaults)
├── server.rs            MCP server (rmcp, stdio, tool routing)
├── indexer.rs           Incremental vault indexing (hash-based change detection)
├── search.rs            Hybrid search orchestration (vector + FTS merge)
└── core/
    ├── embedder.rs      Embedding API client (OpenAI-compatible /v1/embeddings)
    ├── chunker.rs       Markdown chunking (heading-aware, token-budgeted)
    ├── vector_store.rs  In-memory vector index (JSON persistence, cosine similarity)
    ├── fts.rs           Full-text search (tantivy)
    ├── vault.rs         Vault filesystem scanning, hashing, file I/O
    └── frontmatter.rs   YAML frontmatter parser
```

## Key Design Decisions

- **Local-first**: All data stays in `{vault}/.vault-mcp/` — vectors, hashes, FTS index, config.
- **Incremental indexing**: SHA-256 hash per file; only re-embeds changed files.
- **Provider-agnostic embedding**: Supports Ollama (local), OpenAI, or any OpenAI-compatible endpoint.
- **Dimension mismatch detection**: Automatically forces full reindex when switching embedding models.
- **Auto-detection**: Detects Ollama endpoint (via `OLLAMA_HOST` env + port probing) and vault location (CWD traversal + common directories).

## Building

```bash
cargo build --release
# Binary at: target/release/vault-search-mcp
```

## Running

```bash
# Interactive setup
vault-search-mcp init

# Index vault
vault-search-mcp index --vault /path/to/vault

# Start MCP server
vault-search-mcp serve --vault /path/to/vault

# CLI search (for testing)
vault-search-mcp search --vault /path/to/vault "query"
```

## Configuration

Config is loaded from `{vault}/.vault-mcp/config.toml`, with env var overrides:

```toml
[embedding]
provider = "ollama"           # ollama | openai | custom
endpoint = "http://localhost:11434"
model = "bge-m3"
# api_key = "$OPENAI_API_KEY"  # prefix with $ to read from env
# dimensions = 1536            # optional, for models that support it

[search]
vector_weight = 0.7
fts_weight = 0.3
default_limit = 10
```

Environment variables (override config file):
- `EMBEDDING_ENDPOINT` — embedding API base URL
- `EMBEDDING_MODEL` — model name
- `EMBEDDING_PROVIDER` — ollama/openai/custom
- `EMBEDDING_API_KEY` — API key (or set provider-specific like `OPENAI_API_KEY`)
- `VAULT_PATH` — vault path (used by all subcommands via `--vault`)

## MCP Tools Exposed

| Tool | Description |
|------|-------------|
| `hybrid_search` | Semantic + full-text search with folder/tag filters |
| `index_vault` | Trigger indexing (incremental or forced full) |
| `get_note` | Read a note by relative path |
| `list_notes` | List files/directories in vault |
| `vault_status` | Health check: embedding service, config, vault info |

## Testing

```bash
# Quick search test
vault-search-mcp search --vault ~/my-vault "some query"

# Check embedding service connectivity
vault-search-mcp serve --vault ~/my-vault
# Then send vault_status via MCP
```

## Release Process

1. Tag a version: `git tag v0.1.0 && git push origin v0.1.0`
2. CI builds multi-platform binaries (macOS x86/arm64, Linux x86/arm64)
3. GitHub Release created automatically with checksums
4. Homebrew formula auto-updated in tap repo

## Code Conventions

- Rust edition 2024, stable toolchain
- Error handling: `anyhow::Result` throughout, `tracing` for logging
- Serialization: `serde` + `serde_json` for data, `toml` for config
- HTTP: `ureq` (sync, minimal dependencies)
- CLI: `clap` derive API with env var support
- Interactive prompts: `dialoguer` + `console` for styling
