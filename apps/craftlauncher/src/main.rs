#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use clap::Parser;
use craftlauncher_core::{
    Manager, Result, fail,
    model::{Channel, Theme},
    platform::{Native, Platform},
    self_update,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

#[derive(Parser)]
#[command(name = "craftlauncher", about = "Your native ArtCraft creative toolbox", version = self_update::build_version())]
struct Args {
    /// Library location; defaults to the native per-user data directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Start hidden if a system tray is available.
    #[arg(long)]
    background: bool,
    #[arg(long)]
    list: bool,
    /// Refresh upstream release metadata without installing app updates.
    #[arg(long)]
    check_releases: bool,
    #[arg(long)]
    install: Option<String>,
    #[arg(long)]
    launch: Option<String>,
    #[arg(long)]
    link: Option<String>,
    #[arg(long, requires = "link")]
    app_path: Option<PathBuf>,
    #[arg(long)]
    remove: Option<String>,
    #[arg(long)]
    rollback: Option<String>,
    #[arg(long)]
    discard_pending: Option<String>,
    /// Version tag to install instead of the latest eligible release.
    #[arg(long, requires = "install")]
    version_choice: Option<String>,
    /// Save packages for this machine without installing (comma-separated app IDs).
    #[arg(long, value_delimiter = ',', num_args = 1..)]
    download: Vec<String>,
    #[arg(long, requires = "download")]
    save_folder: Option<PathBuf>,
    /// Check and apply eligible app updates; active apps retain their current version.
    #[arg(long)]
    update_apps: bool,
    #[arg(long, value_parser = ["stable", "preview"])]
    channel: Option<String>,
    #[arg(long, value_parser = ["light", "dark", "system"])]
    theme: Option<String>,
    #[arg(long)]
    disable_startup_check: bool,
    /// Enable or pause automatic downloads and activation of app updates.
    #[arg(long, value_parser = ["on", "off"])]
    auto_updates: Option<String>,
    #[arg(long)]
    configure_local_feed: Option<PathBuf>,
    #[arg(long, requires = "configure_local_feed")]
    public_key: Option<String>,
    #[arg(long)]
    launcher_check: bool,
    #[arg(long)]
    launcher_restart: bool,
    /// Register a per-user native launcher shortcut.
    #[arg(long)]
    register_launcher: bool,
    /// Generate a local signing key pair; the private key is never printed.
    #[arg(long)]
    generate_update_key: Option<PathBuf>,
    /// Sign an existing update manifest with a local private key.
    #[arg(long)]
    sign_manifest: Option<PathBuf>,
    #[arg(long, requires = "sign_manifest")]
    signing_key: Option<PathBuf>,
    /// Export the launcher icon for platform packaging.
    #[arg(long)]
    export_icon: Option<PathBuf>,
    #[arg(long, default_value_t = 256)]
    icon_size: u32,
    #[arg(long, hide = true)]
    update_health: Option<PathBuf>,
    #[arg(long, hide = true)]
    expected_version: Option<String>,
    /// Exit after N seconds, useful for native UI smoke tests.
    #[arg(long, hide = true)]
    exit_after: Option<u64>,
}
fn main() {
    let args = Args::parse();
    if let Err(error) = run(args) {
        eprintln!("CraftLauncher: {error}");
        std::process::exit(1);
    }
}
fn run(args: Args) -> Result<()> {
    if let Some(folder) = args.generate_update_key {
        return generate_key(&folder);
    }
    if let Some(manifest) = args.sign_manifest {
        return sign(
            &manifest,
            args.signing_key
                .as_deref()
                .ok_or_else(|| fail("Provide --signing-key."))?,
        );
    }
    if let Some(file) = args.export_icon {
        let size = args.icon_size.clamp(16, 1024);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        image::save_buffer(
            &file,
            &craftlauncher_ui_egui::icon_rgba(size as usize),
            size,
            size,
            image::ColorType::Rgba8,
        )
        .map_err(|e| fail(e.to_string()))?;
        return Ok(());
    }
    let root = args.data_dir.clone().unwrap_or(Manager::default_root()?);
    // Old shortcuts continue to open the current launcher after a version-pointer update.
    if args.update_health.is_none()
        && let Some(active) = self_update::current_executable(&root)?
    {
        let current = fs::canonicalize(std::env::current_exe()?)?;
        if fs::canonicalize(&active)? != current {
            let status = std::process::Command::new(active)
                .args(std::env::args_os().skip(1))
                .status()?;
            if !status.success() {
                return Err(fail("The active launcher exited unsuccessfully."));
            }
            return Ok(());
        }
    }
    let cli = args.list
        || args.check_releases
        || args.install.is_some()
        || args.launch.is_some()
        || args.link.is_some()
        || args.remove.is_some()
        || args.rollback.is_some()
        || args.discard_pending.is_some()
        || !args.download.is_empty()
        || args.update_apps
        || args.channel.is_some()
        || args.theme.is_some()
        || args.disable_startup_check
        || args.auto_updates.is_some()
        || args.configure_local_feed.is_some()
        || args.launcher_check
        || args.launcher_restart
        || args.register_launcher;
    let mut manager = match Manager::open(root.clone()) {
        Ok(manager) => manager,
        Err(error)
            if error.to_string().contains("already using this library")
                && !cli
                && args.update_health.is_none() =>
        {
            fs::write(root.join("bring-to-front"), b"show")?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    let executable = std::env::current_exe()?;
    let cancel = AtomicBool::new(false);
    if args.channel.is_some()
        || args.theme.is_some()
        || args.disable_startup_check
        || args.auto_updates.is_some()
        || args.configure_local_feed.is_some()
    {
        let mut settings = manager.state.settings.clone();
        if let Some(channel) = args.channel {
            settings.channel = if channel == "preview" {
                Channel::Preview
            } else {
                Channel::Stable
            };
        }
        if let Some(theme) = args.theme {
            settings.theme = match theme.as_str() {
                "dark" => Theme::Dark,
                "system" => Theme::System,
                _ => Theme::Light,
            };
        }
        if args.disable_startup_check {
            settings.check_on_startup = false;
        }
        if let Some(auto_updates) = args.auto_updates {
            settings.auto_app_update = auto_updates == "on";
        }
        if let Some(feed) = args.configure_local_feed {
            settings.update_feed = Some(fs::canonicalize(feed)?);
            let key = args
                .public_key
                .ok_or_else(|| fail("Provide the trusted update public key."))?;
            if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(fail("Invalid update public key."));
            }
            settings.update_public_key = Some(key);
        }
        manager.configure(settings, &executable)?;
    }
    if args.check_releases || args.update_apps {
        manager.refresh_all(&cancel, &mut |_| {})?;
    }
    if !args.download.is_empty() {
        for id in &args.download {
            manager.refresh_app(id)?;
        }
        let folder = args
            .save_folder
            .clone()
            .or_else(|| manager.state.settings.download_folder.clone())
            .ok_or_else(|| fail("Choose a destination with --save-folder."))?;
        manager.download_packages(&args.download, &folder, &cancel, &mut |p| {
            eprintln!("{}: {} {:.0}%", p.app_id, p.phase, p.percent)
        })?;
    }
    if let Some(id) = args.install {
        manager.refresh_app(&id)?;
        manager.stage_install(&id, args.version_choice.as_deref(), &cancel, &mut |p| {
            eprintln!("{}: {} {:.0}%", p.app_id, p.phase, p.percent)
        })?;
    }
    if let Some(id) = args.link {
        manager.link(
            &id,
            args.app_path.ok_or_else(|| fail("Provide --app-path."))?,
        )?;
    }
    if let Some(id) = args.launch {
        let pid = manager.launch(&id)?;
        eprintln!(
            "App launched{}",
            pid.map(|pid| format!(" (PID {pid})")).unwrap_or_default()
        );
    }
    if let Some(id) = args.remove {
        manager.remove(&id)?;
    }
    if let Some(id) = args.rollback {
        manager.rollback(&id)?;
    }
    if let Some(id) = args.discard_pending {
        manager.discard_pending(&id)?;
    }
    if args.update_apps {
        manager.auto_updates(
            &cancel,
            &mut |p| eprintln!("{}: {} {:.0}%", p.app_id, p.phase, p.percent),
            &mut |_| {},
        )?;
    }
    if args.launcher_check {
        manager.check_launcher_update(true, &cancel)?;
    }
    if args.launcher_restart {
        let prepared = manager
            .state
            .launcher_update
            .as_ref()
            .ok_or_else(|| fail("No verified launcher update is prepared."))?;
        let key = manager
            .state
            .settings
            .update_public_key
            .as_deref()
            .ok_or_else(|| fail("No update public key is configured."))?;
        self_update::start_helper(&manager.root, prepared, &executable, key, 30)?;
    }
    if args.register_launcher {
        register_launcher(&executable)?;
    }
    if !cli
        && args.update_health.is_none()
        && manager.state.settings.auto_launcher_update
        && let Some(prepared) = manager
            .state
            .launcher_update
            .as_ref()
            .filter(|p| self_update::build_version() != p.version)
    {
        let key = manager
            .state
            .settings
            .update_public_key
            .as_deref()
            .ok_or_else(|| fail("No trusted update key is configured."))?;
        match self_update::start_helper(&manager.root, prepared, &executable, key, 30) {
            Ok(()) => return Ok(()),
            Err(error) => {
                manager.state.failed_launcher_version = Some(prepared.version.clone());
                manager.state.launcher_update = None;
                manager.record(
                    None,
                    "launcher update",
                    format!("Update discarded: {error}. Kept the current launcher."),
                    true,
                );
                manager.save()?;
            }
        }
    }
    if cli {
        println!("{}", serde_json::to_string_pretty(&manager.snapshot())?);
        return Ok(());
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if args.data_dir.is_none() && args.update_health.is_none() {
        register_launcher(&std::env::current_exe()?)?;
    }
    if let (Some(path), Some(expected_version)) = (&args.update_health, &args.expected_version) {
        if expected_version != self_update::build_version() {
            return Err(fail(
                "The update payload version does not match its signed manifest.",
            ));
        }
        if !path.starts_with(root.join("launcher")) {
            return Err(fail("Invalid update health acknowledgement path."));
        }
    }
    let health = args
        .update_health
        .zip(args.expected_version)
        .map(
            |(path, expected_version)| craftlauncher_ui_egui::HealthCheck {
                path,
                expected_version,
            },
        );
    let icon = egui::IconData {
        rgba: craftlauncher_ui_egui::icon_rgba(128),
        width: 128,
        height: 128,
    };
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("CraftLauncher")
            .with_app_id(craftlauncher_ui_egui::APP_ID)
            .with_inner_size([1180.0, 830.0])
            .with_min_inner_size([780.0, 580.0])
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "CraftLauncher",
        options,
        Box::new(move |cc| {
            Ok(Box::new(craftlauncher_ui_egui::Launcher::new(
                cc,
                manager,
                args.background,
                health,
                args.exit_after.map(Duration::from_secs),
            )))
        }),
    )
    .map_err(|e| fail(e.to_string()))
}
fn generate_key(folder: &Path) -> Result<()> {
    fs::create_dir_all(folder)?;
    let private = folder.join("update-private.key");
    let public = folder.join("update-public.key");
    if private.exists() || public.exists() {
        return Err(fail("Update keys already exist in this folder."));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|_| fail("Could not obtain secure random bytes."))?;
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(&private)?
        .write_all(hex::encode(seed).as_bytes())?;
    fs::write(&public, hex::encode(key.verifying_key().as_bytes()))?;
    println!(
        "Created local update keys in {}. Keep update-private.key outside source control.",
        folder.display()
    );
    Ok(())
}
fn sign(manifest: &Path, private: &Path) -> Result<()> {
    use ed25519_dalek::Signer;
    let seed: [u8; 32] = hex::decode(fs::read_to_string(private)?.trim())
        .map_err(|_| fail("Invalid signing key."))?
        .try_into()
        .map_err(|_| fail("Invalid signing key size."))?;
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let bytes = fs::read(manifest)?;
    let signature = key.sign(&bytes);
    fs::write(
        PathBuf::from(format!("{}.sig", manifest.display())),
        hex::encode(signature.to_bytes()),
    )?;
    println!("Signed {}", manifest.display());
    Ok(())
}
fn register_launcher(executable: &Path) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        // XDG data root is independent of the launcher library override.
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| fail("Cannot find the user data directory."))?;
        let icon = base.join("craftlauncher/icon.png");
        craftlauncher_core::persistence::atomic_write(&icon, craftlauncher_ui_egui::LOGO_PNG)?;
        let bootstrap = executable
            .parent()
            .map(|parent| parent.join("craftlauncher-bootstrap"));
        let executable = bootstrap
            .as_deref()
            .filter(|path| path.is_file())
            .unwrap_or(executable);
        let text = launcher_desktop_entry(executable, &icon);
        craftlauncher_core::persistence::atomic_write(
            &base.join(format!(
                "applications/{}.desktop",
                craftlauncher_ui_egui::APP_ID
            )),
            text.as_bytes(),
        )?;
        let _ = std::process::Command::new("update-desktop-database")
            .arg(base.join("applications"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|desktop| desktop.to_uppercase().contains("KDE"))
        {
            let _ = std::process::Command::new("kbuildsycoca6")
                .arg("--noincremental")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let _ = executable;
        return Err(fail(
            "Use the native installer or application bundle to register CraftLauncher.",
        ));
    }
    let _ = Native.os();
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn launcher_desktop_entry(executable: &Path, icon: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=CraftLauncher\nComment=Independent launcher for creative apps\nExec={}\nIcon={}\nTerminal=false\nCategories=Graphics;Utility;\nStartupWMClass={}\nX-CraftLauncher-Managed=true\n",
        craftlauncher_core::platform::desktop_quote(executable),
        icon.display(),
        craftlauncher_ui_egui::APP_ID
    )
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod tests {
    #[test]
    fn desktop_identity_matches_wayland_app_id_and_includes_real_icon() {
        let entry = super::launcher_desktop_entry(
            std::path::Path::new("/opt/creative tools/craftlauncher-bootstrap"),
            std::path::Path::new("/data/creative tools/logo.png"),
        );
        assert!(entry.contains("StartupWMClass=craftlauncher\n"));
        assert_eq!(craftlauncher_ui_egui::APP_ID, "craftlauncher");
        assert!(entry.contains("Icon=/data/creative tools/logo.png\n"));
        assert!(entry.contains("Exec=\"/opt/creative tools/craftlauncher-bootstrap\"\n"));
        assert!(entry.contains("X-CraftLauncher-Managed=true\n"));
    }
}
