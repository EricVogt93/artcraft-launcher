use craftlauncher_core::{
    platform::{Native, PackageKind, Platform},
    releases::{asset_for, checksum_from_manifest, trusted_asset_url},
    *,
};
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
        let fixture = self.0.lock().unwrap();
        fs::write(path, &fixture.bytes)?;
        progress(100.0);
        if fixture.cancel_download {
            cancel.store(true, Ordering::Relaxed);
        }
        use sha2::Digest;
        Ok(format!("{:x}", sha2::Sha256::digest(&fixture.bytes)))
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
    let script = format!("#!/bin/sh\nprintf '%s' launched > '{}'\n", marker.display());
    let (mut manager, fixture, _) = setup(&tmp.path().join("library"));
    fixture.lock().unwrap().bytes = archive(script.as_bytes());
    install(&mut manager);
    assert!(manager.launch("photocraft").unwrap().is_some());
    for _ in 0..50 {
        if marker.exists() {
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
