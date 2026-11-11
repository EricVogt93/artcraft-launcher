use craftlauncher_core::{
    Channel, Manager,
    persistence::atomic_json,
    self_update::{self, BootPointer, UpdateJob, UpdateManifest},
};
use ed25519_dalek::{Signer, SigningKey};
use flate2::{Compression, write::GzEncoder};
use std::{fs, io::Write, path::Path, sync::atomic::AtomicBool};

fn fixture(
    root: &Path,
    executable: &[u8],
    version: &str,
) -> (std::path::PathBuf, String, UpdateManifest) {
    let feed = root.join("feed");
    fs::create_dir_all(&feed).unwrap();
    let archive = feed.join("launcher.tar.gz");
    let mut tar = tar::Builder::new(GzEncoder::new(
        fs::File::create(&archive).unwrap(),
        Compression::fast(),
    ));
    for (name, bytes) in [
        ("craftlauncher", executable),
        ("craftlauncher-updater", b"#!/bin/sh\nexit 0\n".as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    let manifest = UpdateManifest {
        schema: 1,
        version: version.into(),
        channel: Channel::Stable,
        target: self_update::target_id(),
        archive: "launcher.tar.gz".into(),
        sha256: self_update::hash_file(&archive).unwrap(),
        bytes: archive.metadata().unwrap().len(),
        executable: "craftlauncher".into(),
        helper: "craftlauncher-updater".into(),
    };
    let key = SigningKey::from_bytes(&[19; 32]);
    let public = hex::encode(key.verifying_key().as_bytes());
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let file = feed.join(format!("manifest-{}-stable.json", self_update::target_id()));
    fs::write(&file, &bytes).unwrap();
    fs::write(
        format!("{}.sig", file.display()),
        hex::encode(key.sign(&bytes).to_bytes()),
    )
    .unwrap();
    (feed, public, manifest)
}
#[test]
fn signatures_bind_manifest_platform_paths_and_version() {
    let tmp = tempfile::tempdir().unwrap();
    let (feed, key, mut manifest) = fixture(tmp.path(), b"payload", "1.1.0");
    let file = feed.join(format!("manifest-{}-stable.json", self_update::target_id()));
    let bytes = fs::read(&file).unwrap();
    let signature = fs::read_to_string(format!("{}.sig", file.display())).unwrap();
    assert!(self_update::verify_manifest(&bytes, &signature, &key).is_ok());
    assert!(self_update::verify_manifest(&bytes, &signature, &"00".repeat(32)).is_err());
    let mut changed = bytes.clone();
    changed.push(b' ');
    assert!(self_update::verify_manifest(&changed, &signature, &key).is_err());
    for path in ["../craftlauncher", "/craftlauncher", "C:\\outside.exe"] {
        manifest.executable = path.into();
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let signing = SigningKey::from_bytes(&[19; 32]);
        assert!(
            self_update::verify_manifest(
                &bytes,
                &hex::encode(signing.sign(&bytes).to_bytes()),
                &key
            )
            .is_err()
        );
    }
    assert!(
        self_update::prepare(
            tmp.path(),
            &feed,
            &key,
            Channel::Stable,
            "1.2.0",
            &AtomicBool::new(false)
        )
        .unwrap()
        .is_none()
    );
    assert!(
        self_update::prepare(
            tmp.path(),
            &feed,
            &key,
            Channel::Preview,
            "1.0.0",
            &AtomicBool::new(false)
        )
        .is_err()
    );
}
#[test]
fn archive_corruption_and_cancel_do_not_change_the_active_version() {
    let tmp = tempfile::tempdir().unwrap();
    let (feed, key, _) = fixture(tmp.path(), b"payload", "1.1.0");
    let pointer = tmp.path().join("launcher/current.json");
    atomic_json(
        &pointer,
        &BootPointer {
            version: "1.0.0".into(),
            executable: std::env::current_exe().unwrap(),
        },
    )
    .unwrap();
    let before = fs::read(&pointer).unwrap();
    assert!(
        self_update::prepare(
            tmp.path(),
            &feed,
            &key,
            Channel::Stable,
            "1.0.0",
            &AtomicBool::new(true)
        )
        .is_err()
    );
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(feed.join("launcher.tar.gz"))
        .unwrap();
    file.write_all(b"broken").unwrap();
    assert!(
        self_update::prepare(
            tmp.path(),
            &feed,
            &key,
            Channel::Stable,
            "1.0.0",
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert_eq!(fs::read(&pointer).unwrap(), before);
}
#[test]
fn tampered_payload_is_rejected_even_if_local_state_hash_is_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let (feed, key, _) = fixture(tmp.path(), b"original", "1.1.0");
    let mut prepared = self_update::prepare(
        tmp.path(),
        &feed,
        &key,
        Channel::Stable,
        "1.0.0",
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();
    let trust = self_update::UpdateTrust::Signed {
        public_key: key.clone(),
    };
    assert!(self_update::verify_prepared(tmp.path(), &prepared, &trust).is_ok());
    fs::write(tmp.path().join(&prepared.helper), b"changed").unwrap();
    prepared.helper_sha256 = self_update::hash_file(&tmp.path().join(&prepared.helper)).unwrap();
    assert!(
        self_update::start_helper(
            tmp.path(),
            &prepared,
            &std::env::current_exe().unwrap(),
            &trust,
            1
        )
        .is_err()
    );
    assert!(!tmp.path().join("launcher/update-job.json").exists());
}
#[cfg(unix)]
fn run_job(payload: &[u8], expected_success: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir(&library).unwrap();
    // Exercise aliases such as macOS /var -> /private/var on every Unix runner.
    let alias = tmp.path().join("library-alias");
    std::os::unix::fs::symlink(&library, &alias).unwrap();
    let library = alias;
    let previous = tmp.path().join("previous");
    let marker = tmp.path().join("restored");
    fs::write(
        &previous,
        format!(
            "#!/bin/sh\n: > '{}'\nsleep 0.05\nprintf restored > '{}'\n",
            marker.display(),
            marker.display()
        ),
    )
    .unwrap();
    craftlauncher_core::installer::make_executable(&previous).unwrap();
    let (feed, key, _) = fixture(tmp.path(), payload, "1.1.0");
    let prepared = self_update::prepare(
        &library,
        &feed,
        &key,
        Channel::Stable,
        "1.0.0",
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();
    let job = UpdateJob {
        prepared: prepared.clone(),
        previous: BootPointer {
            version: "1.0.0".into(),
            executable: previous.clone(),
        },
        trust: self_update::UpdateTrust::Signed { public_key: key },
        timeout_seconds: 1,
    };
    atomic_json(&library.join("launcher/update-job.json"), &job).unwrap();
    assert_eq!(self_update::apply_job(&library).unwrap(), expected_success);
    let current = self_update::current_executable(&library).unwrap().unwrap();
    assert_eq!(
        current.canonicalize().unwrap(),
        if expected_success {
            library.join(&prepared.executable)
        } else {
            previous
        }
        .canonicalize()
        .unwrap()
    );
    if !expected_success {
        for _ in 0..50 {
            if fs::read_to_string(&marker).is_ok_and(|value| value == "restored") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(marker).unwrap(), "restored");
    }
    let mut manager = Manager::open(library.clone()).unwrap();
    assert_eq!(
        manager.state.failed_launcher_version.as_deref(),
        if expected_success {
            None
        } else {
            Some("1.1.0")
        }
    );
    assert!(
        manager
            .state
            .activity
            .iter()
            .any(|event| event.action == "launcher update" && event.error != expected_success)
    );
    manager.state.launcher_update = Some(prepared);
    manager.save().unwrap();
    atomic_json(
        &library.join("launcher/last-update.json"),
        &self_update::UpdateReport {
            version: "1.1.0".into(),
            success: expected_success,
            restored: Some("1.0.0".into()),
        },
    )
    .unwrap();
    manager.reconcile_launcher_update().unwrap();
    assert!(manager.state.launcher_update.is_none());
    assert!(!library.join("launcher/update-job.json").exists());
}
#[cfg(unix)]
#[test]
fn successful_start_activates_pointer_and_records_health() {
    run_job(b"#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in --update-health) health=$2; shift;; --expected-version) version=$2; shift;; esac; shift; done\nprintf '%s' \"$version\" > \"$health\"\nsleep 0.2\n", true);
}
#[cfg(unix)]
#[test]
fn unhealthy_start_restores_previous_launcher() {
    run_job(b"#!/bin/sh\nexit 7\n", false);
}
#[cfg(unix)]
#[test]
fn spawn_failure_restores_previous_launcher() {
    run_job(b"#!/nonexistent/interpreter\n", false);
}
#[cfg(unix)]
#[test]
fn health_timeout_restores_previous_launcher() {
    run_job(b"#!/bin/sh\nexec sleep 10\n", false);
}

#[cfg(unix)]
#[test]
fn helper_integrity_failure_restores_the_old_process_and_bootstrap() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir(&library).unwrap();
    let previous = tmp.path().join("previous");
    let marker = tmp.path().join("restored");
    fs::write(
        &previous,
        format!("#!/bin/sh\nprintf restored > '{}'\n", marker.display()),
    )
    .unwrap();
    craftlauncher_core::installer::make_executable(&previous).unwrap();
    let (feed, key, _) = fixture(tmp.path(), b"#!/bin/sh\nexit 0\n", "1.1.0");
    let prepared = self_update::prepare(
        &library,
        &feed,
        &key,
        Channel::Stable,
        "1.0.0",
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();
    let job = UpdateJob {
        prepared: prepared.clone(),
        previous: BootPointer {
            version: "1.0.0".into(),
            executable: previous.clone(),
        },
        trust: self_update::UpdateTrust::Signed { public_key: key },
        timeout_seconds: 1,
    };
    atomic_json(&library.join("launcher/update-job.json"), &job).unwrap();
    // Simulate an interrupted pointer transition. Bootstrap still chooses the old build.
    atomic_json(
        &library.join("launcher/current.json"),
        &BootPointer {
            version: "1.1.0".into(),
            executable: library.join(&prepared.executable),
        },
    )
    .unwrap();
    assert_eq!(
        self_update::current_executable(&library).unwrap(),
        Some(previous.clone())
    );
    fs::write(
        library.join(&prepared.helper),
        b"modified after preparation",
    )
    .unwrap();
    assert!(self_update::apply_job(&library).is_err());
    assert_eq!(
        self_update::current_executable(&library).unwrap(),
        Some(previous)
    );
    for _ in 0..50 {
        if fs::read_to_string(&marker).is_ok_and(|value| value == "restored") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(marker).unwrap(), "restored");
    let manager = Manager::open(library).unwrap();
    assert_eq!(
        manager.state.failed_launcher_version.as_deref(),
        Some("1.1.0")
    );
}

#[test]
fn reconcile_cleans_obsolete_launcher_versions_and_staging_while_keeping_active() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(library.join("launcher/versions/.staging-stale")).unwrap();

    let (feed_v1, key_v1, _) = fixture(&tmp.path().join("f1"), b"#!/bin/sh\nexit 0\n", "1.1.0");
    let v1 = self_update::prepare(
        &library,
        &feed_v1,
        &key_v1,
        Channel::Stable,
        "1.0.0",
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();

    let (feed_v2, key_v2, _) = fixture(&tmp.path().join("f2"), b"#!/bin/sh\nexit 0\n", "1.2.0");
    let v2 = self_update::prepare(
        &library,
        &feed_v2,
        &key_v2,
        Channel::Stable,
        "1.1.0",
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();

    assert!(library.join(&v1.directory).is_dir());
    assert!(library.join(&v2.directory).is_dir());

    atomic_json(
        &library.join("launcher/current.json"),
        &BootPointer {
            version: "1.2.0".into(),
            executable: fs::canonicalize(library.join(&v2.executable)).unwrap(),
        },
    )
    .unwrap();
    atomic_json(
        &library.join("launcher/last-update.json"),
        &self_update::UpdateReport {
            version: "1.2.0".into(),
            success: true,
            restored: None,
        },
    )
    .unwrap();

    let manager = Manager::open(library.clone()).unwrap();
    assert!(!library.join("launcher/versions/.staging-stale").exists());
    assert!(!library.join(&v1.directory).exists());
    assert!(library.join(&v2.directory).is_dir());
    assert!(manager.storage_report().unwrap().launcher_bytes > 0);
}

fn unsigned_archive(root: &Path, version: &str) -> (std::path::PathBuf, Vec<u8>) {
    let archive = root.join("launcher.tar.gz");
    let mut tar = tar::Builder::new(GzEncoder::new(
        fs::File::create(&archive).unwrap(),
        Compression::fast(),
    ));
    for (name, bytes) in [
        ("craftlauncher", b"#!/bin/sh\nexit 0\n".as_slice()),
        ("craftlauncher-updater", b"#!/bin/sh\nexit 0\n".as_slice()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    let manifest = UpdateManifest {
        schema: 1,
        version: version.into(),
        channel: Channel::Stable,
        target: self_update::target_id(),
        archive: "launcher.tar.gz".into(),
        sha256: self_update::hash_file(&archive).unwrap(),
        bytes: archive.metadata().unwrap().len(),
        executable: "craftlauncher".into(),
        helper: "craftlauncher-updater".into(),
    };
    (archive, serde_json::to_vec(&manifest).unwrap())
}

#[test]
fn downloaded_github_update_prepares_and_verifies_without_signature() {
    let tmp = tempfile::tempdir().unwrap();
    let (archive, bytes) = unsigned_archive(tmp.path(), "1.1.0");
    let prepared = self_update::prepare_downloaded(
        tmp.path(),
        &archive,
        &bytes,
        Channel::Stable,
        "1.0.0",
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap();
    assert!(
        !archive.exists(),
        "the downloaded archive is moved into staging"
    );
    let trust = self_update::UpdateTrust::Github;
    self_update::verify_prepared(tmp.path(), &prepared, &trust).unwrap();
    // A forged signature must not silently upgrade an unsigned GitHub update.
    fs::write(
        tmp.path()
            .join(&prepared.directory)
            .join("manifest.json.sig"),
        "00".repeat(64),
    )
    .unwrap();
    assert!(self_update::verify_prepared(tmp.path(), &prepared, &trust).is_err());
}

#[test]
fn downloaded_update_rejects_stale_versions_and_corrupt_archives() {
    let tmp = tempfile::tempdir().unwrap();
    let (archive, bytes) = unsigned_archive(tmp.path(), "1.0.0");
    assert!(
        self_update::prepare_downloaded(
            tmp.path(),
            &archive,
            &bytes,
            Channel::Stable,
            "1.0.0",
            &AtomicBool::new(false)
        )
        .unwrap()
        .is_none()
    );
    let mut file = fs::OpenOptions::new().append(true).open(&archive).unwrap();
    file.write_all(b"corrupt").unwrap();
    assert!(
        self_update::prepare_downloaded(
            tmp.path(),
            &archive,
            &bytes,
            Channel::Stable,
            "0.9.0",
            &AtomicBool::new(false)
        )
        .is_err()
    );
}
