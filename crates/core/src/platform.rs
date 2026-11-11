use crate::{Asset, Result, fail, installer};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    TarGz,
    AppImage,
    Zip,
    Dmg,
    Nsis,
}
pub fn package_kind(asset: &Asset) -> Result<PackageKind> {
    let name = asset.name.to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Ok(PackageKind::TarGz)
    } else if name.ends_with(".appimage") {
        Ok(PackageKind::AppImage)
    } else if name.ends_with(".zip") {
        Ok(PackageKind::Zip)
    } else if name.ends_with(".dmg") {
        Ok(PackageKind::Dmg)
    } else if name.ends_with("-setup.exe") {
        Ok(PackageKind::Nsis)
    } else {
        Err(fail("This package format cannot be managed directly."))
    }
}

/// Only installers with a documented and implemented lifecycle may be run.
pub fn supports_managed(id: &str, os: &str, asset: &Asset) -> bool {
    match package_kind(asset) {
        Ok(PackageKind::TarGz) => matches!(os, "linux" | "freebsd" | "macos"),
        Ok(PackageKind::AppImage) => os == "linux",
        Ok(PackageKind::Zip) => matches!(os, "windows" | "macos"),
        Ok(PackageKind::Dmg) => os == "macos",
        Ok(PackageKind::Nsis) => os == "windows" && id == "artcraft",
        Err(_) => false,
    }
}

pub trait Platform: Send {
    fn os(&self) -> &str;
    fn arch(&self) -> &str;
    fn is_running(&self, executable: &Path) -> Result<bool>;
    fn running_apps(
        &self,
        executables: &std::collections::BTreeMap<String, PathBuf>,
    ) -> std::collections::BTreeMap<String, bool> {
        executables
            .iter()
            .map(|(id, path)| (id.clone(), self.is_running(path).unwrap_or(true)))
            .collect()
    }
    fn launch(&self, executable: &Path) -> Result<Option<u32>>;
    fn launch_with_log(&self, executable: &Path, _log: &Path) -> Result<Option<u32>> {
        self.launch(executable)
    }
    fn open_project(&self, _executable: &Path, _project: &Path) -> Result<Option<u32>> {
        Err(fail(
            "Opening project files is unavailable on this platform.",
        ))
    }
    /// Returns (path to the app, optional deferred installer), both relative to staging.
    fn unpack(
        &self,
        id: &str,
        kind: PackageKind,
        archive: &Path,
        stage: &Path,
        cancel: &AtomicBool,
    ) -> Result<(PathBuf, Option<PathBuf>)>;
    fn unpack_product(
        &self,
        product: &crate::catalog::Product,
        kind: PackageKind,
        archive: &Path,
        stage: &Path,
        cancel: &AtomicBool,
    ) -> Result<(PathBuf, Option<PathBuf>)> {
        self.unpack(&product.id, kind, archive, stage, cancel)
    }
    fn unpack_product_with_entrypoint(
        &self,
        product: &crate::catalog::Product,
        kind: PackageKind,
        archive: &Path,
        stage: &Path,
        cancel: &AtomicBool,
        _entrypoint: Option<&str>,
    ) -> Result<(PathBuf, Option<PathBuf>)> {
        self.unpack_product(product, kind, archive, stage, cancel)
    }
    fn shortcut_product(&self, product: &crate::catalog::Product, executable: &Path) -> Result<()> {
        self.shortcut(&product.id, executable)
    }
    /// None means this platform cannot inspect native shortcut targets.
    fn check_shortcut_product(
        &self,
        _product: &crate::catalog::Product,
        _executable: &Path,
    ) -> Result<Option<bool>> {
        Ok(None)
    }
    fn remove_shortcut_product(
        &self,
        product: &crate::catalog::Product,
        executable: &Path,
    ) -> Result<()> {
        self.remove_shortcut_for(&product.id, executable)
    }
    fn finish_installer(&self, installer: &Path, app: &Path) -> Result<()>;
    fn shortcut(&self, id: &str, executable: &Path) -> Result<()>;
    fn remove_shortcut(&self, id: &str) -> Result<()>;
    fn remove_shortcut_for(&self, id: &str, executable: &Path) -> Result<()> {
        let _ = executable;
        self.remove_shortcut(id)
    }
    fn autostart(&self, enabled: bool, launcher: &Path, library: &Path) -> Result<()>;
}
#[derive(Default)]
pub struct Native;

