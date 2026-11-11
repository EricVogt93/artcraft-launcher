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
/// Managed paths use the host's separators; archive and manifest paths remain strict.
pub fn safe_native_relative(path: &Path) -> bool {
    #[cfg(windows)]
    {
        safe_relative(Path::new(&path.to_string_lossy().replace('\\', "/")))
    }
    #[cfg(not(windows))]
    {
        safe_relative(path)
    }
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(crate::cancelled("Download cancelled."))
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
        if !safe_relative(Path::new(entry.name())) {
            return Err(fail("The ZIP contains an unsafe path."));
        }
        let name = entry
            .enclosed_name()
            .ok_or_else(|| fail("The ZIP contains an unsafe path."))?;
        if !safe_native_relative(&name) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_paths_accept_native_separators_but_reject_escape_and_stream_paths() {
        let path = PathBuf::from("versions")
            .join("one")
            .join("payload")
            .join("tool.exe");
        assert!(safe_native_relative(&path));
        for path in [
            "",
            "../outside",
            "/absolute",
            r"C:\outside",
            r"C:outside",
            r"\\server\share",
            r"versions\..\outside",
            "tool.exe:stream",
        ] {
            assert!(!safe_native_relative(Path::new(path)), "accepted {path}");
        }
        let mut deep = PathBuf::new();
        for _ in 0..33 {
            deep.push("directory");
        }
        assert!(!safe_native_relative(&deep));
    }

    #[test]
    fn archive_paths_keep_portable_separator_rules_on_every_host() {
        assert!(safe_relative(Path::new("payload/folder/tool.exe")));
        for path in [
            r"payload\folder\tool.exe",
            r"payload\..\outside",
            "payload/../outside",
            "tool.exe:stream",
            r"\\server\share",
            "/absolute",
        ] {
            assert!(!safe_relative(Path::new(path)), "accepted {path}");
        }
    }

    #[test]
    fn zip_nested_files_extract_with_host_paths_and_backslash_names_are_rejected() {
        use zip::{ZipWriter, write::SimpleFileOptions};
        let fixture = tempfile::tempdir().unwrap();
        let archive = fixture.path().join("package.zip");
        let mut zip = ZipWriter::new(File::create(&archive).unwrap());
        zip.start_file("payload/folder/tool.exe", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"native payload").unwrap();
        zip.finish().unwrap();
        let destination = fixture.path().join("valid");
        fs::create_dir(&destination).unwrap();
        extract_zip(&archive, &destination, &AtomicBool::new(false), false).unwrap();
        assert_eq!(
            fs::read(destination.join("payload/folder/tool.exe")).unwrap(),
            b"native payload"
        );

        let mut zip = ZipWriter::new(File::create(&archive).unwrap());
        zip.start_file(r"payload\folder\tool.exe", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"unsafe name").unwrap();
        zip.finish().unwrap();
        let destination = fixture.path().join("invalid");
        fs::create_dir(&destination).unwrap();
        assert!(extract_zip(&archive, &destination, &AtomicBool::new(false), false).is_err());
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }
}

