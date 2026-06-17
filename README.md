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
  <a href="https://github.com/eggyShrimp/pearl/blob/main/LICENSE"><img src="https://img.shields.io/github/license/eggyShrimp/pearl" alt="MIT" /></a>
</p>

---

**Pearl** indexes your local Markdown notes and exposes hybrid search (vector + full-text) through a CLI and an [MCP](https://modelcontextprotocol.io/) server, so AI agents and editors can search your vault semantically.

## Why Pearl?

**You have thousands of notes in Obsidian, but the built-in search can't find what you're thinking of.**

- You remember writing about "reducing hallucination in RAG pipelines", but search for "hallucination" returns nothing — because the note says "factuality" instead.
- You want your AI coding agent to automatically pull context from your notes, but there's no local-first MCP tool for that.
- You use ripgrep to search notes, but it only matches exact strings — it won't connect "CAP theorem" with "distributed systems tradeoffs".

Pearl combines **vector similarity** (understanding meaning) with **full-text search** (matching exact terms), exposed through a CLI and an MCP server — so both you and your agents can search your vault semantically.

| Use case | What Pearl does |
|----------|----------------|
| "Where did I write that?" | Finds conceptually related notes even with different wording |
| Agent-powered workflows | AI agents search your vault via MCP without leaving the editor |
| Code + notes context | Pull design decisions from your vault into your coding workflow |
| Knowledge exploration | Surface related notes you forgot about |

## Install

```bash
brew tap eggyShrimp/tap
brew install eggyShrimp/tap/pearl
pearl --version
```

```text
Read https://raw.githubusercontent.com/eggyShrimp/pearl/main/install.md and follow the steps to install the pearl binary. After installation, run `pearl --version` to verify it works.
```

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

## Comparison

| Feature | Pearl | Obsidian Search | grep / ripgrep | Obsidian Copilot |
|---------|-------|-----------------|----------------|------------------|
| Semantic search | ✅ | ❌ | ❌ | ✅ |
| Full-text search | ✅ | ✅ | ✅ | ❌ |
| MCP server | ✅ | ❌ | ❌ | ❌ |
| Local-first | ✅ | ✅ | ✅ | ❌ (cloud API) |
| Obsidian-native | ✅ | ✅ | ❌ | ✅ |
| CLI interface | ✅ | ❌ | ✅ | ❌ |

## License

[MIT](LICENSE)
