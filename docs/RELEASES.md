# Build, qualify, package

The main CI workflow `ci.yml` runs on PRs into `main`, merge-queue candidates and every push to `main`. It checks format/lints, runs unit and lifecycle tests, builds all eight native OS/architecture targets, and uploads installers and archives as Actions artifacts. Its **All platforms** job requires every dependency to succeed and is the protected branch’s mandatory check. Native graphics and installer qualification run separately, using manual dispatch.

The workflow `qualify.yml` builds and runs core tests on native Linux x86_64/ARM64, Windows x86/x64/ARM64 and macOS Intel/Apple Silicon runners; FreeBSD 14.3 uses a VM. Its UI smoke requires a successful native first-frame acknowledgement and persisted reload, rather than treating compilation as runtime proof. Windows also installs, runs and removes the per-user NSIS package. macOS constructs universal binaries, verifies both Mach-O architectures and smoke-tests the signed app bundle. Read the Actions results for current build status; the manual graphics qualification workflow is a separate requirement.

Desktop graphics remain required. Hosted Windows/macOS runners may need graphics/session changes before UI smoke can pass. A failing job is a failed qualification, not permission to drop its runtime assertions. Windows 10, macOS 11 and representative real Wayland environments need separate baseline qualification; modern hosted runner success alone does not certify those baselines.

`python3 scripts/package.py --help` documents archive and installer creation. `--native-installer` requires `dpkg-deb`/`rpmbuild` on Linux, `makensis` on Windows and Apple's `hdiutil`/`codesign`/`iconutil` tools on macOS. Optional `--appimagetool /verified/path/to/appimagetool` creates an AppImage; no binary is downloaded implicitly. FreeBSD packages are portable tar archives.

## Self-update protocol

1. Build all three executables with the version actually embedded in the launcher. Preserve the bootstrap at the installation location.
2. Package the immutable payload and generate exact-target manifests. A macOS universal archive gets separate manifests for `macos-x86_64` and `macos-aarch64`.
3. Sign the exact JSON bytes using `craftlauncher --sign-manifest ... --signing-key ...`. Keep private signing keys outside this repository. Configure only the corresponding public key in the launcher.
4. Preparation verifies the signature, target, channel, version, byte count, archive checksum, archive paths and extracted executable contents. It writes into a unique per-user version directory.
5. At the user's restart or next normal start, the separate helper waits for the library lock to be released, switches an atomic version pointer and starts the new executable.
6. The new app acknowledges health only after its first native frame and only when its embedded version matches the manifest. Failure, timeout or spawn failure restores the previous pointer and launches the previous executable.
7. The worker consumes the update report into local Activity and clears the prepared state. Failed versions are blocked from automatic retries until a newer version is available. A manual check can explicitly retry.

Existing shortcuts route through the original executable/bootstrap to the current pointer. Updated launcher versions stay immutable; the previous launcher remains available for recovery. Old successful launcher payloads are retained deliberately. Production disk-retention policy and a public network feed are future work; neither is silently enabled here.

## Signing and publishing

macOS bundles currently receive ad-hoc signatures for local verification. Developer ID signing/notarization, Windows Authenticode, public release feeds and release publication require the owning project's credentials and a separate release operation. The CI has read-only repository permissions and uploads unsigned local manifests as build artifacts; it does not publish releases or embed a signing key.

See `scripts/update_smoke.py` for healthy and mismatched-version counterprobes against actual separate launcher/helper processes. Core tests additionally cover signature changes, wrong keys, archive substitution, extracted helper tampering, no-health failure, spawn failure and timeout recovery.

## Portable Linux build

`packaging/Dockerfile.linux` uses Ubuntu 22.04 and the official Rust 1.99.0 toolchain. This avoids inheriting the build host's newer glibc requirements. Build with `docker build -t craftlauncher-build:local -f packaging/Dockerfile.linux packaging`, then mount the source at `/work`, an output directory at `/build` and a writable Cargo cache at `/cargo`. The local qualified binaries are in `target/portable-linux/release`; the resulting tar.gz, deb and rpm files are in `target/packages`.

Linux package manifests explicitly declare X11/Xcursor/XInput/XRandR and xkbcommon libraries, as well as Wayland. Graphics drivers and the desktop portal remain desktop-runtime prerequisites. The local Arch build in `target/release` uses the host libc; use the portable build for distribution.
