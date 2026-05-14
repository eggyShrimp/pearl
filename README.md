# vault-search

Local-first semantic search for Obsidian vaults. `vault-search` indexes local
Markdown notes and exposes hybrid search through a CLI and MCP server.

## Install

For agent-driven installation, follow [install.md](install.md). The expected
path is Homebrew installing the published release binary, not building from
source.

Quick install:

```bash
brew tap eggyShrimp/tap
brew install eggyShrimp/tap/vault-search
vault-search --version
```

## Quick Start

```bash
vault-search init
vault-search index
vault-search search "my query" --json
```

If you are outside the vault directory, pass the vault path:

```bash
vault-search init --vault /path/to/vault
vault-search index --vault /path/to/vault
vault-search search --vault /path/to/vault "my query" --json
```

## MCP Server

Start the MCP server for an agent or editor:

```bash
vault-search serve --vault /path/to/vault
```

Available tools:

| Tool | Purpose |
| --- | --- |
| `hybrid_search` | Search notes with semantic and keyword matching |
| `index_vault` | Refresh the vault index |
| `get_note` | Read a note by relative path |
| `list_notes` | List notes and folders |
| `vault_status` | Check vault and embedding status |

## Configuration

Config lives in the vault at `.vault-mcp/config.toml`.

```toml
[embedding]
provider = "ollama"
endpoint = "http://localhost:11434"
model = "bge-m3"

[search]
vector_weight = 0.7
fts_weight = 0.3
default_limit = 10
```

Environment overrides:

| Variable | Purpose |
| --- | --- |
| `VAULT_PATH` | Default vault path |
| `EMBEDDING_PROVIDER` | `ollama`, `openai`, or `custom` |
| `EMBEDDING_ENDPOINT` | OpenAI-compatible embedding endpoint |
| `EMBEDDING_MODEL` | Embedding model name |
| `EMBEDDING_API_KEY` | Embedding API key |

## Build From Source

Users should normally install the release binary with Homebrew. For development:

```bash
cargo build --release
```

The binary is written to `target/release/vault-search`.
