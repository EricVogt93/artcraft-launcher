//! Project routing, update previews and bounded local application discovery.
use crate::Snapshot;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    pub app_id: String,
    pub opened_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveredApp {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub github_id: Option<u64>,
    #[serde(default)]
    pub unavailable: bool,
    pub repository: String,
    pub description: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveryPage {
    pub etag: Option<String>,
    pub apps: Vec<DiscoveredApp>,
    pub last: bool,
}
#[derive(Clone, Debug)]
pub struct LocalApp {
    pub app_id: String,
    pub executable: PathBuf,
}
#[derive(Clone, Debug)]
pub struct UpdatePreview {
    pub app_id: String,
    pub current: String,
    pub next: String,
    pub bytes: u64,
    pub pinned: bool,
}
pub fn update_preview(snapshot: &Snapshot) -> Vec<UpdatePreview> {
    snapshot
        .products
        .iter()
        .filter_map(|app| {
            let install = snapshot.state.installations.get(&app.id)?;
            let preference = snapshot.state.preferences.get(&app.id);
            let channel = preference
                .and_then(|p| p.channel)
                .unwrap_or(snapshot.state.settings.channel);
            let release = snapshot.state.releases.get(&app.id)?.release_for(channel)?;
            if !install.managed
                || snapshot.state.pending.contains_key(&app.id)
                || !crate::model::is_newer(&release.tag_name, &install.version)
            {
                return None;
            }
            let key = crate::releases::asset_preference_key(&snapshot.platform, &snapshot.arch);
            let asset = crate::releases::resolve_asset(
                release,
                &snapshot.platform,
                &snapshot.arch,
                preference
                    .and_then(|p| p.selected_assets.get(&key))
                    .map(String::as_str),
            )
            .selected()?;
            if !crate::platform::supports_managed(&app.id, &snapshot.platform, asset) {
                return None;
            }
            Some(UpdatePreview {
                app_id: app.id.clone(),
                current: install.version.clone(),
                next: release.tag_name.clone(),
                bytes: asset.size,
                pinned: snapshot
                    .state
                    .preferences
                    .get(&app.id)
                    .is_some_and(|p| p.pinned_version.is_some()),
            })
        })
        .collect()
}
/// These are routing suggestions; formats are ultimately interpreted by each app.
pub fn project_apps(path: &Path) -> Vec<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "artcraft" | "blend" | "glb" | "gltf" | "obj" | "fbx" | "usd" | "usda" | "usdc"
        | "usdz" => vec!["artcraft"],
        "anicraft" | "gif" | "apng" => vec!["anicraft"],
        "psd" | "psb" | "ora" | "xcf" => vec!["photocraft"],
        "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" => vec!["photocraft", "lightcraft"],
        "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf" | "orf" => vec!["lightcraft"],
        "svg" | "eps" | "ai" => vec!["vectorcraft", "figcraft"],
        "pdf" => vec!["printcraft", "designcraft"],
        "mp4" | "mov" | "mkv" | "webm" | "avi" => vec!["filmcraft", "effectcraft"],
        "wav" | "mp3" | "flac" | "ogg" | "aiff" | "mid" | "midi" => vec!["soundcraft"],
        "docx" | "odt" | "rtf" | "txt" => vec!["wordcraft"],
        "xlsx" | "ods" | "csv" | "tsv" => vec!["gridcraft"],
        "pptx" | "odp" => vec!["deckcraft"],
        "dxf" | "dwg" => vec!["cadcraft"],
        "idml" | "indd" => vec!["designcraft"],
        _ => Vec::new(),
    }
}

