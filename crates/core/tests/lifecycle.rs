use craftlauncher_core::{
    platform::{Native, PackageKind, Platform},
    releases::{asset_for, checksum_from_manifest, trusted_asset_url},
    *,
};
use ed25519_dalek::Signer;
use flate2::{Compression, write::GzEncoder};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone)]
struct Fixture {
    version: String,
    bytes: Vec<u8>,
    checksum_bad: bool,
    network_bad: bool,
    cancel_download: bool,
    download_count: usize,
    native_arch: bool,
    launcher: Option<LauncherFeed>,
}
#[derive(Clone)]
struct LauncherFeed {
    version: String,
    archive: Vec<u8>,
    manifest: Vec<u8>,
    sums: Vec<u8>,
    bad_checksum: bool,
    downloads: usize,
}
fn archive(contents: &[u8]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, "photocraft", contents)
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}
fn release(version: &str, bytes: usize) -> Release {
    Release {
        tag_name: version.into(),
        name: None,
        body: Some("Test release".into()),
        published_at: Some("2026-10-07T00:00:00Z".into()),
        prerelease: false,
        draft: false,
        assets: vec![Asset {
            name: format!("photocraft-{version}-linux-x86_64.tar.gz"),
            browser_download_url: format!(
                "https://github.com/storytold/photocraft/releases/download/{version}/photocraft.tar.gz"
            ),
            size: bytes as u64,
            digest: None,
        }],
    }
}
struct FakeRemote(Arc<Mutex<Fixture>>);
impl Remote for FakeRemote {
    fn discover(&self, _: &AtomicBool) -> Result<Vec<workspace::DiscoveredApp>> {
        if self.0.lock().unwrap().network_bad {
            return Err(fail("offline fixture"));
        }
        Ok(vec![workspace::DiscoveredApp {
            app_id: "newcraft".into(),
            github_id: Some(42),
            unavailable: false,
            repository: "NewCraft".into(),
            description: "New app".into(),
        }])
    }
    fn release(
        &self,
        id: &str,
        channel: Channel,
        _: Option<&ReleaseCache>,
    ) -> Result<ReleaseCache> {
        let fixture = self.0.lock().unwrap();
        if fixture.network_bad {
            return Err(fail("offline fixture"));
        }
        let mut release = release(&fixture.version, fixture.bytes.len());
        for asset in &mut release.assets {
            asset.name = asset.name.replace("photocraft", id);
            if fixture.native_arch {
                asset.name = asset.name.replace("x86_64", std::env::consts::ARCH);
            }
            asset.browser_download_url = asset.browser_download_url.replace("photocraft", id);
        }
        Ok(ReleaseCache {
            channel,
            etag: None,
            checked_at: model::now(),
            release: Some(release.clone()),
            versions: vec![release],
            error: None,
        })
    }
    fn checksum(&self, _: &str, _: &Asset, _: &Release) -> Result<String> {
        let fixture = self.0.lock().unwrap();
        if fixture.checksum_bad {
            return Ok("0".repeat(64));
        }
        use sha2::Digest;
        Ok(format!("{:x}", sha2::Sha256::digest(&fixture.bytes)))
    }
    fn download(
        &self,
        _: &str,
        _: &Asset,
        path: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<String> {
        let mut fixture = self.0.lock().unwrap();
        fixture.download_count += 1;
        fs::write(path, &fixture.bytes)?;
        progress(100.0);
        if fixture.cancel_download {
            cancel.store(true, Ordering::Relaxed);
        }
        use sha2::Digest;
        Ok(format!("{:x}", sha2::Sha256::digest(&fixture.bytes)))
    }
    fn launcher_releases(
        &self,
        channel: Channel,
        _: Option<&ReleaseCache>,
    ) -> Result<ReleaseCache> {
        let fixture = self.0.lock().unwrap();
        if fixture.network_bad {
            return Err(fail("offline fixture"));
        }
        let feed = fixture
            .launcher
            .as_ref()
            .ok_or_else(|| fail("no launcher feed fixture"))?;
        let target = self_update::target_id();
        let stem = format!("craftlauncher-{}-{target}", feed.version);
        let archive_name = format!("{stem}.tar.gz");
        let url = |name: &str| {
            format!(
                "https://github.com/EricVogt93/artcraft-launcher/releases/download/v{}/{name}",
                feed.version
            )
        };
        let release = Release {
            tag_name: format!("v{}+build.9.1", feed.version),
            name: None,
            body: None,
            published_at: Some("2026-11-11T00:00:00Z".into()),
            prerelease: false,
            draft: false,
            assets: vec![
                Asset {
                    name: archive_name.clone(),
                    browser_download_url: url(&archive_name),
                    size: feed.archive.len() as u64,
                    digest: None,
                },
                Asset {
                    name: format!("manifest-{target}-stable.json"),
                    browser_download_url: url(&format!("manifest-{target}-stable.json")),
                    size: feed.manifest.len() as u64,
                    digest: None,
                },
                Asset {
                    name: format!("{stem}-SHA256SUMS.txt"),
                    browser_download_url: url(&format!("{stem}-SHA256SUMS.txt")),
                    size: feed.sums.len() as u64,
                    digest: None,
                },
            ],
        };
        Ok(ReleaseCache {
            channel,
            etag: None,
            checked_at: model::now(),
            release: Some(release.clone()),
            versions: vec![release],
            error: None,
        })
    }
    fn launcher_asset_text(&self, asset: &Asset) -> Result<String> {
        let fixture = self.0.lock().unwrap();
        let feed = fixture
            .launcher
            .as_ref()
            .ok_or_else(|| fail("no launcher feed fixture"))?;
        let bytes = if asset.name.starts_with("manifest-") {
            &feed.manifest
        } else {
            &feed.sums
        };
        Ok(String::from_utf8(bytes.clone()).unwrap())
    }
    fn download_launcher_asset(
        &self,
        _asset: &Asset,
        path: &Path,
        _cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<String> {
        let mut fixture = self.0.lock().unwrap();
        let feed = fixture
            .launcher
            .as_mut()
            .ok_or_else(|| fail("no launcher feed fixture"))?;
        feed.downloads += 1;
        fs::write(path, &feed.archive)?;
        progress(100.0);
        use sha2::Digest;
        let bytes: &[u8] = if feed.bad_checksum {
            b"corrupted"
        } else {
            &feed.archive
        };
        Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
    }
}
struct FakePlatform {
    running: Arc<AtomicBool>,
}
impl Platform for FakePlatform {
    fn os(&self) -> &str {
        "linux"
    }
    fn arch(&self) -> &str {
        "x86_64"
    }
    fn is_running(&self, _: &Path) -> Result<bool> {
        Ok(self.running.load(Ordering::Relaxed))
    }
    fn launch(&self, executable: &Path) -> Result<Option<u32>> {
        Native.launch(executable)
    }
    fn open_project(&self, executable: &Path, project: &Path) -> Result<Option<u32>> {
        Native.open_project(executable, project)
    }
    fn unpack(
        &self,
        id: &str,
        _kind: PackageKind,
        archive: &Path,
        stage: &Path,
        cancel: &AtomicBool,
    ) -> Result<(PathBuf, Option<PathBuf>)> {
        let payload = stage.join("payload");
        fs::create_dir(&payload)?;
        installer::extract(archive, &payload, cancel)?;
        Ok((
            installer::find_executable(&payload, id)?
                .unwrap()
                .strip_prefix(stage)
                .unwrap()
                .into(),
            None,
        ))
    }
    fn finish_installer(&self, _: &Path, _: &Path) -> Result<()> {
        Err(fail("not a fixture installer"))
    }
    fn shortcut(&self, _: &str, _: &Path) -> Result<()> {
        Ok(())
    }
    fn remove_shortcut(&self, _: &str) -> Result<()> {
        Ok(())
    }
    fn autostart(&self, _: bool, _: &Path, _: &Path) -> Result<()> {
        Ok(())
    }
}
fn setup(root: &Path) -> (Manager, Arc<Mutex<Fixture>>, Arc<AtomicBool>) {
    let fixture = Arc::new(Mutex::new(Fixture {
        version: "v1.0.0".into(),
        bytes: archive(b"#!/bin/sh\nexit 0\n"),
        checksum_bad: false,
        network_bad: false,
        cancel_download: false,
        download_count: 0,
        native_arch: false,
        launcher: None,
    }));
    let running = Arc::new(AtomicBool::new(false));
    let mut manager = Manager::with_services(
        root.into(),
        Box::new(FakeRemote(fixture.clone())),
        Box::new(FakePlatform {
            running: running.clone(),
        }),
    )
    .unwrap();
    manager.state.settings.notifications = false;
    (manager, fixture, running)
}
fn install(manager: &mut Manager) {
    manager.refresh_app("photocraft").unwrap();
    manager
        .stage_install("photocraft", None, &AtomicBool::new(false), &mut |_| {})
        .unwrap();
}

#[test]
fn install_update_reload_rollback_and_remove_keep_documents() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let document = tmp.path().join("art.psd");
    fs::write(&document, b"my work").unwrap();
    let (mut manager, fixture, running) = setup(&root);
    install(&mut manager);
    assert_eq!(manager.state.installations["photocraft"].version, "v1.0.0");
    let old_path = manager.state.installations["photocraft"].executable.clone();
    fixture.lock().unwrap().version = "v2.0.0".into();
    running.store(true, Ordering::Relaxed);
    install(&mut manager);
    assert_eq!(
        manager.state.installations["photocraft"].version, "v1.0.0",
        "active app must keep its executable"
    );
    assert_eq!(
        manager.state.pending["photocraft"].version.version,
        "v2.0.0"
    );
    drop(manager);
    let mut manager = Manager::with_services(
        root.clone(),
        Box::new(FakeRemote(fixture)),
        Box::new(FakePlatform {
            running: running.clone(),
        }),
    )
    .unwrap();
    assert!(!manager.activate_pending("photocraft").unwrap());
    running.store(false, Ordering::Relaxed);
    assert!(manager.activate_pending("photocraft").unwrap());
    assert_eq!(manager.state.installations["photocraft"].version, "v2.0.0");
    manager.rollback("photocraft").unwrap();
    assert_eq!(
        manager.state.installations["photocraft"].executable,
        old_path
    );
    assert_eq!(
        manager.preferences("photocraft").pinned_version.as_deref(),
        Some("v1.0.0")
    );
    manager.remove("photocraft").unwrap();
    assert!(manager.state.installations.is_empty());
    assert_eq!(fs::read(document).unwrap(), b"my work");
    drop(manager);
    let (manager, _, _) = setup(&root);
    assert!(manager.state.installations.is_empty());
}
#[test]
fn deferred_installer_inventory_covers_files_created_during_activation_and_rollback() {
    struct DeferredPlatform;
    impl Platform for DeferredPlatform {
        fn os(&self) -> &str {
            "linux"
        }
        fn arch(&self) -> &str {
            "x86_64"
        }
        fn is_running(&self, _: &Path) -> Result<bool> {
            Ok(false)
        }
        fn launch(&self, _: &Path) -> Result<Option<u32>> {
            Ok(None)
        }
        fn unpack(
            &self,
            _: &str,
            _: PackageKind,
            _: &Path,
            stage: &Path,
            _: &AtomicBool,
        ) -> Result<(PathBuf, Option<PathBuf>)> {
            fs::create_dir_all(stage.join("payload"))?;
            fs::write(stage.join("setup.exe"), b"installer")?;
            Ok((
                PathBuf::from("payload/photocraft"),
                Some(PathBuf::from("setup.exe")),
            ))
        }
        fn finish_installer(&self, _: &Path, app: &Path) -> Result<()> {
            fs::write(app, b"installed application")?;
            Ok(())
        }
        fn shortcut(&self, _: &str, _: &Path) -> Result<()> {
            Ok(())
        }
        fn remove_shortcut(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn autostart(&self, _: bool, _: &Path, _: &Path) -> Result<()> {
            Ok(())
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let (manager, fixture, _) = setup(tmp.path());
    drop(manager);
    let mut manager = Manager::with_services(
        tmp.path().into(),
        Box::new(FakeRemote(fixture.clone())),
        Box::new(DeferredPlatform),
    )
    .unwrap();
    install(&mut manager);
    assert!(
        manager
            .check_installation("photocraft")
            .unwrap()
            .contains("verified")
    );
    let executable = manager
        .executable("photocraft", &manager.state.installations["photocraft"])
        .unwrap();
    fs::write(&executable, b"corrupted application").unwrap();
    assert!(manager.check_installation("photocraft").is_err());
    fixture.lock().unwrap().version = "v2.0.0".into();
    install(&mut manager);
    manager.rollback("photocraft").unwrap();
    assert!(
        manager
            .check_installation("photocraft")
            .unwrap()
            .contains("verified")
    );
    let executable = manager
        .executable("photocraft", &manager.state.installations["photocraft"])
        .unwrap();
    fs::write(executable, b"corrupted application").unwrap();
    assert!(manager.check_installation("photocraft").is_err());
}

#[test]
fn integrity_check_and_repair_preserve_pins_and_defer_running_app() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, _, running) = setup(tmp.path());
    install(&mut manager);
    assert!(
        manager
            .check_installation("photocraft")
            .unwrap()
            .contains("verified")
    );
    let path = manager
        .executable("photocraft", &manager.state.installations["photocraft"])
        .unwrap();
    fs::write(&path, b"damaged").unwrap();
    assert!(manager.check_installation("photocraft").is_err());
    let preferences = manager.preferences("photocraft");
    running.store(true, Ordering::Relaxed);
    manager
        .repair("photocraft", &AtomicBool::new(false), &mut |_| {})
        .unwrap();
    assert!(manager.state.pending.contains_key("photocraft"));
    assert_eq!(fs::read(&path).unwrap(), b"damaged");
    running.store(false, Ordering::Relaxed);
    assert!(manager.activate_pending("photocraft").unwrap());
    assert!(
        manager
            .check_installation("photocraft")
            .unwrap()
            .contains("verified")
    );
    assert_eq!(
        manager.preferences("photocraft").pinned_version,
        preferences.pinned_version
    );
}

#[test]
fn repair_does_not_modify_linked_installations() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, _, _) = setup(tmp.path());
    let executable = tmp.path().join("external");
    fs::write(&executable, b"external content").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    manager.link("photocraft", executable.clone()).unwrap();
    assert!(
        manager
            .repair("photocraft", &AtomicBool::new(false), &mut |_| {})
            .is_err()
    );
    assert_eq!(fs::read(executable).unwrap(), b"external content");
}

