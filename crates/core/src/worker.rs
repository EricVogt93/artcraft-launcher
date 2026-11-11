use crate::{AppPreferences, Manager, Progress, Result, Settings, Snapshot, fail};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub enum Command {
    Refresh,
    Queued {
        id: usize,
        task_key: String,
        checkpoint: Option<crate::queue::QueueCheckpoint>,
        command: Box<Command>,
    },
    Install {
        id: String,
        version: Option<String>,
    },
    UpdateReviewed {
        id: String,
        version: String,
    },
    Repair(String),
    CheckInstallation(String),
    ScanStorage,
    RemoveCachedDownloads(Vec<String>),
    RemoveRollback {
        id: String,
        version: String,
    },
    DownloadSelected {
        ids: Vec<String>,
        folder: PathBuf,
    },
    Launch(String),
    OpenProject {
        id: String,
        path: PathBuf,
    },
    Discover,
    ScanLocal,
    ClearRecent,
    Link(String, PathBuf),
    Remove(String),
    Rollback(String),
    Discard(String),
    Configure(Settings),
    Preferences(String, AppPreferences),
    Reveal(Option<String>),
    CheckLauncher,
    ActivatePending,
    Shutdown,
}
impl Command {
    pub fn is_fast(&self) -> bool {
        matches!(
            self,
            Self::Launch(_)
                | Self::OpenProject { .. }
                | Self::Reveal(_)
                | Self::ClearRecent
                | Self::Discard(_)
                | Self::Configure(_)
                | Self::Preferences(_, _)
        )
    }
}
#[derive(Debug)]
pub enum Event {
    State(Box<Snapshot>),
    Progress(Progress),
    Finished(std::result::Result<String, String>),
    LocalApps(Vec<crate::workspace::LocalApp>),
    Storage(crate::storage::StorageReport),
    Entrypoints {
        app_id: String,
        paths: Vec<String>,
    },
    TaskFinished {
        id: usize,
        result: std::result::Result<String, String>,
    },
}
struct EventSender {
    send: Sender<Event>,
    waker: Arc<dyn Fn() + Send + Sync>,
}
impl EventSender {
    fn send(&self, event: Event) {
        if self.send.send(event).is_ok() {
            (self.waker)();
        }
    }
}
pub struct Worker {
    pub events: Receiver<Event>,
    commands: Sender<Command>,
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn new(manager: Manager, launcher: PathBuf) -> Self {
        Self::with_waker(manager, launcher, Arc::new(|| {}))
    }
    pub fn with_waker(
        manager: Manager,
        launcher: PathBuf,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (commands, received) = mpsc::channel();
        let (send, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_busy = busy.clone();
        let sender = EventSender { send, waker };
        let thread = thread::spawn(move || {
            run(
                manager,
                launcher,
                received,
                sender,
                worker_cancel,
                worker_busy,
            )
        });
        Self {
            events,
            commands,
            cancel,
            busy,
            thread: Some(thread),
        }
    }
    pub fn send(&self, command: Command) -> Result<()> {
        if command.is_fast() {
            if self.commands.send(command).is_err() {
                return Err(fail("The background service has stopped."));
            }
            return Ok(());
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return Err(fail("Wait for the current operation to finish."));
        }
        self.cancel.store(false, Ordering::Relaxed);
        if self.commands.send(command).is_err() {
            self.busy.store(false, Ordering::Release);
            return Err(fail("The background service has stopped."));
        }
        Ok(())
    }
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn run(
    mut manager: Manager,
    launcher: PathBuf,
    commands: Receiver<Command>,
    send: EventSender,
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
) {
    let mut next_check = if manager.state.settings.check_on_startup {
        Instant::now()
    } else {
        Instant::now() + Duration::from_secs(6 * 3600)
    };
    let mut next_activation = Instant::now() + Duration::from_secs(15);
    send.send(Event::State(Box::new(manager.snapshot())));
    let local = crate::workspace::discover_local(
        &crate::workspace::local_search_roots(),
        &manager.snapshot(),
    );
    send.send(Event::LocalApps(local));
    loop {
        let command = match commands.recv_timeout(Duration::from_secs(1)) {
            Ok(Command::Shutdown) => break,
            Ok(command) => Some(command),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => None,
        };
        if let Some(command) = command {
            let fast = command.is_fast();
            let (command, task_id, task_key, checkpoint) = match command {
                Command::Queued {
                    id,
                    task_key,
                    checkpoint,
                    command,
                } => (*command, Some(id), Some(task_key), checkpoint),
                command => (command, None, None, None),
            };
            let refresh = matches!(command, Command::Refresh);
            let app_for_choice = match &command {
                Command::Install { id, .. }
                | Command::UpdateReviewed { id, .. }
                | Command::Repair(id) => Some(id.clone()),
                _ => None,
            };
            let result = if task_key
                .as_deref()
                .is_some_and(|id| !crate::queue::valid_id(id))
            {
                Err(fail("Invalid queue task identifier."))
            } else {
                manager.queue_owner = task_key;
                let result = match recover_queued(&mut manager, &command, checkpoint.as_ref()) {
                    Some(result) => result,
                    None => execute(
                        &mut manager,
                        command,
                        &launcher,
                        &cancel,
                        &send,
                        checkpoint.as_ref(),
                    ),
                };
                manager.queue_owner = None;
                result
            };
            send.send(Event::State(Box::new(manager.snapshot())));
            if let (Err(crate::Error::Entrypoints(paths)), Some(app_id)) = (&result, app_for_choice)
            {
                send.send(Event::Entrypoints {
                    app_id,
                    paths: paths.clone(),
                });
            }
            let result = result.map_err(|e| e.to_string());
            let event = match task_id {
                Some(id) => Event::TaskFinished { id, result },
                None => Event::Finished(result),
            };
            send.send(event);
            if !fast {
                busy.store(false, Ordering::Release);
            }
            // An explicit refresh satisfies the periodic check too.
            if refresh {
                next_check = Instant::now() + Duration::from_secs(6 * 3600);
            }
        } else if Instant::now() >= next_check && !busy.swap(true, Ordering::AcqRel) {
            cancel.store(false, Ordering::Relaxed);
            let result = execute(
                &mut manager,
                Command::Refresh,
                &launcher,
                &cancel,
                &send,
                None,
            );
            if let Err(e) = result {
                send.send(Event::Finished(Err(e.to_string())));
            }
            send.send(Event::State(Box::new(manager.snapshot())));
            busy.store(false, Ordering::Release);
            next_check = Instant::now() + Duration::from_secs(6 * 3600);
        } else if Instant::now() >= next_activation && !busy.swap(true, Ordering::AcqRel) {
            let _ = manager.reconcile_launcher_update();
            let _ = manager.activate_all_pending();
            send.send(Event::State(Box::new(manager.snapshot())));
            busy.store(false, Ordering::Release);
            next_activation = Instant::now() + Duration::from_secs(15);
        }
    }
}
fn recover_queued(
    manager: &mut Manager,
    command: &Command,
    checkpoint: Option<&crate::queue::QueueCheckpoint>,
) -> Option<Result<String>> {
    let checkpoint = checkpoint?;
    let id = match command {
        Command::Install { id, version } => {
            if version
                .as_ref()
                .is_some_and(|version| version != &checkpoint.version)
            {
                return Some(Err(fail(
                    "Queued installation version does not match its checkpoint.",
                )));
            }
            id
        }
        Command::UpdateReviewed { id, version } => {
            if version != &checkpoint.version {
                return Some(Err(fail(
                    "Queued update version does not match its checkpoint.",
                )));
            }
            id
        }
        _ => return None,
    };
    if let Some(pending) = manager.state.pending.get(id)
        && pending.version.version == checkpoint.version
        && Some(&pending.version.directory) != checkpoint.previous_pending_directory.as_ref()
    {
        return Some(manager.activate_pending(id).map(|activated| {
            if activated {
                "Previously staged update activated.".into()
            } else {
                "Previously staged update will activate after the app closes.".into()
            }
        }));
    }
    if let Some(installed) = manager.state.installations.get(id)
        && installed.managed
        && installed.version == checkpoint.version
        && installed.directory.as_ref() != checkpoint.previous_install_directory.as_ref()
    {
        return Some(manager.executable(id, installed).and_then(|path| {
            if path.exists() {
                Ok("Previously completed installation confirmed.".into())
            } else {
                Err(fail(
                    "The completed installation is missing its executable.",
                ))
            }
        }));
    }
    None
}

fn execute(
    manager: &mut Manager,
    command: Command,
    launcher: &std::path::Path,
    cancel: &AtomicBool,
    send: &EventSender,
    checkpoint: Option<&crate::queue::QueueCheckpoint>,
) -> Result<String> {
    let mut progress = |p| {
        send.send(Event::Progress(p));
    };
    let mut changed = |s| {
        send.send(Event::State(Box::new(s)));
    };
    match command {
        Command::DownloadSelected { ids, folder } => {
            let count = manager.download_packages(&ids, &folder, cancel, &mut progress)?;
            Ok(format!(
                "{count} verified packages saved to {}.",
                folder.display()
            ))
        }
        Command::Refresh => {
            manager.discover_products(cancel)?;
            let discovery_error = manager.state.discovery_error.clone();
            manager.refresh_all(cancel, &mut changed)?;
            if let Some(error) = discovery_error {
                return Err(fail(format!("Product discovery unavailable: {error}")));
            }
            manager.auto_updates(cancel, &mut progress, &mut changed)?;
            manager.check_launcher_update(false, cancel)?;
            Ok("Release check complete.".into())
        }
        Command::Repair(id) => {
            manager.repair(&id, cancel, &mut progress)?;
            Ok("Repair completed, or activation will follow after the app closes.".into())
        }
        Command::CheckInstallation(id) => manager.check_installation(&id),
        Command::UpdateReviewed { id, version } => {
            manager.update_reviewed(&id, &version, cancel, &mut progress)?;
            Ok("Update verified. Running apps activate after closing.".into())
        }
        Command::Install { id, version } => {
            if let Some(checkpoint) = checkpoint {
                manager.stage_queued_install(
                    &id,
                    &checkpoint.version,
                    version.is_some(),
                    cancel,
                    &mut progress,
                )?;
            } else {
                manager.stage_install(&id, version.as_deref(), cancel, &mut progress)?;
            }
            Ok(if manager.state.pending.contains_key(&id) {
                "Update verified. It will activate after the app closes.".into()
            } else {
                "App ready.".into()
            })
        }
        Command::ClearRecent => {
            manager.state.recent_projects.clear();
            manager.save()?;
            Ok("Recent project history cleared.".into())
        }
        Command::Discover => {
            manager.discover_products(cancel)?;
            if let Some(error) = &manager.state.discovery_error {
                return Err(fail(error));
            }
            Ok("GitHub product discovery complete.".into())
        }
        Command::ScanLocal => {
            let apps = crate::workspace::discover_local(
                &crate::workspace::local_search_roots(),
                &manager.snapshot(),
            );
            let count = apps.len();
            send.send(Event::LocalApps(apps));
            Ok(format!("Found {count} local installations."))
        }
        Command::OpenProject { id, path } => {
            manager.open_project(&id, path)?;
            Ok("Project opened.".into())
        }
        Command::Launch(id) => {
            manager.launch(&id)?;
            Ok(format!("{} launched.", manager.product(&id)?.name))
        }
        Command::Link(id, path) => {
            manager.link(&id, path)?;
            Ok("Existing installation linked.".into())
        }
        Command::Remove(id) => {
            manager.remove(&id)?;
            Ok("App removed. Project files were kept.".into())
        }
        Command::Rollback(id) => {
            manager.rollback(&id)?;
            Ok("Previous version restored and pinned.".into())
        }
        Command::Discard(id) => {
            manager.discard_pending(&id)?;
            Ok("Staged update discarded.".into())
        }
        Command::Configure(settings) => {
            let changed_channel = settings.channel != manager.state.settings.channel;
            manager.configure(settings, launcher)?;
            if changed_channel
                && (manager.state.releases.is_empty()
                    || manager
                        .state
                        .releases
                        .values()
                        .any(|c| c.versions.is_empty()))
            {
                manager.refresh_all(cancel, &mut changed)?;
            }
            Ok("Settings saved.".into())
        }
        Command::Preferences(id, preferences) => {
            let changed_channel = preferences.channel != manager.preferences(&id).channel;
            manager.set_preferences(&id, preferences)?;
            if changed_channel
                && !manager
                    .state
                    .releases
                    .get(&id)
                    .is_some_and(|c| !c.versions.is_empty())
            {
                manager.refresh_app(&id)?;
            }
            Ok("App preferences saved.".into())
        }
        Command::ScanStorage => {
            let report = manager.storage_report()?;
            send.send(Event::Storage(report));
            Ok("Storage scan complete.".into())
        }
        Command::RemoveCachedDownloads(names) => {
            let count = manager.remove_cached_downloads(&names)?;
            send.send(Event::Storage(manager.storage_report()?));
            Ok(format!("Removed {count} cached downloads."))
        }
        Command::RemoveRollback { id, version } => {
            manager.remove_rollback(&id, &version)?;
            send.send(Event::Storage(manager.storage_report()?));
            Ok("Previous version removed.".into())
        }
        Command::Reveal(id) => {
            manager.reveal(id.as_deref())?;
            Ok("Folder opened.".into())
        }
        Command::CheckLauncher => {
            manager.check_launcher_update(true, cancel)?;
            Ok(if manager.state.launcher_update.is_some() {
                "Launcher update verified. Restart to activate.".into()
            } else {
                "Launcher is up to date, or no update feed is configured.".into()
            })
        }
        Command::ActivatePending => {
            manager.activate_all_pending()?;
            Ok("Pending updates checked.".into())
        }
        Command::Queued { .. } => Err(fail("Nested queue commands are unsupported.")),
        Command::Shutdown => Ok(String::new()),
    }
}
