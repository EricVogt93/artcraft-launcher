use crate::{Launcher, Page};
use craftlauncher_core::{
    Command,
    queue::{self, QueueAction, QueueEntry, QueueStatus},
    workspace::{LocalApp, UpdatePreview, project_apps, update_preview},
};
use egui::{Context, RichText, Ui};
use std::{collections::BTreeSet, path::PathBuf, time::Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TaskStatus {
    Waiting,
    Paused,
    Running,
    Done,
    Failed(String),
    Cancelled,
}
#[derive(Clone, Copy)]
pub(crate) enum StopIntent {
    Pause,
    Cancel,
}
#[derive(Clone)]
pub(crate) struct Task {
    pub journal_id: String,
    pub checkpoint: Option<queue::QueueCheckpoint>,
    pub command: Command,
    pub label: String,
    pub status: TaskStatus,
}
#[derive(Default)]
pub(crate) struct Workflows {
    pub tasks: Vec<Task>,
    pub active: Option<usize>,
    pub stop_intent: Option<StopIntent>,
    pub review: Option<Vec<UpdatePreview>>,
    pub selected_updates: BTreeSet<String>,
    pub projects: Vec<PathBuf>,
    pub project_app: Option<String>,
    pub local: Vec<LocalApp>,
    pub journal_error: Option<String>,
    pub entrypoint_choices: Option<(String, Vec<String>)>,
}
impl Launcher {
    pub(crate) fn enqueue(&mut self, command: Command) {
        if let Command::DownloadSelected { ids, folder } = command {
            for id in ids {
                self.add_task(Command::DownloadSelected {
                    ids: vec![id],
                    folder: folder.clone(),
                });
            }
        } else {
            self.add_task(command);
        }
    }
    fn add_task(&mut self, command: Command) {
        if self.workflows.add_task(command) {
            self.persist_queue();
            if self.workflows.journal_error.is_none() {
                self.toast = Some(("Added to queue.".into(), false, Instant::now()));
            }
        }
    }
    pub(crate) fn dispatch_task(&mut self) {
        if self.workflows.active.is_some()
            || self.worker.busy()
            || self.workflows.journal_error.is_some()
        {
            return;
        }
        if let Some(index) = self
            .workflows
            .tasks
            .iter()
            .position(|t| t.status == TaskStatus::Waiting)
        {
            if self.workflows.tasks[index].checkpoint.is_none() {
                let task = self.workflows.tasks[index].command.clone();
                let target = match &task {
                    Command::Install { id, version } => version
                        .clone()
                        .or_else(|| self.release(id).map(|release| release.tag_name.clone()))
                        .map(|v| (id, v)),
                    Command::UpdateReviewed { id, version } => Some((id, version.clone())),
                    _ => None,
                };
                if let Some((id, version)) = target {
                    self.workflows.tasks[index].checkpoint = Some(queue::QueueCheckpoint {
                        version,
                        previous_install_directory: self
                            .snapshot
                            .state
                            .installations
                            .get(id)
                            .and_then(|install| install.directory.clone()),
                        previous_pending_directory: self
                            .snapshot
                            .state
                            .pending
                            .get(id)
                            .map(|pending| pending.version.directory.clone()),
                    });
                }
            }
            self.workflows.tasks[index].status = TaskStatus::Running;
            self.persist_queue();
            if self.workflows.journal_error.is_some() {
                self.workflows.tasks[index].status = TaskStatus::Paused;
                return;
            }
            match self.worker.send(Command::Queued {
                id: index,
                task_key: self.workflows.tasks[index].journal_id.clone(),
                checkpoint: self.workflows.tasks[index].checkpoint.clone(),
                command: Box::new(self.workflows.tasks[index].command.clone()),
            }) {
                Ok(()) => {
                    self.workflows.tasks[index].status = TaskStatus::Running;
                    self.workflows.active = Some(index);
                }
                Err(_) if self.worker.busy() => {
                    self.workflows.tasks[index].status = TaskStatus::Waiting;
                    self.persist_queue();
                }
                Err(error) => {
                    self.workflows.tasks[index].status = TaskStatus::Failed(error.to_string());
                    self.persist_queue();
                }
            }
        }
    }
    pub(crate) fn stop_active(&mut self, intent: StopIntent) {
        if self.workflows.active.is_some() {
            self.workflows.stop_intent = Some(intent);
        }
        self.worker.cancel();
    }
    pub(crate) fn discard_task_cache(&mut self, id: &str) {
        if let Err(error) = queue::discard_partial(&self.snapshot.root, id) {
            self.toast = Some((
                format!("Could not remove partial download: {error}"),
                true,
                Instant::now(),
            ));
        }
    }
    pub(crate) fn queue_page(&mut self, ui: &mut Ui) {
        self.headline(
            ui,
            "Your download queue.",
            "Downloads and installations are restored paused after a restart.",
        );
        let before = self.workflows.entries();
        if let Some(error) = &self.workflows.journal_error {
            ui.colored_label(
                self.palette.danger,
                format!("Queue storage unavailable: {error}"),
            );
        }
        if self.workflows.tasks.is_empty() {
            ui.label("Install an app or select packages to start a queue.");
        }
        ui.horizontal(|ui| {
            if ui.button("Resume all").clicked() {
                for task in &mut self.workflows.tasks {
                    if task.status == TaskStatus::Paused {
                        task.status = TaskStatus::Waiting;
                    }
                }
            }
            if ui.button("Pause waiting").clicked() {
                for task in &mut self.workflows.tasks {
                    if task.status == TaskStatus::Waiting {
                        task.status = TaskStatus::Paused;
                    }
                }
            }
            if ui.button("Cancel unfinished").clicked() {
                for task in &mut self.workflows.tasks {
                    if matches!(task.status, TaskStatus::Waiting | TaskStatus::Paused) {
                        task.status = TaskStatus::Cancelled;
                    }
                }
                if self.workflows.active.is_some() {
                    self.stop_active(StopIntent::Cancel);
                }
            }
            if ui
                .add_enabled(
                    self.workflows.active.is_none(),
                    egui::Button::new("Clear completed"),
                )
                .clicked()
            {
                self.workflows.tasks.retain(|t| {
                    matches!(
                        t.status,
                        TaskStatus::Waiting
                            | TaskStatus::Paused
                            | TaskStatus::Running
                            | TaskStatus::Failed(_)
                    )
                });
            }
        });
        for index in 0..self.workflows.tasks.len() {
            let task = self.workflows.tasks[index].clone();
            ui.push_id(index, |ui| {
                ui.group(|ui| {
                    ui.label(RichText::new(&task.label).strong());
                    match &task.status {
                        TaskStatus::Waiting => {
                            ui.horizontal(|ui| {
                                ui.label("Waiting");
                                if ui.small_button("Cancel").clicked() {
                                    self.workflows.tasks[index].status = TaskStatus::Cancelled;
                                }
                            });
                        }
                        TaskStatus::Paused => {
                            ui.label("Paused");
                            if ui.button("Resume").clicked() {
                                self.workflows.tasks[index].status = TaskStatus::Waiting;
                            }
                            if ui.small_button("Cancel").clicked() {
                                self.workflows.tasks[index].status = TaskStatus::Cancelled;
                            }
                        }
                        TaskStatus::Running => {
                            ui.spinner();
                            if let Some(progress) = &self.progress {
                                ui.add(
                                    egui::ProgressBar::new(progress.percent / 100.0)
                                        .text(&progress.phase),
                                );
                                if let Some(detail) = crate::transfer_details(progress) {
                                    ui.label(detail);
                                }
                            }
                            if ui.small_button("Pause").clicked() {
                                self.stop_active(StopIntent::Pause);
                            }
                            if ui.small_button("Cancel").clicked() {
                                self.stop_active(StopIntent::Cancel);
                            }
                        }
                        TaskStatus::Done => {
                            ui.label("Complete");
                        }
                        TaskStatus::Failed(error) => {
                            ui.colored_label(self.palette.danger, error);
                            if let Command::Launch(id) = &task.command {
                                let log = self.snapshot.root.join("logs").join(format!("{id}.log"));
                                if log.is_file() {
                                    if ui.button("Open launch log").clicked() {
                                        let _ = open::that(&log);
                                    }
                                    if ui.button("Copy diagnostic report").clicked() {
                                        let text = std::fs::read(&log)
                                            .map(|v| String::from_utf8_lossy(&v).into_owned())
                                            .unwrap_or_default();
                                        let home = std::env::var("HOME")
                                            .or_else(|_| std::env::var("USERPROFILE"))
                                            .unwrap_or_default();
                                        let name = self
                                            .snapshot
                                            .products
                                            .iter()
                                            .find(|p| p.id == *id)
                                            .map(|p| p.name.as_str())
                                            .unwrap_or(id);
                                        let version = self
                                            .snapshot
                                            .state
                                            .installations
                                            .get(id)
                                            .map(|v| v.version.as_str())
                                            .unwrap_or("unknown");
                                        let report = format!(
                                            "CraftLauncher {} · {} / {}\n{} {}\n{}\n\n{}",
                                            craftlauncher_core::self_update::build_version(),
                                            self.snapshot.platform,
                                            self.snapshot.arch,
                                            name,
                                            version,
                                            error,
                                            text
                                        );
                                        ui.ctx().copy_text(if home.is_empty() {
                                            report
                                        } else {
                                            report.replace(&home, "<home>")
                                        });
                                    }
                                }
                            }
                            if ui.button("Retry").clicked() {
                                self.workflows.tasks[index].status = TaskStatus::Waiting;
                            }
                            if ui.button("Cancel").clicked() {
                                self.workflows.tasks[index].status = TaskStatus::Cancelled;
                            }
                        }
                        TaskStatus::Cancelled => {
                            ui.label("Cancelled");
                            if ui.button("Retry").clicked() {
                                self.workflows.tasks[index].status = TaskStatus::Waiting;
                            }
                        }
                    }
                });
            });
            ui.add_space(8.0);
        }
        let after = self.workflows.entries();
        if before != after {
            self.persist_queue();
            if self.workflows.journal_error.is_none() {
                for old in &before {
                    if old.status == QueueStatus::Cancelled
                        && !after.iter().any(|entry| entry.id == old.id)
                    {
                        self.discard_task_cache(&old.id);
                    }
                }
                for entry in &after {
                    if entry.status == QueueStatus::Cancelled
                        && before
                            .iter()
                            .any(|old| old.id == entry.id && old.status != QueueStatus::Cancelled)
                    {
                        self.discard_task_cache(&entry.id);
                    }
                }
            }
        }
    }
    pub(crate) fn persist_queue(&mut self) {
        if self.workflows.journal_error.is_some() {
            return;
        }
        if let Err(error) = queue::save(&self.snapshot.root, &self.workflows.entries()) {
            self.workflows.journal_error = Some(error.to_string());
            self.toast = Some((
                format!("Could not save queue: {error}"),
                true,
                Instant::now(),
            ));
        }
    }
    pub(crate) fn review_updates(&mut self) {
        let preview = update_preview(&self.snapshot);
        self.workflows.selected_updates = preview
            .iter()
            .filter(|p| !p.pinned)
            .map(|p| p.app_id.clone())
            .collect();
        self.workflows.review = Some(preview);
    }
    pub(crate) fn workflow_dialogs(&mut self, ctx: &Context) {
        if let Some((id, paths)) = self.workflows.entrypoint_choices.clone() {
            let mut open = true;
            let mut selected = None;
            egui::Window::new("Choose application entry").open(&mut open).collapsible(false).default_width(500.0).show(ctx, |ui| {
                ui.label("The package contains more than one application entry. Select the one to launch.");
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for path in &paths {
                        if ui.button(path).clicked() { selected = Some(path.clone()); }
                    }
                });
                ui.label("After saving your choice, retry the installation from the queue.");
            });
            if let Some(path) = selected {
                let mut preferences = self.preferences(&id);
                let key = craftlauncher_core::releases::asset_preference_key(
                    &self.snapshot.platform,
                    &self.snapshot.arch,
                );
                preferences.selected_entrypoints.insert(key, path);
                self.command(Command::Preferences(id, preferences));
                self.entrypoint_app = None;
                self.workflows.entrypoint_choices = None;
            } else if !open {
                self.workflows.entrypoint_choices = None;
            }
        }
        if let Some(preview) = self.workflows.review.clone() {
            let mut open = true;
            egui::Window::new("Review app updates").open(&mut open).collapsible(false).resizable(true).default_width(570.0).show(ctx, |ui| {
                ui.label("Choose the updates to download. Version pins stay in place; running apps activate their updates after closing.");
                let mut total = 0u64;
                for item in &preview {
                    let mut selected = self.workflows.selected_updates.contains(&item.app_id);
                    if ui.add_enabled(!item.pinned, egui::Checkbox::new(&mut selected, format!("{} · {} → {} · {:.1} MB{}", self.product_name(&item.app_id), item.current, item.next, item.bytes as f64 / 1_000_000.0, if item.pinned { " · pinned" } else { "" }))).changed() {
                        if selected { self.workflows.selected_updates.insert(item.app_id.clone()); } else { self.workflows.selected_updates.remove(&item.app_id); }
                    }
                    if selected && !item.pinned { total = total.saturating_add(item.bytes); }
                }
                ui.label(format!("Selected download size: {:.1} MB", total as f64 / 1_000_000.0));
                if ui.add_enabled(!self.workflows.selected_updates.is_empty(), egui::Button::new("Queue selected updates")).clicked() {
                    for item in preview.iter().filter(|item| !item.pinned && self.workflows.selected_updates.contains(&item.app_id)).cloned().collect::<Vec<_>>() {
                        self.enqueue(Command::UpdateReviewed { id: item.app_id, version: item.next });
                    }
                    self.workflows.review = None;
                    self.page = Page::Queue;
                }
            });
            if !open {
                self.workflows.review = None;
            }
        }
        let Some(path) = self.workflows.projects.first().cloned() else {
            return;
        };
        let mut open = true;
        egui::Window::new("Open project with a Craft app")
            .open(&mut open)
            .collapsible(false)
            .default_width(500.0)
            .show(ctx, |ui| {
                ui.label(path.display().to_string());
                ui.label("Choose an installed app. Suggested formats depend on the app version.");
                let suggestions = project_apps(&path);
                let products = self.snapshot.products.clone();
                let mut apps: Vec<_> = products
                    .iter()
                    .filter(|app| self.snapshot.state.installations.contains_key(&app.id))
                    .collect();
                apps.sort_by_key(|app| !suggestions.contains(&app.id.as_str()));
                if apps.is_empty() {
                    ui.label("Install or link a Craft app first.");
                }
                for app in apps {
                    ui.radio_value(
                        &mut self.workflows.project_app,
                        Some(app.id.clone()),
                        format!(
                            "{}{}",
                            app.name,
                            if suggestions.contains(&app.id.as_str()) {
                                " · suggested"
                            } else {
                                ""
                            }
                        ),
                    );
                }
                if ui
                    .add_enabled(
                        self.workflows
                            .project_app
                            .as_ref()
                            .is_some_and(|id| self.snapshot.state.installations.contains_key(id)),
                        egui::Button::new("Open project"),
                    )
                    .clicked()
                {
                    if let Some(id) = self.workflows.project_app.take() {
                        self.command(Command::OpenProject {
                            id,
                            path: path.clone(),
                        });
                    }
                    self.workflows.projects.remove(0);
                }
                if ui.button("Skip file").clicked() {
                    self.workflows.projects.remove(0);
                    self.workflows.project_app = None;
                }
            });
        if !open {
            self.workflows.projects.clear();
            self.workflows.project_app = None;
        }
    }
    pub(crate) fn project_controls(&mut self, ui: &mut Ui, id: Option<&str>) {
        if ui.button("Open project file…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Open a project with Craft")
                .pick_file()
        {
            self.workflows.projects.push(path);
            self.workflows.project_app = id.map(str::to_owned);
        }
        let recent: Vec<_> = self
            .snapshot
            .state
            .recent_projects
            .iter()
            .filter(|p| id.is_none_or(|id| id == p.app_id))
            .take(if id.is_some() { 5 } else { 8 })
            .cloned()
            .collect();
        if !recent.is_empty() {
            ui.collapsing("Recent projects", |ui| {
                for project in recent {
                    let available = self.existing_recent.contains(&project.path)
                        && self
                            .snapshot
                            .state
                            .installations
                            .contains_key(&project.app_id);
                    if ui
                        .add_enabled(
                            available,
                            egui::Button::new(format!(
                                "{} · {}",
                                project
                                    .path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy(),
                                self.product_name(&project.app_id)
                            )),
                        )
                        .on_hover_text(project.path.display().to_string())
                        .clicked()
                    {
                        self.command(Command::OpenProject {
                            id: project.app_id,
                            path: project.path,
                        });
                    }
                }
                if ui.button("Clear recent projects").clicked() {
                    self.command(Command::ClearRecent);
                }
            });
        }
    }
    pub(crate) fn discovery_page(&mut self, ui: &mut Ui) {
        self.headline(
            ui,
            "Discover your next tool.",
            "Find existing installations and new Craft products from the official GitHub account.",
        );
        ui.heading("On this computer");
        if ui
            .add_enabled(
                !self.worker.busy(),
                egui::Button::new("Scan common app folders"),
            )
            .clicked()
        {
            self.command(Command::ScanLocal);
        }
        ui.label("Checks PATH and common application folders. Select a result to link it.");
        let local = self.workflows.local.clone();
        if local.is_empty() {
            ui.label("No unlinked installations found in the last scan. You can also link an executable from its app menu.");
        }
        for app in local {
            ui.group(|ui| {
                ui.label(self.product_name(&app.app_id));
                ui.label(app.executable.display().to_string());
                if ui
                    .add_enabled(
                        !self.worker.busy()
                            && !self.snapshot.state.installations.contains_key(&app.app_id),
                        egui::Button::new("Link installation"),
                    )
                    .clicked()
                {
                    self.command(Command::Link(app.app_id, app.executable));
                }
            });
        }
        ui.add_space(20.0);
        ui.heading("New on GitHub");
        ui.label("Checked with release refreshes every six hours while the launcher is running. New candidates are shown here for review.");
        if ui
            .add_enabled(
                !self.worker.busy(),
                egui::Button::new("Discover products now"),
            )
            .clicked()
        {
            self.command(Command::Discover);
        }
        if let Some(at) = &self.snapshot.state.discovery_checked_at {
            ui.label(format!("Last successful check: {at}"));
        }
        if let Some(error) = &self.snapshot.state.discovery_error {
            ui.colored_label(
                self.palette.danger,
                format!("Discovery unavailable: {error}. Previous results are kept."),
            );
        }
        if self.snapshot.state.discovery_checked_at.is_some()
            && self.snapshot.state.discovered_apps.is_empty()
        {
            ui.label("No new Craft products beyond the current catalog.");
        }
        for app in self.snapshot.state.discovered_apps.clone() {
            ui.group(|ui| {
                ui.label(RichText::new(&app.repository).strong());
                ui.label(&app.description);
                if app.unavailable {
                    ui.label("No longer returned by the official discovery scan.");
                }
                if ui.link("Review official repository ↗").clicked() {
                    self.external(format!("https://github.com/storytold/{}", app.repository));
                }
                if ui.link("View published releases ↗").clicked() {
                    self.external(format!(
                        "https://github.com/storytold/{}/releases",
                        app.repository
                    ));
                }
            });
        }
    }
    pub(crate) fn product_name(&self, id: &str) -> String {
        self.snapshot
            .products
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| name(id))
    }
}
fn name(id: &str) -> String {
    if let Ok(app) = craftlauncher_core::app(id) {
        return app.name.to_owned();
    }
    let lower = id.to_ascii_lowercase();
    if let Some(prefix) = lower.strip_suffix("craft")
        && !prefix.is_empty()
    {
        let mut chars = prefix.chars();
        if let Some(first) = chars.next() {
            return format!("{}{}Craft", first.to_ascii_uppercase(), chars.as_str());
        }
    }
    id.to_owned()
}