#[test]
fn cache_cleanup_removes_only_selected_unreferenced_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, _, _) = setup(tmp.path());
    let protected_id = queue::new_id();
    let orphan_id = queue::new_id();
    let name = format!("{}.part", "a".repeat(64));
    for id in [&protected_id, &orphan_id] {
        let dir = tmp.path().join("downloads").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(&name), b"partial data").unwrap();
    }
    queue::save(
        tmp.path(),
        &[queue::QueueEntry {
            id: protected_id.clone(),
            action: queue::QueueAction::Install {
                id: "photocraft".into(),
                version: None,
            },
            status: queue::QueueStatus::Paused,
            checkpoint: None,
        }],
    )
    .unwrap();
    let report = manager.storage_report().unwrap();
    let protected = format!("{protected_id}/{name}");
    let orphan = format!("{orphan_id}/{name}");
    assert!(
        report
            .downloads
            .iter()
            .find(|f| f.name == protected)
            .unwrap()
            .protected
    );
    assert!(
        !report
            .downloads
            .iter()
            .find(|f| f.name == orphan)
            .unwrap()
            .protected
    );
    assert!(
        manager
            .remove_cached_downloads(&[orphan.clone(), protected.clone()])
            .is_err()
    );
    assert!(tmp.path().join("downloads").join(&orphan).exists());
    assert!(
        manager
            .remove_cached_downloads(&["../../outside".into()])
            .is_err()
    );
    assert_eq!(
        manager
            .remove_cached_downloads(std::slice::from_ref(&orphan))
            .unwrap(),
        1
    );
    assert!(!tmp.path().join("downloads").join(orphan).exists());
    assert!(tmp.path().join("downloads").join(protected).exists());
}

