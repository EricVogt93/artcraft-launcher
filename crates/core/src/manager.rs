use crate::{
    AppPreferences, CATALOG, Channel, Installation, PendingUpdate, Progress, Release, ReleaseCache,
    Result, Settings, Snapshot, State, Version, fail,
    installer::{safe_native_relative, safe_relative},
    model::{STATE_SCHEMA, is_newer, now},
    persistence::atomic_json,
    platform::{Native, Platform, package_kind},
    releases::{Github, Remote, asset_for},
};
use fs2::FileExt;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub struct Manager {
    pub root: PathBuf,
    pub state: State,
    remote: Box<dyn Remote>,
    platform: Box<dyn Platform>,
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
        lock.try_lock_exclusive().map_err(|_| {
            fail("CraftLauncher is already using this library. Close the other instance first.")
        })?;
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
            _lock: lock,
        };
        manager.validate_state()?;
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
            crate::app(id)?;
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
        crate::app(id)?;
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
        for info in CATALOG {
            let base = self.root.join("apps").join(info.id);
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
        Ok(())
    }
    pub fn check_launcher_update(&mut self, force: bool, cancel: &AtomicBool) -> Result<()> {
        if self.state.launcher_update.is_some()
            || (!force && !self.state.settings.auto_launcher_update)
        {
            return Ok(());
        }
        let settings = &self.state.settings;
        let (Some(feed), Some(key)) = (&settings.update_feed, &settings.update_public_key) else {
            return Ok(());
        };
        let current = crate::self_update::build_version();
        let threshold = self
            .state
            .failed_launcher_version
            .as_deref()
            .filter(|v| !force && is_newer(v, current))
            .unwrap_or(current);
        match crate::self_update::prepare(
            &self.root,
            feed,
            key,
            settings.channel,
            threshold,
            cancel,
        ) {
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
    pub fn snapshot(&self) -> Snapshot {
        let running = self
            .state
            .installations
            .iter()
            .map(|(id, install)| {
                let running = self
                    .executable(id, install)
                    .and_then(|p| self.platform.is_running(&p))
                    .unwrap_or(true);
                (id.clone(), running)
            })
            .collect();
        Snapshot {
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
    pub fn preferences(&self, id: &str) -> AppPreferences {
        self.state.preferences.get(id).cloned().unwrap_or_default()
    }
    pub fn channel(&self, id: &str) -> Channel {
        self.preferences(id)
            .channel
            .unwrap_or(self.state.settings.channel)
    }
    pub fn selected_release(&self, id: &str) -> Option<&Release> {
        let cache = self
            .state
            .releases
            .get(id)
            .filter(|c| c.channel == self.channel(id))?;
        if let Some(pin) = self.preferences(id).pinned_version {
            cache
                .versions
                .iter()
                .find(|r| r.tag_name == pin)
                .or_else(|| cache.release.as_ref().filter(|r| r.tag_name == pin))
        } else {
            cache.release.as_ref()
        }
    }
    pub fn compatible(&self, id: &str) -> bool {
        self.selected_release(id)
            .and_then(|r| asset_for(r, self.platform.os(), self.platform.arch()))
            .is_some()
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
            crate::app(id)?;
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
                return Err(fail(format!(
                    "Download batch cancelled. {saved} completed packages were kept."
                )));
            }
            let result = (|| {
                let release = self
                    .selected_release(id)
                    .cloned()
                    .ok_or_else(|| fail("Check for releases first."))?;
                let asset = asset_for(&release, self.platform.os(), self.platform.arch())
                    .cloned()
                    .ok_or_else(|| {
                        fail("No compatible published package is available for this platform.")
                    })?;
                if !safe_relative(Path::new(&asset.name))
                    || Path::new(&asset.name).components().count() != 1
                {
                    return Err(fail("The package has an unsafe filename."));
                }
                progress(Progress {
                    app_id: (*id).clone(),
                    phase: format!("Checking package {}/{}", index + 1, ids.len()),
                    percent: index as f32 * 100.0 / ids.len() as f32,
                });
                let checksum = self.remote.checksum(id, &asset, &release)?;
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
                let stage = tempfile::Builder::new()
                    .prefix(".craftlauncher-download-")
                    .tempdir_in(&folder)?;
                let file = stage.path().join("payload.part");
                let digest = self
                    .remote
                    .download(id, &asset, &file, cancel, &mut |percent| {
                        progress(Progress {
                            app_id: (*id).clone(),
                            phase: format!("Downloading {}/{}", index + 1, ids.len()),
                            percent: (index as f32 * 100.0 + percent) / ids.len() as f32,
                        })
                    })?;
                if cancel.load(Ordering::Relaxed) {
                    return Err(fail("Download cancelled."));
                }
                if digest != checksum || file.metadata()?.len() != asset.size {
                    return Err(fail(
                        "Checksum or byte count mismatch. The download was discarded.",
                    ));
                }
                let mut output = tempfile::NamedTempFile::new_in(&folder)?;
                std::io::copy(&mut File::open(file)?, output.as_file_mut())?;
                output.as_file().sync_all()?;
                output.persist_noclobber(&destination).map_err(|e| {
                    fail(format!(
                        "Could not save the package without overwriting a file: {}",
                        e.error
                    ))
                })?;
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
                    errors.push(format!("{}: {error}", crate::app(id)?.name));
                }
            }
            self.save()?;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(fail(format!(
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
        crate::app(id)?;
        let channel = self.channel(id);
        match self
            .remote
            .release(id, channel, self.state.releases.get(id))
        {
            Ok(cache) => {
                self.state.releases.insert(id.into(), cache);
                self.save()?;
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
                self.save()?;
                Err(fail(message))
            }
        }
    }
    pub fn refresh_all(
        &mut self,
        cancel: &AtomicBool,
        changed: &mut dyn FnMut(Snapshot),
    ) -> Result<()> {
        for info in CATALOG {
            if cancel.load(Ordering::Relaxed) {
                return Err(fail("Release check cancelled."));
            }
            let _ = self.refresh_app(info.id);
            changed(self.snapshot());
        }
        self.save()
    }
    pub fn configure(&mut self, settings: Settings, launcher: &Path) -> Result<()> {
        let before = self.state.settings.clone();
        if settings.launch_at_login != before.launch_at_login {
            self.platform
                .autostart(settings.launch_at_login, launcher, &self.root)?;
        }
        self.state.settings = settings;
        if let Err(e) = self.save() {
            self.state.settings = before.clone();
            let _ = self
                .platform
                .autostart(before.launch_at_login, launcher, &self.root);
            return Err(e);
        }
        Ok(())
    }
    pub fn set_preferences(&mut self, id: &str, preferences: AppPreferences) -> Result<()> {
        crate::app(id)?;
        let before = self.state.clone();
        self.state.preferences.insert(id.into(), preferences);
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
        let info = crate::app(id)?;
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
        crate::app(id)?;
        let install = self
            .state
            .installations
            .get(id)
            .ok_or_else(|| fail("Install or link this app first."))?;
        let executable = self.executable(id, install)?;
        let pid = self.platform.launch(&executable)?;
        if let Some(install) = self.state.installations.get_mut(id) {
            install.last_opened_at = Some(now());
        }
        self.record(
            Some(id),
            "launch",
            format!("{} launched.", crate::app(id)?.name),
            false,
        );
        self.save()?;
        Ok(pid)
    }
    pub fn stage_install(
        &mut self,
        id: &str,
        requested_version: Option<&str>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        self.stage_install_for(id, requested_version, cancel, progress, false)
    }
    fn stage_install_for(
        &mut self,
        id: &str,
        requested_version: Option<&str>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
        automatic: bool,
    ) -> Result<()> {
        let info = crate::app(id)?;
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
            .filter(|c| c.channel == self.channel(id))
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
        let asset = asset_for(&release, self.platform.os(), self.platform.arch())
            .cloned()
            .ok_or_else(|| {
                fail("No compatible published package is available for this platform.")
            })?;
        let kind = package_kind(&asset)?;
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
            });
            let checksum = self.remote.checksum(id, &asset, &release)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(fail("Download cancelled."));
            }
            let archive = stage.join("download");
            let digest = self
                .remote
                .download(id, &asset, &archive, cancel, &mut |percent| {
                    progress(Progress {
                        app_id: id.into(),
                        phase: "Downloading".into(),
                        percent,
                    })
                })?;
            if digest != checksum {
                return Err(fail("Checksum mismatch. The download was discarded."));
            }
            progress(Progress {
                app_id: id.into(),
                phase: "Extracting".into(),
                percent: 100.0,
            });
            let (executable, installer) =
                self.platform.unpack(id, kind, &archive, &stage, cancel)?;
            fs::remove_file(archive)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(fail("Download cancelled."));
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
            if requested_version.is_some() {
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
        crate::app(id)?;
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
                crate::app(id)?.name,
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
        if let Err(e) = self.platform.shortcut(id, &executable) {
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
                crate::app(id)?.name,
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
        crate::app(id)?;
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
                crate::app(id)?.name,
                previous.version
            ),
            false,
        );
        if let Err(e) = self.save() {
            self.state = before;
            return Err(e);
        }
        self.platform.shortcut(id, &executable)?;
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
                crate::app(id)?.name,
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
            self.platform.remove_shortcut_for(id, &executable)?;
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
        for info in CATALOG {
            if cancel.load(Ordering::Relaxed) {
                return Err(fail("Update check cancelled."));
            }
            let preference = self.preferences(info.id);
            if !preference.auto_update
                || preference.pinned_version.is_some()
                || self.state.pending.contains_key(info.id)
            {
                continue;
            }
            let should_update = self
                .state
                .installations
                .get(info.id)
                .is_some_and(|installed| {
                    installed.managed
                        && self
                            .selected_release(info.id)
                            .is_some_and(|release| is_newer(&release.tag_name, &installed.version))
                });
            if should_update && self.compatible(info.id) {
                if let Err(e) = self.stage_install_for(info.id, None, cancel, progress, true) {
                    self.record(Some(info.id), "auto update", e.to_string(), true);
                }
                changed(self.snapshot());
            }
        }
        self.activate_all_pending()
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
