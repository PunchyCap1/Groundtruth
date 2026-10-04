//! Minimal look after Hundred Rabbits' Orca: black ground, white and greys,
//! one accent for live numbers and one for trouble. Monospace everywhere,
//! square corners, 1 px lines.

use eframe::egui::{self, Color32, FontFamily, FontId, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
pub const PANEL: Color32 = BG;
/// Rules and borders.
pub const EDGE: Color32 = Color32::from_gray(0x2a);
/// Faint marks: holes, grid.
pub const LOW: Color32 = Color32::from_gray(0x44);
pub const DIM: Color32 = Color32::from_gray(0x77);
pub const TEXT: Color32 = Color32::from_gray(0xdd);
/// Selection and emphasis.
pub const HI: Color32 = Color32::WHITE;
/// Simulated numbers and "all good".
pub const ACCENT: Color32 = Color32::from_rgb(0x72, 0xde, 0xc2);
/// Faults and warnings.
pub const ALERT: Color32 = Color32::from_rgb(0xff, 0xb5, 0x45);

pub fn apply(ctx: &egui::Context) {
    // Every font family is the monospace one.
    let mut fonts = egui::FontDefinitions::default();
    let mono = fonts.families[&FontFamily::Monospace].clone();
    fonts.families.insert(FontFamily::Proportional, mono);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(15.0, FontFamily::Monospace)),
            (TextStyle::Body, FontId::new(12.5, FontFamily::Monospace)),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(12.0, FontFamily::Monospace)),
            (TextStyle::Small, FontId::new(10.5, FontFamily::Monospace)),
        ]
        .into();
        let v = &mut style.visuals;
        v.panel_fill = PANEL;
        v.window_fill = BG;
        v.extreme_bg_color = Color32::from_gray(0x0e);
        v.faint_bg_color = Color32::from_gray(0x0a);
        v.code_bg_color = Color32::from_gray(0x0e);
        v.override_text_color = None;
        v.hyperlink_color = ACCENT;
        v.window_corner_radius = 0.into();
        v.menu_corner_radius = 0.into();
        v.window_stroke = Stroke::new(1.0, EDGE);
        v.window_shadow = egui::Shadow::NONE;
        v.popup_shadow = egui::Shadow::NONE;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.3);
        v.selection.stroke = Stroke::new(1.0, ACCENT);

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = BG;
        w.noninteractive.weak_bg_fill = BG;
        w.noninteractive.bg_stroke = Stroke::new(1.0, EDGE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.bg_fill = Color32::from_gray(0x22);
        w.inactive.weak_bg_fill = BG;
        w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(0x38));
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_fill = Color32::from_gray(0x33);
        w.hovered.weak_bg_fill = Color32::from_gray(0x16);
        w.hovered.bg_stroke = Stroke::new(1.0, DIM);
        w.hovered.fg_stroke = Stroke::new(1.0, HI);
        w.active.bg_fill = ACCENT;
        w.active.weak_bg_fill = ACCENT;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.fg_stroke = Stroke::new(1.0, BG);
        w.open.bg_fill = Color32::from_gray(0x16);
        w.open.weak_bg_fill = Color32::from_gray(0x16);
        w.open.bg_stroke = Stroke::new(1.0, DIM);
        w.open.fg_stroke = Stroke::new(1.0, HI);
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = 0.into();
            wv.expansion = 0.0;
        }
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
    });
}

/// Small all-caps section label.
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).font(FontId::monospace(10.5)).color(DIM));
    ui.add_space(2.0);
}

/// A bordered box with square corners.
pub fn card(fill: Color32, stroke: Color32) -> egui::Frame {
    egui::Frame::new().fill(fill).stroke(Stroke::new(1.0, stroke)).inner_margin(8)
}

/// Solid status tag (dark text on a signal color), so FAULT / WARN / PASS read by
/// shape and fill as well as hue.
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(egui::RichText::new(text).font(FontId::monospace(11.5)).color(BG).strong()))
        .response
}

/// Hollow tag for states that aren't a verdict (empty board, sim not run).
pub fn badge_outline(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, color))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(egui::RichText::new(text).font(FontId::monospace(11.5)).color(color).strong()))
        .response
}