#[test]
fn storage_report_and_rollback_cleanup_preserve_active_and_external_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(tmp.path());
    install(&mut manager);
    fixture.lock().unwrap().version = "v2.0.0".into();
    install(&mut manager);
    let active = manager
        .executable("photocraft", &manager.state.installations["photocraft"])
        .unwrap();
    let previous = manager.state.installations["photocraft"]
        .previous
        .as_ref()
        .unwrap()
        .clone();
    let old_dir = manager
        .managed_path("photocraft", &previous.directory)
        .unwrap();
    let doc = tmp.path().join("my-project.psd");
    fs::write(&doc, b"my content").unwrap();
    let report = manager.storage_report().unwrap();
    let app = report
        .apps
        .iter()
        .find(|app| app.id == "photocraft")
        .unwrap();
    assert!(app.active_bytes > 0 && app.rollback_bytes > 0);
    assert_eq!(app.rollback_version.as_deref(), Some("v1.0.0"));
    assert!(
        manager
            .remove_rollback("photocraft", "wrong-version")
            .is_err()
    );
    assert!(old_dir.exists());
    manager.remove_rollback("photocraft", "v1.0.0").unwrap();
    assert!(!old_dir.exists());
    assert!(active.exists());
    assert!(manager.state.installations["photocraft"].previous.is_none());
    assert_eq!(fs::read(doc).unwrap(), b"my content");
}

#[test]
fn checksum_failure_and_cancellation_preserve_old_installation() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(tmp.path());
    install(&mut manager);
    let old = manager.state.installations["photocraft"].executable.clone();
    {
        let mut f = fixture.lock().unwrap();
        f.version = "v2.0.0".into();
        f.checksum_bad = true;
    }
    manager.refresh_app("photocraft").unwrap();
    assert!(
        manager
            .stage_install("photocraft", None, &AtomicBool::new(false), &mut |_| {})
            .unwrap_err()
            .to_string()
            .contains("Checksum")
    );
    assert_eq!(manager.state.installations["photocraft"].executable, old);
    assert!(manager.state.pending.is_empty());
    {
        let mut f = fixture.lock().unwrap();
        f.checksum_bad = false;
        f.cancel_download = true;
    }
    assert!(
        manager
            .stage_install("photocraft", None, &AtomicBool::new(false), &mut |_| {})
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert_eq!(manager.state.installations["photocraft"].executable, old);
    assert!(
        fs::read_dir(tmp.path().join("apps/photocraft"))
            .unwrap()
            .all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".staging-"))
    );
}
#[test]
fn offline_checks_keep_cache_and_auto_updates_obey_pin_and_semver() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(tmp.path());
    install(&mut manager);
    fixture.lock().unwrap().network_bad = true;
    assert!(manager.refresh_app("photocraft").is_err());
    assert_eq!(
        manager.state.releases["photocraft"]
            .release
            .as_ref()
            .unwrap()
            .tag_name,
        "v1.0.0"
    );
    assert!(manager.state.releases["photocraft"].error.is_some());
    {
        let mut f = fixture.lock().unwrap();
        f.network_bad = false;
        f.version = "v2.0.0".into();
    }
    manager.refresh_app("photocraft").unwrap();
    manager
        .set_preferences(
            "photocraft",
            AppPreferences {
                pinned_version: Some("v1.0.0".into()),
                ..Default::default()
            },
        )
        .unwrap();
    manager
        .auto_updates(&AtomicBool::new(false), &mut |_| {}, &mut |_| {})
        .unwrap();
    assert_eq!(manager.state.installations["photocraft"].version, "v1.0.0");
    manager
        .set_preferences("photocraft", AppPreferences::default())
        .unwrap();
    manager
        .auto_updates(&AtomicBool::new(false), &mut |_| {}, &mut |_| {})
        .unwrap();
    assert_eq!(manager.state.installations["photocraft"].version, "v2.0.0");
    fixture.lock().unwrap().version = "v1.9.0".into();
    manager.refresh_app("photocraft").unwrap();
    manager
        .auto_updates(&AtomicBool::new(false), &mut |_| {}, &mut |_| {})
        .unwrap();
    assert_eq!(
        manager.state.installations["photocraft"].version, "v2.0.0",
        "never downgrade automatically"
    );
}
#[test]
fn linked_apps_are_not_overwritten_or_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let external = tmp.path().join("outside");
    fs::write(&external, b"#!/bin/sh\nexit 0\n").unwrap();
    installer::make_executable(&external).unwrap();
    let (mut manager, _, _) = setup(&tmp.path().join("library"));
    manager.link("photocraft", external.clone()).unwrap();
    manager.refresh_app("photocraft").unwrap();
    assert!(
        manager
            .stage_install("photocraft", None, &AtomicBool::new(false), &mut |_| {})
            .is_err()
    );
    manager.remove("photocraft").unwrap();
    assert!(external.is_file());
}
#[test]
fn write_failure_does_not_commit_new_settings_or_installation() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, _, _) = setup(tmp.path());
    install(&mut manager);
    fs::remove_file(tmp.path().join("state.json")).unwrap();
    fs::create_dir(tmp.path().join("state.json")).unwrap();
    let before = manager.state.settings.theme;
    let mut settings = manager.state.settings.clone();
    settings.theme = Theme::Dark;
    assert!(manager.configure(settings, Path::new("/unused")).is_err());
    assert_eq!(manager.state.settings.theme, before);
}
#[test]
fn library_lock_and_newer_schema_are_enforced() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, _, _) = setup(tmp.path());
    assert!(Manager::open(tmp.path().into()).is_err());
    drop(manager);
    fs::write(tmp.path().join("state.json"), b"{\"schema\":999}").unwrap();
    assert!(Manager::open(tmp.path().into()).is_err());
    assert!(
        fs::read_to_string(tmp.path().join("state.json"))
            .unwrap()
            .contains("999")
    );
}
#[test]
fn damaged_state_is_saved_and_stale_downloads_are_cleaned() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("state.json"), b"{broken").unwrap();
    let stale = tmp
        .path()
        .join("apps/photocraft")
        .join(format!(".staging-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&stale).unwrap();
    let (manager, _, _) = setup(tmp.path());
    assert!(!stale.exists());
    assert!(!manager.state.activity.is_empty());
    assert!(fs::read_dir(tmp.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("state-damaged-")
    }));
}
#[test]
fn full_platform_asset_selection_never_uses_the_wrong_architecture() {
    let names = [
        "photocraft-v1-linux-x86_64.tar.gz",
        "photocraft-v1-linux-aarch64.AppImage",
        "photocraft-v1-freebsd-x86_64.tar.gz",
        "photocraft-v1-windows-x64-portable.zip",
        "photocraft-v1-windows-x86-portable.zip",
        "photocraft-v1-windows-arm64-portable.zip",
        "photocraft-v1-macos-universal.dmg",
    ];
    let mut release = release("v1.0.0", 10);
    release.assets = names
        .iter()
        .map(|name| Asset {
            name: name.to_string(),
            browser_download_url: String::new(),
            size: 10,
            digest: None,
        })
        .collect();
    for (os, arch, expected) in [
        ("linux", "x86_64", names[0]),
        ("linux", "aarch64", names[1]),
        ("freebsd", "x86_64", names[2]),
        ("windows", "x86_64", names[3]),
        ("windows", "x86", names[4]),
        ("windows", "aarch64", names[5]),
        ("macos", "aarch64", names[6]),
        ("macos", "x86_64", names[6]),
    ] {
        assert_eq!(asset_for(&release, os, arch).unwrap().name, expected);
    }
    assert!(asset_for(&release, "linux", "x86").is_none());
    assert!(asset_for(&release, "freebsd", "aarch64").is_none());
    assert!(is_newer("artcraft-v0.41.0", "artcraft-v0.40.0"));
    assert!(!is_newer("v1.0.0-rc.1", "v1.0.0"));
}
#[test]
fn trusted_urls_and_checksum_filenames_are_bound_to_the_requested_app() {
    assert!(
        trusted_asset_url(
            "photocraft",
            "https://github.com/storytold/photocraft/releases/download/v1/a.zip"
        )
        .is_ok()
    );
    for url in [
        "http://github.com/storytold/photocraft/releases/download/v1/a.zip",
        "https://evil.test/storytold/photocraft/releases/download/v1/a.zip",
        "https://github.com/storytold/vectorcraft/releases/download/v1/a.zip",
        "https://github.com@evil.test/storytold/photocraft/releases/download/v1/a.zip",
    ] {
        assert!(trusted_asset_url("photocraft", url).is_err());
    }
    let hash = "a".repeat(64);
    assert_eq!(
        checksum_from_manifest(&format!("{hash}  *a.zip\n"), "a.zip").unwrap(),
        hash
    );
    assert!(checksum_from_manifest(&format!("{hash}  b.zip\n"), "a.zip").is_err());
}
#[test]
fn zip_traversal_and_tar_symlink_cannot_write_outside_staging() {
    let tmp = tempfile::tempdir().unwrap();
    let zip_file = tmp.path().join("bad.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&zip_file).unwrap());
    zip.start_file("../escaped", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"bad").unwrap();
    zip.finish().unwrap();
    let dest = tmp.path().join("out");
    fs::create_dir(&dest).unwrap();
    assert!(installer::extract_zip(&zip_file, &dest, &AtomicBool::new(false), false).is_err());
    assert!(!tmp.path().join("escaped").exists());
    let encoder = GzEncoder::new(
        fs::File::create(tmp.path().join("link.tar.gz")).unwrap(),
        Compression::fast(),
    );
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_link_name("/tmp/escaped").unwrap();
    header.set_cksum();
    tar.append_data(&mut header, "link", &[][..]).unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    assert!(
        installer::extract(
            &tmp.path().join("link.tar.gz"),
            &dest,
            &AtomicBool::new(false)
        )
        .is_err()
    );
}
#[cfg(unix)]
#[test]
fn internal_bundle_links_work_but_escaped_links_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("bundle.tar.gz");
    let encoder = GzEncoder::new(fs::File::create(&archive).unwrap(), Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    let mut file = tar::Header::new_gnu();
    file.set_size(1);
    file.set_mode(0o755);
    file.set_cksum();
    tar.append_data(&mut file, "Craft.app/Versions/A/tool", &b"x"[..])
        .unwrap();
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o777);
    link.set_link_name("A").unwrap();
    link.set_cksum();
    tar.append_data(&mut link, "Craft.app/Versions/Current", &[][..])
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    installer::extract_tar(&archive, &out, &AtomicBool::new(false), true).unwrap();
    assert_eq!(
        fs::read(out.join("Craft.app/Versions/Current/tool")).unwrap(),
        b"x"
    );
}
#[cfg(unix)]
#[test]
fn native_launch_executes_the_selected_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("launched");
    let script = format!(
        "#!/bin/sh\n: > '{}'\nsleep 0.05\nprintf '%s' launched > '{}'\n",
        marker.display(),
        marker.display()
    );
    let (mut manager, fixture, _) = setup(&tmp.path().join("library"));
    fixture.lock().unwrap().bytes = archive(script.as_bytes());
    install(&mut manager);
    assert!(manager.launch("photocraft").unwrap().is_some());
    for _ in 0..50 {
        if fs::read_to_string(&marker).is_ok_and(|value| value == "launched") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(marker).unwrap(), "launched");
}

