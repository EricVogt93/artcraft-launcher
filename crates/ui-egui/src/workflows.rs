use crate::{Launcher, Page};
use craftlauncher_core::{
    CATALOG, Command,
    workspace::{LocalApp, UpdatePreview, project_apps, update_preview},
};
use egui::{Context, RichText, Ui};
use std::{collections::BTreeSet, path::PathBuf, time::Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TaskStatus {
    Waiting,
    Running,
    Done,
    Failed(String),
    Cancelled,
}
#[derive(Clone)]
pub(crate) struct Task {
    pub command: Command,
    pub label: String,
    pub status: TaskStatus,
}
#[derive(Default)]
pub(crate) struct Workflows {
    pub tasks: Vec<Task>,
    pub active: Option<usize>,
    pub review: Option<Vec<UpdatePreview>>,
    pub selected_updates: BTreeSet<String>,
    pub projects: Vec<PathBuf>,
    pub project_app: Option<String>,
    pub local: Vec<LocalApp>,
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
            self.toast = Some(("Added to queue.".into(), false, Instant::now()));
        }
    }
    pub(crate) fn dispatch_task(&mut self) {
        if self.workflows.active.is_some() || self.worker.busy() {
            return;
        }
        if let Some(index) = self
            .workflows
            .tasks
            .iter()
            .position(|t| t.status == TaskStatus::Waiting)
        {
            match self.worker.send(Command::Queued {
                id: index,
                command: Box::new(self.workflows.tasks[index].command.clone()),
            }) {
                Ok(()) => {
                    self.workflows.tasks[index].status = TaskStatus::Running;
                    self.workflows.active = Some(index);
                }
                Err(_) if self.worker.busy() => {}
                Err(error) => {
                    self.workflows.tasks[index].status = TaskStatus::Failed(error.to_string());
                }
            }
        }
    }
    pub(crate) fn queue_page(&mut self, ui: &mut Ui) {
        self.headline(ui, "Your download queue.", "Installations, downloads and project launches run one at a time. Closing the launcher cancels unfinished work.");
        if self.workflows.tasks.is_empty() {
            ui.label("Install an app or select packages to start a queue.");
        }
        ui.horizontal(|ui| {
            if ui.button("Cancel unfinished").clicked() {
                for task in &mut self.workflows.tasks {
                    if task.status == TaskStatus::Waiting {
                        task.status = TaskStatus::Cancelled;
                    }
                }
                if self.workflows.active.is_some() {
                    self.worker.cancel();
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
                        TaskStatus::Waiting | TaskStatus::Running | TaskStatus::Failed(_)
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
                        TaskStatus::Running => {
                            ui.spinner();
                            if let Some(progress) = &self.progress {
                                ui.add(
                                    egui::ProgressBar::new(progress.percent / 100.0)
                                        .text(&progress.phase),
                                );
                            }
                            if ui.small_button("Cancel").clicked() {
                                self.worker.cancel();
                            }
                        }
                        TaskStatus::Done => {
                            ui.label("Complete");
                        }
                        TaskStatus::Failed(error) => {
                            ui.colored_label(self.palette.danger, error);
                            if ui.button("Retry").clicked() {
                                self.workflows.tasks[index].status = TaskStatus::Waiting;
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
        if let Some(preview) = self.workflows.review.clone() {
            let mut open = true;
            egui::Window::new("Review app updates").open(&mut open).collapsible(false).resizable(true).default_width(570.0).show(ctx, |ui| {
                ui.label("Choose the updates to download. Version pins stay in place; running apps activate their updates after closing.");
                let mut total = 0u64;
                for item in &preview {
                    let mut selected = self.workflows.selected_updates.contains(&item.app_id);
                    if ui.add_enabled(!item.pinned, egui::Checkbox::new(&mut selected, format!("{} · {} → {} · {:.1} MB{}", name(&item.app_id), item.current, item.next, item.bytes as f64 / 1_000_000.0, if item.pinned { " · pinned" } else { "" }))).changed() {
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
                let mut apps: Vec<_> = CATALOG
                    .into_iter()
                    .filter(|app| self.snapshot.state.installations.contains_key(app.id))
                    .collect();
                apps.sort_by_key(|app| !suggestions.contains(&app.id));
                if apps.is_empty() {
                    ui.label("Install or link a Craft app first.");
                }
                for app in apps {
                    ui.radio_value(
                        &mut self.workflows.project_app,
                        Some(app.id.into()),
                        format!(
                            "{}{}",
                            app.name,
                            if suggestions.contains(&app.id) {
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
                        self.enqueue(Command::OpenProject {
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
                    let available = project.path.is_file()
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
                                name(&project.app_id)
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
                if ui
                    .add_enabled(
                        !self.worker.busy(),
                        egui::Button::new("Clear recent projects"),
                    )
                    .clicked()
                {
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
                ui.label(name(&app.app_id));
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
}
fn name(id: &str) -> String {
    craftlauncher_core::app(id)
        .map(|app| app.name.to_owned())
        .unwrap_or_else(|_| id.to_owned())
}

impl Workflows {
    fn add_task(&mut self, command: Command) -> bool {
        let label = match &command {
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
