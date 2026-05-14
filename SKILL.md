---
name: vault-search
description: Semantic search over Obsidian vaults using vault-search CLI. Use when the user asks to search their notes, find related content, look up something in their vault, or needs context from their knowledge base. Supports hybrid (vector + keyword), semantic-only, and full-text search with folder/tag filtering.
---

# vault-search CLI

Local-first semantic search for Obsidian vaults. Combines vector similarity (embeddings) with full-text keyword search to find relevant notes.

Binary: `vault-search` (must be installed and vault must be indexed).

## Core workflow

```bash
# Search (primary use case)
vault-search search "your query" --json

# Ensure index is fresh (run if search returns stale/no results)
vault-search index
```

## Search command

```bash
vault-search search [OPTIONS] <QUERY>
```

### Key options

| Flag | Short | Description |
|------|-------|-------------|
| `--top-k <N>` | `-k` | Number of results (default: 10) |
| `--mode <MODE>` | `-m` | `hybrid` (default), `semantic`, `fts` |
| `--folder <PATH>` | `-f` | Restrict to folder (repeatable) |
| `--exclude <PATH>` | `-e` | Exclude folder (repeatable) |
| `--tag <TAG>` | `-t` | Filter by tag, AND logic (repeatable) |
| `--threshold <SCORE>` | | Minimum score (0.0–1.0) |
| `--context <LINES>` | `-C` | Lines of context around match |
| `--since <YYYY-MM-DD>` | | Only notes modified after date |
| `--json` | | Machine-readable JSON output |
| `--vault <PATH>` | `-v` | Vault path (auto-detected if omitted) |

### JSON output format

Each result in the JSON array:

```json
{
  "path": "wiki/topics/RAG.md",
  "title": "RAG",
  "chunk": "Text content of the matching chunk...",
  "score": 0.847,
  "start_line": 15,
  "end_line": 28,
  "tags": ["ai", "search"],
  "heading": "## Retrieval strategies",
  "match_type": "hybrid"
}
```

`match_type` is one of: `semantic`, `fts`, `hybrid`.

## Common patterns

### Find notes about a topic

```bash
vault-search search -k 5 --json "how does RAG work"
```

### Semantic search (meaning-based, good for concepts)

```bash
vault-search search -m semantic --json "strategies for reducing hallucination"
```

### Keyword search (exact terms, good for names/identifiers)

```bash
vault-search search -m fts --json "LangChain LCEL"
```

### Search within a folder

```bash
vault-search search -f projects/ -k 3 --json "deployment pipeline"
```

### Search excluding archive

```bash
vault-search search -e archive/ -e templates/ --json "weekly review"
```

### Find recent notes on a topic

```bash
vault-search search --since 2025-01-01 --json "product roadmap"
```

### High-confidence results only

```bash
vault-search search --threshold 0.5 -k 20 --json "authentication flow"
```

### Get context lines for precise location

```bash
vault-search search -C 3 "error handling"
```

### Read a specific note after finding it

```bash
# Search returns path, then read the file directly
cat "$(vault-search search -k 1 --json 'topic' | jq -r '.[0].path')"
```

## Index management

```bash
# Incremental index (only changed files, fast)
vault-search index

# Full reindex (after switching embedding model)
vault-search index --force
```

The watcher auto-starts in the background when `index` runs, keeping the index fresh as files change.

## Watcher (auto-indexing)

```bash
vault-search watch            # Foreground
vault-search watch --daemon   # Background daemon
vault-search watch --status   # Check if running
vault-search watch --stop     # Stop daemon
```

The watcher monitors `.md` files, debounces changes (2s), and runs incremental indexing. Other commands auto-spawn the watcher if not running.

## When to use which mode

| Scenario | Mode | Why |
|----------|------|-----|
| Conceptual question ("notes about X") | `hybrid` or `semantic` | Meaning-based matching |
| Exact keyword/name lookup | `fts` | Precise term matching |
| Finding code snippets | `fts` | Identifiers are literal |
| Exploring related ideas | `semantic` | Finds conceptually similar content |
| General search | `hybrid` (default) | Best of both worlds |

## Tips

- Always use `--json` when processing results programmatically
- Use `-k` generously (e.g. `-k 20`) with `--threshold` to get high-quality results without artificial limits
- Combine `-f` and `-e` for precise scoping in large vaults
- If results seem stale, run `vault-search index` to refresh
- The vault path is auto-detected (walks up from CWD looking for `.obsidian` or `.vault-mcp`)
