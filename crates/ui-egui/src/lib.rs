mod assets;
mod theme;
mod tray;
mod views;
mod widgets;
mod window;

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
    Settings,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTab {
    General,
    Changelog,
    About,
}
pub struct HealthCheck {
    pub path: PathBuf,
    pub expected_version: String,
}
pub struct Launcher {
    worker: Worker,
    snapshot: Snapshot,
    assets: Assets,
    palette: Palette,
    page: Page,
    settings_tab: SettingsTab,
    search: String,
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
    exit_after: Option<Instant>,
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
        let worker = Worker::new(manager, launcher);
        Self {
            worker,
            snapshot,
            assets: Assets::load(&cc.egui_ctx),
            palette,
            page,
            settings_tab: SettingsTab::General,
            search: String::new(),
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
            exit_after: exit_after.map(|duration| Instant::now() + duration),
        }
    }
    fn command(&mut self, command: Command) {
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
                Event::State(snapshot) => self.snapshot = *snapshot,
                Event::Progress(progress) => self.progress = Some(progress),
                Event::Finished(result) => {
                    self.progress = None;
                    self.toast = Some(match result {
                        Ok(message) => (message, false, Instant::now()),
                        Err(message) => (message, true, Instant::now()),
                    });
                }
            }
        }
        self.palette = theme::apply(ctx, self.snapshot.state.settings.theme);
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
        ctx.request_repaint_after(Duration::from_millis(500));
    }
    fn restart_update(&mut self, ctx: &Context) {
        let Some(prepared) = self.snapshot.state.launcher_update.as_ref() else {
            return;
        };
        let key = self
            .snapshot
            .state
            .settings
            .update_public_key
            .as_deref()
            .unwrap_or("");
        let result = std::env::current_exe()
            .map_err(craftlauncher_core::Error::from)
            .and_then(|exe| {
                self_update::start_helper(&self.snapshot.root, prepared, &exe, key, 30)
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
                                            craftlauncher_core::app(&p.app_id)
                                                .map(|a| a.name)
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
                        if ui.small_button("Cancel").clicked() {
                            self.worker.cancel();
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
                        Page::Settings => self.settings(ui),
                    });
            });
        self.details(&ctx);
        self.remove_dialog(&ctx);
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
