use std::path::{Path, PathBuf};

use anyhow::Result;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::config::EXCLUDE_DIRS;

#[derive(Debug, Clone)]
pub struct VaultFile {
    /// Relative path from vault root
    pub path: String,
    /// Absolute path
    pub abs_path: PathBuf,
}

/// Scan the vault for .md files, excluding configured directories.
pub fn scan_vault(vault_path: &Path) -> Vec<VaultFile> {
    let mut files = Vec::new();

    for entry in WalkDir::new(vault_path)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                let name = e.file_name().to_str().unwrap_or("");
                !EXCLUDE_DIRS.contains(&name)
            } else {
                true
            }
        })
        .flatten()
    {
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension() {
                if ext == "md" {
                    let rel_path = entry
                        .path()
                        .strip_prefix(vault_path)
                        .unwrap_or(entry.path())
                        .to_string_lossy()
                        .to_string();
                    files.push(VaultFile {
                        path: rel_path,
                        abs_path: entry.into_path(),
                    });
                }
            }
        }
    }

    files
}

/// Compute SHA-256 hash of file content.
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Read a vault file as UTF-8 string.
pub fn read_vault_file(vault_path: &Path, relative_path: &str) -> Result<String> {
    let abs_path = vault_path.join(relative_path);
    Ok(std::fs::read_to_string(abs_path)?)
}

/// List directory entries.
pub fn list_directory(vault_path: &Path, folder: &str, recursive: bool) -> Vec<DirEntry> {
    let dir = vault_path.join(folder);
    let mut entries = Vec::new();

    if !dir.exists() {
        return entries;
    }

    if recursive {
        for entry in WalkDir::new(&dir)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_str().unwrap_or("");
                !EXCLUDE_DIRS.contains(&name)
            })
            .flatten()
            .skip(1)
        // skip the root dir itself
        {
            let rel = entry
                .path()
                .strip_prefix(vault_path)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir: entry.file_type().is_dir(),
                path: rel,
            });
        }
    } else {
        if let Ok(read_dir) = std::fs::read_dir(&dir) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if EXCLUDE_DIRS.contains(&name.as_str()) {
                    continue;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let rel = entry
                    .path()
                    .strip_prefix(vault_path)
                    .unwrap_or(&entry.path())
                    .to_string_lossy()
                    .to_string();
                entries.push(DirEntry {
                    name,
                    is_dir,
                    path: rel,
                });
            }
        }
    }

    entries
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn hash_content_consistent() {
        let h1 = hash_content("hello world");
        let h2 = hash_content("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_content_different() {
        let h1 = hash_content("hello");
        let h2 = hash_content("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_content_empty() {
        let h = hash_content("");
        assert!(!h.is_empty());
    }

    #[test]
    fn scan_vault_finds_md_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("note1.md"), "content").unwrap();
        fs::write(root.join("note2.md"), "content").unwrap();
        fs::write(root.join("ignore.txt"), "content").unwrap();

        let files = scan_vault(root);
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.path.ends_with(".md")));
    }

    #[test]
    fn scan_vault_excludes_obsidian_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("note.md"), "content").unwrap();
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        fs::write(root.join(".obsidian/config.json"), "{}").unwrap();

        let files = scan_vault(root);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "note.md");
    }

    #[test]
    fn scan_vault_excludes_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::write(root.join("note.md"), "content").unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

        let files = scan_vault(root);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn scan_vault_nested_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        fs::create_dir_all(root.join("wiki/topics")).unwrap();
        fs::write(root.join("wiki/topics/RAG.md"), "content").unwrap();
        fs::write(root.join("root.md"), "content").unwrap();

        let files = scan_vault(root);
        assert_eq!(files.len(), 2);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"wiki/topics/RAG.md"));
        assert!(paths.contains(&"root.md"));
    }

    #[test]
    fn scan_vault_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let files = scan_vault(dir.path());
        assert!(files.is_empty());
    }

    #[test]
    fn read_vault_file_ok() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("test.md"), "hello").unwrap();
        let content = read_vault_file(dir.path(), "test.md").unwrap();
        assert_eq!(content, "hello");
    }

    #[test]
    fn read_vault_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_vault_file(dir.path(), "missing.md").is_err());
    }

    #[test]
    fn list_directory_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::write(dir.path().join("b.md"), "").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let entries = list_directory(dir.path(), "", false);
        assert_eq!(entries.len(), 3); // a.md, b.md, subdir
    }

    #[test]
    fn list_directory_recursive() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("root.md"), "").unwrap();
        fs::write(dir.path().join("sub/nested.md"), "").unwrap();

        let entries = list_directory(dir.path(), "", true);
        assert_eq!(entries.len(), 3); // root.md, sub, sub/nested.md
    }

    #[test]
    fn list_directory_excludes_hidden() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("note.md"), "").unwrap();
        fs::create_dir(dir.path().join(".obsidian")).unwrap();

        let entries = list_directory(dir.path(), "", false);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"note.md"));
        assert!(!names.contains(&".obsidian"));
    }
}