pub fn local_search_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = directories::BaseDirs::new() {
        for relative in [".local/bin", ".local/opt", "Applications"] {
            roots.push(home.home_dir().join(relative));
        }
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    roots.extend([
        PathBuf::from("/opt"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    #[cfg(target_os = "macos")]
    roots.push(PathBuf::from("/Applications"));
    #[cfg(windows)]
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(path) = std::env::var_os(variable) {
            roots.push(PathBuf::from(&path));
            if variable == "LOCALAPPDATA" {
                roots.push(PathBuf::from(path).join("Programs"));
            }
        }
    }
    roots.retain(|p| p.is_absolute());
    roots.sort();
    roots.dedup();
    roots
}
/// Inspect common locations, never execute probes or recursively scan user documents.
pub fn discover_local(roots: &[PathBuf], snapshot: &Snapshot) -> Vec<LocalApp> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        let Ok(read_dir) = std::fs::read_dir(root) else {
            continue;
        };
        let entries: Vec<String> = read_dir
            .take(2000)
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        if entries.is_empty() {
            continue;
        }
        for app in &snapshot.products {
            if snapshot.state.installations.contains_key(&app.id) {
                continue;
            }
            let names = [
                app.executable_name.as_str(),
                app.id.as_str(),
                app.name.as_str(),
            ];
            let mut candidates = Vec::new();
            for entry in &entries {
                let entry_lower = entry.to_ascii_lowercase();
                let matches_direct = names.iter().any(|name| {
                    let n = name.to_ascii_lowercase();
                    entry_lower == n
                        || entry_lower == format!("{n}.exe")
                        || entry_lower == format!("{n}.appimage")
                        || entry_lower == format!("{n}.app")
                        || (entry_lower.ends_with(".appimage")
                            && [format!("{n}-"), format!("{n}_")].iter().any(|prefix| {
                                entry_lower.strip_prefix(prefix).is_some_and(|rest| {
                                    rest.starts_with(|c: char| c.is_ascii_digit())
                                        || rest.strip_prefix('v').is_some_and(|v| {
                                            v.starts_with(|c: char| c.is_ascii_digit())
                                        })
                                })
                            }))
                });
                if matches_direct {
                    candidates.push(PathBuf::from(entry));
                }
                if names.iter().any(|name| entry.eq_ignore_ascii_case(name)) {
                    for exec in names {
                        candidates.push(PathBuf::from(entry).join(exec));
                        candidates.push(PathBuf::from(entry).join(format!("{exec}.exe")));
                        candidates.push(PathBuf::from(entry).join("bin").join(exec));
                        candidates
                            .push(PathBuf::from(entry).join("bin").join(format!("{exec}.exe")));
                    }
                }
            }
            for relative in candidates {
                let path = root.join(relative);
                let Ok(path) = path.canonicalize() else {
                    continue;
                };
                // Ignore this launcher's managed wrappers and payloads.
                if path.starts_with(&snapshot.root) {
                    continue;
                }
                if path.is_file() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if path
                            .metadata()
                            .map(|m| m.permissions().mode() & 0o111 == 0)
                            .unwrap_or(true)
                        {
                            continue;
                        }
                    }
                    let mut bytes = Vec::new();
                    if std::fs::File::open(&path)
                        .and_then(|f| f.take(256).read_to_end(&mut bytes))
                        .is_ok()
                        && String::from_utf8_lossy(&bytes).contains("# Managed by CraftLauncher:")
                    {
                        continue;
                    }
                } else if !(cfg!(target_os = "macos")
                    && path.extension().is_some_and(|e| e == "app")
                    && path.is_dir())
                {
                    continue;
                }
                if seen.insert((app.id.clone(), path.clone())) {
                    found.push(LocalApp {
                        app_id: app.id.clone(),
                        executable: path,
                    });
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_routing_handles_case_and_shared_formats_without_guessing_unknown_files() {
        assert_eq!(project_apps(Path::new("scene.GLB")), vec!["artcraft"]);
        assert_eq!(project_apps(Path::new("clip.anicraft")), vec!["anicraft"]);
        assert_eq!(project_apps(Path::new("work.PSD")), vec!["photocraft"]);
        assert_eq!(
            project_apps(Path::new("report.PDF")),
            vec!["printcraft", "designcraft"]
        );
        assert_eq!(project_apps(Path::new("sheet.xlsx")), vec!["gridcraft"]);
        assert_eq!(project_apps(Path::new("music.wav")), vec!["soundcraft"]);
        assert!(project_apps(Path::new("unrecognized.bin")).is_empty());
        assert!(project_apps(Path::new("no-extension")).is_empty());
    }
}
