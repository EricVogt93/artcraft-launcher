use crate::{Page, theme::Palette};
use egui::{Color32, Response, RichText, Stroke, Ui, Vec2};

pub const CONTROL_HEIGHT: f32 = 40.0;

pub fn nav_button(
    ui: &mut Ui,
    palette: Palette,
    page: Page,
    label: &str,
    selected: bool,
    count: Option<usize>,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), CONTROL_HEIGHT),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let painter = ui.painter();
    if selected || response.hovered() {
        painter.rect_filled(rect, 6, palette.raised);
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            6,
            Stroke::new(1.5, palette.accent),
            egui::StrokeKind::Inside,
        );
    }
    let color = if selected {
        palette.accent
    } else {
        palette.muted
    };
    let stroke = Stroke::new(1.5, color);
    let origin = egui::pos2(rect.left() + 12.0, rect.center().y - 9.0);
    let point = |x, y| origin + Vec2::new(x, y);
    match page {
        Page::AllApps => {
            for x in [0.0, 10.0] {
                for y in [0.0, 10.0] {
                    painter.rect_stroke(
                        egui::Rect::from_min_size(point(x, y), Vec2::splat(6.0)),
                        1,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        Page::MyApps => {
            painter.rect_stroke(
                egui::Rect::from_min_max(point(1.0, 5.0), point(17.0, 17.0)),
                2,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([point(3.0, 2.0), point(8.0, 2.0)], stroke);
            painter.line_segment([point(3.0, 2.0), point(3.0, 5.0)], stroke);
        }
        Page::Updates => {
            painter.line_segment([point(9.0, 1.0), point(9.0, 12.0)], stroke);
            painter.add(egui::Shape::line(
                vec![point(4.0, 8.0), point(9.0, 13.0), point(14.0, 8.0)],
                stroke,
            ));
            painter.line_segment([point(2.0, 17.0), point(16.0, 17.0)], stroke);
        }
        Page::Activity => {
            painter.circle_stroke(point(9.0, 9.0), 8.0, stroke);
            painter.add(egui::Shape::line(
                vec![point(9.0, 4.0), point(9.0, 9.0), point(13.0, 11.0)],
                stroke,
            ));
        }
        Page::Settings => {
            for (y, x) in [(3.0, 5.0), (9.0, 12.0), (15.0, 7.0)] {
                painter.line_segment([point(1.0, y), point(17.0, y)], stroke);
                painter.circle_filled(point(x, y), 3.0, palette.sunken);
                painter.circle_stroke(point(x, y), 2.5, stroke);
            }
        }
    }
    painter.text(
        egui::pos2(rect.left() + 42.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        color,
    );
    if let Some(count) = count.filter(|count| *count > 0) {
        let badge = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 18.0, rect.center().y),
            Vec2::new(24.0, 20.0),
        );
        painter.rect_filled(badge, 5, palette.sunken);
        painter.text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            count,
            egui::FontId::proportional(11.0),
            color,
        );
    }
    response
}

#[derive(Clone, Copy)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Quiet,
}

/// Shared control sizing and state colors, retaining egui's keyboard/accessibility behavior.
pub fn button(
    ui: &mut Ui,
    palette: Palette,
    text: &str,
    kind: ButtonKind,
    enabled: bool,
    width: f32,
) -> Response {
    ui.scope(|ui| {
        let (idle, hover, text_color, border) = match kind {
            ButtonKind::Primary if enabled => (
                palette.action,
                palette.action_hover,
                Color32::WHITE,
                Stroke::NONE,
            ),
            ButtonKind::Primary => (
                palette.sunken,
                palette.sunken,
                palette.muted,
                Stroke::new(1.0, palette.line),
            ),
            ButtonKind::Secondary => (
                palette.raised,
                palette.sunken,
                palette.ink,
                Stroke::new(1.0, palette.control_border),
            ),
            ButtonKind::Quiet => (
                Color32::TRANSPARENT,
                palette.sunken,
                palette.muted,
                Stroke::NONE,
            ),
        };
        let style = ui.style_mut();
        style.visuals.widgets.inactive.weak_bg_fill = idle;
        style.visuals.widgets.inactive.bg_stroke = border;
        style.visuals.widgets.hovered.weak_bg_fill = hover;
        style.visuals.widgets.hovered.bg_stroke = if matches!(kind, ButtonKind::Primary) {
            border
        } else {
            Stroke::new(1.0, palette.accent)
        };
        style.visuals.widgets.active.weak_bg_fill = hover;
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.5, palette.accent);
        ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(text).size(13.0).strong().color(text_color))
                .min_size(Vec2::new(width, CONTROL_HEIGHT))
                .corner_radius(6),
        )
    })
    .inner
}

/// A visible 20px box and 40px hit target; partial selection uses the native dash state.
pub fn checkbox(
    ui: &mut Ui,
    palette: Palette,
    checked: &mut bool,
    label: &str,
    enabled: bool,
    partial: bool,
) -> Response {
    ui.scope(|ui| {
        let selected = *checked || partial;
        let style = ui.style_mut();
        style.spacing.icon_width = 20.0;
        style.spacing.icon_width_inner = 12.0;
        style.spacing.interact_size = Vec2::splat(CONTROL_HEIGHT);
        for visuals in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
        ] {
            visuals.bg_fill = if selected {
                palette.action
            } else {
                palette.sunken
            };
            visuals.bg_stroke = Stroke::new(
                1.5,
                if selected {
                    palette.action
                } else {
                    palette.control_border
                },
            );
            visuals.fg_stroke = Stroke::new(
                2.0,
                if selected {
                    Color32::WHITE
                } else {
                    palette.ink
                },
            );
            visuals.corner_radius = egui::CornerRadius::same(4);
        }
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(2.0, palette.accent);
        style.visuals.widgets.active.bg_stroke = Stroke::new(2.0, palette.accent);
        ui.add_enabled(
            enabled,
            egui::Checkbox::new(checked, RichText::new(label).size(13.0)).indeterminate(partial),
        )
    })
    .inner
}
