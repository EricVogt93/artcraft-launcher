# Changelog

This changelog describes the launcher only. Changes to third-party apps are published by their own developers.

## Unreleased

- Add an update review with selectable apps, version comparisons, download totals and version-pin protection.
- Queue installations and package downloads with per-item progress, cancellation, failure isolation and retries.
- Open dropped project files in a selected Craft app and keep a clearable local recent-project history.
- Discover local Craft installations and new public products on the official GitHub account.
- Bundle official icons and screenshots for additional products and show release/platform availability in product details.

- Group tools into eight categories with shared catalog metadata, category filters and searchable category names.

- Add AniCraft, CADCraft, DeckCraft, FigCraft, GridCraft, SoundCraft and WordCraft to the app catalog and discipline filters.
- Follow the PrintCraft rename to PdfCraft for release downloads and executable detection while retaining existing local settings and installations.

## 0.1.0 — 2026-10-07

### Your creative workspace
- Native Rust desktop launcher for the ArtCraft application family, with search, filters, favorites and Light/Dark/System themes.
- Clear app selection, aligned download controls, automatic OS/CPU detection and a remembered save folder.
- Original CraftLauncher logo for the sidebar, native window, tray and desktop entry.
- Settings tabs for general preferences, launcher changes and project independence.
- Closing the window quits on Wayland, where hiding to the tray is unavailable; supported window backends retain the optional tray behavior.

### Installs and updates
- Verified package downloads, per-user installation, native launch, uninstall and previous-version rollback.
- Linux AppImages are extracted once and registered with a stable bin command and a desktop entry for KDE Plasma and GNOME.
- Global and per-app automatic-update options, version pins and deferred activation while an app runs.
- Signed local launcher updates with a separate helper, first-frame health checks and automatic recovery.

### Project
- Native build and test CI for Linux, Windows, macOS and FreeBSD; successful main builds publish all platform packages and checksums as GitHub Releases.
- Apache-2.0 source license, preserved third-party attributions, contribution and security guidance.
- CraftLauncher is developed independently by Eric Vogt and is not affiliated with the applications it manages.
