mod assets;
mod palette;
mod theme;
mod tray;
mod views;
mod widgets;
mod window;
mod workflows;

use assets::Assets;
pub use assets::LOGO_PNG;
pub use assets::icon_rgba;
pub const APP_ID: &str = "craftlauncher";
use craftlauncher_core::{Command, Event, Manager, Progress, Snapshot, Worker, self_update};
use egui::{Context, RichText};
use raw_window_handle::HasWindowHandle;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};
use theme::Palette;
use tray::{Tray, TrayAction};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    MyApps,
    AllApps,
    Updates,
    Activity,
    Queue,
    Discover,
    Settings,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    General,
    Storage,
    Changelog,
    About,
}
pub struct HealthCheck {
    pub path: PathBuf,
    pub expected_version: String,
}
pub struct Launcher {
    workflows: workflows::Workflows,
    worker: Worker,
    snapshot: Snapshot,
    assets: Assets,
    palette: Palette,
    page: Page,
    settings_tab: SettingsTab,
    search: String,
    palette_open: bool,
    palette_query: String,
    palette_selected: usize,
    palette_focus: bool,
    selected_apps: BTreeSet<String>,
    discipline: usize,
    detail: Option<String>,
    confirm_remove: Option<String>,
    progress: Option<Progress>,
    toast: Option<(String, bool, Instant)>,
    tray: Option<Tray>,
    tray_attempted: bool,
    can_hide_window: bool,
    quit: bool,
    initially_hidden: bool,
    health: Option<HealthCheck>,
    first_frame: bool,
    feed_path: String,
    public_key: String,
    selected_version: String,
    entrypoint_app: Option<String>,
    entrypoint_draft: String,
    storage_report: Option<craftlauncher_core::storage::StorageReport>,
    confirm_storage: Option<(String, String, u64)>,
    selected_cache: BTreeSet<String>,
    confirm_cache: bool,
    exit_after: Option<Instant>,
    applied_theme: Option<(craftlauncher_core::Theme, Option<egui::Theme>)>,
    existing_recent: BTreeSet<PathBuf>,
}
fn recent_file_set(snapshot: &Snapshot) -> BTreeSet<PathBuf> {
    snapshot
        .state
        .recent_projects
        .iter()
        .filter(|p| p.path.is_file())
        .map(|p| p.path.clone())
        .collect()
}
fn transfer_details(progress: &Progress) -> Option<String> {
    let transferred = progress.transferred_bytes? as f64 / 1_000_000.0;
    let total = progress.total_bytes? as f64 / 1_000_000.0;
    let mut text = format!("{transferred:.1} / {total:.1} MB");
    if let Some(speed) = progress.bytes_per_second {
        text.push_str(&format!(" · {:.1} MB/s", speed / 1_000_000.0));
    }
    if let Some(seconds) = progress.seconds_remaining {
        text.push_str(&format!(" · ~{seconds} s remaining"));
    }
    Some(text)
}