impl Platform for Native {
    fn os(&self) -> &str {
        std::env::consts::OS
    }
    fn arch(&self) -> &str {
        std::env::consts::ARCH
    }
    fn is_running(&self, executable: &Path) -> Result<bool> {
        if !sysinfo::IS_SUPPORTED_SYSTEM {
            return Err(fail(
                "Process detection is unavailable on this system; update activation is deferred.",
            ));
        }
        let dir = app_process_directory(executable);
        let running = collect_running_executables();
        Ok(running.iter().any(|exe| process_matches_dir(exe, &dir)))
    }
    fn running_apps(
        &self,
        executables: &std::collections::BTreeMap<String, PathBuf>,
    ) -> std::collections::BTreeMap<String, bool> {
        if executables.is_empty() {
            return std::collections::BTreeMap::new();
        }
        if !sysinfo::IS_SUPPORTED_SYSTEM {
            return executables.keys().map(|id| (id.clone(), true)).collect();
        }
        let running = collect_running_executables();
        executables
            .iter()
            .map(|(id, executable)| {
                let dir = app_process_directory(executable);
                let active = running.iter().any(|exe| process_matches_dir(exe, &dir));
                (id.clone(), active)
            })
            .collect()
    }
    fn launch(&self, executable: &Path) -> Result<Option<u32>> {
        if !executable.exists() {
            return Err(fail(
                "The app executable is missing. Reinstall or link it again.",
            ));
        }
        #[cfg(target_os = "macos")]
        if executable.extension().is_some_and(|ext| ext == "app") {
            let result = Command::new("/usr/bin/open")
                .args(["-n", "-a"])
                .arg(executable)
                .status()?;
            if !result.success() {
                return Err(fail("macOS could not open this app."));
            }
            return Ok(None);
        }
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if executable.extension().is_some_and(|ext| ext == "AppImage") {
            command.env("APPIMAGE_EXTRACT_AND_RUN", "1");
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        // Only a non-zero exit that we can actually observe is a start error.
        // A successful early exit may be an app forwarding to another process.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1200);
        loop {
            match child.try_wait()? {
                Some(status) if !status.success() => {
                    return Err(fail(format!("The app exited during startup ({status}).")));
                }
                Some(_) => return Ok(Some(pid)),
                None if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(25))
                }
                None => break,
            }
        }
        // Reap asynchronously. The app has independent stdio and can outlive the launcher.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(Some(pid))
    }
    fn launch_with_log(&self, executable: &Path, log: &Path) -> Result<Option<u32>> {
        #[cfg(target_os = "macos")]
        let resolved_bundle_exec = if executable.extension().is_some_and(|ext| ext == "app") {
            let Some(inner) = macos_bundle_executable(executable) else {
                return self.launch(executable);
            };
            Some(inner)
        } else {
            None
        };
        #[cfg(target_os = "macos")]
        let executable = resolved_bundle_exec.as_deref().unwrap_or(executable);
        let helper = std::env::current_exe()?;
        let is_launcher = helper
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("craftlauncher"));
        if !is_launcher {
            return self.launch(executable);
        }
        use std::io::{BufRead, BufReader};
        let mut child = Command::new(helper)
            .arg("--internal-monitor")
            .arg(executable)
            .arg("--internal-log")
            .arg(log)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| fail("Could not read app monitor status."))?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        let first = rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .map_err(|_| fail("The app monitor could not start the application."))?;
        let pid: u32 = first
            .strip_prefix("START ")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| fail("Invalid app monitor response."))?;
        match rx.recv_timeout(std::time::Duration::from_millis(1200)) {
            Ok(line) if line.starts_with("EXIT 0 ") => Err(fail(format!(
                "The app exited during startup (code {}).",
                line.trim_start_matches("EXIT 0 ")
            ))),
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(Some(pid)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(fail("The app monitor exited before reporting its status."))
            }
        }
    }
    fn open_project(&self, executable: &Path, project: &Path) -> Result<Option<u32>> {
        if !executable.exists() || !project.is_file() {
            return Err(fail("The application or project file is missing."));
        }
        #[cfg(target_os = "macos")]
        if executable.extension().is_some_and(|ext| ext == "app") {
            let status = Command::new("/usr/bin/open")
                .arg("-a")
                .arg(executable)
                .arg(project)
                .status()?;
            if !status.success() {
                return Err(fail("macOS could not open this project."));
            }
            return Ok(None);
        }
        let mut command = Command::new(executable);
        command
            .arg(project)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if executable.extension().is_some_and(|ext| ext == "AppImage") {
            command.env("APPIMAGE_EXTRACT_AND_RUN", "1");
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(Some(pid))
    }
    fn unpack(
        &self,
        id: &str,
        kind: PackageKind,
        archive: &Path,
        stage: &Path,
        cancel: &AtomicBool,
    ) -> Result<(PathBuf, Option<PathBuf>)> {
        self.unpack_product(
            &crate::catalog::Product::from(crate::app(id)?),
            kind,
            archive,
            stage,
            cancel,
        )
    }
    fn unpack_product(
        &self,
        product: &crate::catalog::Product,
        kind: PackageKind,
        archive: &Path,
        stage: &Path,
        cancel: &AtomicBool,
    ) -> Result<(PathBuf, Option<PathBuf>)> {
        self.unpack_product_with_entrypoint(product, kind, archive, stage, cancel, None)
    }
    fn unpack_product_with_entrypoint(
        &self,
        product: &crate::catalog::Product,
        kind: PackageKind,
        archive: &Path,
        stage: &Path,
        cancel: &AtomicBool,
        entrypoint: Option<&str>,
    ) -> Result<(PathBuf, Option<PathBuf>)> {
        let legacy_id = product.id.as_str();
        let id = product.executable_name.as_str();
        let payload = stage.join("payload");
        fs::create_dir_all(&payload)?;
        match kind {
            PackageKind::TarGz => {
                installer::extract_tar(archive, &payload, cancel, self.os() == "macos")?
            }
            PackageKind::Zip => {
                installer::extract_zip(archive, &payload, cancel, self.os() == "macos")?
            }
            PackageKind::AppImage => {
                if self.os() != "linux" {
                    return Err(fail("AppImage extraction requires Linux."));
                }
                let file = stage.join(format!("{id}.AppImage"));
                fs::copy(archive, &file)?;
                installer::make_executable(&file)?;
                unpack_appimage(&file, &payload, cancel)?;
                fs::remove_file(file)?;
                let entry = payload.join("squashfs-root/AppRun");
                if !entry.is_file() {
                    return Err(fail("The AppImage has no AppRun entry point."));
                }
                installer::make_executable(&entry)?;
                return Ok((
                    entry
                        .strip_prefix(stage)
                        .map_err(|_| fail("Invalid payload path."))?
                        .into(),
                    None,
                ));
            }
            PackageKind::Dmg => unpack_dmg(archive, &payload, id, entrypoint)?,
            PackageKind::Nsis => {
                if self.os() != "windows" || id != "artcraft" {
                    return Err(fail("Unsupported native installer."));
                }
                let setup = stage.join("setup.exe");
                fs::copy(archive, &setup)?;
                return Ok((
                    PathBuf::from("payload/ArtCraft.exe"),
                    Some(PathBuf::from("setup.exe")),
                ));
            }
        }
        let selected_entry = if kind == PackageKind::Dmg {
            entrypoint
                .and_then(|entry| Path::new(entry).file_name())
                .and_then(|name| name.to_str())
        } else {
            entrypoint
        };
        let executable = installer::resolve_entrypoint(
            &payload,
            &[id, legacy_id],
            selected_entry,
            self.os() == "macos",
        )?;
        if self.os() != "macos" {
            installer::make_executable(&executable)?;
        }
        Ok((
            executable
                .strip_prefix(stage)
                .map_err(|_| fail("Invalid payload path."))?
                .into(),
            None,
        ))
    }
    fn finish_installer(&self, installer: &Path, app: &Path) -> Result<()> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // NSIS /D must be the final argument, without quotes, even when it has spaces.
            let directory = app
                .parent()
                .ok_or_else(|| fail("Invalid installation directory."))?;
            let status = Command::new(installer)
                .arg("/S")
                .raw_arg(format!("/D={}", directory.display()))
                .status()?;
            if !status.success() || !app.is_file() {
                return Err(fail(
                    "The ArtCraft installer did not complete successfully.",
                ));
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = (installer, app);
            Err(fail("This installer requires Windows."))
        }
    }
    fn shortcut(&self, id: &str, executable: &Path) -> Result<()> {
        self.shortcut_product(&crate::catalog::Product::from(crate::app(id)?), executable)
    }
    fn shortcut_product(&self, product: &crate::catalog::Product, executable: &Path) -> Result<()> {
        let info = product;
        let id = product.id.as_str();
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let base = directories::BaseDirs::new()
                .ok_or_else(|| fail("Cannot find the user data directory."))?;
            crate::linux_integration::register_product_in(
                &native_bin_directory(base.home_dir())?,
                base.data_dir(),
                product,
                executable,
            )?;
        }
        #[cfg(windows)]
        windows_shortcut(&info.name, executable)?;
        #[cfg(target_os = "macos")]
        {
            let home = directories::BaseDirs::new()
                .ok_or_else(|| fail("Cannot find the home directory."))?;
            let dir = home.home_dir().join("Applications/CraftLauncher Apps");
            fs::create_dir_all(&dir)?;
            let link = dir.join(format!("{}.app", info.name));
            if fs::symlink_metadata(&link).is_ok_and(|m| !m.file_type().is_symlink()) {
                return Err(fail(
                    "A non-launcher application already exists at the shortcut path.",
                ));
            }
            if link.symlink_metadata().is_ok() {
                fs::remove_file(&link)?;
            }
            std::os::unix::fs::symlink(executable, link)?;
        }
        let _ = (info, id, executable);
        Ok(())
    }
    fn check_shortcut_product(
        &self,
        product: &crate::catalog::Product,
        executable: &Path,
    ) -> Result<Option<bool>> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let base = directories::BaseDirs::new()
                .ok_or_else(|| fail("Cannot find the user data directory."))?;
            Ok(Some(crate::linux_integration::check_product_in(
                &native_bin_directory(base.home_dir())?,
                base.data_dir(),
                product,
                executable,
            )?))
        }
        #[cfg(windows)]
        {
            Ok(Some(check_windows_shortcut(&product.name, executable)?))
        }
        #[cfg(target_os = "macos")]
        {
            let base = directories::BaseDirs::new()
                .ok_or_else(|| fail("Cannot find the home directory."))?;
            let link = base
                .home_dir()
                .join("Applications/CraftLauncher Apps")
                .join(format!("{}.app", product.name));
            let same_target = matches!((fs::canonicalize(&link), fs::canonicalize(executable)), (Ok(actual), Ok(expected)) if actual == expected);
            Ok(Some(
                link.symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                    && same_target,
            ))
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "freebsd",
            windows,
            target_os = "macos"
        )))]
        {
            let _ = (product, executable);
            Ok(None)
        }
    }
    fn remove_shortcut_product(
        &self,
        product: &crate::catalog::Product,
        executable: &Path,
    ) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let base = directories::BaseDirs::new()
                .ok_or_else(|| fail("Cannot find the user data directory."))?;
            crate::linux_integration::remove_product_in(
                &native_bin_directory(base.home_dir())?,
                base.data_dir(),
                product,
                executable,
            )
        }
        #[cfg(windows)]
        {
            let _ = executable;
            if let Some(base) = directories::BaseDirs::new() {
                remove_if_exists(
                    &base
                        .data_dir()
                        .join("Microsoft/Windows/Start Menu/Programs")
                        .join(format!("{} (CraftLauncher).lnk", product.name)),
                )?;
            }
            Ok(())
        }
        #[cfg(target_os = "macos")]
        {
            let _ = executable;
            if let Some(base) = directories::BaseDirs::new() {
                let link = base
                    .home_dir()
                    .join("Applications/CraftLauncher Apps")
                    .join(format!("{}.app", product.name));
                if link
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    fs::remove_file(link)?;
                }
            }
            Ok(())
        }
    }
    fn remove_shortcut_for(&self, id: &str, executable: &Path) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let base = directories::BaseDirs::new()
                .ok_or_else(|| fail("Cannot find the user data directory."))?;
            crate::linux_integration::remove_in(
                &native_bin_directory(base.home_dir())?,
                base.data_dir(),
                id,
                executable,
            )
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        {
            let _ = executable;
            self.remove_shortcut(id)
        }
    }
    fn remove_shortcut(&self, id: &str) -> Result<()> {
        let info = crate::app(id)?;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            if let Some(base) = directories::BaseDirs::new() {
                remove_if_exists(
                    &base
                        .data_dir()
                        .join("applications")
                        .join(format!("craftlauncher-{id}.desktop")),
                )?;
            }
        }
        #[cfg(windows)]
        if let Some(base) = directories::BaseDirs::new() {
            remove_if_exists(
                &base
                    .data_dir()
                    .join("Microsoft/Windows/Start Menu/Programs")
                    .join(format!("{} (CraftLauncher).lnk", info.name)),
            )?;
        }
        #[cfg(target_os = "macos")]
        if let Some(base) = directories::BaseDirs::new() {
            let link = base
                .home_dir()
                .join("Applications/CraftLauncher Apps")
                .join(format!("{}.app", info.name));
            if link
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                fs::remove_file(link)?;
            }
        }
        let _ = info;
        Ok(())
    }
    fn autostart(&self, enabled: bool, launcher: &Path, library: &Path) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let base = directories::BaseDirs::new()
                .ok_or_else(|| fail("Cannot find the user config directory."))?;
            let file = base.config_dir().join("autostart/craftlauncher.desktop");
            if enabled {
                crate::persistence::atomic_write(&file, format!("[Desktop Entry]\nType=Application\nName=CraftLauncher\nExec={} --background --data-dir {}\nTerminal=false\n", desktop_quote(launcher), desktop_quote(library)).as_bytes())?;
            } else {
                remove_if_exists(&file)?;
            }
        }
        #[cfg(windows)]
        {
            let name = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
            let mut cmd = Command::new("reg.exe");
            if enabled {
                cmd.args([
                    "add",
                    name,
                    "/v",
                    "CraftLauncher",
                    "/t",
                    "REG_SZ",
                    "/d",
                    &format!(
                        "\"{}\" --background --data-dir \"{}\"",
                        launcher.display(),
                        library.display()
                    ),
                    "/f",
                ]);
            } else {
                cmd.args(["delete", name, "/v", "CraftLauncher", "/f"]);
            }
            let result = cmd.output()?;
            if enabled && !result.status.success() {
                return Err(fail("Could not register login startup."));
            }
        }
        #[cfg(target_os = "macos")]
        {
            let base = directories::BaseDirs::new()
                .ok_or_else(|| fail("Cannot find the home directory."))?;
            let file = base
                .home_dir()
                .join("Library/LaunchAgents/dev.craftlauncher.desktop.plist");
            if enabled {
                let path = xml_escape(&launcher.to_string_lossy());
                let library_path = xml_escape(&library.to_string_lossy());
                let text = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>dev.craftlauncher.desktop</string><key>ProgramArguments</key><array><string>{path}</string><string>--background</string><string>--data-dir</string><string>{library_path}</string></array><key>RunAtLoad</key><true/></dict></plist>"
                );
                crate::persistence::atomic_write(&file, text.as_bytes())?;
            } else {
                remove_if_exists(&file)?;
            }
        }
        let _ = (enabled, launcher, library);
        Ok(())
    }
}

