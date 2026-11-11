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
fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 100
        || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err(fail("Invalid product identifier."));
    }
    Ok(())
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
fn app_icon(executable: &Path, info: &crate::catalog::Product) -> Option<PathBuf> {
    let folder = executable.parent()?;
    let roots = [folder.to_path_buf(), folder.parent()?.to_path_buf()];
    let stems = [&info.id, &info.executable_name, &info.name];
    for root in roots {
        let mut names = vec![".DirIcon".to_owned()];
        for stem in stems {
            names.push(format!("{stem}.png"));
            names.push(format!("{stem}.svg"));
        }
        for name in names {
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

fn desktop_categories(discipline: &str) -> &'static str {
    match discipline {
        "Audio production" => "AudioVideo;Audio;",
        "Video & animation" => "AudioVideo;Video;Graphics;",
        "Office & productivity" | "PDF & publishing" => "Office;Graphics;",
        _ => "Graphics;",
    }
}

pub fn register(home: &Path, data: &Path, id: &str, executable: &Path) -> Result<()> {
    register_in(&home.join(".local/bin"), data, id, executable)
}
pub fn register_in(bin_directory: &Path, data: &Path, id: &str, executable: &Path) -> Result<()> {
    register_product_in(
        bin_directory,
        data,
        &crate::catalog::Product::from(app(id)?),
        executable,
    )
}
pub fn register_product_in(
    bin_directory: &Path,
    data: &Path,
    info: &crate::catalog::Product,
    executable: &Path,
) -> Result<()> {
    let id = info.id.as_str();
    validate_id(id)?;
    let script = wrapper(id, executable)?;
    let (bin, desktop) = paths(bin_directory, data, id);
    let desktop_marker = format!("X-CraftLauncher-App={id}\n");
    check_destination(&bin, &marker(id))?;
    check_destination(&desktop, &desktop_marker)?;
    let icon = app_icon(executable, info)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "applications-graphics".into());
    let categories = desktop_categories(&info.discipline);
    let text = format!(
        "[Desktop Entry]\nVersion=1.0\nType=Application\nName={}\nGenericName={}\nComment={}\nExec={}\nTryExec={}\nIcon={}\nTerminal=false\nCategories={}\nKeywords={};\nStartupNotify=true\n{}X-CraftLauncher-Executable={}\n",
        info.name,
        info.category,
        info.description.replace(['\n', '\r'], " "),
        desktop_quote(&bin),
        bin.display(),
        icon,
        categories,
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

/// Verify ownership and executable routing without overwriting any files.
pub fn check_product_in(
    bin_directory: &Path,
    data: &Path,
    product: &crate::catalog::Product,
    executable: &Path,
) -> Result<bool> {
    let id = product.id.as_str();
    validate_id(id)?;
    let (bin, desktop) = paths(bin_directory, data, id);
    if !owned_regular(&bin, &marker(id))
        || !owned_regular(&desktop, &format!("X-CraftLauncher-App={id}\n"))
    {
        return Ok(false);
    }
    if fs::read_to_string(&bin)? != wrapper(id, executable)? {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&bin)?.permissions().mode() & 0o111 == 0 {
            return Ok(false);
        }
    }
    let text = fs::read_to_string(desktop)?;
    let expected_exec = format!("Exec={}", desktop_quote(&bin));
    let expected_target = format!("X-CraftLauncher-Executable={}", executable.display());
    let exec_lines: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("Exec="))
        .collect();
    let target_lines: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("X-CraftLauncher-Executable="))
        .collect();
    Ok(exec_lines == [expected_exec.as_str()] && target_lines == [expected_target.as_str()])
}

pub fn remove(home: &Path, data: &Path, id: &str, executable: &Path) -> Result<()> {
    remove_in(&home.join(".local/bin"), data, id, executable)
}
pub fn remove_in(bin_directory: &Path, data: &Path, id: &str, executable: &Path) -> Result<()> {
    remove_product_in(
        bin_directory,
        data,
        &crate::catalog::Product::from(app(id)?),
        executable,
    )
}
pub fn remove_product_in(
    bin_directory: &Path,
    data: &Path,
    product: &crate::catalog::Product,
    executable: &Path,
) -> Result<()> {
    let id = product.id.as_str();
    validate_id(id)?;
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
