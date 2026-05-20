<div align="center">
<pre>
                       _
 _ __   ___  __ _ _ __| |
| '_ \ / _ \/ _` | '__| |
| |_) |  __/ (_| | |  | |
| .__/ \___|\__,_|_|  |_|
|_|
</pre>
</div>

<p align="center">
  <a href="https://github.com/eggyShrimp/pearl/releases"><img src="https://img.shields.io/github/v/release/eggyShrimp/pearl" alt="Release" /></a>
  <a href="https://github.com/eggyShrimp/pearl/blob/main/LICENSE"><img src="https://img.shields.io/github/license/eggyShrimp/pearl" alt="Unlicense" /></a>
</p>

---

**Pearl** indexes your local Markdown notes and exposes hybrid search (vector + full-text) through a CLI and an [MCP](https://modelcontextprotocol.io/) server, so AI agents and editors can search your vault semantically.

## Install

```bash
brew tap eggyShrimp/tap
brew install eggyShrimp/tap/pearl
pearl --version
```

For agent-driven installation details, see [install.md](install.md).

## Quick Start

```bash
pearl init                        # interactive setup
pearl index                       # build index
pearl search "my query" --json    # search
```

If you are outside the vault directory, pass the vault path explicitly:

```bash
pearl init --vault /path/to/vault
pearl index --vault /path/to/vault
pearl search --vault /path/to/vault "my query" --json
```

## MCP Server

Start the MCP server for use with AI agents or editors:

```bash
pearl serve --vault /path/to/vault
```

Exposed tools:

| Tool | Purpose |
| --- | --- |
| `hybrid_search` | Semantic + keyword search with folder/tag filters |
| `index_vault` | Refresh the vault index (incremental or full) |
| `get_note` | Read a note by relative path |
| `list_notes` | List notes and folders |
| `vault_status` | Check vault health, embedding service, and config |

## Configuration

Config lives at `{vault}/.vault-mcp/config.toml`. Run `pearl init` to generate it interactively.

```toml
[embedding]
provider = "ollama"              # ollama | openai | custom
endpoint = "http://localhost:11434"
model = "bge-m3"

[search]
vector_weight = 0.7
fts_weight = 0.3
default_limit = 10
```

Environment variable overrides:

| Variable | Purpose |
| --- | --- |
| `VAULT_PATH` | Default vault path |
| `EMBEDDING_PROVIDER` | `ollama`, `openai`, or `custom` |
| `EMBEDDING_ENDPOINT` | OpenAI-compatible embedding endpoint |
| `EMBEDDING_MODEL` | Embedding model name |
| `EMBEDDING_API_KEY` | Embedding API key |

## Build From Source

Most users should install with Homebrew. For development:

```bash
cargo build --release
# Binary at target/release/pearl
```

## License

[Unlicense](LICENSE)