#[test]
fn manual_version_selection_is_pinned_and_favorites_preserve_staged_updates() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, running) = setup(tmp.path());
    install(&mut manager);
    fixture.lock().unwrap().version = "v2.0.0".into();
    manager.refresh_app("photocraft").unwrap();
    running.store(true, Ordering::Relaxed);
    manager
        .stage_install(
            "photocraft",
            Some("v2.0.0"),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(
        manager.preferences("photocraft").pinned_version.as_deref(),
        Some("v2.0.0")
    );
    let mut preferences = manager.preferences("photocraft");
    preferences.favorite = true;
    manager.set_preferences("photocraft", preferences).unwrap();
    assert!(manager.state.pending.contains_key("photocraft"));
    running.store(false, Ordering::Relaxed);
    manager.activate_pending("photocraft").unwrap();
    fixture.lock().unwrap().version = "v3.0.0".into();
    manager.refresh_app("photocraft").unwrap();
    manager
        .auto_updates(&AtomicBool::new(false), &mut |_| {}, &mut |_| {})
        .unwrap();
    assert_eq!(manager.state.installations["photocraft"].version, "v2.0.0");
}
#[test]
fn failed_activation_persistence_keeps_the_old_version() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, running) = setup(tmp.path());
    install(&mut manager);
    fixture.lock().unwrap().version = "v2.0.0".into();
    running.store(true, Ordering::Relaxed);
    install(&mut manager);
    running.store(false, Ordering::Relaxed);
    fs::remove_file(tmp.path().join("state.json")).unwrap();
    fs::create_dir(tmp.path().join("state.json")).unwrap();
    assert!(manager.activate_pending("photocraft").is_err());
    assert_eq!(manager.state.installations["photocraft"].version, "v1.0.0");
    assert_eq!(
        manager.state.pending["photocraft"].version.version,
        "v2.0.0"
    );
    assert!(
        manager
            .managed_path(
                "photocraft",
                &manager.state.installations["photocraft"].executable
            )
            .unwrap()
            .is_file()
    );
}
#[cfg(unix)]
#[test]
fn bundle_symlinks_cannot_escape_even_to_existing_files() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("outside"), b"safe").unwrap();
    let archive = tmp.path().join("bundle.tar.gz");
    let encoder = GzEncoder::new(fs::File::create(&archive).unwrap(), Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    let mut file = tar::Header::new_gnu();
    file.set_size(1);
    file.set_mode(0o755);
    file.set_cksum();
    tar.append_data(&mut file, "Craft.app/Contents/tool", &b"x"[..])
        .unwrap();
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o777);
    link.set_link_name("../../../outside").unwrap();
    link.set_cksum();
    tar.append_data(&mut link, "Craft.app/Contents/link", &[][..])
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    let out = tmp.path().join("out");
    fs::create_dir(&out).unwrap();
    assert!(installer::extract_tar(&archive, &out, &AtomicBool::new(false), true).is_err());
    assert_eq!(fs::read(tmp.path().join("outside")).unwrap(), b"safe");
    assert!(!out.join("Craft.app/Contents/link").exists());
}

