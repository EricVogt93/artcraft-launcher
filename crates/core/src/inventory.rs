//! File integrity inventory for managed application versions.
use crate::{
    Result, fail, installer::safe_native_relative, persistence::atomic_json, self_update::hash_file,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const FILE: &str = ".craftlauncher-inventory.json";
#[derive(Debug, Serialize, Deserialize)]
struct Item {
    path: PathBuf,
    size: u64,
    sha256: String,
}
#[derive(Debug, Serialize, Deserialize)]
struct Inventory {
    schema: u32,
    files: Vec<Item>,
}

fn collect(root: &Path, dir: &Path, depth: usize, files: &mut Vec<Item>) -> Result<()> {
    if depth > 32 || files.len() > 100_000 {
        return Err(fail("Application inventory is too large."));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            collect(root, &path, depth + 1, files)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| fail("Invalid inventory path."))?;
            if !safe_native_relative(relative) {
                return Err(fail("Invalid inventory path."));
            }
            if relative == Path::new(FILE) {
                continue;
            }
            if files.len() >= 100_000 {
                return Err(fail("Application inventory is too large."));
            }
            files.push(Item {
                path: relative.into(),
                size: entry.metadata()?.len(),
                sha256: hash_file(&path)?,
            });
        }
        // macOS bundles may contain internal symbolic links. They are checked by
        // the extractor and are not followed while hashing.
    }
    Ok(())
}
pub fn create(root: &Path) -> Result<()> {
    let mut files = Vec::new();
    collect(root, root, 0, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    atomic_json(&root.join(FILE), &Inventory { schema: 1, files })
}
/// None indicates an installation made before inventories were introduced.
pub fn verify(root: &Path) -> Result<Option<Vec<PathBuf>>> {
    use std::io::Read;
    let file = match fs::File::open(root.join(FILE)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut raw = Vec::new();
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut raw)?;
    if raw.len() > 16 * 1024 * 1024 {
        return Err(fail("Application inventory is too large."));
    }
    let inventory: Inventory = serde_json::from_slice(&raw)?;
    if inventory.schema != 1 || inventory.files.len() > 100_000 {
        return Err(fail("Unsupported application inventory."));
    }
    let canonical_root = root.canonicalize()?;
    let mut damaged = Vec::new();
    for item in inventory.files {
        if !safe_native_relative(&item.path) {
            return Err(fail("Invalid inventory path."));
        }
        let path = root.join(&item.path);
        let valid = path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_file() && m.len() == item.size)
            && path
                .canonicalize()
                .is_ok_and(|p| p.starts_with(&canonical_root))
            && hash_file(&path).is_ok_and(|digest| digest == item.sha256);
        if !valid {
            damaged.push(item.path);
        }
    }
    Ok(Some(damaged))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rebuilding_the_inventory_does_not_hash_the_previous_inventory() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("executable"), b"content").unwrap();
        create(root.path()).unwrap();
        create(root.path()).unwrap();
        assert!(verify(root.path()).unwrap().unwrap().is_empty());
        fs::write(root.path().join("executable"), b"changed").unwrap();
        assert_eq!(
            verify(root.path()).unwrap().unwrap(),
            vec![PathBuf::from("executable")]
        );
    }
}