/// Resolve a payload entry without evaluating desktop-file commands or shell syntax.
/// Explicit paths and discovered candidates must stay inside the extracted payload.
pub fn resolve_entrypoint(
    root: &Path,
    names: &[&str],
    explicit: Option<&str>,
    macos: bool,
) -> Result<PathBuf> {
    use std::collections::BTreeSet;
    fn walk(
        root: &Path,
        dir: &Path,
        macos: bool,
        depth: usize,
        files: &mut Vec<PathBuf>,
        bundles: &mut Vec<PathBuf>,
    ) -> Result<()> {
        if depth > 32 || files.len() + bundles.len() > MAX_FILES {
            return Err(fail("Too many application files."));
        }
        for entry in fs::read_dir(dir)? {
            if files.len() + bundles.len() >= MAX_FILES {
                return Err(fail("Too many application files."));
            }
            let entry = entry?;
            let ty = entry.file_type()?;
            let path = entry.path();
            if ty.is_file() {
                files.push(
                    path.strip_prefix(root)
                        .map_err(|_| fail("Invalid package path."))?
                        .to_path_buf(),
                );
            } else if ty.is_dir() {
                if macos
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
                {
                    bundles.push(
                        path.strip_prefix(root)
                            .map_err(|_| fail("Invalid package path."))?
                            .to_path_buf(),
                    );
                } else {
                    walk(root, &path, macos, depth + 1, files, bundles)?;
                }
            }
        }
        Ok(())
    }
    fn checked(root: &Path, relative: &Path, macos: bool) -> Result<PathBuf> {
        if !safe_native_relative(relative) {
            return Err(fail("Invalid application entry path."));
        }
        let path = root.join(relative);
        let metadata = path.symlink_metadata()?;
        if metadata.file_type().is_symlink()
            || !(metadata.is_file()
                || (macos
                    && metadata.is_dir()
                    && path
                        .extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("app"))))
        {
            return Err(fail("The selected entry is not an application file."));
        }
        if !path.canonicalize()?.starts_with(root.canonicalize()?) {
            return Err(fail("The application entry escapes the package."));
        }
        Ok(path)
    }
    if let Some(path) = explicit.filter(|s| !s.is_empty()) {
        return checked(root, Path::new(path), macos);
    }
    let mut files = Vec::new();
    let mut bundles = Vec::new();
    walk(root, root, macos, 0, &mut files, &mut bundles)?;
    let mut matches = BTreeSet::new();
    if macos {
        for bundle in &bundles {
            let name = bundle.file_stem().and_then(|n| n.to_str()).unwrap_or("");
            if names
                .iter()
                .any(|candidate| name.eq_ignore_ascii_case(candidate))
            {
                matches.insert(bundle.clone());
            }
        }
        if matches.is_empty() && bundles.len() == 1 {
            matches.insert(bundles[0].clone());
        }
        if matches.is_empty() {
            matches.extend(bundles);
        }
    } else {
        for file in &files {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if names.iter().any(|candidate| {
                name.eq_ignore_ascii_case(candidate)
                    || name.eq_ignore_ascii_case(&format!("{candidate}.exe"))
            }) {
                matches.insert(file.clone());
            }
        }
        if matches.is_empty() {
            for file in &files {
                if file.file_name().is_some_and(|name| name == "AppRun") {
                    matches.insert(file.clone());
                }
            }
        }
        if matches.is_empty() {
            for desktop in files.iter().filter(|file| {
                file.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("desktop"))
            }) {
                let path = root.join(desktop);
                if fs::metadata(&path)?.len() > 64 * 1024 {
                    continue;
                }
                let content = fs::read_to_string(path).unwrap_or_default();
                let Some(command) = content.lines().find_map(|line| line.strip_prefix("Exec="))
                else {
                    continue;
                };
                let Some(first) = command.split_whitespace().next() else {
                    continue;
                };
                if first.is_empty()
                    || !first
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._/".contains(&b))
                {
                    continue;
                }
                let command_name = Path::new(first)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if [
                    "sh", "bash", "dash", "zsh", "env", "python", "python3", "perl", "ruby",
                ]
                .contains(&command_name)
                {
                    continue;
                }
                let relative = Path::new(first.trim_start_matches("./"));
                if safe_native_relative(relative) && root.join(relative).is_file() {
                    matches.insert(relative.to_path_buf());
                } else {
                    for file in &files {
                        if file
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.eq_ignore_ascii_case(command_name))
                        {
                            matches.insert(file.clone());
                        }
                    }
                }
            }
        }
        if matches.len() > 1 {
            let non_resource: BTreeSet<PathBuf> = matches
                .iter()
                .filter(|path| {
                    !path.components().any(|c| {
                        c.as_os_str().to_str().is_some_and(|part| {
                            matches!(
                                part.to_ascii_lowercase().as_str(),
                                "share"
                                    | "doc"
                                    | "docs"
                                    | "man"
                                    | "completions"
                                    | "icons"
                                    | "locale"
                                    | "licenses"
                            )
                        })
                    })
                })
                .cloned()
                .collect();
            if !non_resource.is_empty() {
                matches = non_resource;
            }
        }
        #[cfg(unix)]
        if matches.len() > 1 {
            use std::os::unix::fs::PermissionsExt;
            let executable_only: BTreeSet<PathBuf> = matches
                .iter()
                .filter(|path| {
                    root.join(path)
                        .metadata()
                        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
                })
                .cloned()
                .collect();
            if !executable_only.is_empty() {
                matches = executable_only;
            }
        }
    }
    match matches.len() {
        0 => Err(fail("The release has no identifiable application entry.")),
        1 => checked(root, matches.iter().next().unwrap(), macos),
        _ => Err(crate::Error::Entrypoints(
            matches
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        )),
    }
}