#[test]
fn multiple_packages_use_native_target_and_persist_save_folder_without_installing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let folder = tmp.path().join("exports");
    let (mut manager, fixture, _) = setup(&root);
    manager.refresh_app("photocraft").unwrap();
    manager.refresh_app("vectorcraft").unwrap();
    let ids = vec![
        "photocraft".into(),
        "vectorcraft".into(),
        "photocraft".into(),
    ];
    assert_eq!(
        manager
            .download_packages(&ids, &folder, &AtomicBool::new(false), &mut |_| {})
            .unwrap(),
        2
    );
    for id in ["photocraft", "vectorcraft"] {
        assert_eq!(
            fs::read(folder.join(format!("{id}-v1.0.0-linux-x86_64.tar.gz"))).unwrap(),
            fixture.lock().unwrap().bytes
        );
    }
    assert!(manager.state.installations.is_empty());
    assert!(manager.state.pending.is_empty());
    assert_eq!(
        manager
            .download_packages(&ids, &folder, &AtomicBool::new(false), &mut |_| {})
            .unwrap(),
        2,
        "existing identical packages are verified and reused"
    );
    assert_eq!(
        fs::read_dir(&folder).unwrap().count(),
        2,
        "no partial/staging files remain"
    );
    drop(manager);
    let (manager, _, _) = setup(&root);
    assert_eq!(
        manager.state.settings.download_folder,
        Some(fs::canonicalize(folder).unwrap())
    );
}
#[test]
fn batch_download_preserves_foreign_files_and_continues_other_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, _, _) = setup(&tmp.path().join("library"));
    let folder = tmp.path().join("exports");
    fs::create_dir(&folder).unwrap();
    manager.refresh_app("photocraft").unwrap();
    manager.refresh_app("vectorcraft").unwrap();
    let foreign = folder.join("vectorcraft-v1.0.0-linux-x86_64.tar.gz");
    fs::write(&foreign, b"keep me").unwrap();
    let result = manager
        .download_packages(
            &["vectorcraft".into(), "photocraft".into()],
            &folder,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err()
        .to_string();
    assert!(result.contains("1 packages saved"));
    assert!(result.contains("already exists"));
    assert_eq!(fs::read(foreign).unwrap(), b"keep me");
    assert!(
        folder
            .join("photocraft-v1.0.0-linux-x86_64.tar.gz")
            .is_file()
    );
    assert!(manager.state.installations.is_empty());
}
#[test]
fn bad_or_cancelled_batch_download_cannot_publish_partial_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(&tmp.path().join("library"));
    let folder = tmp.path().join("exports");
    manager.refresh_app("photocraft").unwrap();
    fixture.lock().unwrap().checksum_bad = true;
    assert!(
        manager
            .download_packages(
                &["photocraft".into()],
                &folder,
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .unwrap_err()
            .to_string()
            .contains("Checksum")
    );
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
    {
        let mut f = fixture.lock().unwrap();
        f.checksum_bad = false;
        f.cancel_download = true;
    }
    assert!(
        manager
            .download_packages(
                &["photocraft".into()],
                &folder,
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
    fixture.lock().unwrap().cancel_download = false;
    manager
        .state
        .releases
        .get_mut("photocraft")
        .unwrap()
        .release
        .as_mut()
        .unwrap()
        .assets[0]
        .name = "../escaped-linux-x86_64.tar.gz".into();
    assert!(
        manager
            .download_packages(
                &["photocraft".into()],
                &folder,
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .unwrap_err()
            .to_string()
            .contains("unsafe filename")
    );
    assert!(!tmp.path().join("escaped-linux-x86_64.tar.gz").exists());
}

#[test]
fn global_auto_update_pause_survives_reload_and_does_not_block_manual_installs() {
    let root = tempfile::tempdir().unwrap();
    let (mut manager, fixture, running) = setup(root.path());
    let cancel = AtomicBool::new(false);
    manager.refresh_app("photocraft").unwrap();
    manager
        .stage_install("photocraft", None, &cancel, &mut |_| {})
        .unwrap();
    fixture.lock().unwrap().version = "v2.0.0".into();
    manager.refresh_app("photocraft").unwrap();
    let mut settings = manager.state.settings.clone();
    settings.auto_app_update = false;
    manager.configure(settings, Path::new("launcher")).unwrap();
    manager
        .auto_updates(&cancel, &mut |_| {}, &mut |_| {})
        .unwrap();
    assert!(manager.state.pending.is_empty());
    assert_eq!(manager.state.installations["photocraft"].version, "v1.0.0");
    manager.state.settings.auto_app_update = true;
    manager.save().unwrap();
    running.store(true, Ordering::Relaxed);
    manager
        .auto_updates(&cancel, &mut |_| {}, &mut |_| {})
        .unwrap();
    assert!(manager.state.pending["photocraft"].automatic);
    manager.state.settings.auto_app_update = false;
    manager.save().unwrap();
    running.store(false, Ordering::Relaxed);
    manager.activate_all_pending().unwrap();
    assert_eq!(manager.state.installations["photocraft"].version, "v1.0.0");
    drop(manager);
    let (mut manager, fixture, _) = setup(root.path());
    assert!(!manager.state.settings.auto_app_update);
    manager.activate_all_pending().unwrap();
    assert!(manager.state.pending.contains_key("photocraft"));
    manager.state.settings.auto_app_update = true;
    manager.save().unwrap();
    manager.activate_all_pending().unwrap();
    assert_eq!(manager.state.installations["photocraft"].version, "v2.0.0");
    manager.state.settings.auto_app_update = false;
    manager.save().unwrap();
    fixture.lock().unwrap().version = "v3.0.0".into();
    manager.refresh_app("photocraft").unwrap();
    manager
        .stage_install("photocraft", None, &cancel, &mut |_| {})
        .unwrap();
    assert_eq!(manager.state.installations["photocraft"].version, "v3.0.0");
}

#[test]
fn reviewed_updates_obey_pins_reject_stale_previews_and_do_not_create_pins() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(tmp.path());
    install(&mut manager);
    fixture.lock().unwrap().version = "v2.0.0".into();
    manager.refresh_app("photocraft").unwrap();
    let preview = workspace::update_preview(&manager.snapshot());
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].next, "v2.0.0");
    assert!(preview[0].bytes > 0);
    let mut prefs = manager.preferences("photocraft");
    prefs.pinned_version = Some("v1.0.0".into());
    manager.set_preferences("photocraft", prefs).unwrap();
    assert!(workspace::update_preview(&manager.snapshot())[0].pinned);
    assert!(
        manager
            .update_reviewed("photocraft", "v2.0.0", &AtomicBool::new(false), &mut |_| {})
            .is_err()
    );
    manager
        .set_preferences("photocraft", AppPreferences::default())
        .unwrap();
    assert!(
        manager
            .update_reviewed("photocraft", "v1.5.0", &AtomicBool::new(false), &mut |_| {})
            .is_err()
    );
    manager
        .update_reviewed("photocraft", "v2.0.0", &AtomicBool::new(false), &mut |_| {})
        .unwrap();
    assert_eq!(manager.state.installations["photocraft"].version, "v2.0.0");
    assert!(manager.preferences("photocraft").pinned_version.is_none());
    assert!(workspace::update_preview(&manager.snapshot()).is_empty());
}

#[cfg(unix)]
#[test]
fn startup_failure_is_reported_but_successful_early_exit_is_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let failed = tmp.path().join("failed");
    fs::write(&failed, b"#!/bin/sh\nexit 7\n").unwrap();
    installer::make_executable(&failed).unwrap();
    let error = Native.launch(&failed).unwrap_err().to_string();
    assert!(error.contains('7'), "{error}");
    let ok = tmp.path().join("ok");
    fs::write(&ok, b"#!/bin/sh\nexit 0\n").unwrap();
    installer::make_executable(&ok).unwrap();
    assert!(Native.launch(&ok).is_ok());
    let (mut manager, _, _) = setup(&tmp.path().join("library"));
    manager.link("photocraft", failed).unwrap();
    assert!(manager.launch("photocraft").is_err());
    assert!(
        manager
            .state
            .activity
            .iter()
            .any(|event| event.action == "launch" && event.error && event.message.contains('7'))
    );
}

#[cfg(unix)]
#[test]
fn project_arguments_are_literal_and_recent_history_survives_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let (mut manager, _, _) = setup(&root);
    let executable = tmp.path().join("photo app");
    let record = tmp.path().join("argument");
    fs::write(
        &executable,
        format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", record.display()),
    )
    .unwrap();
    installer::make_executable(&executable).unwrap();
    manager.link("photocraft", executable).unwrap();
    let project = tmp.path().join("project ; $(touch nope).PSD");
    fs::write(&project, b"fixture").unwrap();
    let project = fs::canonicalize(project).unwrap();
    manager.open_project("photocraft", project.clone()).unwrap();
    for _ in 0..100 {
        if fs::read_to_string(&record).is_ok_and(|value| value == project.display().to_string()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(record).unwrap(),
        project.display().to_string()
    );
    assert_eq!(manager.state.recent_projects.len(), 1);
    manager.open_project("photocraft", project.clone()).unwrap();
    assert_eq!(manager.state.recent_projects.len(), 1);
    assert!(
        manager
            .open_project("photocraft", tmp.path().join("missing"))
            .is_err()
    );
    drop(manager);
    let (manager, _, _) = setup(&root);
    assert_eq!(manager.state.recent_projects[0].path, project);
}

