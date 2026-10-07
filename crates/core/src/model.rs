use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

pub const STATE_SCHEMA: u32 = 1;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    Preview,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Light,
    Dark,
    System,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub channel: Channel,
    pub theme: Theme,
    pub check_on_startup: bool,
    pub launch_at_login: bool,
    pub background: bool,
    pub auto_launcher_update: bool,
    pub auto_app_update: bool,
    pub notifications: bool,
    pub update_feed: Option<PathBuf>,
    pub update_public_key: Option<String>,
    pub download_folder: Option<PathBuf>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: Channel::Stable,
            theme: Theme::Light,
            check_on_startup: true,
            launch_at_login: false,
            background: true,
            auto_launcher_update: true,
            auto_app_update: true,
            notifications: true,
            update_feed: None,
            update_public_key: None,
            download_folder: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppPreferences {
    pub channel: Option<Channel>,
    pub auto_update: bool,
    pub pinned_version: Option<String>,
    pub favorite: bool,
}
impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            channel: None,
            auto_update: true,
            pinned_version: None,
            favorite: false,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub published_at: Option<String>,
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    pub assets: Vec<Asset>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseCache {
    pub channel: Channel,
    pub etag: Option<String>,
    pub checked_at: String,
    pub release: Option<Release>,
    #[serde(default)]
    pub versions: Vec<Release>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub version: String,
    pub executable: PathBuf,
    pub directory: PathBuf,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub installer: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installation {
    pub managed: bool,
    pub version: String,
    pub executable: PathBuf,
    pub directory: Option<PathBuf>,
    pub previous: Option<Version>,
    pub installed_at: String,
    pub last_opened_at: Option<String>,
    pub size: u64,
    #[serde(default)]
    pub installer: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpdate {
    pub version: Version,
    pub staged_at: String,
    #[serde(default)]
    pub automatic: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub app_id: Option<String>,
    pub action: String,
    pub message: String,
    pub error: bool,
    pub at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub schema: u32,
    pub settings: Settings,
    pub preferences: BTreeMap<String, AppPreferences>,
    pub installations: BTreeMap<String, Installation>,
    pub pending: BTreeMap<String, PendingUpdate>,
    pub releases: BTreeMap<String, ReleaseCache>,
    pub activity: Vec<Activity>,
    pub launcher_update: Option<crate::self_update::PreparedUpdate>,
    pub failed_launcher_version: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA,
            settings: Settings::default(),
            preferences: BTreeMap::new(),
            installations: BTreeMap::new(),
            pending: BTreeMap::new(),
            releases: BTreeMap::new(),
            activity: Vec::new(),
            launcher_update: None,
            failed_launcher_version: None,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub state: State,
    pub root: PathBuf,
    pub platform: String,
    pub arch: String,
    pub running: BTreeMap<String, bool>,
    pub runtime_verified: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub app_id: String,
    pub phase: String,
    pub percent: f32,
}

pub fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".into())
}

pub fn version_number(tag: &str) -> Option<semver::Version> {
    let tag = tag
        .strip_prefix("artcraft-")
        .unwrap_or(tag)
        .trim_start_matches('v');
    semver::Version::parse(tag).ok()
}
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (version_number(candidate), version_number(current)) {
        (Some(new), Some(old)) => new > old,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_keep_defaults_for_new_update_options() {
        let settings: Settings =
            serde_json::from_str(r#"{"theme":"dark","check_on_startup":false}"#).unwrap();
        assert_eq!(settings.theme, Theme::Dark);
        assert!(!settings.check_on_startup);
        assert!(settings.auto_app_update);
        assert!(settings.auto_launcher_update);
        assert!(settings.download_folder.is_none());
    }

    #[test]
    fn explicit_update_pause_and_save_folder_survive_serialization() {
        let settings: Settings = serde_json::from_str(r#"{"auto_app_update":false,"auto_launcher_update":false,"theme":"system","channel":"preview","download_folder":"creative tools/downloads"}"#).unwrap();
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(!restored.auto_app_update);
        assert!(!restored.auto_launcher_update);
        assert_eq!(restored.theme, Theme::System);
        assert_eq!(restored.channel, Channel::Preview);
        assert_eq!(
            restored.download_folder,
            Some(PathBuf::from("creative tools/downloads"))
        );
    }

    #[test]
    fn pending_update_origin_round_trips_and_legacy_manual_updates_remain_manual() {
        let legacy = r#"{"version":{"version":"v1.0.0","executable":"AppRun","directory":"versions/one"},"staged_at":"2026-10-07T00:00:00Z"}"#;
        let mut update: PendingUpdate = serde_json::from_str(legacy).unwrap();
        assert!(!update.automatic);
        update.automatic = true;
        let restored: PendingUpdate =
            serde_json::from_slice(&serde_json::to_vec(&update).unwrap()).unwrap();
        assert!(restored.automatic);
        assert_eq!(restored.version.version, "v1.0.0");
    }

    #[test]
    fn version_ordering_uses_semver_and_rejects_unknown_tags() {
        assert!(is_newer("artcraft-v0.41.0", "artcraft-v0.9.0"));
        assert!(is_newer("v1.0.0", "v1.0.0-rc.1"));
        assert!(!is_newer("v1.0.0-rc.1", "v1.0.0"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        assert!(!is_newer("v0.9.0", "v0.41.0"));
        for tag in ["latest", "", "v1.0", "v1.2.3/../../other"] {
            assert!(version_number(tag).is_none(), "accepted {tag}");
            assert!(!is_newer(tag, "v1.0.0"));
            assert!(!is_newer("v1.1.0", tag));
        }
    }
}
