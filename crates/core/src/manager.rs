use crate::{
    AppPreferences, Channel, Installation, PendingUpdate, Progress, Release, ReleaseCache, Result,
    Settings, Snapshot, State, Version, fail,
    installer::{safe_native_relative, safe_relative},
    model::{STATE_SCHEMA, is_newer, now},
    persistence::atomic_json,
    platform::{Native, Platform, package_kind},
    releases::{AssetResolution, Github, Remote, asset_preference_key, resolve_asset},
};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
struct TransferMeter {
    last: Option<(f32, std::time::Instant)>,
    bytes_per_second: Option<f64>,
}
impl TransferMeter {
    fn sample(&mut self, percent: f32, total: u64) -> (u64, Option<f64>, Option<u64>) {
        self.sample_at(percent, total, std::time::Instant::now())
    }
    fn sample_at(
        &mut self,
        percent: f32,
        total: u64,
        now: std::time::Instant,
    ) -> (u64, Option<f64>, Option<u64>) {
        let percent = percent.clamp(0.0, 100.0);
        if let Some((previous, time)) = self.last {
            let seconds = now.saturating_duration_since(time).as_secs_f64();
            if percent < previous {
                self.bytes_per_second = None;
                self.last = Some((percent, now));
            } else if seconds >= 0.25 && percent > previous {
                let instantaneous =
                    (f64::from(percent - previous) / 100.0) * total as f64 / seconds;
                self.bytes_per_second = Some(
                    self.bytes_per_second
                        .map_or(instantaneous, |old| 0.7 * old + 0.3 * instantaneous),
                );
                self.last = Some((percent, now));
            }
        } else {
            self.last = Some((percent, now));
        }
        let transferred = (f64::from(percent) / 100.0 * total as f64) as u64;
        let remaining = self
            .bytes_per_second
            .filter(|speed| *speed > 0.0)
            .map(|speed| ((total.saturating_sub(transferred) as f64 / speed).ceil()) as u64);
        (transferred, self.bytes_per_second, remaining)
    }
}

fn root_backup_path(root: &Path, schema: u32) -> PathBuf {
    root.join(format!(
        "state-before-schema-{schema}-{}.json",
        uuid::Uuid::new_v4()
    ))
}