impl Workflows {
    pub(crate) fn restore(root: &std::path::Path) -> Self {
        let mut workflows = Self::default();
        match queue::load(root) {
            Ok(entries) => {
                for entry in entries {
                    let command = match entry.action {
                        QueueAction::Install { id, version } => Command::Install { id, version },
                        QueueAction::Update { id, version } => {
                            Command::UpdateReviewed { id, version }
                        }
                        QueueAction::Repair { id } => Command::Repair(id),
                        QueueAction::Download { id, folder } => Command::DownloadSelected {
                            ids: vec![id],
                            folder,
                        },
                    };
                    if workflows.add_task(command) {
                        let task = workflows.tasks.last_mut().unwrap();
                        task.journal_id = entry.id;
                        task.checkpoint = entry.checkpoint;
                        task.status = match entry.status {
                            QueueStatus::Waiting | QueueStatus::Running | QueueStatus::Paused => {
                                TaskStatus::Paused
                            }
                            QueueStatus::Done => TaskStatus::Done,
                            QueueStatus::Failed(error) => TaskStatus::Failed(error),
                            QueueStatus::Cancelled => TaskStatus::Cancelled,
                        };
                    }
                }
            }
            Err(error) => workflows.journal_error = Some(error.to_string()),
        }
        workflows
    }
    fn entries(&self) -> Vec<QueueEntry> {
        self.tasks
            .iter()
            .filter_map(|task| {
                let action = match &task.command {
                    Command::Install { id, version } => QueueAction::Install {
                        id: id.clone(),
                        version: version.clone(),
                    },
                    Command::Repair(id) => QueueAction::Repair { id: id.clone() },
                    Command::UpdateReviewed { id, version } => QueueAction::Update {
                        id: id.clone(),
                        version: version.clone(),
                    },
                    Command::DownloadSelected { ids, folder } if ids.len() == 1 => {
                        QueueAction::Download {
                            id: ids[0].clone(),
                            folder: folder.clone(),
                        }
                    }
                    _ => return None,
                };
                let status = match &task.status {
                    TaskStatus::Waiting => QueueStatus::Waiting,
                    TaskStatus::Running => QueueStatus::Running,
                    TaskStatus::Paused => QueueStatus::Paused,
                    TaskStatus::Done => QueueStatus::Done,
                    TaskStatus::Failed(error) => QueueStatus::Failed(error.clone()),
                    TaskStatus::Cancelled => QueueStatus::Cancelled,
                };
                Some(QueueEntry {
                    id: task.journal_id.clone(),
                    action,
                    status,
                    checkpoint: task.checkpoint.clone(),
                })
            })
            .collect()
    }
    fn add_task(&mut self, command: Command) -> bool {
        let label = match &command {
            Command::Repair(id) => format!("Repair {}", name(id)),
            Command::UpdateReviewed { id, version } => format!("Update {} → {version}", name(id)),
            Command::Install { id, version } => format!(
                "Install / update {} · {}",
                name(id),
                version.as_deref().unwrap_or("latest")
            ),
            Command::DownloadSelected { ids, folder } => format!(
                "Download {} → {}",
                ids.iter().map(|id| name(id)).collect::<Vec<_>>().join(", "),
                folder.display()
            ),
            Command::OpenProject { id, path } => {
                format!("Open {} with {}", path.display(), name(id))
            }
            Command::Launch(id) => format!("Launch {}", name(id)),
            _ => return false,
        };
        if self.tasks.iter().any(|t| {
            t.label == label && matches!(t.status, TaskStatus::Waiting | TaskStatus::Running)
        }) {
            return false;
        }
        self.tasks.push(Task {
            journal_id: queue::new_id(),
            checkpoint: None,
            command,
            label,
            status: TaskStatus::Waiting,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_restores_durable_work_paused_and_excludes_project_launches() {
        let root =
            std::env::temp_dir().join(format!("craftlauncher-queue-test-{}", queue::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut original = Workflows::default();
        original.add_task(Command::Install {
            id: "newcraft".into(),
            version: None,
        });
        original.add_task(Command::OpenProject {
            id: "newcraft".into(),
            path: "/tmp/example".into(),
        });
        original.tasks[0].status = TaskStatus::Running;
        original.tasks[0].checkpoint = Some(queue::QueueCheckpoint {
            version: "v1".into(),
            previous_install_directory: None,
            previous_pending_directory: None,
        });
        let stable_id = original.tasks[0].journal_id.clone();
        queue::save(&root, &original.entries()).unwrap();
        let restored = Workflows::restore(&root);
        assert!(restored.journal_error.is_none());
        assert_eq!(restored.tasks.len(), 1);
        assert_eq!(restored.tasks[0].journal_id, stable_id);
        assert_eq!(restored.tasks[0].status, TaskStatus::Paused);
        assert_eq!(restored.tasks[0].checkpoint.as_ref().unwrap().version, "v1");
        assert!(matches!(restored.tasks[0].command, Command::Install { .. }));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn corrupt_queue_is_reported_without_overwriting_it() {
        let root =
            std::env::temp_dir().join(format!("craftlauncher-queue-test-{}", queue::new_id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("queue.json"), "broken").unwrap();
        assert!(Workflows::restore(&root).journal_error.is_some());
        assert_eq!(
            std::fs::read_to_string(root.join("queue.json")).unwrap(),
            "broken"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn queue_deduplicates_active_work_but_keeps_other_downloads_and_retries() {
        let mut queue = Workflows::default();
        let command = Command::DownloadSelected {
            ids: vec!["photocraft".into()],
            folder: PathBuf::from("downloads"),
        };
        assert!(queue.add_task(command.clone()));
        assert!(!queue.add_task(command.clone()));
        queue.tasks[0].status = TaskStatus::Running;
        assert!(!queue.add_task(command.clone()));
        assert!(queue.add_task(Command::Install {
            id: "wordcraft".into(),
            version: None
        }));
        queue.tasks[0].status = TaskStatus::Failed("offline".into());
        assert!(queue.add_task(command));
        assert_eq!(queue.tasks.len(), 3);
        assert_eq!(queue.tasks[1].status, TaskStatus::Waiting);
    }
    #[test]
    fn project_queue_distinguishes_equal_filenames_in_different_folders() {
        let mut queue = Workflows::default();
        for directory in ["first", "second"] {
            assert!(queue.add_task(Command::OpenProject {
                id: "photocraft".into(),
                path: PathBuf::from(directory).join("image.psd")
            }));
        }
        assert_eq!(queue.tasks.len(), 2);
    }
    #[test]
    fn reviewed_update_tasks_keep_the_reviewed_version() {
        let mut queue = Workflows::default();
        assert!(queue.add_task(Command::UpdateReviewed {
            id: "wordcraft".into(),
            version: "v2.0.0".into()
        }));
        assert!(
            matches!(&queue.tasks[0].command, Command::UpdateReviewed { version, .. } if version == "v2.0.0")
        );
    }
}