pub fn desktop_quote(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('%', "%%")
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('\n', "")
    )
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn native_bin_directory(home: &Path) -> Result<PathBuf> {
    let directory = std::env::var_os("CRAFTLAUNCHER_BIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/bin"));
    if !directory.is_absolute() {
        return Err(fail("CRAFTLAUNCHER_BIN_DIR must be an absolute directory."));
    }
    Ok(directory)
}

fn app_process_directory(executable: &Path) -> PathBuf {
    let target = fs::canonicalize(executable).unwrap_or_else(|_| executable.to_path_buf());
    if target.extension().is_some_and(|x| x == "app") {
        target
    } else {
        target.parent().unwrap_or(&target).to_path_buf()
    }
}

fn collect_running_executables() -> Vec<PathBuf> {
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for process in system.processes().values() {
        if let Some(exe) = process.exe()
            && seen.insert(exe.to_path_buf())
        {
            result.push(fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf()));
        }
    }
    result
}

fn process_matches_dir(exe: &Path, directory: &Path) -> bool {
    #[cfg(windows)]
    {
        exe.to_string_lossy()
            .to_lowercase()
            .starts_with(&format!("{}\\", directory.to_string_lossy().to_lowercase()))
    }
    #[cfg(not(windows))]
    {
        exe.starts_with(directory)
    }
}

