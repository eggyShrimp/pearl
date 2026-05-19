# TODO

## UX Improvements

- [ ] 搜索结果对用户不友好：agent 应直接总结回答问题，不应展示原始搜索结果或额外确认"从 vault 中搜索到哪些内容"

## Low Priority Optimizations

- [ ] MCP 多端口支持：作为后续隔离能力评估，例如不同模型、不同 vault、本机和局域网分开服务。不要把它当作并发性能的优先优化；单端口共享应先保持稳定，并且状态返回需要能列出所有运行中的 MCP 地址，避免后启动的端口覆盖旧状态。

## Graph-Aware Retrieval (GraphRAG + Wiki Wikilinks)

Pearl should evolve from a flat search engine into a graph-aware retrieval engine. Obsidian `[[wikilinks]]` are explicit, high-quality relationships that form a knowledge graph without LLM extraction. This aligns with the GraphRAG approach (Microsoft Research, 2024) but skips the most expensive step — entity/relationship extraction from unstructured text.

### Why

The Agent Workflow Supervisor project needs Pearl to support graph-based memory retrieval and invalidation propagation. Flat search returns isolated matches; graph retrieval returns local subgraphs with causal context, enabling better policy judgment and memory lifecycle management.

### Indexing: Parse and Store the Link Graph

- [ ] During indexing, parse `[[wikilinks]]` (including aliases `[[target|alias]]`) from each markdown file
- [ ] Store the link graph in SQLite: `links(source_path, target_path, context_snippet)`
- [ ] Build reverse index for backlink queries
- [ ] Incremental update: when a file changes, update only its outgoing links

### New Retrieval Capabilities

- [ ] `expand(node, depth) → subgraph` — follow wikilinks outward N hops from a node, return the local subgraph with node content and edge directions
- [ ] `backlinks(node) → nodes[]` — return all nodes that link to the given node (reverse edges)
- [ ] `propagate(node) → affected_nodes[]` — from an invalidated node, traverse links to find dependent nodes that may need re-evaluation (bounded traversal)

### MCP Tool Extensions

- [ ] Expose `graph_expand` tool: given a note path and depth, return the local subgraph as structured JSON
- [ ] Expose `backlinks` tool: given a note path, return all notes linking to it
- [ ] Expose `graph_search` tool: hybrid search as entry point, then auto-expand top results by 1 hop to include linked context

### Future: Community Detection

- [ ] Implement Leiden or similar community detection algorithm over the wikilink graph to identify knowledge clusters
- [ ] Generate community-level summaries for macro queries (LLM call, cacheable and incrementally updatable)
- [ ] Expose `communities` tool: list knowledge clusters with summaries

### Design Constraints

- Graph parsing, storage, and traversal are Rust-native (performance-critical local operations)
- Semantic judgment (relevance, validity, invalidation decisions) stays in the Supervisor layer (TypeScript + LLM)
- Pearl does not make policy decisions — it provides graph structure and content; Supervisor interprets it
- All graph data stored locally in `{vault}/.vault-mcp/` alongside existing vector and FTS indexes

### References

- Microsoft GraphRAG (2024): entity extraction → graph → community detection → subgraph retrieval. Pearl skips extraction by using wikilinks directly.
- Truth Maintenance Systems (Doyle 1979): dependency-directed retraction propagates along justification chains. Pearl's `propagate` operation follows the same principle over wikilinks.
- Spreading Activation (Collins & Loftus 1975): bounded subgraph expansion from activated nodes. Pearl's `expand` is bounded spreading activation over the wikilink graph.
