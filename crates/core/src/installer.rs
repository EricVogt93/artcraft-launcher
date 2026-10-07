use crate::{Result, fail};
use flate2::read::GzDecoder;
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_FILES: usize = 50_000;
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;
pub fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().count() <= 32
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && !path.to_string_lossy().contains('\\')
        && !path.to_string_lossy().contains(':')
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(fail("Download cancelled."))
    } else {
        Ok(())
    }
}
fn budget(index: usize, total: u64) -> Result<()> {
    if index >= MAX_FILES || total > MAX_UNPACKED {
        Err(fail("The release archive exceeds its extraction limits."))
    } else {
        Ok(())
    }
}

/// Safe extraction into a NEW, empty staging directory. Bundle symlinks are created only
/// after all ordinary files, and only if they resolve to an existing target in that bundle.
pub fn extract(archive: &Path, destination: &Path, cancel: &AtomicBool) -> Result<()> {
    extract_tar(archive, destination, cancel, false)
}
pub fn extract_tar(
    archive: &Path,
    destination: &Path,
    cancel: &AtomicBool,
    bundle_links: bool,
) -> Result<()> {
    let mut archive = tar::Archive::new(GzDecoder::new(File::open(archive)?));
    let mut total = 0u64;
    let mut links = Vec::new();
    for (index, entry) in archive.entries()?.enumerate() {
        check_cancel(cancel)?;
        let mut entry = entry?;
        let name = entry.path()?.into_owned();
        let kind = entry.header().entry_type();
        if !safe_relative(&name) {
            return Err(fail("The archive contains an unsafe path."));
        }
        total = total.saturating_add(entry.size());
        budget(index, total)?;
        if kind.is_symlink() && bundle_links {
            let target = entry
                .link_name()?
                .ok_or_else(|| fail("Invalid bundle link."))?
                .into_owned();
            links.push((name, target));
        } else if kind.is_file() || kind.is_dir() {
            if !entry.unpack_in(destination)? {
                return Err(fail("The archive escapes the installation folder."));
            }
        } else {
            return Err(fail(
                "The archive contains an unsupported link or special file.",
            ));
        }
    }
    create_bundle_links(destination, links)?;
    Ok(())
}

pub fn extract_zip(
    archive: &Path,
    destination: &Path,
    cancel: &AtomicBool,
    bundle_links: bool,
) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(File::open(archive)?).map_err(|e| fail(e.to_string()))?;
    let mut total = 0u64;
    let mut links = Vec::new();
    for index in 0..archive.len() {
        check_cancel(cancel)?;
        let mut entry = archive.by_index(index).map_err(|e| fail(e.to_string()))?;
        let name = entry
            .enclosed_name()
            .ok_or_else(|| fail("The ZIP contains an unsafe path."))?;
        if !safe_relative(&name) {
            return Err(fail("The ZIP contains an unsafe path."));
        }
        total = total.saturating_add(entry.size());
        budget(index, total)?;
        let full = destination.join(&name);
        if entry.is_symlink() {
            if !bundle_links || entry.size() > 4096 {
                return Err(fail("The ZIP contains an unsupported link."));
            }
            let mut target = String::new();
            entry.read_to_string(&mut target)?;
            links.push((name, PathBuf::from(target)));
        } else if entry.is_dir() {
            fs::create_dir_all(full)?;
        } else {
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&full)?;
            io::copy(&mut entry, &mut file)?;
            file.flush()?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&full, fs::Permissions::from_mode(mode & 0o755))?;
            }
        }
    }
    create_bundle_links(destination, links)?;
    Ok(())
}

fn create_bundle_links(root: &Path, mut links: Vec<(PathBuf, PathBuf)>) -> Result<()> {
    if links.is_empty() {
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        let _ = links;
        return Err(fail("Bundle links require a Unix filesystem."));
    }
    #[cfg(unix)]
    {
        // Resolve chained links in passes. Missing targets, cycles and escaped targets fail.
        let canonical_root = fs::canonicalize(root)?;
        while !links.is_empty() {
            let mut remaining = Vec::new();
            let before = links.len();
            for (name, target) in links {
                let Some(bundle) = bundle_root(&name) else {
                    return Err(fail("Links are only allowed inside app bundles."));
                };
                if target.is_absolute() || target.to_string_lossy().contains('\\') {
                    return Err(fail("Unsafe bundle link."));
                }
                let full = root.join(&name);
                let parent = full.parent().ok_or_else(|| fail("Invalid bundle link."))?;
                match fs::canonicalize(parent.join(&target)) {
                    Ok(resolved) => {
                        let bundle_path = canonical_root.join(bundle);
                        if !resolved.starts_with(&bundle_path) {
                            return Err(fail("A bundle link escapes its app."));
                        }
                        fs::create_dir_all(parent)?;
                        if !fs::canonicalize(parent)?.starts_with(&bundle_path) {
                            return Err(fail("Unsafe bundle link parent."));
                        }
                        std::os::unix::fs::symlink(&target, full)?;
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => remaining.push((name, target)),
                    Err(e) => return Err(e.into()),
                }
            }
            if remaining.len() == before {
                return Err(fail("Unresolved or cyclic bundle links."));
            }
            links = remaining;
        }
        Ok(())
    }
}
fn bundle_root(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for part in path.components() {
        result.push(part);
        if result.extension().is_some_and(|ext| ext == "app") {
            return Some(result);
        }
    }
    None
}

pub fn find_executable(folder: &Path, id: &str) -> io::Result<Option<PathBuf>> {
    find_file(
        folder,
        &|name| name.eq_ignore_ascii_case(id) || name.eq_ignore_ascii_case(&format!("{id}.exe")),
        0,
    )
}
fn find_file(
    folder: &Path,
    matches: &dyn Fn(&str) -> bool,
    depth: usize,
) -> io::Result<Option<PathBuf>> {
    if depth > 32 {
        return Err(io::Error::other(
            "The release has excessive directory nesting.",
        ));
    }
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_file() && matches(&entry.file_name().to_string_lossy()) {
            return Ok(Some(entry.path()));
        }
        if ty.is_dir()
            && let Some(found) = find_file(&entry.path(), matches, depth + 1)?
        {
            return Ok(Some(found));
        }
    }
    Ok(None)
}
pub fn find_bundle(folder: &Path, id: &str) -> io::Result<Option<PathBuf>> {
    find_bundle_at(folder, id, 0)
}
fn find_bundle_at(folder: &Path, id: &str, depth: usize) -> io::Result<Option<PathBuf>> {
    if depth > 32 {
        return Err(io::Error::other(
            "The release has excessive directory nesting.",
        ));
    }
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(&format!("{id}.app"))
            {
                return Ok(Some(entry.path()));
            }
            if let Some(found) = find_bundle_at(&entry.path(), id, depth + 1)? {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}
pub fn make_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