impl Launcher {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        manager: Manager,
        background: bool,
        health: Option<HealthCheck>,
        exit_after: Option<Duration>,
    ) -> Self {
        theme::fonts(&cc.egui_ctx);
        let snapshot = manager.snapshot();
        let palette = theme::apply(&cc.egui_ctx, snapshot.state.settings.theme);
        let feed_path = snapshot
            .state
            .settings
            .update_feed
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let public_key = snapshot
            .state
            .settings
            .update_public_key
            .clone()
            .unwrap_or_default();
        let page = if snapshot.state.installations.is_empty() {
            Page::AllApps
        } else {
            Page::MyApps
        };
        let launcher = std::env::current_exe().unwrap_or_default();
        let workflows = workflows::Workflows::restore(&snapshot.root);
        let applied_theme = Some((snapshot.state.settings.theme, cc.egui_ctx.system_theme()));
        let existing_recent = recent_file_set(&snapshot);
        let egui_ctx = cc.egui_ctx.clone();
        let worker = Worker::with_waker(
            manager,
            launcher,
            std::sync::Arc::new(move || egui_ctx.request_repaint()),
        );
        Self {
            workflows,
            worker,
            snapshot,
            assets: Assets::load(&cc.egui_ctx),
            palette,
            page,
            settings_tab: SettingsTab::General,
            search: String::new(),
            palette_open: false,
            palette_query: String::new(),
            palette_selected: 0,
            palette_focus: false,
            selected_apps: BTreeSet::new(),
            discipline: 0,
            detail: None,
            confirm_remove: None,
            progress: None,
            toast: None,
            tray: None,
            tray_attempted: false,
            can_hide_window: window::supports_hiding(cc.window_handle().ok().map(|h| h.as_raw())),
            quit: false,
            initially_hidden: background,
            health,
            first_frame: true,
            feed_path,
            public_key,
            selected_version: String::new(),
            entrypoint_app: None,
            entrypoint_draft: String::new(),
            storage_report: None,
            confirm_storage: None,
            selected_cache: BTreeSet::new(),
            confirm_cache: false,
            exit_after: exit_after.map(|duration| Instant::now() + duration),
            applied_theme,
            existing_recent,
        }
    }
    fn command(&mut self, command: Command) {
        if matches!(
            command,
            Command::Install { .. }
                | Command::Repair(_)
                | Command::UpdateReviewed { .. }
                | Command::DownloadSelected { .. }
        ) {
            self.enqueue(command);
            return;
        }
        match &command {
            Command::Preferences(id, prefs) => {
                let channel = prefs
                    .channel
                    .unwrap_or(self.snapshot.state.settings.channel);
                self.snapshot
                    .state
                    .preferences
                    .insert(id.clone(), prefs.clone());
                if let Some(cache) = self.snapshot.state.releases.get_mut(id) {
                    cache.set_channel(channel);
                }
            }
            Command::Configure(settings) => {
                let channel_changed = settings.channel != self.snapshot.state.settings.channel;
                self.snapshot.state.settings = settings.clone();
                if channel_changed {
                    let global = self.snapshot.state.settings.channel;
                    for (id, cache) in &mut self.snapshot.state.releases {
                        let ch = self
                            .snapshot
                            .state
                            .preferences
                            .get(id)
                            .and_then(|p| p.channel)
                            .unwrap_or(global);
                        cache.set_channel(ch);
                    }
                }
            }
            _ => {}
        }
        if let Err(e) = self.worker.send(command) {
            self.toast = Some((e.to_string(), true, Instant::now()));
        }
    }
    fn external(&mut self, url: String) {
        std::thread::spawn(move || {
            let _ = open::that(url);
        });
    }
    fn poll(&mut self, ctx: &Context) {
        let wake = self.snapshot.root.join("bring-to-front");
        if wake.is_file() {
            let _ = std::fs::remove_file(wake);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::State(snapshot) => {
                    self.existing_recent = recent_file_set(&snapshot);
                    self.snapshot = *snapshot;
                }
                Event::LocalApps(apps) => self.workflows.local = apps,
                Event::Entrypoints { app_id, paths } => {
                    self.workflows.entrypoint_choices = Some((app_id, paths));
                    self.page = Page::Queue;
                }
                Event::Storage(report) => {
                    self.selected_cache = report
                        .downloads
                        .iter()
                        .filter(|entry| !entry.protected)
                        .map(|entry| entry.name.clone())
                        .collect();
                    self.storage_report = Some(report);
                }
                Event::Progress(progress) => self.progress = Some(progress),
                Event::TaskFinished { id, result } => {
                    let mut cancelled_task = None;
                    if self.workflows.active == Some(id) {
                        self.workflows.active = None;
                        let intent = self.workflows.stop_intent.take();
                        self.workflows.tasks[id].status = match &result {
                            Ok(_) => workflows::TaskStatus::Done,
                            Err(error) if error.to_ascii_lowercase().contains("cancelled") => {
                                match intent {
                                    Some(workflows::StopIntent::Pause) => {
                                        workflows::TaskStatus::Paused
                                    }
                                    _ => {
                                        cancelled_task =
                                            Some(self.workflows.tasks[id].journal_id.clone());
                                        workflows::TaskStatus::Cancelled
                                    }
                                }
                            }
                            Err(error) => workflows::TaskStatus::Failed(error.clone()),
                        };
                    }
                    self.persist_queue();
                    if self.workflows.journal_error.is_none()
                        && let Some(task_id) = cancelled_task
                    {
                        self.discard_task_cache(&task_id);
                    }
                    self.progress = None;
                    self.toast = Some(match result {
                        Ok(message) => (message, false, Instant::now()),
                        Err(message) => (message, true, Instant::now()),
                    });
                }
                Event::Finished(result) => {
                    self.progress = None;
                    self.toast = Some(match result {
                        Ok(message) => (message, false, Instant::now()),
                        Err(message) => (message, true, Instant::now()),
                    });
                }
            }
        }
        self.dispatch_task();
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        for file in dropped {
            let path = file.path();
            if path.is_file() {
                self.workflows.projects.push(path.to_path_buf());
            }
        }
        let theme_key = (self.snapshot.state.settings.theme, ctx.system_theme());
        if self.applied_theme != Some(theme_key) {
            self.palette = theme::apply(ctx, self.snapshot.state.settings.theme);
            self.applied_theme = Some(theme_key);
        }
        if !self.tray_attempted {
            self.tray_attempted = true;
            match Tray::new(ctx) {
                Ok(tray) => self.tray = Some(tray),
                Err(_) => self.tray = None,
            }
            if self.initially_hidden && self.tray.is_some() && self.can_hide_window {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }
        if let Some(action) = self.tray.as_ref().and_then(Tray::poll) {
            match action {
                TrayAction::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayAction::Refresh => self.command(Command::Refresh),
                TrayAction::Quit => {
                    self.quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if self
            .exit_after
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        window::handle_close(
            ctx,
            !self.quit
                && self.can_hide_window
                && self.tray.is_some()
                && self.snapshot.state.settings.background,
        );
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() > Duration::from_secs(8))
        {
            self.toast = None;
        }
        let mut next_repaint = Duration::from_secs(2);
        if let Some((_, _, at)) = &self.toast {
            next_repaint = next_repaint.min(Duration::from_secs(8).saturating_sub(at.elapsed()));
        }
        if let Some(deadline) = self.exit_after {
            next_repaint = next_repaint.min(deadline.saturating_duration_since(Instant::now()));
        }
        ctx.request_repaint_after(next_repaint);
    }
    fn restart_update(&mut self, ctx: &Context) {
        let Some(prepared) = self.snapshot.state.launcher_update.as_ref() else {
            return;
        };
        let trust = self
            .snapshot
            .state
            .settings
            .update_public_key
            .as_ref()
            .map(|key| self_update::UpdateTrust::Signed {
                public_key: key.clone(),
            })
            .or_else(|| {
                self.snapshot
                    .state
                    .settings
                    .github_update_feed
                    .then_some(self_update::UpdateTrust::Github)
            });
        let Some(trust) = trust else {
            self.toast = Some((
                "No trusted update source is configured.".into(),
                true,
                Instant::now(),
            ));
            return;
        };
        let result = std::env::current_exe()
            .map_err(craftlauncher_core::Error::from)
            .and_then(|exe| {
                self_update::start_helper(&self.snapshot.root, prepared, &exe, &trust, 30)
            });
        match result {
            Ok(()) => {
                self.quit = true;
                self.worker.cancel();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.toast = Some((e.to_string(), true, Instant::now())),
        }
    }
}
impl eframe::App for Launcher {
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
    }
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.palette_shortcut(&ctx);
        self.sidebar(root);
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(self.palette.background)
                    .inner_margin(egui::Margin::symmetric(22, 10)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    if self.worker.busy() {
                        ui.spinner();
                        ui.label(
                            RichText::new(
                                self.progress
                                    .as_ref()
                                    .map(|p| {
                                        format!(
                                            "{} · {} {:.0}%",
                                            self.snapshot
                                                .products
                                                .iter()
                                                .find(|a| a.id == p.app_id)
                                                .map(|a| a.name.as_str())
                                                .unwrap_or("Launcher"),
                                            p.phase,
                                            p.percent
                                        )
                                    })
                                    .unwrap_or_else(|| "Checking releases…".into()),
                            )
                            .size(11.0)
                            .color(self.palette.muted),
                        );
                        if let Some(detail) = self.progress.as_ref().and_then(transfer_details) {
                            ui.label(RichText::new(detail).size(10.0).color(self.palette.muted));
                        }
                        if ui.small_button("Cancel").clicked() {
                            self.stop_active(workflows::StopIntent::Cancel);
                        }
                    } else {
                        ui.label(RichText::new("●").color(self.palette.accent).size(9.0));
                        ui.label(
                            RichText::new(format!(
                                "{} / {}",
                                self.snapshot.platform, self.snapshot.arch
                            ))
                            .monospace()
                            .size(10.0)
                            .color(self.palette.muted),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "CraftLauncher {}",
                                self_update::build_version()
                            ))
                            .monospace()
                            .size(10.0)
                            .color(self.palette.faint),
                        );
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(self.palette.background)
                    .inner_margin(egui::Margin::symmetric(30, 22)),
            )
            .show(root, |ui| {
                self.topbar(ui);
                if let Some((message, error, _)) = self.toast.clone() {
                    egui::Frame::new()
                        .fill(self.palette.raised)
                        .stroke(egui::Stroke::new(
                            1.0,
                            if error {
                                self.palette.danger
                            } else {
                                self.palette.line
                            },
                        ))
                        .inner_margin(12)
                        .corner_radius(5)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let message_width = (ui.available_width() - 52.0).max(100.0);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(message_width, widgets::CONTROL_HEIGHT),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.set_min_width(message_width);
                                        ui.label(RichText::new(&message).size(13.0).color(
                                            if error {
                                                self.palette.danger
                                            } else {
                                                self.palette.ink
                                            },
                                        ));
                                    },
                                );
                                if widgets::button(
                                    ui,
                                    self.palette,
                                    "×",
                                    widgets::ButtonKind::Quiet,
                                    true,
                                    40.0,
                                )
                                .on_hover_text("Dismiss message")
                                .clicked()
                                {
                                    self.toast = None;
                                }
                            });
                        });
                    ui.add_space(10.0);
                }
                egui::ScrollArea::vertical()
                    .id_salt("main-scroll")
                    .show(ui, |ui| match self.page {
                        Page::AllApps | Page::MyApps => self.apps(ui),
                        Page::Updates => self.updates(ui, &ctx),
                        Page::Activity => self.activity(ui),
                        Page::Queue => self.queue_page(ui),
                        Page::Discover => self.discovery_page(ui),
                        Page::Settings => self.settings(ui),
                    });
            });
        self.details(&ctx);
        self.remove_dialog(&ctx);
        self.workflow_dialogs(&ctx);
        self.command_palette(&ctx);
        if self.first_frame {
            self.first_frame = false;
            ctx.request_repaint();
        } else if let Some(health) = self.health.take() {
            if health.expected_version == self_update::build_version() {
                if let Err(e) = craftlauncher_core::persistence::atomic_write(
                    &health.path,
                    health.expected_version.as_bytes(),
                ) {
                    self.toast = Some((
                        format!("Update health check failed: {e}"),
                        true,
                        Instant::now(),
                    ));
                }
            } else {
                self.quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}
