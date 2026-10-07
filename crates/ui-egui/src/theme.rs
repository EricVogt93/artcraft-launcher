use craftlauncher_core::Theme;
use egui::{
    Color32, Context, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy)]
pub struct Palette {
    pub background: Color32,
    pub raised: Color32,
    pub sunken: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub line: Color32,
    pub control_border: Color32,
    pub selected: Color32,
    pub accent: Color32,
    pub action: Color32,
    pub action_hover: Color32,
    pub danger: Color32,
}
impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                background: rgb(24, 24, 27),
                raised: rgb(32, 32, 36),
                sunken: rgb(20, 20, 23),
                ink: rgb(242, 241, 238),
                muted: rgb(173, 172, 170),
                faint: rgb(135, 135, 137),
                line: rgb(57, 57, 62),
                control_border: rgb(111, 113, 123),
                selected: rgb(34, 46, 65),
                accent: rgb(86, 151, 255),
                action: rgb(38, 111, 224),
                action_hover: rgb(49, 127, 245),
                danger: rgb(245, 128, 118),
            }
        } else {
            Self {
                background: rgb(242, 241, 238),
                raised: rgb(250, 249, 247),
                sunken: rgb(233, 232, 228),
                ink: rgb(16, 16, 20),
                muted: rgb(99, 99, 102),
                faint: rgb(130, 130, 132),
                line: rgb(213, 212, 208),
                control_border: rgb(145, 146, 151),
                selected: rgb(230, 238, 253),
                accent: rgb(45, 129, 255),
                action: rgb(33, 105, 218),
                action_hover: rgb(24, 87, 188),
                danger: rgb(179, 38, 30),
            }
        }
    }
}
pub const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}
pub fn fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "archivo".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/archivo.ttf"
        ))),
    );
    fonts.font_data.insert(
        "instrument".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/instrument-serif.ttf"
        ))),
    );
    fonts.font_data.insert(
        "instrument-italic".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/instrument-serif-italic.ttf"
        ))),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "archivo".into());
    fonts.families.insert(
        FontFamily::Name("display".into()),
        vec!["instrument".into()],
    );
    fonts.families.insert(
        FontFamily::Name("display-italic".into()),
        vec!["instrument-italic".into()],
    );
    ctx.set_fonts(fonts);
}
pub fn apply(ctx: &Context, theme: Theme) -> Palette {
    let dark = match theme {
        Theme::Dark => true,
        Theme::Light => false,
        Theme::System => ctx.system_theme().is_some_and(|t| t == egui::Theme::Dark),
    };
    ctx.set_theme(match theme {
        Theme::Dark => egui::ThemePreference::Dark,
        Theme::Light => egui::ThemePreference::Light,
        Theme::System => egui::ThemePreference::System,
    });
    let p = Palette::new(dark);
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.override_text_color = Some(p.ink);
    visuals.panel_fill = p.background;
    visuals.window_fill = p.raised;
    visuals.extreme_bg_color = p.raised;
    visuals.faint_bg_color = p.sunken;
    visuals.selection.bg_fill = p.accent;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
    visuals.widgets.inactive.weak_bg_fill = p.raised;
    visuals.widgets.inactive.bg_fill = p.sunken;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, p.control_border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.5, p.ink);
    visuals.widgets.hovered.weak_bg_fill = p.sunken;
    visuals.widgets.hovered.bg_fill = p.sunken;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, p.ink);
    visuals.widgets.active.bg_fill = p.selected;
    visuals.widgets.active.weak_bg_fill = p.selected;
    visuals.widgets.active.bg_stroke = Stroke::new(1.5, p.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, p.ink);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
    ] {
        widget.corner_radius = egui::CornerRadius::same(6);
        widget.expansion = 0.0;
    }
    ctx.set_visuals(visuals);
    ctx.style_mut_of(
        if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        },
        |style| {
            style.spacing.item_spacing = egui::vec2(12.0, 10.0);
            style.spacing.button_padding = egui::vec2(14.0, 10.0);
            style.spacing.interact_size = egui::vec2(40.0, 40.0);
            style.spacing.icon_width = 20.0;
            style.spacing.icon_width_inner = 12.0;
            style.text_styles = BTreeMap::from([
                (TextStyle::Small, FontId::proportional(11.0)),
                (TextStyle::Body, FontId::proportional(14.0)),
                (TextStyle::Button, FontId::proportional(13.0)),
                (TextStyle::Heading, FontId::proportional(24.0)),
                (TextStyle::Monospace, FontId::monospace(11.0)),
            ]);
        },
    );
    p
}
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}
pub fn italic(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display-italic".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_theme_overrides_the_context_theme_and_keeps_widget_sizes() {
        let ctx = Context::default();
        ctx.set_theme(egui::ThemePreference::Dark);
        apply(&ctx, Theme::Light);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert!(!ctx.style_of(ctx.theme()).visuals.dark_mode);
        let spacing = ctx.style_of(ctx.theme()).spacing.button_padding;
        apply(&ctx, Theme::Dark);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        assert!(ctx.style_of(ctx.theme()).visuals.dark_mode);
        assert_eq!(ctx.style_of(ctx.theme()).spacing.button_padding, spacing);
    }
}
