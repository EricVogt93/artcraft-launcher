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
    Install { id: String, version: Option<String> },
    DownloadSelected { ids: Vec<String>, folder: PathBuf },
    Launch(String),
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
#[derive(Debug)]
pub enum Event {
    State(Box<Snapshot>),
    Progress(Progress),
    Finished(std::result::Result<String, String>),
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
        let (commands, received) = mpsc::channel();
        let (send, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_busy = busy.clone();
        let thread = thread::spawn(move || {
            run(
                manager,
                launcher,
                received,
                send,
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
    send: Sender<Event>,
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
) {
    let mut next_check = if manager.state.settings.check_on_startup {
        Instant::now()
    } else {
        Instant::now() + Duration::from_secs(6 * 3600)
    };
    let mut next_activation = Instant::now() + Duration::from_secs(15);
    let _ = send.send(Event::State(Box::new(manager.snapshot())));
    loop {
        let command = match commands.recv_timeout(Duration::from_secs(1)) {
            Ok(Command::Shutdown) => break,
            Ok(command) => Some(command),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => None,
        };
        if let Some(command) = command {
            let refresh = matches!(command, Command::Refresh);
            let result = execute(&mut manager, command, &launcher, &cancel, &send);
            let _ = send.send(Event::State(Box::new(manager.snapshot())));
            let _ = send.send(Event::Finished(result.map_err(|e| e.to_string())));
            busy.store(false, Ordering::Release);
            // An explicit refresh satisfies the periodic check too.
            if refresh {
                next_check = Instant::now() + Duration::from_secs(6 * 3600);
            }
        } else if Instant::now() >= next_check && !busy.swap(true, Ordering::AcqRel) {
            cancel.store(false, Ordering::Relaxed);
            let result = execute(&mut manager, Command::Refresh, &launcher, &cancel, &send);
            if let Err(e) = result {
                let _ = send.send(Event::Finished(Err(e.to_string())));
            }
            let _ = send.send(Event::State(Box::new(manager.snapshot())));
            busy.store(false, Ordering::Release);
            next_check = Instant::now() + Duration::from_secs(6 * 3600);
        } else if Instant::now() >= next_activation && !busy.swap(true, Ordering::AcqRel) {
            let _ = manager.reconcile_launcher_update();
            let _ = manager.activate_all_pending();
            let _ = send.send(Event::State(Box::new(manager.snapshot())));
            busy.store(false, Ordering::Release);
            next_activation = Instant::now() + Duration::from_secs(15);
        }
    }
}
fn execute(
    manager: &mut Manager,
    command: Command,
    launcher: &std::path::Path,
    cancel: &AtomicBool,
    send: &Sender<Event>,
) -> Result<String> {
    let mut progress = |p| {
        let _ = send.send(Event::Progress(p));
    };
    let mut changed = |s| {
        let _ = send.send(Event::State(Box::new(s)));
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
            manager.refresh_all(cancel, &mut changed)?;
            manager.auto_updates(cancel, &mut progress, &mut changed)?;
            manager.check_launcher_update(false, cancel)?;
            Ok("Release check complete.".into())
        }
        Command::Install { id, version } => {
            manager.stage_install(&id, version.as_deref(), cancel, &mut progress)?;
            Ok(if manager.state.pending.contains_key(&id) {
                "Update verified. It will activate after the app closes.".into()
            } else {
                "App ready.".into()
            })
        }
        Command::Launch(id) => {
            manager.launch(&id)?;
            Ok(format!("{} launched.", crate::app(&id)?.name))
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
            if changed_channel {
                manager.refresh_all(cancel, &mut changed)?;
            }
            Ok("Settings saved.".into())
        }
        Command::Preferences(id, preferences) => {
            let changed_channel = preferences.channel != manager.preferences(&id).channel;
            manager.set_preferences(&id, preferences)?;
            if changed_channel {
                let _ = manager.refresh_app(&id);
            }
            Ok("App preferences saved.".into())
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
                "Launcher is up to date, or no local feed is configured.".into()
            })
        }
        Command::ActivatePending => {
            manager.activate_all_pending()?;
            Ok("Pending updates checked.".into())
        }
        Command::Shutdown => Ok(String::new()),
    }
}
