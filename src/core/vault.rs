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
            .skip(1) // skip the root dir itself
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
                entries.push(DirEntry { name, is_dir, path: rel });
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
