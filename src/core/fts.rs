use std::path::Path;

use anyhow::Result;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, doc};

/// Full-text search engine backed by tantivy.
pub struct FtsEngine {
    index: Index,
    reader: IndexReader,
    path_field: Field,
    title_field: Field,
    body_field: Field,
    tags_field: Field,
}

impl FtsEngine {
    /// Open or create the FTS index.
    pub fn open(index_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(index_path)?;

        let mut schema_builder = Schema::builder();
        let path_field = schema_builder.add_text_field("path", STRING | STORED);
        let title_field = schema_builder.add_text_field("title", TEXT | STORED);
        let body_field = schema_builder.add_text_field("body", TEXT);
        let tags_field = schema_builder.add_text_field("tags", TEXT | STORED);
        let schema = schema_builder.build();

        let index = if index_path.join("meta.json").exists() {
            Index::open_in_dir(index_path)?
        } else {
            Index::create_in_dir(index_path, schema.clone())?
        };

        let reader = index.reader()?;

        Ok(Self {
            index,
            reader,
            path_field,
            title_field,
            body_field,
            tags_field,
        })
    }

    /// Check if the FTS index has any documents.
    pub fn has_documents(&self) -> Result<bool> {
        let searcher = self.reader.searcher();
        Ok(searcher.num_docs() > 0)
    }

    /// Rebuild the entire FTS index from scratch.
    pub fn rebuild(&self, documents: Vec<FtsDocument>) -> Result<()> {
        let mut writer: IndexWriter = self.index.writer(50_000_000)?;
        writer.delete_all_documents()?;

        for doc_data in documents {
            writer.add_document(doc!(
                self.path_field => doc_data.path,
                self.title_field => doc_data.title,
                self.body_field => doc_data.body,
                self.tags_field => doc_data.tags,
            ))?;
        }

        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// Incrementally update the FTS index: delete changed/removed docs, then add new versions.
    pub fn update(&self, new_docs: Vec<FtsDocument>, deleted_paths: &[&str]) -> Result<()> {
        let mut writer: IndexWriter = self.index.writer(50_000_000)?;

        // Delete documents that were changed or removed (by path term)
        for doc in &new_docs {
            let term = tantivy::Term::from_field_text(self.path_field, &doc.path);
            writer.delete_term(term);
        }
        for path in deleted_paths {
            let term = tantivy::Term::from_field_text(self.path_field, path);
            writer.delete_term(term);
        }

        // Add new/updated documents
        for doc_data in new_docs {
            writer.add_document(doc!(
                self.path_field => doc_data.path,
                self.title_field => doc_data.title,
                self.body_field => doc_data.body,
                self.tags_field => doc_data.tags,
            ))?;
        }

        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// Search the FTS index.
    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<FtsHit>> {
        let searcher = self.reader.searcher();
        let query_parser = QueryParser::for_index(
            &self.index,
            vec![self.title_field, self.body_field, self.tags_field],
        );

        // Tantivy doesn't do well with CJK out of the box,
        // so we also try splitting query into individual characters for CJK
        let query = query_parser.parse_query(query_str).unwrap_or_else(|_| {
            // Fallback: treat entire query as a phrase
            query_parser
                .parse_query(&format!("\"{}\"", query_str))
                .unwrap_or_else(|_| Box::new(tantivy::query::AllQuery))
        });

        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit))?;

        let mut results = Vec::new();
        for (score, doc_addr) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_addr)?;
            let path = doc
                .get_first(self.path_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title = doc
                .get_first(self.title_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            results.push(FtsHit { path, title, score });
        }

        Ok(results)
    }
}

pub struct FtsDocument {
    pub path: String,
    pub title: String,
    pub body: String,
    pub tags: String,
}

#[derive(Debug, Clone)]
pub struct FtsHit {
    pub path: String,
    pub title: String,
    pub score: f32,
}
