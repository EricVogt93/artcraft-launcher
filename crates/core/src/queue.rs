//! Durable descriptions of user-requested downloads and installations.
use crate::{Result, persistence::atomic_json};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueAction {
    Install { id: String, version: Option<String> },
    Update { id: String, version: String },
    Repair { id: String },
    Download { id: String, folder: PathBuf },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    Waiting,
    Running,
    Paused,
    Done,
    Failed(String),
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueCheckpoint {
    pub version: String,
    pub previous_install_directory: Option<PathBuf>,
    pub previous_pending_directory: Option<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: String,
    pub action: QueueAction,
    pub status: QueueStatus,
    #[serde(default)]
    pub checkpoint: Option<QueueCheckpoint>,
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
}
/// Delete only one durable task's cache. Never follow cache directory links.
pub fn discard_partial(root: &Path, id: &str) -> Result<()> {
    if !valid_id(id) {
        return Err(crate::fail("Invalid queue task identifier."));
    }
    let cache = root.join("downloads");
    if cache
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(crate::fail("Unsafe download cache directory."));
    }
    let path = cache.join(id);
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path)?
        }
        Ok(_) => return Err(crate::fail("Unsafe task cache directory.")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
pub fn save(root: &Path, entries: &[QueueEntry]) -> Result<()> {
    atomic_json(&root.join("queue.json"), &entries)
}
pub fn load(root: &Path) -> Result<Vec<QueueEntry>> {
    let file = match std::fs::File::open(root.join("queue.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut entries: Vec<QueueEntry> = serde_json::from_reader(file)?;
    for entry in &mut entries {
        if matches!(entry.status, QueueStatus::Waiting | QueueStatus::Running) {
            entry.status = QueueStatus::Paused;
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discarding_one_task_keeps_other_tasks_and_rejects_invalid_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let one = new_id();
        let two = new_id();
        let first = tmp.path().join("downloads").join(&one);
        let second = tmp.path().join("downloads").join(&two);
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("partial"), b"first").unwrap();
        std::fs::write(second.join("partial"), b"second").unwrap();
        assert!(discard_partial(tmp.path(), "../other").is_err());
        discard_partial(tmp.path(), &one).unwrap();
        assert!(!first.exists());
        assert_eq!(std::fs::read(second.join("partial")).unwrap(), b"second");
    }
    #[cfg(unix)]
    #[test]
    fn discarding_rejects_symlinked_task_and_cache_directories() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep"), b"data").unwrap();
        let cache = tmp.path().join("downloads");
        std::fs::create_dir(&cache).unwrap();
        let id = new_id();
        symlink(outside.path(), cache.join(&id)).unwrap();
        assert!(discard_partial(tmp.path(), &id).is_err());
        assert!(outside.path().join("keep").exists());
    }
}
