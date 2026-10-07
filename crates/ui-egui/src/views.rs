use crate::{
    Launcher, Page, SettingsTab, theme,
    widgets::{self, ButtonKind, CONTROL_HEIGHT},
};
use craftlauncher_core::{
    App, AppPreferences, CATALOG, Channel, Command, Theme, model::is_newer, releases::asset_for,
    self_update,
};
use egui::{Align, Color32, Context, Frame, Layout, RichText, Stroke, Ui, Vec2};
use std::time::Instant;

impl Launcher {
    pub(crate) fn sidebar(&mut self, root: &mut Ui) {
        egui::Panel::left("sidebar")
            .exact_size(200.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(self.palette.sunken)
                    .inner_margin(egui::Margin::symmetric(18, 24)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    self.brand(ui, 30.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new("CRAFT").size(16.0).strong());
                        ui.label(
                            RichText::new("LAUNCHER")
                                .size(10.0)
                                .color(self.palette.muted),
                        );
                    });
                });
                ui.add_space(44.0);
                ui.label(
                    RichText::new("YOUR WORKSPACE")
                        .monospace()
                        .size(9.0)
                        .color(self.palette.faint),
                );
                ui.add_space(10.0);
                let installed = self.snapshot.state.installations.len();
                let updates = self.update_count();
                for (page, name, count) in [
                    (Page::MyApps, "My apps", Some(installed)),
                    (Page::AllApps, "All apps", None),
                    (Page::Updates, "Updates", Some(updates)),
                    (Page::Activity, "Activity", None),
                ] {
                    if widgets::nav_button(ui, self.palette, page, name, self.page == page, count)
                        .clicked()
                    {
                        self.page = page;
                        self.search.clear();
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(2.0);
                    if widgets::nav_button(
                        ui,
                        self.palette,
                        Page::Settings,
                        "Settings",
                        self.page == Page::Settings,
                        None,
                    )
                    .clicked()
                    {
                        self.page = Page::Settings;
                    }
                    ui.add_space(20.0);
                    ui.separator();
                    ui.add_space(18.0);
                    ui.label(
                        RichText::new("Made for making.")
                            .font(theme::italic(23.0))
                            .color(self.palette.muted),
                    );
                    ui.label(
                        RichText::new("Open tools. Endless possibilities.")
                            .size(10.0)
                            .color(self.palette.faint),
                    );
                    ui.add_space(18.0);
                    if ui
                        .link(
                            RichText::new("Meet the Craft family ↗")
                                .size(11.0)
                                .color(self.palette.muted),
                        )
                        .clicked()
                    {
                        self.external("https://getartcraft.com/apps".into());
                    }
                });
            });
    }
    pub(crate) fn topbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(match self.page {
                    Page::MyApps => "WORKSPACE / MY APPS",
                    Page::AllApps => "DISCOVER / CRAFT APPS",
                    Page::Updates => "WORKSPACE / UPDATES",
                    Page::Activity => "WORKSPACE / ACTIVITY",
                    Page::Settings => "WORKSPACE / SETTINGS",
                })
                .monospace()
                .size(10.0)
                .color(self.palette.faint),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !self.worker.busy(),
                        egui::Button::new(RichText::new("↻  Check updates").size(12.0))
                            .min_size(Vec2::new(142.0, CONTROL_HEIGHT))
                            .corner_radius(6),
                    )
                    .clicked()
                {
                    self.command(Command::Refresh);
                }
            });
        });
        ui.add_space(12.0);
    }
    fn brand(&self, ui: &mut Ui, size: f32) {
        if let Some(icon) = self.assets.icon("launcher") {
            ui.add(egui::Image::new(icon).fit_to_exact_size(Vec2::splat(size)));
        }
    }
    fn app_icon(&self, ui: &mut Ui, id: &str, size: f32) {
        if let Some(icon) = self.assets.icon(id) {
            ui.add(egui::Image::new(icon).fit_to_exact_size(Vec2::splat(size)));
        } else {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect.shrink(2.0), 12.0, self.palette.accent);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "A",
                egui::FontId::proportional(size * 0.65),
                Color32::WHITE,
            );
        }
    }
    fn primary(&self, ui: &mut Ui, text: &str, enabled: bool) -> bool {
        widgets::button(ui, self.palette, text, ButtonKind::Primary, enabled, 104.0).clicked()
    }
    fn headline(&self, ui: &mut Ui, title: &str, description: &str) {
        ui.label(RichText::new(title).font(theme::display(43.0)));
        ui.add_space(6.0);
        ui.label(
            RichText::new(description)
                .size(13.0)
                .color(self.palette.muted),
        );
        ui.add_space(28.0);
    }
    pub(crate) fn apps(&mut self, ui: &mut Ui) {
        if self.page == Page::AllApps {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new("A place for").font(theme::display(40.0)));
                    ui.label(
                        RichText::new("every idea.")
                            .font(theme::italic(40.0))
                            .color(self.palette.accent),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for id in ["effectcraft", "filmcraft", "vectorcraft", "photocraft"] {
                        self.app_icon(ui, id, 46.0);
                    }
                });
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Your open-source creative toolkit. Find your tools, make them yours.",
                )
                .size(13.0)
                .color(self.palette.muted),
            );
            ui.add_space(18.0);
        } else {
            self.headline(
                ui,
                "Ready when you are.",
                "Your installed tools, together in one place.",
            );
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let filter_width = 186.0;
            let search_width = (ui.available_width() - filter_width - 12.0).max(180.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search your creative tools…   Ctrl / ⌘ K")
                    .desired_width(search_width - 28.0)
                    .font(egui::FontId::proportional(13.0))
                    .min_size(Vec2::new(search_width, CONTROL_HEIGHT))
                    .margin(egui::Margin::symmetric(14, 10)),
            );
            if response.has_focus() {
                ui.painter().rect_stroke(
                    response.rect,
                    6,
                    Stroke::new(1.5, self.palette.accent),
                    egui::StrokeKind::Inside,
                );
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
                response.request_focus();
            }
            egui::ComboBox::from_id_salt("discipline")
                .selected_text(
                    [
                        "All disciplines",
                        "Image & design",
                        "Video & motion",
                        "Documents",
                    ][self.discipline],
                )
                .width(filter_width)
                .show_ui(ui, |ui| {
                    for (index, label) in [
                        "All disciplines",
                        "Image & design",
                        "Video & motion",
                        "Documents",
                    ]
                    .iter()
                    .enumerate()
                    {
                        ui.selectable_value(&mut self.discipline, index, *label);
                    }
                });
        });
        ui.add_space(14.0);
        let search = self.search.to_lowercase();
        let mut apps: Vec<App> = CATALOG
            .into_iter()
            .filter(|app| {
                let installed = self.snapshot.state.installations.contains_key(app.id);
                let discipline = match self.discipline {
                    1 => ["artcraft", "photocraft", "vectorcraft", "lightcraft"].contains(&app.id),
                    2 => ["filmcraft", "effectcraft"].contains(&app.id),
                    3 => ["printcraft", "designcraft"].contains(&app.id),
                    _ => true,
                };
                (self.page == Page::AllApps || installed)
                    && discipline
                    && format!("{} {} {}", app.name, app.category, app.tags.join(" "))
                        .to_lowercase()
                        .contains(&search)
            })
            .collect();
        apps.sort_by_key(|app| !self.preferences(app.id).favorite);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if self.page == Page::AllApps {
                    "THE CRAFT FAMILY"
                } else {
                    "YOUR TOOLS"
                })
                .monospace()
                .size(10.0)
                .color(self.palette.muted),
            );
            ui.label(
                RichText::new(format!("/ {:02}", apps.len()))
                    .monospace()
                    .size(10.0)
                    .color(self.palette.faint),
            );
        });
        ui.add_space(12.0);
        if apps.is_empty() {
            Frame::new()
                .fill(self.palette.raised)
                .stroke(Stroke::new(1.0, self.palette.line))
                .inner_margin(30)
                .corner_radius(6)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        RichText::new(if self.search.is_empty() {
                            "Your next great idea starts here."
                        } else {
                            "No tools found."
                        })
                        .font(theme::display(29.0)),
                    );
                    ui.label(
                        RichText::new(if self.search.is_empty() {
                            "Install a Craft app or link one you already have."
                        } else {
                            "Try another name or discipline."
                        })
                        .color(self.palette.muted),
                    );
                    if ui.button("Explore all apps →").clicked() {
                        self.page = Page::AllApps;
                        self.search.clear();
                        self.discipline = 0;
                    }
                });
            return;
        }
        self.batch_download_controls(ui, &apps);
        let columns = if ui.available_width() >= 620.0 { 2 } else { 1 };
        let width = (ui.available_width() - (columns - 1) as f32 * 14.0) / columns as f32;
        for row in apps.chunks(columns) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                for app in row {
                    ui.allocate_ui_with_layout(
                        Vec2::new(width, 182.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.scope_builder(
                                egui::UiBuilder::new().id(egui::Id::new(("app-card", app.id))),
                                |ui| self.card(ui, *app, width),
                            );
                        },
                    );
                }
            });
            ui.add_space(4.0);
        }
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("OPEN SOURCE")
                    .monospace()
                    .size(9.0)
                    .color(self.palette.faint),
            );
            ui.label(
                RichText::new("Free to use. Yours to keep.")
                    .font(theme::italic(19.0))
                    .color(self.palette.muted),
            );
        });
    }
    fn choose_download_folder(&mut self) -> Option<std::path::PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title("Choose where to save app packages");
        if let Some(folder) = self
            .snapshot
            .state
            .settings
            .download_folder
            .clone()
            .or_else(craftlauncher_core::Manager::default_download_folder)
            && folder.is_dir()
        {
            dialog = dialog.set_directory(folder);
        }
        dialog.pick_folder()
    }
    fn platform_name(&self) -> &str {
        match self.snapshot.platform.as_str() {
            "linux" => "Linux",
            "windows" => "Windows",
            "macos" => "macOS",
            "freebsd" => "FreeBSD",
            other => other,
        }
    }
    fn batch_download_controls(&mut self, ui: &mut Ui, apps: &[App]) {
        let eligible: Vec<String> = apps
            .iter()
            .filter(|app| self.available(app.id))
            .map(|app| app.id.to_owned())
            .collect();
        self.selected_apps = self
            .selected_apps
            .iter()
            .filter(|id| self.available(id))
            .cloned()
            .collect();
        let frame = Frame::new()
            .fill(self.palette.raised)
            .stroke(Stroke::new(1.0, self.palette.line))
            .corner_radius(8)
            .inner_margin(16);
        let width = ui.available_width() - frame.total_margin().sum().x;
        frame.show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
            ui.horizontal(|ui| {
                let mut all = !eligible.is_empty()
                    && eligible.iter().all(|id| self.selected_apps.contains(id));
                let partial = !all && eligible.iter().any(|id| self.selected_apps.contains(id));
                if widgets::checkbox(
                    ui,
                    self.palette,
                    &mut all,
                    "Select available",
                    !eligible.is_empty() && !self.worker.busy(),
                    partial,
                )
                .on_hover_text("Select all compatible packages in this view")
                .changed()
                {
                    if all {
                        self.selected_apps.extend(eligible.iter().cloned());
                    } else {
                        for id in &eligible {
                            self.selected_apps.remove(id);
                        }
                    }
                }
                if widgets::button(
                    ui,
                    self.palette,
                    "Clear",
                    ButtonKind::Secondary,
                    !self.worker.busy() && !self.selected_apps.is_empty(),
                    64.0,
                )
                .clicked()
                {
                    self.selected_apps.clear();
                }
                if width > 640.0 {
                    ui.label(
                        RichText::new(format!("{} selected", self.selected_apps.len()))
                            .size(12.0)
                            .color(self.palette.muted),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let text = format!("Download selected ({})", self.selected_apps.len());
                    if widgets::button(
                        ui,
                        self.palette,
                        &text,
                        ButtonKind::Primary,
                        !self.worker.busy() && !self.selected_apps.is_empty(),
                        196.0,
                    )
                    .clicked()
                    {
                        let folder = self
                            .snapshot
                            .state
                            .settings
                            .download_folder
                            .clone()
                            .or_else(|| self.choose_download_folder());
                        if let Some(folder) = folder {
                            self.command(Command::DownloadSelected {
                                ids: self.selected_apps.iter().cloned().collect(),
                                folder,
                            });
                        }
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if widgets::button(
                    ui,
                    self.palette,
                    "Save folder…",
                    ButtonKind::Secondary,
                    !self.worker.busy(),
                    128.0,
                )
                .clicked()
                    && let Some(folder) = self.choose_download_folder()
                {
                    let mut settings = self.snapshot.state.settings.clone();
                    settings.download_folder = Some(folder);
                    self.command(Command::Configure(settings));
                }
                let path = self
                    .snapshot
                    .state
                    .settings
                    .download_folder
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Choose a destination folder".into());
                let path_width = (ui.available_width() - 144.0).max(80.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(path_width, CONTROL_HEIGHT),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&path).size(12.0).color(self.palette.muted),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&path);
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} · {}", self.platform_name(), self.snapshot.arch))
                            .monospace()
                            .size(11.0)
                            .color(self.palette.muted),
                    )
                    .on_hover_text("Operating system and CPU architecture detected automatically");
                });
            });
        });
        ui.add_space(14.0);
    }
    fn preferences(&self, id: &str) -> AppPreferences {
        self.snapshot
            .state
            .preferences
            .get(id)
            .cloned()
            .unwrap_or_default()
    }
    fn release(&self, id: &str) -> Option<&craftlauncher_core::Release> {
        let pref = self.preferences(id);
        let channel = pref.channel.unwrap_or(self.snapshot.state.settings.channel);
        let cache = self
            .snapshot
            .state
            .releases
            .get(id)
            .filter(|c| c.channel == channel)?;
        if let Some(pin) = pref.pinned_version {
            cache
                .versions
                .iter()
                .find(|r| r.tag_name == pin)
                .or_else(|| cache.release.as_ref().filter(|r| r.tag_name == pin))
        } else {
            cache.release.as_ref()
        }
    }
    fn available(&self, id: &str) -> bool {
        self.release(id)
            .and_then(|release| asset_for(release, &self.snapshot.platform, &self.snapshot.arch))
            .is_some()
    }
    fn update_count(&self) -> usize {
        self.snapshot.state.pending.len()
            + self
                .snapshot
                .state
                .installations
                .iter()
                .filter(|(id, installed)| {
                    installed.managed
                        && !self.snapshot.state.pending.contains_key(*id)
                        && self.preferences(id).pinned_version.is_none()
                        && self.available(id)
                        && self
                            .release(id)
                            .is_some_and(|r| is_newer(&r.tag_name, &installed.version))
                })
                .count()
            + usize::from(self.snapshot.state.launcher_update.is_some())
    }
    fn card(&mut self, ui: &mut Ui, app: App, width: f32) {
        let installed = self.snapshot.state.installations.get(app.id).cloned();
        let pending = self.snapshot.state.pending.get(app.id).cloned();
        let pref = self.preferences(app.id);
        let compatible = self.available(app.id);
        let selected = self.selected_apps.contains(app.id);
        let web_only = app.id == "artcraft" && !compatible && installed.is_none();
        let version = installed
            .as_ref()
            .map(|i| i.version.clone())
            .or_else(|| self.release(app.id).map(|r| r.tag_name.clone()));
        let frame = Frame::new()
            .fill(if selected {
                self.palette.selected
            } else {
                self.palette.raised
            })
            .stroke(Stroke::new(
                1.0,
                if selected {
                    self.palette.accent
                } else {
                    self.palette.line
                },
            ))
            .corner_radius(8)
            .inner_margin(18);
        let inner_width = width - frame.total_margin().sum().x;
        frame.show(ui, |ui| {
            ui.set_width(inner_width);
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
            ui.horizontal(|ui| {
                self.app_icon(ui, app.id, 48.0);
                let controls = if compatible { 90.0 } else { 40.0 };
                let title_width = (ui.available_width() - controls - 10.0).max(100.0);
                ui.allocate_ui_with_layout(Vec2::new(title_width, 48.0), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    if ui.add(egui::Label::new(RichText::new(app.name).size(20.0).strong()).truncate().sense(egui::Sense::click())).clicked() {
                        self.detail = Some(app.id.into());
                        self.selected_version = self.release(app.id).map(|r| r.tag_name.clone()).unwrap_or_default();
                    }
                    ui.label(RichText::new(app.category.to_uppercase()).monospace().size(9.0).color(self.palette.muted));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if compatible {
                        let mut checked = selected;
                        if widgets::checkbox(ui, self.palette, &mut checked, "", !self.worker.busy(), false).on_hover_text(format!("Select {} for download", app.name)).changed() {
                            if checked { self.selected_apps.insert(app.id.into()); } else { self.selected_apps.remove(app.id); }
                        }
                    }
                    if widgets::button(ui, self.palette, if pref.favorite { "★" } else { "☆" }, ButtonKind::Quiet, !self.worker.busy(), 40.0).on_hover_text("Favorite").clicked() {
                        let mut p = pref.clone(); p.favorite = !p.favorite;
                        self.command(Command::Preferences(app.id.into(), p));
                    }
                });
            });
            ui.allocate_ui_with_layout(Vec2::new(inner_width, 34.0), Layout::top_down(Align::Min), |ui| {
                ui.set_min_height(34.0);
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(RichText::new(app.description).size(12.0).color(self.palette.muted));
                if compatible && self.release(app.id).and_then(|release| asset_for(release, &self.snapshot.platform, &self.snapshot.arch)).is_some_and(|asset| asset.name.ends_with(".AppImage")) {
                    ui.label(RichText::new("AppImage · extracted native installation").size(11.0).color(self.palette.muted));
                }
                if web_only && self.release(app.id).is_some() {
                    ui.label(RichText::new(format!("No {} desktop package in this release.", self.platform_name())).size(11.0).color(self.palette.muted));
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let metadata_width = (ui.available_width() - 170.0).max(110.0);
                ui.allocate_ui_with_layout(Vec2::new(metadata_width, CONTROL_HEIGHT), Layout::top_down(Align::Min), |ui| {
                    ui.set_min_height(CONTROL_HEIGHT);
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let status = if pending.as_ref().is_some_and(|pending| pending.automatic && !self.snapshot.state.settings.auto_app_update) { "AUTO-UPDATE PAUSED" } else if pending.is_some() { "UPDATE PREPARED" }
                        else if installed.as_ref().is_some_and(|i| !i.managed) { "LINKED INSTALLATION" }
                        else if installed.is_some() { "INSTALLED" }
                        else if web_only { "WEB STUDIO" }
                        else if !compatible && self.release(app.id).is_some() { "NO COMPATIBLE PACKAGE" }
                        else { app.stage };
                    ui.label(RichText::new(status).monospace().size(9.0).color(if installed.is_some() || pending.is_some() { self.palette.accent } else { self.palette.muted }));
                    ui.add(egui::Label::new(RichText::new(version.clone().unwrap_or_else(|| if self.worker.busy() { "Checking releases…".into() } else { "No release loaded".into() })).monospace().size(10.0).color(self.palette.muted)).truncate());
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.menu_button("···", |ui| self.app_menu(ui, app));
                    if installed.is_some() {
                        if self.primary(ui, "Open", !self.worker.busy()) { self.command(Command::Launch(app.id.into())); }
                    } else if compatible {
                        if self.primary(ui, "Install", !self.worker.busy() && pending.is_none()) { self.command(Command::Install { id: app.id.into(), version: None }); }
                    } else if web_only {
                        if widgets::button(ui, self.palette, "Open web ↗", ButtonKind::Secondary, true, 112.0).on_hover_text("This release has no compatible desktop package. Open ArtCraft's hosted web studio.").clicked() { self.external("https://app.getartcraft.com".into()); }
                    } else {
                        widgets::button(ui, self.palette, "Unavailable", ButtonKind::Secondary, false, 112.0).on_hover_text("No compatible published package is loaded. Check releases, or link an existing app.");
                    }
                });
            });
            if self.snapshot.state.releases.get(app.id).and_then(|c| c.error.as_ref()).is_some() {
                ui.label(RichText::new("Release check unavailable · cached data shown").size(10.0).color(self.palette.danger));
            }
        });
    }
    fn app_menu(&mut self, ui: &mut Ui, app: App) {
        if ui.button("App details").clicked() {
            self.detail = Some(app.id.into());
            self.selected_version = self
                .release(app.id)
                .map(|r| r.tag_name.clone())
                .unwrap_or_default();
            ui.close();
        }
        if ui.button("View source ↗").clicked() {
            self.external(format!("https://github.com/storytold/{}", app.id));
            ui.close();
        }
        if ui.button("All releases ↗").clicked() {
            self.external(format!("https://github.com/storytold/{}/releases", app.id));
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                !self.worker.busy()
                    && !self
                        .snapshot
                        .state
                        .installations
                        .get(app.id)
                        .is_some_and(|i| i.managed),
                egui::Button::new("Link existing installation…"),
            )
            .clicked()
        {
            self.pick_app(app.id);
            ui.close();
        }
        if self.snapshot.state.installations.contains_key(app.id) {
            if ui
                .add_enabled(!self.worker.busy(), egui::Button::new("Show app folder"))
                .clicked()
            {
                self.command(Command::Reveal(Some(app.id.into())));
                ui.close();
            }
            if ui
                .add_enabled(
                    !self.worker.busy()
                        && self
                            .snapshot
                            .state
                            .installations
                            .get(app.id)
                            .is_some_and(|i| i.previous.is_some()),
                    egui::Button::new("Restore previous version"),
                )
                .clicked()
            {
                self.command(Command::Rollback(app.id.into()));
                ui.close();
            }
            if ui
                .add_enabled(
                    !self.worker.busy(),
                    egui::Button::new(
                        RichText::new("Uninstall / unlink…").color(self.palette.danger),
                    ),
                )
                .clicked()
            {
                self.confirm_remove = Some(app.id.into());
                ui.close();
            }
        }
    }
    fn pick_app(&mut self, id: &str) {
        let path = if self.snapshot.platform == "macos" {
            rfd::FileDialog::new()
                .set_title("Choose an application bundle")
                .add_filter("Application", &["app"])
                .pick_folder()
        } else {
            rfd::FileDialog::new()
                .set_title("Choose the app executable")
                .pick_file()
        };
        if let Some(path) = path {
            self.command(Command::Link(id.into(), path));
        }
    }
    pub(crate) fn details(&mut self, ctx: &Context) {
        let Some(id) = self.detail.clone() else {
            return;
        };
        let Ok(app) = craftlauncher_core::app(&id) else {
            self.detail = None;
            return;
        };
        let mut open = true;
        egui::Window::new(app.name).id(egui::Id::new("details")).open(&mut open).collapsible(false).resizable(false).default_width(660.0).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-130.0).max(300.0)).show(ui, |ui| {
                if let Some(hero) = self.assets.hero(app.id) { ui.add(egui::Image::new(hero).fit_to_exact_size(Vec2::new(ui.available_width(), ui.available_width()*0.50)).corner_radius(4)); ui.add_space(12.0); }
                ui.horizontal(|ui| { self.app_icon(ui, app.id, 54.0); ui.vertical(|ui| { ui.label(RichText::new(app.name).font(theme::display(34.0))); ui.label(RichText::new(app.category).color(self.palette.muted)); }); });
                ui.add_space(10.0); ui.label(app.detail);
                ui.horizontal_wrapped(|ui| { for tag in app.tags { ui.label(RichText::new(format!(" {tag} ")).size(10.0).background_color(self.palette.sunken).color(self.palette.muted)); } });
                ui.add_space(12.0); ui.separator();
                let mut preferences = self.preferences(app.id); let before = preferences.clone();
                let mut changed = false;
                ui.add_enabled_ui(!self.worker.busy(), |ui| {
                    changed |= widgets::checkbox(ui, self.palette, &mut preferences.auto_update, "Update automatically when the app is closed", true, false).changed();
                    egui::ComboBox::from_id_salt("app_channel").selected_text(match preferences.channel { None => "Use global channel", Some(Channel::Stable) => "Stable", Some(Channel::Preview) => "Preview" }).show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut preferences.channel, None, "Use global channel").changed();
                        changed |= ui.selectable_value(&mut preferences.channel, Some(Channel::Stable), "Stable").changed();
                        changed |= ui.selectable_value(&mut preferences.channel, Some(Channel::Preview), "Preview").changed();
                    });
                    let mut pinned = preferences.pinned_version.is_some();
                    if widgets::checkbox(ui, self.palette, &mut pinned, "Keep the current version", true, false).changed() {
                        preferences.pinned_version = if pinned { self.snapshot.state.installations.get(app.id).filter(|i| i.managed).map(|i| i.version.clone()).or_else(|| self.release(app.id).map(|r| r.tag_name.clone())) } else { None }; changed = true;
                    }
                });
                if changed && (before.auto_update != preferences.auto_update || before.channel != preferences.channel || before.pinned_version != preferences.pinned_version) { self.command(Command::Preferences(app.id.into(), preferences)); }
                ui.add_space(10.0);
                let versions = self.snapshot.state.releases.get(app.id).map(|r| r.versions.clone()).unwrap_or_default();
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("install_version").selected_text(if self.selected_version.is_empty() { "Choose a release" } else { &self.selected_version }).show_ui(ui, |ui| { for release in &versions { ui.selectable_value(&mut self.selected_version, release.tag_name.clone(), &release.tag_name); } });
                    let can_install = !self.worker.busy() && !self.selected_version.is_empty() && versions.iter().find(|r| r.tag_name == self.selected_version).and_then(|r| asset_for(r, &self.snapshot.platform, &self.snapshot.arch)).is_some() && !self.snapshot.state.installations.get(app.id).is_some_and(|i| !i.managed) && !self.snapshot.state.pending.contains_key(app.id);
                    if self.primary(ui, "Install version", can_install) { self.command(Command::Install { id: app.id.into(), version: Some(self.selected_version.clone()) }); }
                    if self.snapshot.state.installations.contains_key(app.id) && ui.add_enabled(!self.worker.busy(), egui::Button::new("Open app ↗")).clicked() { self.command(Command::Launch(app.id.into())); }
                });
                if !self.available(app.id) { ui.label(RichText::new(format!("No compatible published package for {} / {} is currently loaded.", self.snapshot.platform, self.snapshot.arch)).color(self.palette.muted)); }
                if let Some(error) = self.snapshot.state.releases.get(app.id).and_then(|r| r.error.as_ref()) { ui.label(RichText::new(error).color(self.palette.danger)); }
                ui.collapsing("Release notes", |ui| { let notes = self.release(app.id).and_then(|r| r.body.clone()).unwrap_or_else(|| "No release notes loaded.".into()); ui.label(notes); });
                ui.horizontal(|ui| {
                    if ui.link("Source ↗").clicked() { self.external(format!("https://github.com/storytold/{}", app.id)); }
                    if ui.link("Product website ↗").clicked() { self.external(if app.id == "artcraft" { "https://getartcraft.com".into() } else { format!("https://getartcraft.com/apps/{}", app.id) }); }
                    if app.id == "artcraft" && ui.link("Open web studio ↗").clicked() { self.external("https://app.getartcraft.com".into()); }
                    if let Some(asset) = self.release(app.id).and_then(|r| r.assets.iter().find(|a| a.name.contains("-web-") && a.name.ends_with(".zip"))).cloned()
                        && ui.link("Download WebAssembly build ↗").clicked() { self.external(asset.browser_download_url); }
                });
            });
        });
        if !open {
            self.detail = None;
        }
    }
    pub(crate) fn remove_dialog(&mut self, ctx: &Context) {
        let Some(id) = self.confirm_remove.clone() else {
            return;
        };
        let Ok(app) = craftlauncher_core::app(&id) else {
            self.confirm_remove = None;
            return;
        };
        let managed = self
            .snapshot
            .state
            .installations
            .get(&id)
            .is_some_and(|i| i.managed);
        egui::Window::new(if managed {
            "Uninstall app?"
        } else {
            "Unlink app?"
        })
        .id(egui::Id::new("remove"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!(
                "{} will be {}.",
                app.name,
                if managed {
                    "removed from this library"
                } else {
                    "unlinked from the launcher"
                }
            ));
            ui.label(if managed {
                "Your project files and app preferences will be kept."
            } else {
                "The external installation and all its files will be kept."
            });
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    self.confirm_remove = None;
                }
                if ui
                    .add_enabled(
                        !self.worker.busy(),
                        egui::Button::new(
                            RichText::new(if managed { "Uninstall" } else { "Unlink" })
                                .color(self.palette.danger),
                        ),
                    )
                    .clicked()
                {
                    self.command(Command::Remove(id.clone()));
                    self.confirm_remove = None;
                }
            });
        });
    }
    pub(crate) fn updates(&mut self, ui: &mut Ui, ctx: &Context) {
        self.headline(
            ui,
            "A little better, every day.",
            "New tools, improvements and fixes. Updates happen when your apps are closed.",
        );
        Frame::new()
            .fill(self.palette.raised)
            .stroke(Stroke::new(1.0, self.palette.line))
            .inner_margin(18)
            .corner_radius(6)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    self.brand(ui, 36.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("CraftLauncher").size(19.0));
                        ui.label(
                            RichText::new(format!(
                                "Current version {}",
                                self_update::build_version()
                            ))
                            .size(11.0)
                            .color(self.palette.muted),
                        );
                    });
                });
                ui.add_space(8.0);
                if let Some(prepared) = &self.snapshot.state.launcher_update {
                    ui.label(format!(
                        "Version {} is verified and ready.",
                        prepared.version
                    ));
                    if self.primary(ui, "Restart to update", !self.worker.busy()) {
                        self.restart_update(ctx);
                    }
                } else if self.snapshot.state.settings.update_feed.is_some() {
                    ui.label(
                        RichText::new("Local signed update feed configured.")
                            .color(self.palette.muted),
                    );
                    if ui
                        .add_enabled(
                            !self.worker.busy(),
                            egui::Button::new("Check launcher update"),
                        )
                        .clicked()
                    {
                        self.command(Command::CheckLauncher);
                    }
                } else {
                    ui.label(
                        RichText::new("Local build · no launcher update feed configured yet.")
                            .color(self.palette.muted),
                    );
                    if ui.button("Configure local updates").clicked() {
                        self.page = Page::Settings;
                    }
                }
            });
        ui.add_space(24.0);
        ui.label(
            RichText::new("APP UPDATES")
                .monospace()
                .size(10.0)
                .color(self.palette.muted),
        );
        ui.add_space(12.0);
        let apps: Vec<App> = CATALOG
            .into_iter()
            .filter(|app| {
                self.snapshot.state.pending.contains_key(app.id)
                    || self
                        .snapshot
                        .state
                        .installations
                        .get(app.id)
                        .is_some_and(|i| {
                            i.managed
                                && self.available(app.id)
                                && self
                                    .release(app.id)
                                    .is_some_and(|r| is_newer(&r.tag_name, &i.version))
                        })
            })
            .collect();
        if apps.is_empty() {
            ui.label(RichText::new("You're ready to create.").font(theme::display(31.0)));
            ui.label(
                RichText::new(
                    "No app updates are currently available. Check releases to refresh this list.",
                )
                .color(self.palette.muted),
            );
        }
        for app in apps {
            Frame::new()
                .fill(self.palette.raised)
                .inner_margin(16)
                .corner_radius(6)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        self.app_icon(ui, app.id, 45.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(app.name).size(18.0));
                            if let Some(pending) = self.snapshot.state.pending.get(app.id) {
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {}",
                                        pending.version.version,
                                        if pending.automatic
                                            && !self.snapshot.state.settings.auto_app_update
                                        {
                                            "automatic update paused"
                                        } else {
                                            "verified, waiting for app to close"
                                        }
                                    ))
                                    .size(11.0)
                                    .color(self.palette.accent),
                                );
                            } else {
                                let current = self
                                    .snapshot
                                    .state
                                    .installations
                                    .get(app.id)
                                    .map(|i| i.version.as_str())
                                    .unwrap_or("?");
                                let next = self
                                    .release(app.id)
                                    .map(|r| r.tag_name.as_str())
                                    .unwrap_or("?");
                                ui.label(
                                    RichText::new(format!("{current} → {next}"))
                                        .monospace()
                                        .size(11.0)
                                        .color(self.palette.muted),
                                );
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if self.snapshot.state.pending.contains_key(app.id) {
                                if ui
                                    .add_enabled(!self.worker.busy(), egui::Button::new("Discard"))
                                    .clicked()
                                {
                                    self.command(Command::Discard(app.id.into()));
                                }
                                if ui
                                    .add_enabled(
                                        !self.worker.busy(),
                                        egui::Button::new("Activate if closed"),
                                    )
                                    .clicked()
                                {
                                    self.command(Command::ActivatePending);
                                }
                            } else {
                                let pinned = self.preferences(app.id).pinned_version.is_some();
                                if self.primary(
                                    ui,
                                    if pinned { "Pinned" } else { "Update" },
                                    !self.worker.busy() && !pinned,
                                ) {
                                    self.command(Command::Install {
                                        id: app.id.into(),
                                        version: None,
                                    });
                                }
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
    }
    pub(crate) fn activity(&mut self, ui: &mut Ui) {
        self.headline(
            ui,
            "The story so far.",
            "Installations, updates and launches. A local history of your workspace.",
        );
        if self.snapshot.state.activity.is_empty() {
            ui.label(RichText::new("A fresh start.").font(theme::display(29.0)));
            ui.label("Your activity will appear here as you use the launcher.");
        }
        for event in self.snapshot.state.activity.clone() {
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(if event.error { "!" } else { "●" }).color(
                    if event.error {
                        self.palette.danger
                    } else {
                        self.palette.accent
                    },
                ));
                ui.vertical(|ui| {
                    ui.label(RichText::new(&event.message).color(if event.error {
                        self.palette.danger
                    } else {
                        self.palette.ink
                    }));
                    ui.label(
                        RichText::new(format!(
                            "{} · {}",
                            event.action,
                            event.at.replace('T', " ").trim_end_matches('Z')
                        ))
                        .monospace()
                        .size(10.0)
                        .color(self.palette.faint),
                    );
                });
            });
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);
        }
    }
    pub(crate) fn settings(&mut self, ui: &mut Ui) {
        self.headline(
            ui,
            "Make yourself at home.",
            "A few thoughtful settings for the way you create.",
        );
        ui.horizontal(|ui| {
            for (tab, label, width) in [
                (SettingsTab::General, "General", 110.0),
                (SettingsTab::Changelog, "Changelog", 130.0),
                (SettingsTab::About, "About & independence", 196.0),
            ] {
                let kind = if self.settings_tab == tab {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                };
                if widgets::button(ui, self.palette, label, kind, true, width).clicked() {
                    self.settings_tab = tab;
                }
            }
        });
        ui.add_space(18.0);
        ui.separator();
        ui.add_space(18.0);
        match self.settings_tab {
            SettingsTab::Changelog => {
                self.settings_changelog(ui);
                return;
            }
            SettingsTab::About => {
                self.settings_about(ui);
                return;
            }
            SettingsTab::General => {}
        }
        let mut settings = self.snapshot.state.settings.clone();
        let mut changed = false;
        ui.add_enabled_ui(!self.worker.busy(), |ui| {
            ui.label(RichText::new("APPEARANCE").monospace().size(10.0).color(self.palette.muted));
            ui.horizontal(|ui| { for (theme, label) in [(Theme::Light, "Light"), (Theme::Dark, "Dark"), (Theme::System, "System")] {
                if widgets::button(ui, self.palette, label, if settings.theme == theme { ButtonKind::Primary } else { ButtonKind::Secondary }, true, 76.0).clicked() && settings.theme != theme { settings.theme = theme; changed = true; }
            } });
            ui.add_space(18.0); ui.separator(); ui.add_space(18.0);
            ui.label(RichText::new("UPDATES").monospace().size(10.0).color(self.palette.muted));
            egui::ComboBox::from_id_salt("global_channel").selected_text(if settings.channel == Channel::Stable { "Stable releases" } else { "Preview releases" }).show_ui(ui, |ui| { changed |= ui.selectable_value(&mut settings.channel, Channel::Stable, "Stable releases").changed(); changed |= ui.selectable_value(&mut settings.channel, Channel::Preview, "Preview releases").changed(); });
            changed |= widgets::checkbox(ui, self.palette, &mut settings.check_on_startup, "Check for updates when the launcher starts", true, false).changed();
            changed |= widgets::checkbox(ui, self.palette, &mut settings.auto_app_update, "Automatically update installed apps", true, false).changed();
            changed |= widgets::checkbox(ui, self.palette, &mut settings.auto_launcher_update, "Prepare launcher updates automatically", true, false).changed();
            ui.label(RichText::new("Turn off automatic app updates to pause downloads and prepared automatic updates. Manual installs stay available. Per-app settings and version pins still apply.").size(11.0).color(self.palette.muted));
            ui.add_space(18.0); ui.separator(); ui.add_space(18.0);
            ui.label(RichText::new("DESKTOP").monospace().size(10.0).color(self.palette.muted));
            changed |= widgets::checkbox(ui, self.palette, &mut settings.background, "Keep running in the tray when the window closes", true, false).changed();
            changed |= widgets::checkbox(ui, self.palette, &mut settings.launch_at_login, "Start CraftLauncher when I sign in", true, false).changed();
            changed |= widgets::checkbox(ui, self.palette, &mut settings.notifications, "Notify me when an app update is ready", true, false).changed();
            if self.tray.is_none() { ui.label(RichText::new("No system tray is available in this session. Closing the window will quit the launcher.").size(11.0).color(self.palette.muted)); }
        });
        if changed {
            self.command(Command::Configure(settings));
        }
        ui.add_space(18.0);
        ui.separator();
        ui.add_space(18.0);
        ui.label(
            RichText::new("LIBRARY")
                .monospace()
                .size(10.0)
                .color(self.palette.muted),
        );
        ui.label(
            RichText::new(self.snapshot.root.display().to_string())
                .monospace()
                .size(11.0)
                .color(self.palette.muted),
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.worker.busy(),
                    egui::Button::new("Open library folder"),
                )
                .clicked()
            {
                self.command(Command::Reveal(None));
            }
            ui.label(
                RichText::new(
                    "Apps are installed for your user. Documents stay in their original locations.",
                )
                .size(11.0)
                .color(self.palette.muted),
            );
        });
        ui.add_space(18.0);
        ui.separator();
        ui.add_space(18.0);
        ui.collapsing("Local launcher update feed", |ui| {
            ui.label(RichText::new("Choose the folder containing signed launcher releases and its trusted public key.").size(12.0).color(self.palette.muted));
            ui.horizontal(|ui| { ui.add(egui::TextEdit::singleline(&mut self.feed_path).hint_text("Local release folder").desired_width(380.0)); if ui.button("Browse…").clicked() && let Some(path) = rfd::FileDialog::new().set_title("Choose local update feed").pick_folder() { self.feed_path = path.display().to_string(); } });
            ui.add(egui::TextEdit::singleline(&mut self.public_key).hint_text("Ed25519 public key (64 hexadecimal characters)").desired_width(530.0));
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.worker.busy(), egui::Button::new("Save update feed")).clicked() {
                    if !self.public_key.trim().is_empty() && (self.public_key.trim().len() != 64 || !self.public_key.trim().bytes().all(|b| b.is_ascii_hexdigit())) { self.toast = Some(("Enter a valid 64-character public key.".into(), true, Instant::now())); }
                    else if !self.feed_path.trim().is_empty() && self.public_key.trim().is_empty() { self.toast = Some(("A trusted public key is required for the update feed.".into(), true, Instant::now())); }
                    else { let mut settings = self.snapshot.state.settings.clone(); settings.update_feed = (!self.feed_path.trim().is_empty()).then(|| self.feed_path.trim().into()); settings.update_public_key = (!self.public_key.trim().is_empty()).then(|| self.public_key.trim().into()); self.command(Command::Configure(settings)); }
                }
                if ui.add_enabled(!self.worker.busy(), egui::Button::new("Check local update")).clicked() { self.command(Command::CheckLauncher); }
            });
        });
        ui.add_space(24.0);
        ui.label(
            RichText::new("PLATFORM SUPPORT")
                .monospace()
                .size(10.0)
                .color(self.palette.muted),
        );
        ui.label(RichText::new("Windows x86 / x64 / ARM64 · macOS Intel / Apple Silicon · Linux x86_64 / ARM64 · FreeBSD x86_64").size(12.0).color(self.palette.muted));
        ui.label(RichText::new("Availability depends on the packages published for each app. This is a local development build.").size(11.0).color(self.palette.faint));
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if ui.link("ArtCraft ↗").clicked() {
                self.external("https://getartcraft.com".into());
            }
            if ui.link("Source apps ↗").clicked() {
                self.external("https://github.com/storytold".into());
            }
        });
    }

    fn settings_changelog(&self, ui: &mut Ui) {
        ui.label(
            RichText::new("WHAT'S NEW IN CRAFTLAUNCHER")
                .monospace()
                .size(10.0)
                .color(self.palette.muted),
        );
        ui.add_space(8.0);
        for line in include_str!("../../../CHANGELOG.md").lines() {
            if line.starts_with("# ") {
                continue;
            }
            if let Some(title) = line.strip_prefix("## ") {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(title)
                        .font(theme::display(28.0))
                        .color(self.palette.ink),
                );
                ui.add_space(8.0);
            } else if let Some(title) = line.strip_prefix("### ") {
                ui.add_space(12.0);
                ui.label(RichText::new(title).size(14.0).strong());
            } else if let Some(item) = line.strip_prefix("- ") {
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new("•").color(self.palette.accent));
                    ui.add(egui::Label::new(RichText::new(item).size(13.0)).wrap());
                });
            } else if !line.trim().is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(line).size(12.0).color(self.palette.muted))
                        .wrap(),
                );
            }
        }
    }

    fn settings_about(&mut self, ui: &mut Ui) {
        Frame::new().fill(self.palette.raised).stroke(Stroke::new(1.0, self.palette.line))
            .inner_margin(20).corner_radius(6).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    self.brand(ui, 56.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("CraftLauncher").size(24.0).strong());
                        ui.label(RichText::new(format!("Version {} · Developed by Eric Vogt", self_update::build_version())).size(12.0).color(self.palette.muted));
                    });
                });
                ui.add_space(18.0);
                ui.label(RichText::new("Independent launcher. Separate projects.").font(theme::display(28.0)));
                ui.add_space(8.0);
                for text in [
                    "Eric Vogt develops and maintains CraftLauncher only. He does not develop or maintain ArtCraft or any of the applications offered here.",
                    "Neither Eric Vogt nor CraftLauncher is affiliated with, sponsored by, endorsed by, or otherwise connected to ArtCraft, Storytold, or the developers of those applications.",
                    "Each application is a separate third-party project. Its developers are responsible for its features, updates, support and license terms. Names, logos and artwork belong to their respective owners and identify the applications being managed.",
                ] {
                    ui.add(egui::Label::new(RichText::new(text).size(13.0).color(self.palette.muted)).wrap());
                    ui.add_space(10.0);
                }
                ui.horizontal(|ui| {
                    if widgets::button(ui, self.palette, "Launcher source ↗", ButtonKind::Secondary, true, 168.0).clicked() {
                        self.external("https://github.com/EricVogt93/artcraft-launcher".into());
                    }
                    if widgets::button(ui, self.palette, "App developers ↗", ButtonKind::Secondary, true, 160.0).clicked() {
                        self.external("https://github.com/storytold".into());
                    }
                });
                ui.add_space(16.0);
                ui.label(RichText::new("Launcher source: Apache-2.0. Bundled fonts and third-party artwork retain their own licenses.").size(11.0).color(self.palette.faint));
            });
    }
}