#[cfg(target_os = "macos")]
fn macos_bundle_executable(bundle: &Path) -> Option<PathBuf> {
    let macos_dir = bundle.join("Contents/MacOS");
    let stem = bundle.file_stem()?.to_str()?;
    let direct = macos_dir.join(stem);
    if direct.is_file() {
        return Some(direct);
    }
    let mut entries = fs::read_dir(macos_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()));
    let first = entries.next()?;
    if entries.next().is_none() {
        Some(first.path())
    } else {
        None
    }
}

fn unpack_appimage(archive: &Path, destination: &Path, cancel: &AtomicBool) -> Result<()> {
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::cancelled("AppImage extraction cancelled."));
    }
    let mut child = Command::new(archive)
        .arg("--appimage-extract")
        .current_dir(destination)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancel.load(Ordering::Relaxed) {
                crate::cancelled("AppImage extraction cancelled.")
            } else {
                fail("AppImage extraction timed out.")
            });
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(fail("The AppImage runtime could not extract this package."));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let root = destination.join("squashfs-root");
    if !root.symlink_metadata()?.file_type().is_dir() {
        return Err(fail(
            "The extracted AppImage root must be a real directory.",
        ));
    }
    let root = fs::canonicalize(root)?;
    if !root.starts_with(fs::canonicalize(destination)?) {
        return Err(fail(
            "The AppImage root escapes the installation directory.",
        ));
    }
    let mut entries = 0;
    let mut bytes = 0;
    validate_appdir(&root, &root, 0, &mut entries, &mut bytes)?;
    Ok(())
}
fn validate_appdir(
    root: &Path,
    folder: &Path,
    depth: usize,
    entries: &mut usize,
    bytes: &mut u64,
) -> Result<()> {
    if depth > 32 {
        return Err(fail("The AppImage has excessive directory nesting."));
    }
    for item in fs::read_dir(folder)? {
        let item = item?;
        *entries += 1;
        if *entries > 50_000 {
            return Err(fail("The AppImage has too many files."));
        }
        let kind = item.file_type()?;
        if kind.is_symlink() {
            if !fs::canonicalize(item.path())?.starts_with(root) {
                return Err(fail("An AppImage link escapes its extracted directory."));
            }
        } else if kind.is_dir() {
            validate_appdir(root, &item.path(), depth + 1, entries, bytes)?;
        } else if kind.is_file() {
            *bytes += item.metadata()?.len();
            if *bytes > 4 * 1024 * 1024 * 1024 {
                return Err(fail("The extracted AppImage exceeds the size limit."));
            }
        } else {
            return Err(fail("The AppImage contains a special file."));
        }
    }
    Ok(())
}
fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
#[cfg(target_os = "macos")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(windows)]
fn check_windows_shortcut(name: &str, executable: &Path) -> Result<bool> {
    let base =
        directories::BaseDirs::new().ok_or_else(|| fail("Cannot find the user data directory."))?;
    let link = base
        .data_dir()
        .join("Microsoft/Windows/Start Menu/Programs")
        .join(format!("{name} (CraftLauncher).lnk"));
    if !link.is_file() {
        return Ok(false);
    }
    let bytes = fs::read(&link)?;
    let expected = executable.to_string_lossy();
    let utf16: Vec<u8> = expected.encode_utf16().flat_map(u16::to_le_bytes).collect();
    Ok(bytes
        .windows(expected.len())
        .any(|w| w.eq_ignore_ascii_case(expected.as_bytes()))
        || (!utf16.is_empty() && bytes.windows(utf16.len()).any(|w| w == utf16.as_slice())))
}