#[cfg(test)]
mod entrypoint_tests {
    use super::*;
    #[test]
    fn ambiguous_names_require_a_safe_explicit_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("bin")).unwrap();
        fs::create_dir(tmp.path().join("other")).unwrap();
        fs::write(tmp.path().join("bin/newcraft"), b"one").unwrap();
        fs::write(tmp.path().join("other/newcraft"), b"two").unwrap();
        let error = resolve_entrypoint(tmp.path(), &["newcraft"], None, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("bin") && error.contains("other"));
        assert_eq!(
            resolve_entrypoint(tmp.path(), &["newcraft"], Some("bin/newcraft"), false).unwrap(),
            tmp.path().join("bin/newcraft")
        );
        assert!(resolve_entrypoint(tmp.path(), &["newcraft"], Some("../outside"), false).is_err());
    }
    #[test]
    fn desktop_metadata_is_parsed_as_data_and_never_as_shell() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("bin")).unwrap();
        fs::write(tmp.path().join("bin/editor"), b"app").unwrap();
        fs::write(
            tmp.path().join("app.desktop"),
            b"[Desktop Entry]\nExec=bin/editor %U\n",
        )
        .unwrap();
        assert_eq!(
            resolve_entrypoint(tmp.path(), &["unknown"], None, false).unwrap(),
            tmp.path().join("bin/editor")
        );
        fs::write(
            tmp.path().join("app.desktop"),
            b"[Desktop Entry]\nExec=sh -c malicious\n",
        )
        .unwrap();
        assert!(resolve_entrypoint(tmp.path(), &["unknown"], None, false).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn selected_entry_cannot_be_a_symlink_outside_payload() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        symlink(outside.path(), tmp.path().join("escape")).unwrap();
        assert!(resolve_entrypoint(tmp.path(), &["escape"], Some("escape"), false).is_err());
    }
    #[test]
    fn unnamed_unique_mac_bundle_is_detected_but_multiple_are_not_guessed() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("Nice.app")).unwrap();
        assert_eq!(
            resolve_entrypoint(tmp.path(), &["unknown"], None, true).unwrap(),
            tmp.path().join("Nice.app")
        );
        fs::create_dir(tmp.path().join("Other.app")).unwrap();
        assert!(
            resolve_entrypoint(tmp.path(), &["unknown"], None, true)
                .unwrap_err()
                .to_string()
                .contains("Multiple")
        );
    }
    #[test]
    fn resource_directories_and_non_executables_do_not_shadow_real_binary() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("bin")).unwrap();
        fs::create_dir_all(tmp.path().join("share/doc")).unwrap();
        fs::write(tmp.path().join("bin/newcraft"), b"binary").unwrap();
        fs::write(tmp.path().join("share/doc/newcraft"), b"readme").unwrap();
        assert_eq!(
            resolve_entrypoint(tmp.path(), &["newcraft"], None, false).unwrap(),
            tmp.path().join("bin/newcraft")
        );
        #[cfg(unix)]
        {
            fs::create_dir_all(tmp.path().join("etc")).unwrap();
            fs::write(tmp.path().join("etc/newcraft"), b"config").unwrap();
            make_executable(&tmp.path().join("bin/newcraft")).unwrap();
            assert_eq!(
                resolve_entrypoint(tmp.path(), &["newcraft"], None, false).unwrap(),
                tmp.path().join("bin/newcraft")
            );
        }
    }
}