#[cfg(unix)]
#[test]
fn local_discovery_finds_known_apps_without_executing_them_and_ignores_wrappers() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, _, _) = setup(&tmp.path().join("library"));
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    for (name, bytes) in [
        ("pdfcraft", "#!/bin/sh\nexit 9\n"),
        (
            "wordcraft",
            "#!/bin/sh\n# Managed by CraftLauncher: wordcraft\n",
        ),
        ("figcraft", "not executable"),
        (
            "gridcraft-0.8.0-linux-x86_64.AppImage",
            "#!/bin/sh\nexit 9\n",
        ),
    ] {
        let path = bin.join(name);
        fs::write(&path, bytes).unwrap();
        if name != "figcraft" {
            installer::make_executable(&path).unwrap();
        }
    }
    let result = workspace::discover_local(&[bin.clone(), bin], &manager.snapshot());
    assert_eq!(result.len(), 2);
    assert!(
        result
            .iter()
            .any(|app| app.app_id == "printcraft" && app.executable.ends_with("pdfcraft"))
    );
    assert!(result.iter().any(|app| {
        app.app_id == "gridcraft"
            && app
                .executable
                .ends_with("gridcraft-0.8.0-linux-x86_64.AppImage")
    }));

    let opt = tmp.path().join("opt");
    fs::create_dir_all(opt.join("PhotoCraft/bin")).unwrap();
    let opt_bin = opt.join("PhotoCraft/bin/photocraft");
    fs::write(&opt_bin, b"#!/bin/sh\nexit 0\n").unwrap();
    installer::make_executable(&opt_bin).unwrap();
    let v_appimage = opt.join("LightCraft-v1.2.0-x86_64.appimage");
    fs::write(&v_appimage, b"#!/bin/sh\nexit 0\n").unwrap();
    installer::make_executable(&v_appimage).unwrap();
    let opt_result = workspace::discover_local(&[opt], &manager.snapshot());
    assert!(opt_result.iter().any(|app| {
        app.app_id == "photocraft" && app.executable.ends_with("PhotoCraft/bin/photocraft")
    }));
    assert!(opt_result.iter().any(|app| {
        app.app_id == "lightcraft"
            && app
                .executable
                .ends_with("LightCraft-v1.2.0-x86_64.appimage")
    }));
}

#[test]
fn bulk_refresh_reports_failures_and_keeps_cached_release_data() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(tmp.path());
    manager.refresh_app("photocraft").unwrap();
    let previous = manager.state.releases["photocraft"]
        .release
        .as_ref()
        .unwrap()
        .tag_name
        .clone();
    fixture.lock().unwrap().network_bad = true;
    let error = manager
        .refresh_all(&AtomicBool::new(false), &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(error.contains("release checks failed"));
    assert!(error.contains("offline fixture"));
    assert_eq!(
        manager.state.releases["photocraft"]
            .release
            .as_ref()
            .unwrap()
            .tag_name,
        previous
    );
    assert!(manager.state.releases["photocraft"].error.is_some());
}

#[test]
fn offline_product_discovery_preserves_previous_results() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(tmp.path());
    manager.discover_products(&AtomicBool::new(false)).unwrap();
    assert_eq!(manager.state.discovered_apps[0].repository, "NewCraft");
    let checked = manager.state.discovery_checked_at.clone();
    fixture.lock().unwrap().network_bad = true;
    manager.discover_products(&AtomicBool::new(false)).unwrap();
    assert_eq!(manager.state.discovered_apps.len(), 1);
    assert_eq!(manager.state.discovery_checked_at, checked);
    assert_eq!(
        manager.state.discovery_error.as_deref(),
        Some("offline fixture")
    );
}

#[test]
fn discovered_product_is_refreshed_and_downloaded_without_a_builtin_catalog_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(&tmp.path().join("library"));
    manager.discover_products(&AtomicBool::new(false)).unwrap();
    manager
        .refresh_all(&AtomicBool::new(false), &mut |_| {})
        .unwrap();
    assert!(manager.state.releases.contains_key("newcraft"));
    let downloads = tmp.path().join("downloads");
    assert_eq!(
        manager
            .download_packages(
                &["newcraft".into()],
                &downloads,
                &AtomicBool::new(false),
                &mut |_| {}
            )
            .unwrap(),
        1
    );
    assert_eq!(
        fs::read(downloads.join("newcraft-v1.0.0-linux-x86_64.tar.gz")).unwrap(),
        fixture.lock().unwrap().bytes
    );
}

#[test]
fn schema_migration_backs_up_original_and_preserves_user_preferences() {
    let tmp = tempfile::tempdir().unwrap();
    let original = r#"{"schema":1,"preferences":{"photocraft":{"favorite":true,"pinned_version":"v1.0.0"}},"discovered_apps":[{"repository":"NewCraft","description":"test"}]}"#;
    fs::write(tmp.path().join("state.json"), original).unwrap();
    let (manager, _, _) = setup(tmp.path());
    assert_eq!(manager.state.schema, model::STATE_SCHEMA);
    assert!(manager.preferences("photocraft").favorite);
    assert_eq!(
        manager.preferences("photocraft").pinned_version.as_deref(),
        Some("v1.0.0")
    );
    assert_eq!(manager.state.discovered_apps[0].app_id, "newcraft");
    let backups: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("state-before-schema-1-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), original);
    let saved: State =
        serde_json::from_slice(&fs::read(tmp.path().join("state.json")).unwrap()).unwrap();
    assert_eq!(saved.schema, model::STATE_SCHEMA);
}

#[cfg(target_os = "linux")]
#[test]
fn worker_reports_choices_and_preserves_verified_archive_for_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let (manager, fixture, _) = setup(tmp.path());
    drop(manager);
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    for path in ["one/photocraft", "two/photocraft"] {
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, path, &b"test"[..])
            .unwrap();
    }
    {
        let mut fixture = fixture.lock().unwrap();
        fixture.bytes = archive.into_inner().unwrap().finish().unwrap();
        fixture.native_arch = true;
    }
    let mut manager = Manager::with_services(
        tmp.path().into(),
        Box::new(FakeRemote(fixture)),
        Box::new(Native),
    )
    .unwrap();
    manager.state.settings.check_on_startup = false;
    manager.refresh_app("photocraft").unwrap();
    let worker = Worker::new(manager, PathBuf::from("launcher"));
    let task_key = queue::new_id();
    worker
        .send(Command::Queued {
            id: 5,
            task_key: task_key.clone(),
            checkpoint: None,
            command: Box::new(Command::Install {
                id: "photocraft".into(),
                version: None,
            }),
        })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut paths = None;
    loop {
        assert!(std::time::Instant::now() < deadline);
        match worker
            .events
            .recv_timeout(std::time::Duration::from_millis(200))
        {
            Ok(Event::Entrypoints {
                app_id,
                paths: found,
            }) => {
                assert_eq!(app_id, "photocraft");
                paths = Some(found);
            }
            Ok(Event::TaskFinished { id: 5, result }) => {
                assert!(result.is_err());
                break;
            }
            _ => {}
        }
    }
    let paths = paths.expect("choice event must precede failure");
    assert_eq!(paths, vec!["one/photocraft", "two/photocraft"]);
    assert!(
        fs::read_dir(tmp.path().join("downloads").join(task_key))
            .unwrap()
            .next()
            .is_some()
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn native_unpack_requires_a_choice_for_ambiguous_application_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    for path in ["one/newcraft", "two/newcraft"] {
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, path, &b"test"[..])
            .unwrap();
    }
    let archive_path = tmp.path().join("package.tar.gz");
    fs::write(
        &archive_path,
        archive.into_inner().unwrap().finish().unwrap(),
    )
    .unwrap();
    let product = catalog::products(&[workspace::DiscoveredApp {
        app_id: "newcraft".into(),
        github_id: Some(42),
        unavailable: false,
        repository: "NewCraft".into(),
        description: String::new(),
    }])
    .pop()
    .unwrap();
    let first = tmp.path().join("first");
    fs::create_dir(&first).unwrap();
    let error = Native
        .unpack_product_with_entrypoint(
            &product,
            PackageKind::TarGz,
            &archive_path,
            &first,
            &AtomicBool::new(false),
            None,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("Multiple application entries"));
    let second = tmp.path().join("second");
    fs::create_dir(&second).unwrap();
    let (path, _) = Native
        .unpack_product_with_entrypoint(
            &product,
            PackageKind::TarGz,
            &archive_path,
            &second,
            &AtomicBool::new(false),
            Some("two/newcraft"),
        )
        .unwrap();
    assert_eq!(fs::read(second.join(path)).unwrap(), b"test");
}