#[cfg(windows)]
fn windows_shortcut(name: &str, executable: &Path) -> Result<()> {
    if check_windows_shortcut(name, executable).unwrap_or(false) {
        return Ok(());
    }
    let base =
        directories::BaseDirs::new().ok_or_else(|| fail("Cannot find the user data directory."))?;
    let directory = base
        .data_dir()
        .join("Microsoft/Windows/Start Menu/Programs");
    fs::create_dir_all(&directory)?;
    let link = directory.join(format!("{name} (CraftLauncher).lnk"));
    // Values are environment variables, never interpolated PowerShell code.
    let output = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", "$w=New-Object -ComObject WScript.Shell; $s=$w.CreateShortcut($env:CRAFT_SHORTCUT); $s.TargetPath=$env:CRAFT_EXECUTABLE; $s.Save()"])
        .env("CRAFT_SHORTCUT", link).env("CRAFT_EXECUTABLE", executable).output()?;
    if !output.status.success() {
        return Err(fail("Could not create the Start menu shortcut."));
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn unpack_dmg(
    archive: &Path,
    destination: &Path,
    id: &str,
    entrypoint: Option<&str>,
) -> Result<()> {
    let mount = tempfile::tempdir()?;
    let result = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
        .arg(mount.path())
        .arg(archive)
        .output()?;
    if !result.status.success() {
        return Err(fail("Could not mount the app disk image."));
    }
    struct Mounted(PathBuf);
    impl Drop for Mounted {
        fn drop(&mut self) {
            let _ = Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(&self.0)
                .output();
        }
    }
    let _guard = Mounted(mount.path().into());
    let bundle = installer::resolve_entrypoint(mount.path(), &[id], entrypoint, true)?;
    validate_bundle(&bundle, &bundle, 0)?;
    let target = destination.join(
        bundle
            .file_name()
            .ok_or_else(|| fail("Invalid app bundle."))?,
    );
    let result = Command::new("/usr/bin/ditto")
        .arg(&bundle)
        .arg(target)
        .output()?;
    if !result.status.success() {
        return Err(fail("Could not copy the app bundle."));
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn validate_bundle(root: &Path, folder: &Path, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err(fail("App bundle is too deeply nested."));
    }
    let canonical_root = fs::canonicalize(root)?;
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            if !fs::canonicalize(entry.path())?.starts_with(&canonical_root) {
                return Err(fail("An app bundle link escapes the bundle."));
            }
        } else if ty.is_dir() {
            validate_bundle(root, &entry.path(), depth + 1)?;
        } else if !ty.is_file() {
            return Err(fail("An app bundle contains a special file."));
        }
    }
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn unpack_dmg(
    _archive: &Path,
    _destination: &Path,
    _id: &str,
    _entrypoint: Option<&str>,
) -> Result<()> {
    Err(fail("Disk images require macOS."))
}

pub fn notify(message: &str) {
    let message = message.to_owned();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .summary("CraftLauncher")
            .body(&message)
            .show();
    });
}

#[cfg(test)]
mod supported_package_tests {
    use super::*;
    fn asset(name: &str) -> Asset {
        Asset {
            name: name.into(),
            browser_download_url: String::new(),
            size: 1,
            digest: None,
        }
    }
    #[test]
    fn generic_windows_installers_are_not_offered_as_managed() {
        assert!(supports_managed(
            "artcraft",
            "windows",
            &asset("ArtCraft_x64-setup.exe")
        ));
        assert!(!supports_managed(
            "newcraft",
            "windows",
            &asset("newcraft-x64-setup.exe")
        ));
        assert!(supports_managed(
            "newcraft",
            "windows",
            &asset("newcraft-windows-x64-portable.zip")
        ));
        assert!(!supports_managed(
            "newcraft",
            "linux",
            &asset("newcraft-windows-x64-portable.zip")
        ));
        assert!(supports_managed(
            "newcraft",
            "linux",
            &asset("newcraft-linux-x86_64.appimage")
        ));
    }
}
