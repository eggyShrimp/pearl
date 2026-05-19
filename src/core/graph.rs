//! Graph storage and traversal over Obsidian wikilinks.
//!
//! Uses SQLite for persistent storage of the link graph, with support for:
//! - Forward links (outgoing edges from a note)
//! - Backlinks (incoming edges to a note)
//! - Bounded subgraph expansion (spreading activation)
//! - Invalidation propagation along link chains

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Serialize;

// ─── Wikilink Parsing ────────────────────────────────────────────────────────

/// A parsed wikilink from a markdown file.
#[derive(Debug, Clone)]
pub struct WikiLink {
    /// The target path (resolved relative to vault root, without .md extension in source)
    pub target: String,
    /// Optional display alias (from [[target|alias]])
    pub alias: Option<String>,
    /// Surrounding context snippet (the line containing the link)
    pub context: String,
}

/// Parse all `[[wikilinks]]` from markdown content.
///
/// Supports:
/// - `[[target]]`
/// - `[[target|alias]]`
/// - `[[target#heading]]`
/// - `[[target#heading|alias]]`
///
/// Does NOT parse:
/// - Links inside code blocks (fenced or inline)
/// - Markdown standard links `[text](url)`
pub fn parse_wikilinks(content: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let mut in_code_block = false;

    for line in content.lines() {
        let trimmed = line.trim_start();

        // Track fenced code blocks
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        // Find all [[...]] in this line
        let mut pos = 0;
        let bytes = line.as_bytes();
        while pos + 1 < bytes.len() {
            // Skip inline code spans
            if bytes[pos] == b'`' {
                pos += 1;
                while pos < bytes.len() && bytes[pos] != b'`' {
                    pos += 1;
                }
                pos += 1; // skip closing `
                continue;
            }

            if bytes[pos] == b'[' && pos + 1 < bytes.len() && bytes[pos + 1] == b'[' {
                // Found opening [[
                let start = pos + 2;
                pos = start;

                // Find closing ]]
                let mut end = None;
                while pos + 1 < bytes.len() {
                    if bytes[pos] == b']' && bytes[pos + 1] == b']' {
                        end = Some(pos);
                        break;
                    }
                    pos += 1;
                }

                if let Some(end_pos) = end {
                    let inner = &line[start..end_pos];
                    if !inner.is_empty() && !inner.contains('\n') {
                        // Parse target and alias
                        let (target_raw, alias) = if let Some(pipe_pos) = inner.find('|') {
                            (&inner[..pipe_pos], Some(inner[pipe_pos + 1..].to_string()))
                        } else {
                            (inner, None)
                        };

                        // Strip heading fragment for path resolution
                        let target = if let Some(hash_pos) = target_raw.find('#') {
                            &target_raw[..hash_pos]
                        } else {
                            target_raw
                        };

                        // Only include if target is non-empty (skip [[#heading-only]] links)
                        if !target.is_empty() {
                            links.push(WikiLink {
                                target: target.to_string(),
                                alias,
                                context: line.to_string(),
                            });
                        }
                    }
                    pos = end_pos + 2;
                } else {
                    pos += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    links
}

/// Resolve a wikilink target to a vault-relative path.
///
/// Obsidian wikilinks can be:
/// - Bare filename: `[[note]]` → matches any `**/note.md`
/// - Relative path: `[[folder/note]]` → `folder/note.md`
///
/// `known_paths` is the set of all vault-relative .md file paths (e.g. "wiki/topics/RAG.md").
/// Returns the best match, or None if unresolved.
pub fn resolve_wikilink(
    target: &str,
    source_dir: &str,
    known_paths: &HashSet<String>,
) -> Option<String> {
    let target_normalized = target.replace('\\', "/");

    // If target already has .md extension, try direct match
    let with_ext = if target_normalized.ends_with(".md") {
        target_normalized.clone()
    } else {
        format!("{}.md", target_normalized)
    };

    // 1. Try exact path match
    if known_paths.contains(&with_ext) {
        return Some(with_ext);
    }

    // 2. Try relative to source file's directory
    if !source_dir.is_empty() {
        let relative = format!("{}/{}", source_dir, with_ext);
        if known_paths.contains(&relative) {
            return Some(relative);
        }
    }

    // 3. Shortest-path match (Obsidian behavior: bare filename matches any path)
    let filename = with_ext.rsplit('/').next().unwrap_or(&with_ext);
    let mut matches: Vec<&String> = known_paths
        .iter()
        .filter(|p| {
            p.ends_with(filename)
                && (p.len() == filename.len() || p.as_bytes()[p.len() - filename.len() - 1] == b'/')
        })
        .collect();

    if matches.len() == 1 {
        return Some(matches[0].clone());
    }

    // Multiple matches: prefer shortest path (closest to root)
    if matches.len() > 1 {
        matches.sort_by_key(|p| p.len());
        return Some(matches[0].clone());
    }

    // Unresolved — store the normalized target for future resolution
    None
}

// ─── Graph Database ──────────────────────────────────────────────────────────

/// SQLite-backed graph store for wikilink relationships.
pub struct GraphStore {
    conn: Connection,
}

/// A node in the graph with its content metadata.
#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub path: String,
    pub title: String,
    pub linked_from: Option<String>, // context snippet from the linking note
}

/// An edge in the graph.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub context: String,
}

/// A local subgraph returned by expand operations.
#[derive(Debug, Clone, Serialize)]
pub struct SubGraph {
    pub center: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub depth: usize,
}

impl GraphStore {
    /// Open or create the graph database at the given path.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;

        // Enable WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS links (
                id INTEGER PRIMARY KEY,
                source_path TEXT NOT NULL,
                target_path TEXT NOT NULL,
                context TEXT NOT NULL DEFAULT ''
            );

            CREATE INDEX IF NOT EXISTS idx_links_source ON links(source_path);
            CREATE INDEX IF NOT EXISTS idx_links_target ON links(target_path);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_links_unique ON links(source_path, target_path, context);
            ",
        )?;

        Ok(Self { conn })
    }

    /// Update links for a single source file (incremental).
    /// Removes all existing outgoing links from `source_path` and inserts new ones.
    pub fn update_file_links(&self, source_path: &str, links: &[(String, String)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        // Delete old links from this source
        tx.execute(
            "DELETE FROM links WHERE source_path = ?1",
            params![source_path],
        )?;

        // Insert new links
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO links (source_path, target_path, context) VALUES (?1, ?2, ?3)",
            )?;

            for (target_path, context) in links {
                stmt.execute(params![source_path, target_path, context])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Remove all links from a deleted file.
    pub fn remove_file(&self, source_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM links WHERE source_path = ?1",
            params![source_path],
        )?;
        Ok(())
    }

    /// Clear the entire graph (for forced reindex).
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM links", [])?;
        Ok(())
    }

    /// Get all outgoing links from a node.
    pub fn outgoing_links(&self, path: &str) -> Result<Vec<GraphEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_path, target_path, context FROM links WHERE source_path = ?1",
        )?;

        let edges = stmt
            .query_map(params![path], |row| {
                Ok(GraphEdge {
                    source: row.get(0)?,
                    target: row.get(1)?,
                    context: row.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(edges)
    }

    /// Get all incoming links to a node (backlinks).
    pub fn backlinks(&self, path: &str) -> Result<Vec<GraphEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_path, target_path, context FROM links WHERE target_path = ?1",
        )?;

        let edges = stmt
            .query_map(params![path], |row| {
                Ok(GraphEdge {
                    source: row.get(0)?,
                    target: row.get(1)?,
                    context: row.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(edges)
    }

    /// Expand a subgraph from a center node, following links up to `depth` hops.
    /// Follows both outgoing and incoming links (undirected traversal).
    pub fn expand(&self, center: &str, depth: usize) -> Result<SubGraph> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        let mut all_edges: Vec<GraphEdge> = Vec::new();

        visited.insert(center.to_string());
        queue.push_back((center.to_string(), 0));

        while let Some((node, current_depth)) = queue.pop_front() {
            if current_depth >= depth {
                continue;
            }

            // Get outgoing links
            let outgoing = self.outgoing_links(&node)?;
            for edge in &outgoing {
                if !visited.contains(&edge.target) {
                    visited.insert(edge.target.clone());
                    queue.push_back((edge.target.clone(), current_depth + 1));
                }
            }
            all_edges.extend(outgoing);

            // Get incoming links (backlinks)
            let incoming = self.backlinks(&node)?;
            for edge in &incoming {
                if !visited.contains(&edge.source) {
                    visited.insert(edge.source.clone());
                    queue.push_back((edge.source.clone(), current_depth + 1));
                }
            }
            all_edges.extend(incoming);
        }

        // Deduplicate edges
        let mut seen_edges: HashSet<(String, String)> = HashSet::new();
        all_edges.retain(|e| seen_edges.insert((e.source.clone(), e.target.clone())));

        // Build node list
        let nodes: Vec<GraphNode> = visited
            .iter()
            .map(|path| GraphNode {
                path: path.clone(),
                title: path
                    .trim_end_matches(".md")
                    .rsplit('/')
                    .next()
                    .unwrap_or(path)
                    .to_string(),
                linked_from: None,
            })
            .collect();

        Ok(SubGraph {
            center: center.to_string(),
            nodes,
            edges: all_edges,
            depth,
        })
    }

    /// Propagate from an invalidated node: follow outgoing links (forward direction)
    /// to find all transitively dependent nodes. Bounded by max_depth.
    ///
    /// This follows the Truth Maintenance System principle: invalidation propagates
    /// along justification/dependency edges (outgoing wikilinks = "this note references that").
    /// A note that links TO the invalidated note is potentially affected.
    pub fn propagate(&self, start: &str, max_depth: usize) -> Result<Vec<String>> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        let mut affected: Vec<String> = Vec::new();

        visited.insert(start.to_string());
        queue.push_back((start.to_string(), 0));

        while let Some((node, current_depth)) = queue.pop_front() {
            if current_depth >= max_depth {
                continue;
            }

            // Propagation follows BACKLINKS: notes that reference the invalidated node
            // are potentially affected (their content depends on the invalidated node)
            let incoming = self.backlinks(&node)?;
            for edge in incoming {
                if !visited.contains(&edge.source) {
                    visited.insert(edge.source.clone());
                    affected.push(edge.source.clone());
                    queue.push_back((edge.source, current_depth + 1));
                }
            }
        }

        Ok(affected)
    }

    /// Get graph statistics.
    pub fn stats(&self) -> Result<GraphStats> {
        let node_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM (
                SELECT source_path AS path FROM links
                UNION
                SELECT target_path AS path FROM links
            )",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let edge_count: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM links", [], |row| row.get(0))?;

        Ok(GraphStats {
            node_count,
            edge_count,
        })
    }
}

/// Graph statistics.
#[derive(Debug, Clone, Serialize)]
pub struct GraphStats {
    pub node_count: usize,
    pub edge_count: usize,
}

// ─── Integration Helpers ─────────────────────────────────────────────────────

/// Extract and resolve wikilinks from a file's content, returning (target_path, context) pairs.
pub fn extract_links(
    content: &str,
    source_path: &str,
    known_paths: &HashSet<String>,
) -> Vec<(String, String)> {
    let source_dir = source_path
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or("");

    let wikilinks = parse_wikilinks(content);

    wikilinks
        .into_iter()
        .filter_map(|link| {
            let resolved = resolve_wikilink(&link.target, source_dir, known_paths)?;
            Some((resolved, link.context))
        })
        .collect()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_wikilinks() {
        let content = "Some text with [[note1]] and [[folder/note2]] here.";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "note1");
        assert_eq!(links[1].target, "folder/note2");
    }

    #[test]
    fn test_parse_wikilink_with_alias() {
        let content = "See [[target note|display text]] for details.";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "target note");
        assert_eq!(links[0].alias, Some("display text".to_string()));
    }

    #[test]
    fn test_parse_wikilink_with_heading() {
        let content = "See [[note#section]] and [[note#section|alias]].";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "note");
        assert_eq!(links[1].target, "note");
        assert_eq!(links[1].alias, Some("alias".to_string()));
    }

    #[test]
    fn test_skip_code_blocks() {
        let content = "Normal [[link1]]\n```\n[[code_link]]\n```\n[[link2]]";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "link1");
        assert_eq!(links[1].target, "link2");
    }

    #[test]
    fn test_skip_inline_code() {
        let content = "Normal [[link1]] and `[[not_a_link]]` and [[link2]]";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "link1");
        assert_eq!(links[1].target, "link2");
    }

    #[test]
    fn test_skip_heading_only_links() {
        let content = "See [[#some-heading]] for context.";
        let links = parse_wikilinks(content);
        assert_eq!(links.len(), 0);
    }

    #[test]
    fn test_resolve_wikilink_exact() {
        let known: HashSet<String> = vec![
            "wiki/topics/RAG.md".to_string(),
            "notes/daily.md".to_string(),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            resolve_wikilink("wiki/topics/RAG", "", &known),
            Some("wiki/topics/RAG.md".to_string())
        );
    }

    #[test]
    fn test_resolve_wikilink_bare_filename() {
        let known: HashSet<String> = vec![
            "wiki/topics/RAG.md".to_string(),
            "notes/daily.md".to_string(),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            resolve_wikilink("RAG", "", &known),
            Some("wiki/topics/RAG.md".to_string())
        );
    }

    #[test]
    fn test_resolve_wikilink_relative() {
        let known: HashSet<String> = vec![
            "wiki/topics/RAG.md".to_string(),
            "wiki/topics/sub/detail.md".to_string(),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            resolve_wikilink("sub/detail", "wiki/topics", &known),
            Some("wiki/topics/sub/detail.md".to_string())
        );
    }

    #[test]
    fn test_graph_store_basic() {
        let store = GraphStore::open(Path::new(":memory:")).unwrap();

        store
            .update_file_links(
                "a.md",
                &[
                    ("b.md".to_string(), "links to b".to_string()),
                    ("c.md".to_string(), "links to c".to_string()),
                ],
            )
            .unwrap();

        store
            .update_file_links("b.md", &[("c.md".to_string(), "b links to c".to_string())])
            .unwrap();

        // Test outgoing links
        let out = store.outgoing_links("a.md").unwrap();
        assert_eq!(out.len(), 2);

        // Test backlinks
        let back = store.backlinks("c.md").unwrap();
        assert_eq!(back.len(), 2); // a.md and b.md both link to c.md

        // Test expand
        let subgraph = store.expand("a.md", 1).unwrap();
        assert!(subgraph.nodes.len() >= 3); // a, b, c

        // Test propagate from c.md (who depends on c?)
        let affected = store.propagate("c.md", 2).unwrap();
        assert!(affected.contains(&"a.md".to_string()));
        assert!(affected.contains(&"b.md".to_string()));
    }

    #[test]
    fn test_graph_store_incremental_update() {
        let store = GraphStore::open(Path::new(":memory:")).unwrap();

        // Initial links
        store
            .update_file_links(
                "a.md",
                &[
                    ("b.md".to_string(), "ctx1".to_string()),
                    ("c.md".to_string(), "ctx2".to_string()),
                ],
            )
            .unwrap();

        // Update: a.md now only links to d.md
        store
            .update_file_links("a.md", &[("d.md".to_string(), "ctx3".to_string())])
            .unwrap();

        let out = store.outgoing_links("a.md").unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, "d.md");
    }

    #[test]
    fn test_graph_store_remove_file() {
        let store = GraphStore::open(Path::new(":memory:")).unwrap();

        store
            .update_file_links("a.md", &[("b.md".to_string(), "ctx".to_string())])
            .unwrap();

        store.remove_file("a.md").unwrap();

        let out = store.outgoing_links("a.md").unwrap();
        assert_eq!(out.len(), 0);
    }
}