#[test]
fn discovered_product_install_update_reload_rollback_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    let (mut manager, fixture, running) = setup(&root);
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    let bytes = b"#!/bin/sh\nexit 0\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, "newcraft", &bytes[..])
        .unwrap();
    fixture.lock().unwrap().bytes = builder.into_inner().unwrap().finish().unwrap();
    manager.discover_products(&AtomicBool::new(false)).unwrap();
    manager.refresh_app("newcraft").unwrap();
    manager
        .stage_install("newcraft", None, &AtomicBool::new(false), &mut |_| {})
        .unwrap();
    assert_eq!(manager.state.installations["newcraft"].version, "v1.0.0");
    fixture.lock().unwrap().version = "v2.0.0".into();
    manager.refresh_app("newcraft").unwrap();
    assert_eq!(
        workspace::update_preview(&manager.snapshot())[0].app_id,
        "newcraft"
    );
    manager
        .stage_install("newcraft", None, &AtomicBool::new(false), &mut |_| {})
        .unwrap();
    drop(manager);
    let mut manager = Manager::with_services(
        root,
        Box::new(FakeRemote(fixture)),
        Box::new(FakePlatform { running }),
    )
    .unwrap();
    assert_eq!(manager.state.installations["newcraft"].version, "v2.0.0");
    manager.rollback("newcraft").unwrap();
    assert_eq!(manager.state.installations["newcraft"].version, "v1.0.0");
    manager.remove("newcraft").unwrap();
    assert!(!manager.state.installations.contains_key("newcraft"));
}

#[test]
fn discovery_preserves_stable_identity_on_rename_and_retains_missing_repositories() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, _, _) = setup(tmp.path());
    manager.state.discovered_apps = vec![
        workspace::DiscoveredApp {
            app_id: "oldcraft".into(),
            github_id: Some(42),
            unavailable: false,
            repository: "OldCraft".into(),
            description: "old".into(),
        },
        workspace::DiscoveredApp {
            app_id: "missingcraft".into(),
            github_id: Some(43),
            unavailable: false,
            repository: "MissingCraft".into(),
            description: "missing".into(),
        },
    ];
    manager.discover_products(&AtomicBool::new(false)).unwrap();
    let renamed = manager
        .state
        .discovered_apps
        .iter()
        .find(|app| app.github_id == Some(42))
        .unwrap();
    assert_eq!(renamed.repository, "NewCraft");
    assert_eq!(renamed.app_id, "oldcraft");
    assert!(!renamed.unavailable);
    assert!(
        manager
            .state
            .discovered_apps
            .iter()
            .find(|app| app.github_id == Some(43))
            .unwrap()
            .unavailable
    );
    drop(manager);
    let (manager, _, _) = setup(tmp.path());
    assert_eq!(
        manager
            .state
            .discovered_apps
            .iter()
            .find(|app| app.github_id == Some(42))
            .unwrap()
            .app_id,
        "oldcraft"
    );
}

#[test]
fn resumed_queue_confirms_completed_install_without_downloading_again() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, running) = setup(tmp.path());
    manager.state.settings.check_on_startup = false;
    manager.refresh_app("photocraft").unwrap();
    let checkpoint = queue::QueueCheckpoint {
        version: "v1.0.0".into(),
        previous_install_directory: None,
        previous_pending_directory: None,
    };
    let task_key = queue::new_id();
    queue::save(
        tmp.path(),
        &[queue::QueueEntry {
            id: task_key.clone(),
            action: queue::QueueAction::Install {
                id: "photocraft".into(),
                version: None,
            },
            status: queue::QueueStatus::Running,
            checkpoint: Some(checkpoint.clone()),
        }],
    )
    .unwrap();
    fn run(worker: &Worker, task_key: &str, checkpoint: queue::QueueCheckpoint) -> String {
        worker
            .send(Command::Queued {
                id: 17,
                task_key: task_key.into(),
                checkpoint: Some(checkpoint),
                command: Box::new(Command::Install {
                    id: "photocraft".into(),
                    version: None,
                }),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(std::time::Instant::now() < deadline);
            if let Ok(Event::TaskFinished { id: 17, result }) = worker
                .events
                .recv_timeout(std::time::Duration::from_millis(200))
            {
                return result.unwrap();
            }
        }
    }
    let worker = Worker::new(manager, PathBuf::from("launcher"));
    assert_eq!(run(&worker, &task_key, checkpoint.clone()), "App ready.");
    drop(worker);
    assert_eq!(fixture.lock().unwrap().download_count, 1);
    let restored = queue::load(tmp.path()).unwrap();
    assert_eq!(restored[0].status, queue::QueueStatus::Paused);
    let manager = Manager::with_services(
        tmp.path().into(),
        Box::new(FakeRemote(fixture.clone())),
        Box::new(FakePlatform { running }),
    )
    .unwrap();
    let worker = Worker::new(manager, PathBuf::from("launcher"));
    assert!(
        run(&worker, &task_key, restored[0].checkpoint.clone().unwrap())
            .contains("Previously completed")
    );
    drop(worker);
    assert_eq!(fixture.lock().unwrap().download_count, 1);
}

#[test]
fn resumed_queue_activates_previously_staged_update_without_downloading_again() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, running) = setup(tmp.path());
    manager.state.settings.check_on_startup = false;
    install(&mut manager);
    fixture.lock().unwrap().version = "v2.0.0".into();
    manager.refresh_app("photocraft").unwrap();
    let checkpoint = queue::QueueCheckpoint {
        version: "v2.0.0".into(),
        previous_install_directory: manager.state.installations["photocraft"].directory.clone(),
        previous_pending_directory: None,
    };
    running.store(true, Ordering::Relaxed);
    let task_key = queue::new_id();
    fn run(worker: &Worker, key: &str, checkpoint: queue::QueueCheckpoint) -> String {
        worker
            .send(Command::Queued {
                id: 18,
                task_key: key.into(),
                checkpoint: Some(checkpoint),
                command: Box::new(Command::Install {
                    id: "photocraft".into(),
                    version: None,
                }),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(std::time::Instant::now() < deadline);
            if let Ok(Event::TaskFinished { id: 18, result }) = worker
                .events
                .recv_timeout(std::time::Duration::from_millis(200))
            {
                return result.unwrap();
            }
        }
    }
    let worker = Worker::new(manager, PathBuf::from("launcher"));
    assert!(run(&worker, &task_key, checkpoint.clone()).contains("activate"));
    drop(worker);
    assert_eq!(fixture.lock().unwrap().download_count, 2);
    running.store(false, Ordering::Relaxed);
    let manager = Manager::with_services(
        tmp.path().into(),
        Box::new(FakeRemote(fixture.clone())),
        Box::new(FakePlatform { running }),
    )
    .unwrap();
    assert!(manager.state.pending.contains_key("photocraft"));
    let worker = Worker::new(manager, PathBuf::from("launcher"));
    assert!(run(&worker, &task_key, checkpoint).contains("activated"));
    drop(worker);
    assert_eq!(fixture.lock().unwrap().download_count, 2);
}

#[test]
fn interrupted_queue_download_keeps_owned_cache_until_explicit_discard() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(tmp.path());
    manager.state.settings.check_on_startup = false;
    manager.refresh_app("photocraft").unwrap();
    fixture.lock().unwrap().cancel_download = true;
    let worker = Worker::new(manager, PathBuf::from("launcher"));
    let task_key = queue::new_id();
    worker
        .send(Command::Queued {
            id: 12,
            task_key: task_key.clone(),
            checkpoint: None,
            command: Box::new(Command::Install {
                id: "photocraft".into(),
                version: None,
            }),
        })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        assert!(std::time::Instant::now() < deadline);
        if let Ok(Event::TaskFinished { id, result }) = worker
            .events
            .recv_timeout(std::time::Duration::from_millis(200))
        {
            assert_eq!(id, 12);
            assert!(result.unwrap_err().to_lowercase().contains("cancelled"));
            break;
        }
    }
    let owned = tmp.path().join("downloads").join(&task_key);
    assert!(std::fs::read_dir(&owned).unwrap().next().is_some());
    drop(worker);
    assert!(owned.exists());
    queue::discard_partial(tmp.path(), &task_key).unwrap();
    assert!(!owned.exists());
}

