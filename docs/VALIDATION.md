# Local validation

Validated on 2026-10-07 in this workspace. Version: **0.1.0**. This is local implementation qualification, not a published release or certification of every target platform.

## Source and lifecycle checks

`cargo fmt --all --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, and `cargo test --workspace --locked` passed with Rust 1.99.0.

The 41 tests comprise one Linux/FreeBSD desktop-identity unit test, eight core unit tests, 19 app lifecycle/package tests, three Unix/Linux integration tests, eight launcher update tests, one theme test and one bundled-logo test. They cover checksums and cancellation, atomic persistence failures, library locking, external installation preservation, release pins, deferred activation, global automatic-update pause/reload/resume and manual installation while paused, rollback, archive traversal and bundle symlinks, exact platform/architecture selection, batch downloads and existing-file protection, signed manifests, payload tampering, helper failures, health timeouts and theme overrides. The integration tests execute actual bin wrappers with quoted paths and forwarded arguments, protect unmanaged entries and newer registrations, and check AppDir extraction, internal/escaping links and cancellation. Deferred-running-app lifecycle tests use a controlled platform service; separate tests execute real native payloads.

The initial GitHub matrix exposed an empty-file synchronization race in the native launch test and macOS’s `/var` → `/private/var` alias in a self-update assertion. The corrected tests wait for actual child output, retain the original deadline and compare canonical file identity. The empty-file race was reproduced with a controlled child before fixing the assertion; updater tests now exercise a symlinked library on every Unix runner.

## Observed native behavior

| Check | Observed result |
| --- | --- |
| First frame and persisted reload | Passed under isolated X11/Xvfb and the actual Wayland desktop session. |
| Light / Dark | Changed in the native Settings UI, captured and reloaded from persisted state. See [Light](launcher-light.png) and [Dark](launcher-dark.png). |
| Settings tabs | General, Changelog and About & independence were opened in the actual native UI. The launcher-only changelog and explicit Eric Vogt/non-affiliation statement were inspected. See [Changelog](launcher-changelog.png) and [About](launcher-about.png). |
| Wayland identity | The live Wayland protocol emitted `xdg_toplevel.set_app_id("craftlauncher")`; the matching real user `craftlauncher.desktop` points at the generated logo and stable bootstrap. |
| Auto-update option | Disabled through the native Settings checkbox and confirmed `auto_app_update: false` after the UI exited. Automatic pending-version pause/resume and explicit manual installation while paused also passed the lifecycle counterprobe. |
| Layout and selection | Native checkbox selection, clear and select-all produced counts 2, 0 and 7; incompatible ArtCraft was excluded. Light/Dark and a 780 px-wide window were inspected for control alignment, contrast and overflow. |
| Real upstream installation | PhotoCraft v0.2.0 downloaded, SHA-256 verified, extracted and launched from its managed installation through the UI. Its native window opened. |
| Extracted AppImage integration | PhotoCraft v0.2.0's real AppImage was installed through the manager CLI, extracted and launched via `.desktop` → per-user bin wrapper → native AppDir executable. A visible PhotoCraft window and its exact process executable were verified; the original image was absent from the installed version. Test destinations were isolated. |
| Desktop search | GIO's desktop application search returned the registered PhotoCraft entry; KDE's rebuilt service cache contained it. Full GNOME Shell/KRunner interaction was not exercised. `desktop-file-validate` passed. |
| Native uninstall | Removing the actual running PhotoCraft process was refused with “Close the app before uninstalling it.” After it closed, removal deleted the managed version and its owned command and desktop entries. |
| Search | Typing PhotoCraft reduced the app cards to the matching app before native launch. |
| Multiple downloads | The UI's two checkboxes and **Download selected (2)** saved and verified the real PhotoCraft v0.2.0 and VectorCraft v0.3.1 AppImages. An earlier CLI/UI run also verified their tar archives. See [selected cards](launcher-batch.png). |
| Save destination | A different directory selected through the real desktop portal folder dialog was persisted to state. The UI also reads a destination saved by the CLI. |
| Tray | An actual StatusNotifierItem registered with the local KDE watcher and accepted a D-Bus Activate call. Headless sessions without a watcher use ordinary close-to-quit behavior. |
| Healthy launcher update | A separately built 0.2.0 launcher, signed temporary local feed and separate helper switched the pointer; the actual new native launcher acknowledged its first frame. |
| Bad launcher version | A manifest naming 0.3.0 with a 0.2.0 executable failed the embedded-version health check and restored the previous launcher. |
| Damaged prepared helper | Actual startup rejected the corrupted helper, discarded the prepared update and continued with the original native UI. |

The batch-download byte counts and independently computed hashes were:

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| PhotoCraft Linux x86_64 AppImage | 28,244,472 | `ead0d16f79d058881a24989deed79feec4549b098d659f9d0999bd50607b300c` |
| VectorCraft Linux x86_64 AppImage | 50,342,392 | `5f8a6dd184b2d5b99072176e31408974323beb09a10d7b556605371a205bf717` |

The batch downloads did not create or modify managed installations. The separate native installation probe used its own library and bin directory. Fixtures, private test signing keys and test downloads are disposable; the screenshots intentionally retain the paths used during validation.

## Portable Linux artifacts

All three distribution executables were compiled in Ubuntu 22.04 with the official Rust 1.99.0 toolchain. `objdump -T` confirmed a maximum GLIBC requirement of 2.35 for the launcher and 2.34 for its bootstrap/helper. The launcher also passed the native first-frame/reload test inside Ubuntu 22.04 and in the host Wayland session.

The DEB was installed with `dpkg -i`, its actual installed launcher passed native first-frame/reload, its bootstrap reported 0.1.0, bundled licenses were checked, and the DEB was removed with `dpkg -r`. The RPM was extracted with `rpm2cpio`/`cpio`; its actual payload passed the same native smoke. An RPM transaction on an RPM-based distribution remains untested.

Artifacts are in `target/packages`, with checksums in `craftlauncher-0.1.0-linux-x86_64-SHA256SUMS.txt`. The unsigned manifest is for a local feed and requires signing before use. Use `target/portable-linux/release/craftlauncher` for the tested glibc baseline; the ordinary `target/release` build inherits the newer Arch host libc.

Reproduce the behavior checks after building:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python3 scripts/native_smoke.py --binary target/release/craftlauncher
python3 scripts/qualify_updates.py
```

The last command temporarily builds a different embedded version, exercises healthy update, failed-version recovery and helper-tamper recovery, and restores the original executables. Headless Linux needs `dbus-run-session -- xvfb-run -a` around native probes. See [release operations](RELEASES.md) for the portable build and packaging tools.

## Qualification still required

Windows x86/x64/ARM64, macOS Intel/Apple Silicon, Linux ARM64 and FreeBSD have native build/test/packaging jobs in `.github/workflows/qualify.yml`; none of those remote jobs has run from this workspace. Windows 10, macOS 11 and FreeBSD baseline claims require actual machines/VMs. macOS universal bundle, DMG, ad-hoc signing and Windows NSIS behavior remain unqualified locally. Packaging CraftLauncher itself as an optional AppImage has not been qualified here; installation and native integration of an upstream app's real AppImage have been checked as described above.

No production update feed, public release, Developer ID/notarization or Authenticode operation was performed. Launcher self-updates currently use a local signed feed. App availability depends on upstream's actual published assets; ArtCraft has no compatible Linux native asset in the checked release and opens its hosted studio from this launcher.
