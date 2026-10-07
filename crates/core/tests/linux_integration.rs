#![cfg(any(target_os = "linux", target_os = "freebsd"))]
use craftlauncher_core::linux_integration::{register, remove};
use std::{fs, process::Command};

#[test]
fn native_bin_entry_forwards_arguments_updates_and_only_removes_its_owner() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home with spaces");
    let data = fixture.path().join("xdg data");
    let payload = fixture.path().join("payload's space");
    fs::create_dir_all(&payload).unwrap();
    let first = payload.join("version one");
    let second = payload.join("version two");
    for entry in [&first, &second] {
        fs::write(
            entry,
            b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CRAFT_TEST_RECORD\"\n",
        )
        .unwrap();
        craftlauncher_core::installer::make_executable(entry).unwrap();
    }
    register(&home, &data, "photocraft", &first).unwrap();
    let bin = home.join(".local/bin/photocraft");
    let desktop = data.join("applications/craftlauncher-photocraft.desktop");
    let record = fixture.path().join("arguments");
    assert!(
        Command::new(&bin)
            .args(["one arg", "$literal; 'two'"])
            .env("CRAFT_TEST_RECORD", &record)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read_to_string(record).unwrap(),
        "one arg\n$literal; 'two'\n"
    );
    let text = fs::read_to_string(&desktop).unwrap();
    assert!(text.contains("Name=PhotoCraft\n"));
    assert!(text.contains(&format!(
        "Exec={}\n",
        craftlauncher_core::platform::desktop_quote(&bin)
    )));
    register(&home, &data, "photocraft", &second).unwrap();
    remove(&home, &data, "photocraft", &first).unwrap();
    assert!(
        bin.exists() && desktop.exists(),
        "An old library must not unlink the newer entry"
    );
    remove(&home, &data, "photocraft", &second).unwrap();
    assert!(!bin.exists() && !desktop.exists());
}

#[test]
fn registration_cannot_overwrite_foreign_files_or_symlinks() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let data = fixture.path().join("data");
    let executable = fixture.path().join("app");
    fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
    let bin = home.join(".local/bin/photocraft");
    let desktop = data.join("applications/craftlauncher-photocraft.desktop");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, b"foreign command").unwrap();
    assert!(register(&home, &data, "photocraft", &executable).is_err());
    assert_eq!(fs::read(&bin).unwrap(), b"foreign command");
    assert!(!desktop.exists());
    fs::remove_file(&bin).unwrap();
    std::os::unix::fs::symlink(&executable, &bin).unwrap();
    assert!(register(&home, &data, "photocraft", &executable).is_err());
    assert!(bin.symlink_metadata().unwrap().file_type().is_symlink());
    fs::remove_file(&bin).unwrap();
    fs::create_dir_all(desktop.parent().unwrap()).unwrap();
    fs::write(&desktop, b"[Desktop Entry]\nName=Foreign app\n").unwrap();
    assert!(register(&home, &data, "photocraft", &executable).is_err());
    assert!(!bin.exists());
    remove(&home, &data, "photocraft", &executable).unwrap();
    assert!(desktop.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn appimage_is_extracted_once_and_its_native_entry_runs_without_the_image() {
    use craftlauncher_core::platform::{Native, PackageKind, Platform};
    use std::sync::atomic::AtomicBool;
    let fixture = tempfile::tempdir().unwrap();
    let runtime = fixture.path().join("runtime");
    fs::write(&runtime, b"#!/bin/sh\n[ \"$1\" = --appimage-extract ] || exit 2\nmkdir -p squashfs-root/usr/bin\ncat > squashfs-root/usr/bin/photocraft <<'APP'\n#!/bin/sh\nprintf native > \"$CRAFT_TEST_RECORD\"\nAPP\nchmod 755 squashfs-root/usr/bin/photocraft\nln -s usr/bin/photocraft squashfs-root/AppRun\n").unwrap();
    let stage = fixture.path().join("stage");
    fs::create_dir(&stage).unwrap();
    let cancel = AtomicBool::new(false);
    let (entry, deferred) = Native
        .unpack(
            "photocraft",
            PackageKind::AppImage,
            &runtime,
            &stage,
            &cancel,
        )
        .unwrap();
    assert_eq!(
        entry,
        std::path::PathBuf::from("payload/squashfs-root/AppRun")
    );
    assert!(deferred.is_none());
    assert!(!stage.join("photocraft.AppImage").exists());
    let record = fixture.path().join("native result");
    assert!(
        Command::new(stage.join(&entry))
            .env("CRAFT_TEST_RECORD", &record)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fs::read(record).unwrap(), b"native");
    let bad_stage = fixture.path().join("bad stage");
    fs::create_dir(&bad_stage).unwrap();
    fs::write(
        &runtime,
        b"#!/bin/sh\nmkdir squashfs-root\nln -s /bin/sh squashfs-root/AppRun\n",
    )
    .unwrap();
    assert!(
        Native
            .unpack(
                "photocraft",
                PackageKind::AppImage,
                &runtime,
                &bad_stage,
                &cancel
            )
            .is_err()
    );
    let cancelled_stage = fixture.path().join("cancelled");
    fs::create_dir(&cancelled_stage).unwrap();
    assert!(
        Native
            .unpack(
                "photocraft",
                PackageKind::AppImage,
                &runtime,
                &cancelled_stage,
                &AtomicBool::new(true)
            )
            .is_err()
    );
    assert!(!cancelled_stage.join("payload/squashfs-root").exists());
}