#[test]
fn queued_tasks_report_their_own_ids_and_can_continue_after_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _) = setup(tmp.path());
    manager.state.settings.check_on_startup = false;
    fixture.lock().unwrap().checksum_bad = true;
    manager.refresh_app("photocraft").unwrap();
    let worker = Worker::new(manager, PathBuf::from("launcher"));
    for (id, fails) in [(7, true), (8, false)] {
        if !fails {
            fixture.lock().unwrap().checksum_bad = false;
        }
        worker
            .send(Command::Queued {
                id,
                task_key: queue::new_id(),
                checkpoint: None,
                command: Box::new(Command::Install {
                    id: "photocraft".into(),
                    version: None,
                }),
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "worker completion timed out"
            );
            if let Event::TaskFinished {
                id: completed,
                result,
            } = worker
                .events
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
            {
                assert_eq!(completed, id);
                assert_eq!(result.is_err(), fails);
                break;
            }
        }
        while worker.busy() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}

#[test]
fn native_running_apps_batches_checks_and_fast_commands_enqueue_without_busy_error() {
    let tmp = tempfile::tempdir().unwrap();
    let empty = std::collections::BTreeMap::new();
    assert!(Native.running_apps(&empty).is_empty());

    let mut targets = std::collections::BTreeMap::new();
    targets.insert("current".to_owned(), std::env::current_exe().unwrap());
    targets.insert("missing".to_owned(), tmp.path().join("not-running"));
    let running = Native.running_apps(&targets);
    assert_eq!(running.get("current"), Some(&true));
    assert_eq!(running.get("missing"), Some(&false));

    let (mut manager, _, _) = setup(tmp.path());
    manager.state.settings.check_on_startup = false;
    let woken = Arc::new(AtomicBool::new(false));
    let woken_clone = woken.clone();
    let worker = Worker::with_waker(
        manager,
        PathBuf::from("launcher"),
        Arc::new(move || woken_clone.store(true, Ordering::Release)),
    );
    let prefs = AppPreferences {
        favorite: true,
        ..Default::default()
    };
    worker
        .send(Command::Preferences("photocraft".into(), prefs))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        assert!(std::time::Instant::now() < deadline);
        if let Ok(Event::Finished(result)) = worker
            .events
            .recv_timeout(std::time::Duration::from_millis(200))
        {
            assert!(result.is_ok());
            break;
        }
    }
    assert!(woken.load(Ordering::Acquire));
}

fn launcher_feed(version: &str) -> LauncherFeed {
    let archive = {
        let encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for name in ["craftlauncher", "craftlauncher-updater"] {
            let contents: &[u8] = b"#!/bin/sh\nexit 0\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, contents).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    };
    let target = self_update::target_id();
    let archive_name = format!("craftlauncher-{version}-{target}.tar.gz");
    use sha2::Digest;
    let digest = format!("{:x}", sha2::Sha256::digest(&archive));
    let manifest = serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "version": version,
        "channel": "stable",
        "target": target,
        "archive": archive_name,
        "sha256": digest,
        "bytes": archive.len(),
        "executable": "craftlauncher",
        "helper": "craftlauncher-updater",
    }))
    .unwrap();
    let sums = format!("{digest}  {archive_name}\n").into_bytes();
    LauncherFeed {
        version: version.into(),
        archive,
        manifest,
        sums,
        bad_checksum: false,
        downloads: 0,
    }
}

#[test]
fn github_feed_prepares_verifies_and_respects_version_and_pause() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _running) = setup(tmp.path());
    fixture.lock().unwrap().launcher = Some(launcher_feed("99.0.0"));
    assert!(manager.state.settings.github_update_feed);
    manager
        .check_launcher_update(true, &AtomicBool::new(false))
        .unwrap();
    let prepared = manager
        .state
        .launcher_update
        .as_ref()
        .expect("GitHub feed must prepare the newer launcher");
    assert_eq!(prepared.version, "99.0.0");
    assert_eq!(
        fixture.lock().unwrap().launcher.as_ref().unwrap().downloads,
        1
    );
    assert!(matches!(
        manager.launcher_update_trust(),
        Some(self_update::UpdateTrust::Github)
    ));
    self_update::verify_prepared(tmp.path(), prepared, &self_update::UpdateTrust::Github).unwrap();
    // A second check keeps the prepared update without downloading again.
    manager
        .check_launcher_update(true, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        fixture.lock().unwrap().launcher.as_ref().unwrap().downloads,
        1
    );
    // An older release never replaces the prepared update or the current version.
    manager.state.launcher_update = None;
    fixture.lock().unwrap().launcher = Some(launcher_feed("0.0.1"));
    manager
        .check_launcher_update(true, &AtomicBool::new(false))
        .unwrap();
    assert!(manager.state.launcher_update.is_none());
    assert_eq!(
        fixture.lock().unwrap().launcher.as_ref().unwrap().downloads,
        0
    );
    // Disabling both feeds stops checks entirely.
    fixture.lock().unwrap().launcher = Some(launcher_feed("99.1.0"));
    manager.state.settings.github_update_feed = false;
    manager
        .check_launcher_update(true, &AtomicBool::new(false))
        .unwrap();
    assert!(manager.state.launcher_update.is_none());
    assert_eq!(
        fixture.lock().unwrap().launcher.as_ref().unwrap().downloads,
        0
    );
}

#[test]
fn github_feed_rejects_checksum_mismatch_without_preparing() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _running) = setup(tmp.path());
    let mut feed = launcher_feed("99.0.0");
    feed.bad_checksum = true;
    fixture.lock().unwrap().launcher = Some(feed);
    assert!(
        manager
            .check_launcher_update(true, &AtomicBool::new(false))
            .is_err()
    );
    assert!(manager.state.launcher_update.is_none());
    assert!(
        manager
            .state
            .activity
            .iter()
            .any(|event| event.action == "launcher update" && event.error)
    );
    let leftovers: Vec<_> = fs::read_dir(tmp.path().join("launcher"))
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".update-download-")
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "failed downloads must be cleaned up");
}

#[test]
fn local_signed_feed_takes_priority_over_github() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut manager, fixture, _running) = setup(tmp.path());
    fixture.lock().unwrap().launcher = Some(launcher_feed("99.0.0"));
    let feed_dir = tmp.path().join("local-feed");
    fs::create_dir_all(&feed_dir).unwrap();
    let archive_bytes = launcher_feed("98.0.0").archive;
    let archive_path = feed_dir.join("launcher.tar.gz");
    fs::write(&archive_path, &archive_bytes).unwrap();
    let target = self_update::target_id();
    use sha2::Digest;
    let manifest = serde_json::json!({
        "schema": 1,
        "version": "98.0.0",
        "channel": "stable",
        "target": target,
        "archive": "launcher.tar.gz",
        "sha256": format!("{:x}", sha2::Sha256::digest(&archive_bytes)),
        "bytes": archive_bytes.len(),
        "executable": "craftlauncher",
        "helper": "craftlauncher-updater",
    });
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let manifest_path = feed_dir.join(format!("manifest-{target}-stable.json"));
    fs::write(&manifest_path, &bytes).unwrap();
    fs::write(
        format!("{}.sig", manifest_path.display()),
        hex::encode(key.sign(&bytes).to_bytes()),
    )
    .unwrap();
    manager.state.settings.update_feed = Some(feed_dir);
    manager.state.settings.update_public_key = Some(hex::encode(key.verifying_key().as_bytes()));
    manager
        .check_launcher_update(true, &AtomicBool::new(false))
        .unwrap();
    let prepared = manager.state.launcher_update.as_ref().unwrap();
    assert_eq!(prepared.version, "98.0.0");
    assert_eq!(
        fixture.lock().unwrap().launcher.as_ref().unwrap().downloads,
        0
    );
    assert!(matches!(
        manager.launcher_update_trust(),
        Some(self_update::UpdateTrust::Signed { .. })
    ));
}
