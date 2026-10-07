//! Per-user command entries and freedesktop desktop registration shared by KDE and GNOME.
use crate::{Result, app, fail, installer, persistence, platform::desktop_quote};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn marker(id: &str) -> String {
    format!("#!/bin/sh\n# Managed by CraftLauncher: {id}\n")
}
fn wrapper(id: &str, executable: &Path) -> Result<String> {
    let path = executable
        .to_str()
        .ok_or_else(|| fail("Desktop paths must be valid UTF-8."))?;
    if path.contains(['\n', '\r']) {
        return Err(fail("Desktop paths cannot contain line breaks."));
    }
    Ok(format!(
        "{}exec '{}' \"$@\"\n",
        marker(id),
        path.replace('\'', "'\\''")
    ))
}
fn paths(bin_directory: &Path, data: &Path, id: &str) -> (PathBuf, PathBuf) {
    (
        bin_directory.join(id),
        data.join("applications")
            .join(format!("craftlauncher-{id}.desktop")),
    )
}
fn owned_regular(path: &Path, marker: &str) -> bool {
    path.symlink_metadata()
        .is_ok_and(|m| m.file_type().is_file())
        && fs::read_to_string(path).is_ok_and(|text| text.contains(marker))
}
fn check_destination(path: &Path, marker: &str) -> Result<()> {
    if path.symlink_metadata().is_ok() && !owned_regular(path, marker) {
        return Err(fail(format!(
            "{} is occupied by an unmanaged file. No files were replaced.",
            path.display()
        )));
    }
    Ok(())
}
fn app_icon(executable: &Path, id: &str) -> Option<PathBuf> {
    let folder = executable.parent()?;
    let roots = [folder.to_path_buf(), folder.parent()?.to_path_buf()];
    for root in roots {
        for name in [
            ".DirIcon".to_owned(),
            format!("{id}.png"),
            format!("{id}.svg"),
        ] {
            let path = root.join(name);
            if path.is_file()
                && let Ok(path) = path.canonicalize()
            {
                return Some(path);
            }
        }
    }
    None
}

pub fn register(home: &Path, data: &Path, id: &str, executable: &Path) -> Result<()> {
    register_in(&home.join(".local/bin"), data, id, executable)
}
pub fn register_in(bin_directory: &Path, data: &Path, id: &str, executable: &Path) -> Result<()> {
    let info = app(id)?;
    let script = wrapper(id, executable)?;
    let (bin, desktop) = paths(bin_directory, data, id);
    let desktop_marker = format!("X-CraftLauncher-App={id}\n");
    check_destination(&bin, &marker(id))?;
    check_destination(&desktop, &desktop_marker)?;
    let icon = app_icon(executable, id)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "applications-graphics".into());
    let text = format!(
        "[Desktop Entry]\nVersion=1.0\nType=Application\nName={}\nGenericName={}\nComment={}\nExec={}\nTryExec={}\nIcon={}\nTerminal=false\nCategories=Graphics;\nKeywords={};\nStartupNotify=true\n{}X-CraftLauncher-Executable={}\n",
        info.name,
        info.category,
        info.description,
        desktop_quote(&bin),
        bin.display(),
        icon,
        info.tags.join(";"),
        desktop_marker,
        executable.display()
    );
    // Validate both destinations before touching either. Restore the previous wrapper if registration fails.
    let previous = fs::read(&bin).ok();
    persistence::atomic_write(&bin, script.as_bytes())?;
    if let Err(error) = installer::make_executable(&bin)
        .map_err(crate::Error::from)
        .and_then(|()| persistence::atomic_write(&desktop, text.as_bytes()))
    {
        if let Some(previous) = previous {
            let _ = persistence::atomic_write(&bin, &previous);
            let _ = installer::make_executable(&bin);
        } else {
            let _ = fs::remove_file(&bin);
        }
        return Err(error);
    }
    refresh(data);
    Ok(())
}

pub fn remove(home: &Path, data: &Path, id: &str, executable: &Path) -> Result<()> {
    remove_in(&home.join(".local/bin"), data, id, executable)
}
pub fn remove_in(bin_directory: &Path, data: &Path, id: &str, executable: &Path) -> Result<()> {
    app(id)?;
    let (bin, desktop) = paths(bin_directory, data, id);
    if owned_regular(&bin, &marker(id)) && fs::read_to_string(&bin)? == wrapper(id, executable)? {
        fs::remove_file(bin)?;
    }
    let owner = format!("X-CraftLauncher-Executable={}\n", executable.display());
    if owned_regular(&desktop, &format!("X-CraftLauncher-App={id}\n"))
        && fs::read_to_string(&desktop)?.contains(&owner)
    {
        fs::remove_file(desktop)?;
    }
    refresh(data);
    Ok(())
}
fn refresh(data: &Path) {
    let _ = Command::new("update-desktop-database")
        .arg(data.join("applications"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|desktop| desktop.to_uppercase().contains("KDE"))
    {
        let _ = Command::new("kbuildsycoca6")
            .arg("--noincremental")
            .env("XDG_DATA_HOME", data)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
