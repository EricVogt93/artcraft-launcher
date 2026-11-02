use crate::{Launcher, Page};
use craftlauncher_core::Command;
use egui::{Context, Key, Modifiers};
use std::path::PathBuf;

#[derive(Clone)]
enum Action {
    Launch(String),
    OpenProject(String, PathBuf),
    Navigate(Page),
    Refresh,
}
#[derive(Clone)]
struct Entry {
    label: String,
    detail: String,
    action: Action,
}

fn filter(entries: Vec<Entry>, query: &str) -> Vec<Entry> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    entries
        .into_iter()
        .filter(|entry| {
            let text = format!("{} {}", entry.label, entry.detail).to_lowercase();
            words.iter().all(|word| text.contains(word))
        })
        .collect()
}

impl Launcher {
    pub(crate) fn palette_shortcut(&mut self, ctx: &Context) {
        let open = ctx.input_mut(|input| {
            input.consume_key(Modifiers::CTRL, Key::K)
                || input.consume_key(Modifiers::COMMAND, Key::K)
        });
        if open {
            self.palette_open = true;
            self.palette_query.clear();
            self.palette_selected = 0;
            self.palette_focus = true;
        }
    }
    pub(crate) fn command_palette(&mut self, ctx: &Context) {
        if !self.palette_open {
            return;
        }
        let mut entries = Vec::new();
        for product in &self.snapshot.products {
            if self.snapshot.state.installations.contains_key(&product.id) {
                entries.push(Entry {
                    label: format!("Open {}", product.name),
                    detail: "App".into(),
                    action: Action::Launch(product.id.clone()),
                });
            }
        }
        for recent in &self.snapshot.state.recent_projects {
            if !recent.path.is_file()
                || !self
                    .snapshot
                    .state
                    .installations
                    .contains_key(&recent.app_id)
            {
                continue;
            }
            let name = self
                .snapshot
                .products
                .iter()
                .find(|p| p.id == recent.app_id)
                .map(|p| p.name.as_str())
                .unwrap_or(&recent.app_id);
            entries.push(Entry {
                label: format!(
                    "Open {}",
                    recent
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
                detail: format!("{name} · {}", recent.path.display()),
                action: Action::OpenProject(recent.app_id.clone(), recent.path.clone()),
            });
        }
        for (label, page) in [
            ("My apps", Page::MyApps),
            ("All apps", Page::AllApps),
            ("Updates", Page::Updates),
            ("Activity", Page::Activity),
            ("Queue", Page::Queue),
            ("Discover", Page::Discover),
            ("Settings", Page::Settings),
        ] {
            entries.push(Entry {
                label: label.into(),
                detail: "Navigate".into(),
                action: Action::Navigate(page),
            });
        }
        entries.push(Entry {
            label: "Check for updates".into(),
            detail: "Refresh releases".into(),
            action: Action::Refresh,
        });
        let mut open = true;
        let mut chosen = None;
        let mut escape = false;
        egui::Window::new("Quick actions")
            .id(egui::Id::new("command-palette"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .id(egui::Id::new("command-palette-query"))
                        .hint_text("Search apps, projects and actions…")
                        .desired_width(f32::INFINITY),
                );
                if self.palette_focus {
                    response.request_focus();
                    self.palette_focus = false;
                }
                if response.changed() {
                    self.palette_selected = 0;
                }
                let visible = filter(entries, &self.palette_query);
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown))
                    && !visible.is_empty()
                {
                    self.palette_selected = (self.palette_selected + 1).min(visible.len() - 1);
                }
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                    self.palette_selected = self.palette_selected.saturating_sub(1);
                }
                if !visible.is_empty() {
                    self.palette_selected = self.palette_selected.min(visible.len() - 1);
                }
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                    chosen = visible
                        .get(self.palette_selected)
                        .map(|entry| entry.action.clone());
                }
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    escape = true;
                }
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.label("No matching actions.");
                        }
                        for (index, entry) in visible.iter().enumerate() {
                            let response = ui.selectable_label(
                                index == self.palette_selected,
                                format!("{}  ·  {}", entry.label, entry.detail),
                            );
                            if index == self.palette_selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                chosen = Some(entry.action.clone());
                            }
                        }
                    });
            });
        self.palette_open = open && !escape;
        if let Some(action) = chosen {
            self.palette_open = false;
            match action {
                Action::Launch(id) => self.command(Command::Launch(id)),
                Action::OpenProject(id, path) => self.command(Command::OpenProject { id, path }),
                Action::Navigate(page) => {
                    self.page = page;
                    self.search.clear();
                }
                Action::Refresh => self.command(Command::Refresh),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_matches_all_words_in_any_order_and_is_case_insensitive() {
        let entries = vec![
            Entry {
                label: "Open PhotoCraft".into(),
                detail: "App".into(),
                action: Action::Launch("photocraft".into()),
            },
            Entry {
                label: "Check for updates".into(),
                detail: "Refresh releases".into(),
                action: Action::Refresh,
            },
        ];
        assert_eq!(filter(entries.clone(), "APP PHOTO").len(), 1);
        assert_eq!(filter(entries.clone(), "release check").len(), 1);
        assert!(filter(entries, "photo update").is_empty());
    }
}
