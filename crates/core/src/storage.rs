//! Bounded, non-following accounting for the launcher-managed library.
use crate::{Result, fail};
use std::{fs, path::Path};

#[derive(Clone, Debug)]
pub struct AppStorage {
    pub id: String,
    pub active_bytes: u64,
    pub rollback_bytes: u64,
    pub pending_bytes: u64,
    pub rollback_version: Option<String>,
}
#[derive(Clone, Debug)]
pub struct CachedDownload {
    pub name: String,
    pub bytes: u64,
    pub protected: bool,
}
#[derive(Clone, Debug, Default)]
pub struct StorageReport {
    pub apps: Vec<AppStorage>,
    pub downloads: Vec<CachedDownload>,
    pub launcher_bytes: u64,
}

pub fn directory_size(path: &Path) -> Result<u64> {
    fn walk(path: &Path, depth: usize, entries: &mut usize) -> Result<u64> {
        if depth > 32 {
            return Err(fail("Storage directory is too deeply nested."));
        }
        let mut total = 0u64;
        let dir = match fs::read_dir(path) {
            Ok(dir) => dir,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        for entry in dir {
            let entry = entry?;
            *entries += 1;
            if *entries > 200_000 {
                return Err(fail("Storage scan has too many files."));
            }
            let kind = entry.file_type()?;
            if kind.is_file() {
                total = total.saturating_add(entry.metadata()?.len());
            } else if kind.is_dir() {
                total = total.saturating_add(walk(&entry.path(), depth + 1, entries)?);
            }
        }
        Ok(total)
    }
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(fail("Storage directory is a symbolic link."));
    }
    walk(path, 0, &mut 0)
}
pub fn downloads(root: &Path) -> Result<Vec<CachedDownload>> {
    let path = root.join("downloads");
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(fail("Download cache is a symbolic link."));
    }
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut files = Vec::new();
    let mut seen = 0usize;
    for entry in entries {
        let entry = entry?;
        seen += 1;
        if seen > 100_000 {
            return Err(fail("Too many cached downloads."));
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type()?;
        if kind.is_file() && is_cache_name(&name) {
            files.push(CachedDownload {
                name,
                bytes: entry.metadata()?.len(),
                protected: false,
            });
        } else if kind.is_dir() && crate::queue::valid_id(&name) {
            for child in fs::read_dir(entry.path())? {
                let child = child?;
                seen += 1;
                if seen > 100_000 {
                    return Err(fail("Too many cached downloads."));
                }
                let child_name = child.file_name().to_string_lossy().into_owned();
                if child.file_type()?.is_file() && is_cache_name(&child_name) {
                    files.push(CachedDownload {
                        name: format!("{name}/{child_name}"),
                        bytes: child.metadata()?.len(),
                        protected: false,
                    });
                }
            }
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}
pub fn is_cache_name(name: &str) -> bool {
    name.strip_suffix(".part")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
}