pub struct Manager {
    pub root: PathBuf,
    pub state: State,
    remote: Box<dyn Remote>,
    platform: Box<dyn Platform>,
    pub(crate) queue_owner: Option<String>,
    _lock: File,
}
impl Drop for Manager {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._lock);
    }
}
impl Manager {
    pub fn default_root() -> Result<PathBuf> {
        directories::ProjectDirs::from("dev", "craftlauncher", "CraftLauncher")
            .map(|d| d.data_local_dir().to_path_buf())
            .ok_or_else(|| fail("Cannot locate the user data directory."))
    }
    pub fn open(root: PathBuf) -> Result<Self> {
        Self::with_services(root, Box::new(Github::new()?), Box::new(Native))
    }
    pub fn with_services(
        root: PathBuf,
        remote: Box<dyn Remote>,
        platform: Box<dyn Platform>,
    ) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(root)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| crate::Error::LibraryLocked)?;
        let state_path = root.join("state.json");
        let mut recovery = None;
        let state = match File::open(&state_path) {
            Ok(file) => {
                let mut json = String::new();
                file.take(8 * 1024 * 1024 + 1).read_to_string(&mut json)?;
                if json.len() > 8 * 1024 * 1024 {
                    return Err(fail("The library state is too large."));
                }
                match serde_json::from_str::<State>(&json) {
                    Ok(state) => {
                        if state.schema > STATE_SCHEMA {
                            return Err(fail(
                                "This library was created by a newer launcher. Update CraftLauncher first.",
                            ));
                        }
                        state
                    }
                    Err(_) => {
                        let backup =
                            root.join(format!("state-damaged-{}.json", uuid::Uuid::new_v4()));
                        fs::rename(&state_path, &backup)?;
                        recovery = Some(format!(
                            "Saved damaged library state to {}. Existing app files were preserved.",
                            backup.display()
                        ));
                        State::default()
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.into()),
        };
        let mut manager = Self {
            root,
            state,
            remote,
            platform,
            queue_owner: None,
            _lock: lock,
        };
        manager.validate_state()?;
        if manager.state.schema < STATE_SCHEMA {
            // Keep an exact, separately named copy before persisting the migrated state.
            let backup = root_backup_path(&manager.root, manager.state.schema);
            let mut output = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&backup)?;
            std::io::copy(&mut File::open(&state_path)?, &mut output)?;
            output.sync_all()?;
            for product in &mut manager.state.discovered_apps {
                if product.app_id.is_empty() {
                    product.app_id = product.repository.to_ascii_lowercase();
                }
            }
            manager.state.schema = STATE_SCHEMA;
            manager.save()?;
        }
        manager.cleanup_staging()?;
        manager.reconcile_launcher_update()?;
        if let Some(message) = recovery {
            manager.record(None, "recovery", message, true);
            manager.save()?;
        }
        Ok(manager)
    }
    fn validate_state(&self) -> Result<()> {
        for (id, install) in &self.state.installations {
            self.product(id)?;
            if install.managed {
                self.managed_path(id, &install.executable)?;
                if let Some(dir) = &install.directory {
                    self.managed_path(id, dir)?;
                }
                if let Some(file) = &install.installer {
                    self.managed_path(id, file)?;
                }
                if let Some(previous) = &install.previous {
                    self.validate_version(id, previous)?;
                }
            } else if !install.executable.is_absolute() {
                return Err(fail("Invalid linked app path in the library state."));
            }
        }
        for (id, pending) in &self.state.pending {
            self.validate_version(id, &pending.version)?;
        }
        Ok(())
    }
    fn validate_version(&self, id: &str, version: &Version) -> Result<()> {
        self.managed_path(id, &version.executable)?;
        self.managed_path(id, &version.directory)?;
        if let Some(file) = &version.installer {
            self.managed_path(id, file)?;
        }
        if !version.executable.starts_with(&version.directory)
            || !version.directory.starts_with("versions")
            || version.directory.components().count() != 2
        {
            return Err(fail("Invalid managed version directory."));
        }
        Ok(())
    }
    pub fn managed_path(&self, id: &str, relative: &Path) -> Result<PathBuf> {
        self.product(id)?;
        if !safe_native_relative(relative) {
            return Err(fail("Invalid managed installation path."));
        }
        let base = self.root.join("apps").join(id);
        let result = base.join(relative);
        let mut ancestor = result.as_path();
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or_else(|| fail("Invalid app path."))?;
        }
        // Canonical ancestor checks catch a symlink substituted into a managed path.
        if !fs::canonicalize(ancestor)?.starts_with(&self.root) {
            return Err(fail("An installation path escapes the library."));
        }
        Ok(result)
    }
    fn cleanup_staging(&self) -> Result<()> {
        for info in crate::catalog::products(&self.state.discovered_apps) {
            let base = self.root.join("apps").join(&info.id);
            if base
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(fail("Managed app roots cannot be symlinks."));
            }
            match fs::read_dir(base) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry?;
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if let Some(id) = name.strip_prefix(".staging-")
                            && uuid::Uuid::parse_str(id).is_ok()
                            && entry.file_type()?.is_dir()
                        {
                            fs::remove_dir_all(entry.path())?;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let launcher_versions = self.root.join("launcher/versions");
        if launcher_versions
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(fail("Launcher version root cannot be a symlink."));
        }
        match fs::read_dir(&launcher_versions) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with(".staging-") && entry.file_type()?.is_dir() {
                        fs::remove_dir_all(entry.path())?;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
    pub fn save(&self) -> Result<()> {
        atomic_json(&self.root.join("state.json"), &self.state)
    }
    pub fn reconcile_launcher_update(&mut self) -> Result<()> {
        let path = self.root.join("launcher/last-update.json");
        if !path.is_file() {
            return Ok(());
        }
        let report: crate::self_update::UpdateReport =
            serde_json::from_reader(File::open(&path)?.take(65536))?;
        let before = self.state.clone();
        if self
            .state
            .launcher_update
            .as_ref()
            .is_some_and(|p| p.version == report.version)
        {
            self.state.launcher_update = None;
        }
        self.state.failed_launcher_version = if report.success {
            None
        } else {
            Some(report.version.clone())
        };
        self.record(
            None,
            "launcher update",
            if report.success {
                format!("CraftLauncher {} started successfully.", report.version)
            } else {
                format!(
                    "CraftLauncher {} failed to start. Restored {}.",
                    report.version,
                    report.restored.unwrap_or_default()
                )
            },
            !report.success,
        );
        if let Err(e) = self.save() {
            self.state = before;
            return Err(e);
        }
        fs::remove_file(path)?;
        self.cleanup_launcher_versions()?;
        Ok(())
    }
    fn cleanup_launcher_versions(&self) -> Result<()> {
        if self.root.join("launcher/update-job.json").exists() {
            return Ok(());
        }
        let versions_dir = self.root.join("launcher/versions");
        let Ok(entries) = fs::read_dir(&versions_dir) else {
            return Ok(());
        };
        let versions_canon = fs::canonicalize(&versions_dir)?;
        let active = crate::self_update::current_executable(&self.root)
            .ok()
            .flatten()
            .and_then(|exe| fs::canonicalize(exe).ok());
        let prepared = self
            .state
            .launcher_update
            .as_ref()
            .and_then(|u| fs::canonicalize(self.root.join(&u.directory)).ok());
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".staging-") {
                continue;
            }
            let Ok(path) = fs::canonicalize(entry.path()) else {
                continue;
            };
            if !path.starts_with(&versions_canon) {
                continue;
            }
            let is_active = active.as_ref().is_some_and(|exe| exe.starts_with(&path));
            let is_prepared = prepared.as_ref() == Some(&path);
            if !is_active && !is_prepared {
                let _ = fs::remove_dir_all(path);
            }
        }
        Ok(())
    }
    pub fn check_launcher_update(&mut self, force: bool, cancel: &AtomicBool) -> Result<()> {
        if self.state.launcher_update.is_some()
            || (!force && !self.state.settings.auto_launcher_update)
        {
            return Ok(());
        }
        let current = crate::self_update::build_version();
        let threshold = self
            .state
            .failed_launcher_version
            .as_deref()
            .filter(|v| !force && is_newer(v, current))
            .unwrap_or(current)
            .to_string();
        let settings = self.state.settings.clone();
        let result = if let (Some(feed), Some(key)) =
            (&settings.update_feed, &settings.update_public_key)
        {
            crate::self_update::prepare(&self.root, feed, key, settings.channel, &threshold, cancel)
        } else if settings.github_update_feed {
            self.check_launcher_github(settings.channel, &threshold, cancel)
        } else {
            return Ok(());
        };
        match result {
            Ok(prepared) => {
                self.state.launcher_update = prepared;
                self.save()
            }
            Err(e) => {
                self.record(None, "launcher update", e.to_string(), true);
                self.save()?;
                Err(e)
            }
        }
    }

    /// Check the official GitHub release feed. Downloads are bound to the exact
    /// repository and verified against the published SHA-256 sums and manifest.
    fn check_launcher_github(
        &mut self,
        channel: Channel,
        threshold: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<crate::self_update::PreparedUpdate>> {
        let cache = match self
            .remote
            .launcher_releases(channel, self.state.launcher_release.as_ref())
        {
            Ok(cache) => cache,
            Err(e) => {
                if let Some(previous) = &mut self.state.launcher_release {
                    previous.error = Some(e.to_string());
                }
                return Err(e);
            }
        };
        self.state.launcher_release = Some(cache.clone());
        let Some(release) = cache.release_for(channel) else {
            return Ok(None);
        };
        let Some(version) = crate::model::version_number(&release.tag_name) else {
            return Ok(None);
        };
        if !is_newer(&version.to_string(), threshold) {
            return Ok(None);
        }
        let target = crate::self_update::target_id();
        let manifest_name = format!(
            "manifest-{target}-{}.json",
            match channel {
                Channel::Stable => "stable",
                Channel::Preview => "preview",
            }
        );
        let manifest_asset = release
            .assets
            .iter()
            .find(|a| a.name == manifest_name)
            .ok_or_else(|| fail("The latest release has no update manifest for this platform."))?;
        let manifest_bytes = self
            .remote
            .launcher_asset_text(manifest_asset)?
            .into_bytes();
        let manifest = crate::self_update::parse_manifest(&manifest_bytes)?;
        let manifest_version = crate::model::version_number(&manifest.version);
        let same_base = manifest_version
            .as_ref()
            .zip(Some(&version))
            .is_some_and(|(a, b)| {
                a.major == b.major && a.minor == b.minor && a.patch == b.patch && a.pre == b.pre
            });
        if manifest.channel != channel || !same_base {
            return Err(fail(
                "The release update manifest does not match its release.",
            ));
        }
        let archive = release
            .assets
            .iter()
            .find(|a| a.name == manifest.archive)
            .ok_or_else(|| fail("The release update archive is missing."))?;
        if archive.size != manifest.bytes {
            return Err(fail("The release update archive has an invalid size."));
        }
        let sums_name = format!(
            "{}-SHA256SUMS.txt",
            manifest
                .archive
                .trim_end_matches(".tar.gz")
                .trim_end_matches(".zip")
        );
        let sums_asset = release
            .assets
            .iter()
            .find(|a| a.name == sums_name)
            .ok_or_else(|| fail("The release has no checksums for the update archive."))?;
        let sums = self.remote.launcher_asset_text(sums_asset)?;
        let expected = crate::releases::checksum_from_manifest(&sums, &archive.name)?;
        if expected != manifest.sha256.to_ascii_lowercase() {
            return Err(fail(
                "The release checksums do not match the update manifest.",
            ));
        }
        let folder = self.root.join("launcher");
        fs::create_dir_all(&folder)?;
        let part = folder.join(format!(".update-download-{}.part", uuid::Uuid::new_v4()));
        let result = (|| {
            let digest =
                self.remote
                    .download_launcher_asset(archive, &part, cancel, &mut |_| {})?;
            if digest != expected {
                return Err(fail("Launcher archive checksum mismatch."));
            }
            crate::self_update::prepare_downloaded(
                &self.root,
                &part,
                &manifest_bytes,
                channel,
                threshold,
                cancel,
            )
        })();
        if result.is_err() {
            let _ = fs::remove_file(&part);
        }
        result
    }

    /// Trust anchor for the prepared update: a configured local signing key wins,
    /// otherwise the update came from the official GitHub feed.
    pub fn launcher_update_trust(&self) -> Option<crate::self_update::UpdateTrust> {
        if let Some(key) = &self.state.settings.update_public_key {
            return Some(crate::self_update::UpdateTrust::Signed {
                public_key: key.clone(),
            });
        }
        if self.state.settings.github_update_feed {
            return Some(crate::self_update::UpdateTrust::Github);
        }
        None
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut executables = BTreeMap::new();
        let mut running = BTreeMap::new();
        for (id, install) in &self.state.installations {
            match self.executable(id, install) {
                Ok(path) => {
                    executables.insert(id.clone(), path);
                }
                Err(_) => {
                    running.insert(id.clone(), true);
                }
            }
        }
        for (id, is_running) in self.platform.running_apps(&executables) {
            running.insert(id, is_running);
        }
        Snapshot {
            products: crate::catalog::products(&self.state.discovered_apps),
            state: self.state.clone(),
            root: self.root.clone(),
            platform: self.platform.os().into(),
            arch: self.platform.arch().into(),
            running,
            runtime_verified: option_env!("CRAFTLAUNCHER_VERIFIED_TARGET").is_some_and(|target| {
                target == format!("{}-{}", self.platform.os(), self.platform.arch())
            }),
        }
    }
    pub fn record(
        &mut self,
        id: Option<&str>,
        action: &str,
        message: impl Into<String>,
        error: bool,
    ) {
        self.state.activity.insert(
            0,
            crate::Activity {
                app_id: id.map(str::to_owned),
                action: action.into(),
                message: message.into(),
                error,
                at: now(),
            },
        );
        self.state.activity.truncate(60);
    }
    pub fn product(&self, id: &str) -> Result<crate::catalog::Product> {
        crate::catalog::products(&self.state.discovered_apps)
            .into_iter()
            .find(|product| product.id == id)
            .ok_or_else(|| fail("Unknown app."))
    }
    pub fn preferences(&self, id: &str) -> AppPreferences {
        self.state.preferences.get(id).cloned().unwrap_or_default()
    }
    pub fn channel(&self, id: &str) -> Channel {
        self.preferences(id)
            .channel
            .unwrap_or(self.state.settings.channel)
    }
    pub fn selected_release(&self, id: &str) -> Option<&Release> {
        let channel = self.channel(id);
        let cache = self
            .state
            .releases
            .get(id)
            .filter(|c| c.channel == channel || !c.versions.is_empty())?;
        if let Some(pin) = self.preferences(id).pinned_version {
            cache
                .versions_for(channel)
                .find(|r| r.tag_name == pin)
                .or_else(|| cache.release_for(channel).filter(|r| r.tag_name == pin))
        } else {
            cache.release_for(channel)
        }
    }
    pub fn asset_for_release<'a>(
        &self,
        id: &str,
        release: &'a Release,
    ) -> Option<&'a crate::Asset> {
        let preferences = self.preferences(id);
        let key = asset_preference_key(self.platform.os(), self.platform.arch());
        resolve_asset(
            release,
            self.platform.os(),
            self.platform.arch(),
            preferences.selected_assets.get(&key).map(String::as_str),
        )
        .selected()
    }
    fn require_asset_for_release<'a>(
        &self,
        id: &str,
        release: &'a Release,
    ) -> Result<&'a crate::Asset> {
        let preferences = self.preferences(id);
        let key = asset_preference_key(self.platform.os(), self.platform.arch());
        match resolve_asset(
            release,
            self.platform.os(),
            self.platform.arch(),
            preferences.selected_assets.get(&key).map(String::as_str),
        ) {
            AssetResolution::Ready(asset) => Ok(asset),
            AssetResolution::Ambiguous(candidates) => Err(fail(format!(
                "Choose a compatible package: {}",
                candidates
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
            AssetResolution::NoCompatible => Err(fail(
                "No compatible published package is available for this platform.",
            )),
        }
    }
    pub fn compatible(&self, id: &str) -> bool {
        self.selected_release(id)
            .and_then(|r| self.asset_for_release(id, r))
            .is_some_and(|asset| crate::platform::supports_managed(id, self.platform.os(), asset))
    }
    fn partial_path(
        &self,
        product: &crate::catalog::Product,
        release: &Release,
        asset: &crate::Asset,
        checksum: &str,
    ) -> Result<PathBuf> {
        let directory = self.root.join("downloads");
        fs::create_dir_all(&directory)?;
        if directory.symlink_metadata()?.file_type().is_symlink()
            || !fs::canonicalize(&directory)?.starts_with(&self.root)
        {
            return Err(fail("Unsafe download cache directory."));
        }
        let directory = if let Some(owner) = &self.queue_owner {
            if !crate::queue::valid_id(owner) {
                return Err(fail("Invalid queue task identifier."));
            }
            let owned = directory.join(owner);
            fs::create_dir_all(&owned)?;
            if owned.symlink_metadata()?.file_type().is_symlink()
                || !fs::canonicalize(&owned)?.starts_with(&self.root)
            {
                return Err(fail("Unsafe task cache directory."));
            }
            owned
        } else {
            directory
        };
        let mut hash = Sha256::new();
        for part in [
            &product.id,
            &product.repository,
            &release.tag_name,
            &asset.name,
            &asset.browser_download_url,
            &asset.size.to_string(),
            &checksum.to_ascii_lowercase(),
        ] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        let path = directory.join(format!("{:x}.part", hash.finalize()));
        if path
            .symlink_metadata()
            .is_ok_and(|m| !m.file_type().is_file())
        {
            return Err(fail("Unsafe partial download file."));
        }
        Ok(path)
    }
    pub fn default_download_folder() -> Option<PathBuf> {
        directories::UserDirs::new().and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
    }
    /// Save upstream packages without installing them, using this machine's exact target.
    pub fn download_packages(
        &mut self,
        ids: &[String],
        folder: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<usize> {
        if ids.is_empty() {
            return Err(fail("Select at least one app to download."));
        }
        let ids: std::collections::BTreeSet<_> = ids.iter().collect();
        for id in &ids {
            self.product(id)?;
        }
        fs::create_dir_all(folder)?;
        let folder = fs::canonicalize(folder)?;
        let before = self.state.settings.download_folder.clone();
        self.state.settings.download_folder = Some(folder.clone());
        if let Err(e) = self.save() {
            self.state.settings.download_folder = before;
            return Err(e);
        }
        let mut saved = 0;
        let mut errors = Vec::new();
        for (index, id) in ids.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::cancelled(format!(
                    "Download batch cancelled. {saved} completed packages were kept."
                )));
            }
            let result = (|| {
                let product = self.product(id)?;
                if product.unavailable {
                    return Err(fail("The product repository is unavailable."));
                }
                let release = self
                    .selected_release(id)
                    .cloned()
                    .ok_or_else(|| fail("Check for releases first."))?;
                let asset = self.require_asset_for_release(id, &release)?.clone();
                if !safe_relative(Path::new(&asset.name))
                    || Path::new(&asset.name).components().count() != 1
                {
                    return Err(fail("The package has an unsafe filename."));
                }
                progress(Progress {
                    app_id: (*id).clone(),
                    phase: format!("Checking package {}/{}", index + 1, ids.len()),
                    percent: index as f32 * 100.0 / ids.len() as f32,
                    transferred_bytes: None,
                    total_bytes: None,
                    bytes_per_second: None,
                    seconds_remaining: None,
                });
                let checksum = self.remote.checksum_product(&product, &asset, &release)?;
                let destination = folder.join(&asset.name);
                if destination.symlink_metadata().is_ok() {
                    if destination.symlink_metadata()?.file_type().is_file()
                        && crate::self_update::hash_file(&destination)? == checksum
                    {
                        return Ok(asset.name);
                    }
                    return Err(fail(
                        "A different file already exists at the destination. Choose another folder or rename it first.",
                    ));
                }
                let file = self.partial_path(&product, &release, &asset, &checksum)?;
                let mut meter = TransferMeter::default();
                let digest = self.remote.download_product(
                    &product,
                    &asset,
                    &file,
                    cancel,
                    &mut |percent| {
                        let (transferred, speed, remaining) = meter.sample(percent, asset.size);
                        progress(Progress {
                            app_id: (*id).clone(),
                            phase: format!("Downloading {}/{}", index + 1, ids.len()),
                            percent: (index as f32 * 100.0 + percent) / ids.len() as f32,
                            transferred_bytes: Some(transferred),
                            total_bytes: Some(asset.size),
                            bytes_per_second: speed,
                            seconds_remaining: remaining,
                        })
                    },
                )?;
                if cancel.load(Ordering::Relaxed) {
                    return Err(crate::cancelled("Download cancelled."));
                }
                if digest != checksum || file.metadata()?.len() != asset.size {
                    let _ = fs::remove_file(&file);
                    return Err(fail(
                        "Checksum or byte count mismatch. The download was discarded.",
                    ));
                }
                let mut output = tempfile::NamedTempFile::new_in(&folder)?;
                std::io::copy(&mut File::open(&file)?, output.as_file_mut())?;
                output.as_file().sync_all()?;
                output.persist_noclobber(&destination).map_err(|e| {
                    fail(format!(
                        "Could not save the package without overwriting a file: {}",
                        e.error
                    ))
                })?;
                fs::remove_file(&file)?;
                Ok(asset.name)
            })();
            match result {
                Ok(filename) => {
                    saved += 1;
                    self.record(
                        Some(id),
                        "package download",
                        format!("Saved {filename} to {}.", folder.display()),
                        false,
                    );
                }
                Err(error) => {
                    self.record(Some(id), "package download", error.to_string(), true);
                    errors.push(format!("{}: {error}", self.product(id)?.name));
                }
            }
            self.save()?;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::cancelled(format!(
                "Download batch cancelled. {saved} completed packages were kept."
            )));
        }
        if errors.is_empty() {
            Ok(saved)
        } else {
            Err(fail(format!(
                "{saved} packages saved; {} failed. {}",
                errors.len(),
                errors.join(" · ")
            )))
        }
    }
    pub fn refresh_app(&mut self, id: &str) -> Result<()> {
        let result = self.refresh_app_in_memory(id);
        self.save()?;
        result
    }
    fn refresh_app_in_memory(&mut self, id: &str) -> Result<()> {
        let product = self.product(id)?;
        if product.unavailable {
            return Err(fail("The product repository is unavailable."));
        }
        let channel = self.channel(id);
        match self
            .remote
            .release_product(&product, channel, self.state.releases.get(id))
        {
            Ok(cache) => {
                self.state.releases.insert(id.into(), cache);
                Ok(())
            }
            Err(e) => {
                let message = e.to_string();
                let cache = self
                    .state
                    .releases
                    .entry(id.into())
                    .or_insert(ReleaseCache {
                        channel,
                        etag: None,
                        checked_at: String::new(),
                        release: None,
                        versions: Vec::new(),
                        error: None,
                    });
                cache.error = Some(message.clone());
                self.record(Some(id), "release check", &message, true);
                if matches!(e, crate::Error::RateLimited) {
                    Err(crate::Error::RateLimited)
                } else {
                    Err(fail(message))
                }
            }
        }
    }
    pub fn refresh_all(
        &mut self,
        cancel: &AtomicBool,
        changed: &mut dyn FnMut(Snapshot),
    ) -> Result<()> {
        let mut errors = Vec::new();
        for info in crate::catalog::products(&self.state.discovered_apps) {
            if cancel.load(Ordering::Relaxed) {
                self.save()?;
                return Err(crate::cancelled("Release check cancelled."));
            }
            if !info.unavailable {
                if let Err(error) = self.refresh_app_in_memory(&info.id) {
                    let message = error.to_string();
                    let rate_limited = matches!(error, crate::Error::RateLimited)
                        || message.contains("GitHub rate limit reached");
                    errors.push(format!("{}: {message}", info.name));
                    changed(self.snapshot());
                    if rate_limited {
                        break;
                    }
                } else {
                    changed(self.snapshot());
                }
            }
        }
        self.save()?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(fail(format!(
                "{} release checks failed: {}",
                errors.len(),
                errors.into_iter().take(5).collect::<Vec<_>>().join(" · ")
            )))
        }
    }
    pub fn configure(&mut self, settings: Settings, launcher: &Path) -> Result<()> {
        let before = self.state.clone();
        if settings.launch_at_login != before.settings.launch_at_login {
            self.platform
                .autostart(settings.launch_at_login, launcher, &self.root)?;
        }
        let channel_changed = settings.channel != before.settings.channel;
        self.state.settings = settings;
        if channel_changed {
            let global_channel = self.state.settings.channel;
            for (id, cache) in &mut self.state.releases {
                let app_channel = self
                    .state
                    .preferences
                    .get(id)
                    .and_then(|p| p.channel)
                    .unwrap_or(global_channel);
                cache.set_channel(app_channel);
            }
        }
        if let Err(e) = self.save() {
            self.state = before.clone();
            let _ = self
                .platform
                .autostart(before.settings.launch_at_login, launcher, &self.root);
            return Err(e);
        }
        Ok(())
    }
    pub fn set_preferences(&mut self, id: &str, preferences: AppPreferences) -> Result<()> {
        self.product(id)?;
        let before = self.state.clone();
        self.state.preferences.insert(id.into(), preferences);
        let channel = self.channel(id);
        if let Some(cache) = self.state.releases.get_mut(id) {
            cache.set_channel(channel);
        }
        let pending = if self.state.preferences[id].channel
            != before
                .preferences
                .get(id)
                .cloned()
                .unwrap_or_default()
                .channel
            || self.state.preferences[id].pinned_version
                != before
                    .preferences
                    .get(id)
                    .cloned()
                    .unwrap_or_default()
                    .pinned_version
            || !self.state.preferences[id].auto_update
        {
            self.state.pending.remove(id)
        } else {
            None
        };
        if let Err(e) = self.save() {
            self.state = before;
            return Err(e);
        }
        if let Some(pending) = pending {
            fs::remove_dir_all(self.managed_path(id, &pending.version.directory)?)?;
        }
        Ok(())
    }
    pub fn executable(&self, id: &str, installed: &Installation) -> Result<PathBuf> {
        if installed.managed {
            self.managed_path(id, &installed.executable)
        } else {
            Ok(installed.executable.clone())
        }
    }
    pub fn link(&mut self, id: &str, executable: PathBuf) -> Result<()> {
        let info = self.product(id)?;
        if self.state.installations.get(id).is_some_and(|i| i.managed) {
            return Err(fail(
                "Uninstall the managed app before linking an external installation.",
            ));
        }
        if !executable.is_absolute() {
            return Err(fail("Select an absolute app path."));
        }
        let executable = fs::canonicalize(executable)?;
        if !executable.is_file()
            && !(self.platform.os() == "macos"
                && executable.is_dir()
                && executable.extension().is_some_and(|x| x == "app"))
        {
            return Err(fail("Select an executable application."));
        }
        #[cfg(unix)]
        if executable.is_file() {
            use std::os::unix::fs::PermissionsExt;
            if executable.metadata()?.permissions().mode() & 0o111 == 0 {
                return Err(fail("The selected file is not executable."));
            }
        }
        let before = self.state.clone();
        self.state.installations.insert(
            id.into(),
            Installation {
                managed: false,
                version: "Local".into(),
                executable,
                directory: None,
                previous: None,
                installer: None,
                installed_at: now(),
                last_opened_at: None,
                size: 0,
            },
        );
        self.record(
            Some(id),
            "link",
            format!("{} linked to an existing installation.", info.name),
            false,
        );
        if let Err(e) = self.save() {
            self.state = before;
            return Err(e);
        }
        Ok(())
    }
    pub fn launch(&mut self, id: &str) -> Result<Option<u32>> {
        self.product(id)?;
        let install = self
            .state
            .installations
            .get(id)
            .ok_or_else(|| fail("Install or link this app first."))?;
        let executable = self.executable(id, install)?;
        let log_dir = self.root.join("logs");
        fs::create_dir_all(&log_dir)?;
        if log_dir.symlink_metadata()?.file_type().is_symlink()
            || !fs::canonicalize(&log_dir)?.starts_with(&self.root)
        {
            return Err(fail("Unsafe app log directory."));
        }
        let log = log_dir.join(format!("{id}.log"));
        let pid = match self.platform.launch_with_log(&executable, &log) {
            Ok(pid) => pid,
            Err(error) => {
                self.record(Some(id), "launch", error.to_string(), true);
                self.save()?;
                return Err(error);
            }
        };
        if let Some(install) = self.state.installations.get_mut(id) {
            install.last_opened_at = Some(now());
        }
        self.record(
            Some(id),
            "launch",
            format!("{} launched.", self.product(id)?.name),
            false,
        );
        self.save()?;
        Ok(pid)
    }
    pub fn open_project(&mut self, id: &str, path: PathBuf) -> Result<()> {
        self.product(id)?;
        let path = fs::canonicalize(path)?;
        if !path.is_file() {
            return Err(fail("Select a project file."));
        }
        let install = self
            .state
            .installations
            .get(id)
            .ok_or_else(|| fail("Install or link this app first."))?;
        let executable = self.executable(id, install)?;
        self.platform.open_project(&executable, &path)?;
        self.state
            .recent_projects
            .retain(|p| p.path != path || p.app_id != id);
        self.state.recent_projects.insert(
            0,
            crate::workspace::RecentProject {
                path,
                app_id: id.into(),
                opened_at: now(),
            },
        );
        self.state.recent_projects.truncate(30);
        if let Some(install) = self.state.installations.get_mut(id) {
            install.last_opened_at = Some(now());
        }
        self.record(Some(id), "open project", "Project opened.", false);
        self.save()
    }
    pub fn discover_products(&mut self, cancel: &AtomicBool) -> Result<()> {
        match self
            .remote
            .discover_cached(cancel, &self.state.discovery_pages)
        {
            Ok((apps, pages)) => {
                self.state.discovery_pages = pages;
                // Preserve cached candidates that disappear from GitHub. Match by stable
                // repository ID first, with a name fallback for pre-ID state files.
                let mut merged = self.state.discovered_apps.clone();
                for old in &mut merged {
                    if old.app_id.is_empty() {
                        old.app_id = old.repository.to_ascii_lowercase();
                    }
                    old.unavailable = true;
                }
                for mut incoming in apps {
                    incoming.unavailable = false;
                    let existing = merged.iter_mut().find(|old| {
                        (incoming.github_id.is_some() && incoming.github_id == old.github_id)
                            || (old.github_id.is_none()
                                && old.repository.eq_ignore_ascii_case(&incoming.repository))
                    });
                    if let Some(old) = existing {
                        incoming.app_id = old.app_id.clone();
                        *old = incoming;
                    } else {
                        merged.push(incoming);
                    }
                }
                merged.sort_by_key(|app| app.repository.to_ascii_lowercase());
                self.state.discovered_apps = merged;
                self.state.discovery_checked_at = Some(now());
                self.state.discovery_error = None;
            }
            Err(error) => {
                self.state.discovery_error = Some(error.to_string());
            }
        }
        self.save()
    }
    pub fn update_reviewed(
        &mut self,
        id: &str,
        version: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        crate::workspace::update_preview(&self.snapshot())
            .into_iter()
            .find(|p| p.app_id == id && p.next == version && !p.pinned)
            .ok_or_else(|| fail("This update is no longer eligible. Review updates again."))?;
        self.stage_install(id, None, cancel, progress)
    }
    pub fn check_installation(&self, id: &str) -> Result<String> {
        let install = self
            .state
            .installations
            .get(id)
            .ok_or_else(|| fail("This app is not installed."))?;
        let executable = self.executable(id, install)?;
        if !executable.exists() {
            return Err(fail("The app executable is missing."));
        }
        if !install.managed {
            return Ok("Linked app exists; its files are externally managed.".into());
        }
        let directory = install
            .directory
            .as_ref()
            .ok_or_else(|| fail("The app installation directory is missing."))?;
        let path = self.managed_path(id, directory)?;
        if self
            .platform
            .check_shortcut_product(&self.product(id)?, &executable)?
            == Some(false)
        {
            return Err(fail(
                "The application shortcut is missing or changed. Repair the installation.",
            ));
        }
        match crate::inventory::verify(&path)? {
            None => Ok("App exists; this older installation has no file inventory. Reinstall to enable full verification.".into()),
            Some(files) if files.is_empty() => Ok("Managed application files verified.".into()),
            Some(files) => Err(fail(format!("{} application files are missing or changed: {}", files.len(), files.iter().take(5).map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")))),
        }
    }
    pub fn repair(
        &mut self,
        id: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        let install = self
            .state
            .installations
            .get(id)
            .ok_or_else(|| fail("This app is not installed."))?;
        if !install.managed {
            return Err(fail(
                "This app is externally managed. Relink its executable instead.",
            ));
        }
        let version = install.version.clone();
        let executable = self.executable(id, install)?;
        if executable.exists()
            && let Some(directory) = &install.directory
        {
            let path = self.managed_path(id, directory)?;
            if matches!(crate::inventory::verify(&path)?, Some(files) if files.is_empty())
                && self
                    .platform
                    .check_shortcut_product(&self.product(id)?, &executable)?
                    == Some(false)
            {
                self.platform
                    .shortcut_product(&self.product(id)?, &executable)?;
                self.record(Some(id), "repair", "Application shortcut restored.", false);
                self.save()?;
                return Ok(());
            }
        }
        self.refresh_app(id)?;
        if !self.state.releases.get(id).is_some_and(|cache| {
            cache.versions.iter().any(|r| r.tag_name == version)
                || cache
                    .release
                    .as_ref()
                    .is_some_and(|r| r.tag_name == version)
        }) {
            return Err(fail(
                "The installed version is no longer available upstream. Choose a newer version explicitly.",
            ));
        }
        self.stage_install_for(id, Some(&version), cancel, progress, false, false)
    }
    pub fn stage_queued_install(
        &mut self,
        id: &str,
        version: &str,
        pin_requested: bool,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        self.stage_install_for(id, Some(version), cancel, progress, false, pin_requested)
    }
    pub fn stage_install(
        &mut self,
        id: &str,
        requested_version: Option<&str>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        self.stage_install_for(id, requested_version, cancel, progress, false, true)
    }
    fn stage_install_for(
        &mut self,
        id: &str,
        requested_version: Option<&str>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
        automatic: bool,
        pin_requested: bool,
    ) -> Result<()> {
        let info = self.product(id)?;
        if info.unavailable {
            return Err(fail("The product repository is unavailable."));
        }
        if self.state.installations.get(id).is_some_and(|i| !i.managed) {
            return Err(fail(
                "This app is externally managed. Unlink it before installing a managed version.",
            ));
        }
        if self.state.pending.contains_key(id) {
            return Err(fail(
                "An update is already staged. Activate or discard it first.",
            ));
        }
        let cache = self
            .state
            .releases
            .get(id)
            .filter(|c| c.channel == self.channel(id) || !c.versions.is_empty())
            .ok_or_else(|| fail("Check for releases first."))?;
        let release = if let Some(version) = requested_version {
            cache
                .versions
                .iter()
                .find(|r| r.tag_name == version)
                .or_else(|| cache.release.as_ref().filter(|r| r.tag_name == version))
        } else {
            self.selected_release(id)
        }
        .cloned()
        .ok_or_else(|| fail("The selected release is not available."))?;
        if release.draft || (release.prerelease && self.channel(id) == Channel::Stable) {
            return Err(fail(
                "Switch to the Preview channel to install a prerelease.",
            ));
        }
        let asset = self.require_asset_for_release(id, &release)?.clone();
        let kind = package_kind(&asset)?;
        if !crate::platform::supports_managed(id, self.platform.os(), &asset) {
            return Err(fail(
                "This package format is not supported for managed installation. Use the official release page.",
            ));
        }
        let token = uuid::Uuid::new_v4().to_string();
        let base = self.root.join("apps").join(id);
        fs::create_dir_all(&base)?;
        if !fs::canonicalize(&base)?.starts_with(&self.root) {
            return Err(fail("Unsafe installation directory."));
        }
        let stage = base.join(format!(".staging-{token}"));
        fs::create_dir(&stage)?;
        let destination = base.join("versions").join(&token);
        let result = (|| {
            progress(Progress {
                app_id: id.into(),
                phase: "Checking checksum".into(),
                percent: 0.0,
                transferred_bytes: None,
                total_bytes: None,
                bytes_per_second: None,
                seconds_remaining: None,
            });
            let checksum = self.remote.checksum_product(&info, &asset, &release)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::cancelled("Download cancelled."));
            }
            let archive = self.partial_path(&info, &release, &asset, &checksum)?;
            let mut meter = TransferMeter::default();
            let digest =
                self.remote
                    .download_product(&info, &asset, &archive, cancel, &mut |percent| {
                        let (transferred, speed, remaining) = meter.sample(percent, asset.size);
                        progress(Progress {
                            app_id: id.into(),
                            phase: "Downloading".into(),
                            percent,
                            transferred_bytes: Some(transferred),
                            total_bytes: Some(asset.size),
                            bytes_per_second: speed,
                            seconds_remaining: remaining,
                        })
                    })?;
            if digest != checksum || archive.metadata()?.len() != asset.size {
                let _ = fs::remove_file(&archive);
                return Err(fail("Checksum mismatch. The download was discarded."));
            }
            progress(Progress {
                app_id: id.into(),
                phase: "Extracting".into(),
                percent: 100.0,
                transferred_bytes: None,
                total_bytes: None,
                bytes_per_second: None,
                seconds_remaining: None,
            });
            let preferences = self.preferences(id);
            let key = asset_preference_key(self.platform.os(), self.platform.arch());
            let entrypoint = preferences
                .selected_entrypoints
                .get(&key)
                .map(String::as_str);
            let (executable, installer) = self.platform.unpack_product_with_entrypoint(
                &info, kind, &archive, &stage, cancel, entrypoint,
            )?;
            fs::remove_file(archive)?;
            crate::inventory::create(&stage)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::cancelled("Download cancelled."));
            }
            fs::create_dir_all(
                destination
                    .parent()
                    .ok_or_else(|| fail("Invalid version directory."))?,
            )?;
            fs::rename(&stage, &destination)?;
            let directory = PathBuf::from("versions").join(&token);
            let version = Version {
                version: release.tag_name.clone(),
                executable: directory.join(executable),
                installer: installer.map(|p| directory.join(p)),
                directory,
                size: asset.size,
            };
            self.validate_version(id, &version)?;
            let before = self.state.clone();
            if pin_requested && requested_version.is_some() {
                self.state
                    .preferences
                    .entry(id.into())
                    .or_default()
                    .pinned_version = Some(release.tag_name.clone());
            }
            self.state.pending.insert(
                id.into(),
                PendingUpdate {
                    version,
                    staged_at: now(),
                    automatic,
                },
            );
            if let Err(e) = self.save() {
                self.state = before;
                return Err(e);
            }
            self.record(
                Some(id),
                "download",
                format!(
                    "{} {} verified and ready to activate.",
                    info.name, release.tag_name
                ),
                false,
            );
            self.save()?;
            Ok(())
        })();
        let _ = fs::remove_dir_all(&stage);
        if let Err(e) = &result {
            if !self.state.pending.contains_key(id) {
                let _ = fs::remove_dir_all(&destination);
            }
            self.record(
                Some(id),
                "install",
                e.to_string(),
                !cancel.load(Ordering::Relaxed),
            );
            let _ = self.save();
        }
        result?;
        self.activate_pending(id)?;
        Ok(())
    }
    pub fn activate_pending(&mut self, id: &str) -> Result<bool> {
        self.product(id)?;
        let Some(pending) = self.state.pending.get(id).cloned() else {
            return Ok(false);
        };
        if pending.automatic && !self.state.settings.auto_app_update {
            return Ok(false);
        }
        if let Some(installed) = self.state.installations.get(id)
            && self.platform.is_running(&self.executable(id, installed)?)?
        {
            return Ok(false);
        }
        self.validate_version(id, &pending.version)?;
        let executable = self.managed_path(id, &pending.version.executable)?;
        if let Some(installer) = &pending.version.installer {
            self.platform
                .finish_installer(&self.managed_path(id, installer)?, &executable)?;
        }
        if !executable.exists() {
            return Err(fail("The staged app executable is missing."));
        }
        if pending.version.installer.is_some() {
            crate::inventory::create(&self.managed_path(id, &pending.version.directory)?)?;
        }
        let before = self.state.clone();
        let old = self.state.installations.get(id).cloned();
        let previous = old.as_ref().filter(|i| i.managed).and_then(|i| {
            i.directory.as_ref().map(|directory| Version {
                version: i.version.clone(),
                executable: i.executable.clone(),
                directory: directory.clone(),
                size: i.size,
                installer: i.installer.clone(),
            })
        });
        self.state.installations.insert(
            id.into(),
            Installation {
                managed: true,
                version: pending.version.version.clone(),
                executable: pending.version.executable.clone(),
                directory: Some(pending.version.directory.clone()),
                previous,
                installer: pending.version.installer.clone(),
                installed_at: now(),
                last_opened_at: old.as_ref().and_then(|i| i.last_opened_at.clone()),
                size: pending.version.size,
            },
        );
        self.state.pending.remove(id);
        self.record(
            Some(id),
            if old.is_some() { "update" } else { "install" },
            format!(
                "{} {} {}.",
                self.product(id)?.name,
                pending.version.version,
                if old.is_some() {
                    "updated"
                } else {
                    "installed"
                }
            ),
            false,
        );
        if let Err(e) = self.save() {
            self.state = before;
            return Err(e);
        }
        if let Err(e) = self
            .platform
            .shortcut_product(&self.product(id)?, &executable)
        {
            self.record(Some(id), "shortcut", e.to_string(), true);
            self.save()?;
            return Err(fail(format!(
                "The app is installed, but desktop integration failed: {e}"
            )));
        }
        if let Some(previous) = old.and_then(|i| i.previous)
            && let Ok(path) = self.managed_path(id, &previous.directory)
        {
            let _ = fs::remove_dir_all(path);
        }
        if self.state.settings.notifications {
            crate::platform::notify(&format!(
                "{} {} is ready.",
                self.product(id)?.name,
                pending.version.version
            ));
        }
        Ok(true)
    }
    pub fn activate_all_pending(&mut self) -> Result<()> {
        let ids: Vec<_> = self.state.pending.keys().cloned().collect();
        for id in ids {
            if let Err(e) = self.activate_pending(&id) {
                self.record(Some(&id), "activation", e.to_string(), true);
            }
        }
        self.save()
    }
    pub fn discard_pending(&mut self, id: &str) -> Result<()> {
        self.product(id)?;
        if let Some(pending) = self.state.pending.remove(id) {
            if let Err(e) = self.save() {
                self.state.pending.insert(id.into(), pending);
                return Err(e);
            }
            fs::remove_dir_all(self.managed_path(id, &pending.version.directory)?)?;
        }
        Ok(())
    }
    pub fn rollback(&mut self, id: &str) -> Result<()> {
        let installed = self
            .state
            .installations
            .get(id)
            .cloned()
            .ok_or_else(|| fail("This app is not installed."))?;
        if self
            .platform
            .is_running(&self.executable(id, &installed)?)?
        {
            return Err(fail("Close the app before switching versions."));
        }
        let previous = installed
            .previous
            .clone()
            .ok_or_else(|| fail("No previous version is available."))?;
        self.validate_version(id, &previous)?;
        let executable = self.managed_path(id, &previous.executable)?;
        if let Some(installer) = &previous.installer {
            self.platform
                .finish_installer(&self.managed_path(id, installer)?, &executable)?;
        }
        if !executable.exists() {
            return Err(fail("The previous version is missing."));
        }
        if previous.installer.is_some() {
            crate::inventory::create(&self.managed_path(id, &previous.directory)?)?;
        }
        let before = self.state.clone();
        let directory = installed
            .directory
            .clone()
            .ok_or_else(|| fail("This app is externally managed."))?;
        self.state.installations.insert(
            id.into(),
            Installation {
                version: previous.version.clone(),
                executable: previous.executable,
                directory: Some(previous.directory),
                size: previous.size,
                installer: previous.installer,
                previous: Some(Version {
                    version: installed.version,
                    executable: installed.executable,
                    directory,
                    size: installed.size,
                    installer: installed.installer,
                }),
                ..installed
            },
        );
        // A rollback should stay rolled back until the user unpins it.
        self.state
            .preferences
            .entry(id.into())
            .or_default()
            .pinned_version = Some(previous.version.clone());
        self.state.pending.remove(id);
        self.record(
            Some(id),
            "rollback",
            format!(
                "{} switched to {} and pinned.",
                self.product(id)?.name,
                previous.version
            ),
            false,
        );
        if let Err(e) = self.save() {
            self.state = before;
            return Err(e);
        }
        self.platform
            .shortcut_product(&self.product(id)?, &executable)?;
        Ok(())
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let Some(installed) = self.state.installations.get(id).cloned() else {
            return self.discard_pending(id);
        };
        if installed.managed
            && self
                .platform
                .is_running(&self.executable(id, &installed)?)?
        {
            return Err(fail("Close the app before uninstalling it."));
        }
        let executable = self.executable(id, &installed)?;
        let before = self.state.clone();
        let base = self.root.join("apps").join(id);
        let trash = self
            .root
            .join(format!("removed-{id}-{}", uuid::Uuid::new_v4()));
        let moved = installed.managed && base.exists();
        if moved {
            if !fs::canonicalize(&base)?.starts_with(&self.root) {
                return Err(fail("Unsafe installation directory."));
            }
            fs::rename(&base, &trash)?;
        }
        self.state.installations.remove(id);
        self.state.pending.remove(id);
        self.record(
            Some(id),
            "remove",
            format!(
                "{} {}. Project files were kept.",
                self.product(id)?.name,
                if installed.managed {
                    "uninstalled"
                } else {
                    "unlinked"
                }
            ),
            false,
        );
        if let Err(e) = self.save() {
            self.state = before;
            if moved {
                let _ = fs::rename(&trash, &base);
            }
            return Err(e);
        }
        if moved {
            fs::remove_dir_all(trash)?;
            self.platform
                .remove_shortcut_product(&self.product(id)?, &executable)?;
        }
        Ok(())
    }
    pub fn auto_updates(
        &mut self,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
        changed: &mut dyn FnMut(Snapshot),
    ) -> Result<()> {
        if !self.state.settings.auto_app_update {
            return Ok(());
        }
        for product in crate::catalog::products(&self.state.discovered_apps) {
            let info = product.id.as_str();
            if product.unavailable {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(crate::cancelled("Update check cancelled."));
            }
            let preference = self.preferences(info);
            if !preference.auto_update
                || preference.pinned_version.is_some()
                || self.state.pending.contains_key(info)
            {
                continue;
            }
            let should_update = self.state.installations.get(info).is_some_and(|installed| {
                installed.managed
                    && self
                        .selected_release(info)
                        .is_some_and(|release| is_newer(&release.tag_name, &installed.version))
            });
            if should_update && self.compatible(info) {
                if let Err(e) = self.stage_install_for(info, None, cancel, progress, true, false) {
                    self.record(Some(info), "auto update", e.to_string(), true);
                }
                changed(self.snapshot());
            }
        }
        self.activate_all_pending()
    }
    pub fn storage_report(&self) -> Result<crate::storage::StorageReport> {
        let mut report = crate::storage::StorageReport {
            launcher_bytes: crate::storage::directory_size(&self.root.join("launcher/versions"))?,
            ..Default::default()
        };
        for (id, install) in &self.state.installations {
            if !install.managed {
                continue;
            }
            let measure = |path: &Path| -> Result<u64> {
                crate::storage::directory_size(&self.managed_path(id, path)?)
            };
            let active_bytes = install
                .directory
                .as_deref()
                .map(measure)
                .transpose()?
                .unwrap_or(0);
            let rollback_bytes = install
                .previous
                .as_ref()
                .map(|v| measure(&v.directory))
                .transpose()?
                .unwrap_or(0);
            let pending_bytes = self
                .state
                .pending
                .get(id)
                .map(|v| measure(&v.version.directory))
                .transpose()?
                .unwrap_or(0);
            report.apps.push(crate::storage::AppStorage {
                id: id.clone(),
                active_bytes,
                rollback_bytes,
                pending_bytes,
                rollback_version: install.previous.as_ref().map(|v| v.version.clone()),
            });
        }
        // An installation can be staged before its first activation.
        for (id, pending) in &self.state.pending {
            if !self.state.installations.contains_key(id) {
                report.apps.push(crate::storage::AppStorage {
                    id: id.clone(),
                    active_bytes: 0,
                    rollback_bytes: 0,
                    pending_bytes: crate::storage::directory_size(
                        &self.managed_path(id, &pending.version.directory)?,
                    )?,
                    rollback_version: None,
                });
            }
        }
        let queue = crate::queue::load(&self.root)?;
        report.downloads = crate::storage::downloads(&self.root)?;
        for item in &mut report.downloads {
            item.protected = item
                .name
                .split_once('/')
                .is_some_and(|(owner, _)| queue.iter().any(|task| task.id == owner));
        }
        Ok(report)
    }
    pub fn remove_cached_downloads(&self, names: &[String]) -> Result<usize> {
        let report = self.storage_report()?;
        let cache = self.root.join("downloads");
        if cache
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(fail("Unsafe download cache directory."));
        }
        let mut targets = Vec::new();
        for name in names {
            let item = report
                .downloads
                .iter()
                .find(|item| item.name == *name)
                .ok_or_else(|| fail("A cache entry changed; scan storage again."))?;
            if item.protected {
                return Err(fail("A selected download is referenced by the queue."));
            }
            let parts: Vec<&str> = name.split('/').collect();
            let valid = match parts.as_slice() {
                [file] => crate::storage::is_cache_name(file),
                [owner, file] => {
                    crate::queue::valid_id(owner) && crate::storage::is_cache_name(file)
                }
                _ => false,
            };
            if !valid {
                return Err(fail("Invalid cache entry."));
            }
            let path = cache.join(name);
            if let Some(parent) = path.parent()
                && parent
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(fail("Unsafe cache path."));
            }
            if !path.symlink_metadata()?.file_type().is_file() {
                return Err(fail("Unsafe cache file."));
            }
            targets.push(path);
        }
        targets.sort();
        targets.dedup();
        for path in &targets {
            fs::remove_file(path)?;
        }
        for path in &targets {
            if let Some(parent) = path.parent()
                && parent != cache
            {
                let _ = fs::remove_dir(parent);
            }
        }
        Ok(targets.len())
    }
    pub fn remove_rollback(&mut self, id: &str, expected_version: &str) -> Result<()> {
        let install = self
            .state
            .installations
            .get(id)
            .ok_or_else(|| fail("This app is not installed."))?;
        let previous = install
            .previous
            .clone()
            .ok_or_else(|| fail("No rollback version is available."))?;
        if previous.version != expected_version {
            return Err(fail("The rollback version changed; review storage again."));
        }
        self.validate_version(id, &previous)?;
        if install.directory.as_ref() == Some(&previous.directory)
            || self
                .state
                .pending
                .get(id)
                .is_some_and(|p| p.version.directory == previous.directory)
        {
            return Err(fail("The version is still in use."));
        }
        let path = self.managed_path(id, &previous.directory)?;
        let base = self.root.join("apps").join(id);
        let mut checked = self.root.clone();
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| fail("Unsafe rollback path."))?;
        for component in relative.components() {
            checked.push(component);
            if checked
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(fail("Rollback path contains a symbolic link."));
            }
        }
        if !path.starts_with(base.join("versions")) {
            return Err(fail("Unsafe rollback path."));
        }
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(fail("Rollback directory is a symbolic link."));
        }
        let before = self.state.clone();
        self.state.installations.get_mut(id).unwrap().previous = None;
        if let Err(e) = self.save() {
            self.state = before;
            return Err(e);
        }
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        Ok(())
    }
    pub fn running_apps(&self) -> BTreeMap<String, bool> {
        self.snapshot().running
    }
    pub fn reveal(&self, id: Option<&str>) -> Result<()> {
        let path = if let Some(id) = id {
            let install = self
                .state
                .installations
                .get(id)
                .ok_or_else(|| fail("The app is not installed."))?;
            let executable = self.executable(id, install)?;
            executable.parent().unwrap_or(&executable).to_path_buf()
        } else {
            self.root.clone()
        };
        open::that(path).map_err(|e| fail(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod transfer_meter_tests {
    use super::TransferMeter;
    use std::time::{Duration, Instant};
    #[test]
    fn rate_ignores_bytes_received_before_resume_and_smooths_later_samples() {
        let mut meter = TransferMeter::default();
        let start = Instant::now();
        assert_eq!(
            meter.sample_at(50.0, 1_000_000, start),
            (500_000, None, None)
        );
        let (bytes, speed, eta) = meter.sample_at(60.0, 1_000_000, start + Duration::from_secs(1));
        assert_eq!(bytes, 600_000);
        assert_eq!(speed, Some(100_000.0));
        assert_eq!(eta, Some(4));
        let (_, speed, _) = meter.sample_at(80.0, 1_000_000, start + Duration::from_secs(2));
        assert_eq!(speed, Some(130_000.0));
        assert_eq!(
            meter
                .sample_at(0.0, 1_000_000, start + Duration::from_secs(3))
                .1,
            None
        );
    }
}
