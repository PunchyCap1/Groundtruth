//! Bench-instrument look: dark chassis, monospace readouts, signal colors.

use eframe::egui::{self, Color32, FontFamily, FontId, TextStyle};

pub const BG: Color32 = Color32::from_rgb(0x12, 0x14, 0x17);
pub const PANEL: Color32 = Color32::from_rgb(0x1a, 0x1d, 0x21);
pub const EDGE: Color32 = Color32::from_rgb(0x2c, 0x31, 0x38);
pub const TEXT: Color32 = Color32::from_rgb(0xd8, 0xde, 0xe4);
pub const DIM: Color32 = Color32::from_rgb(0x7d, 0x85, 0x90);
pub const AMBER: Color32 = Color32::from_rgb(0xf2, 0xa9, 0x3b);
pub const GREEN: Color32 = Color32::from_rgb(0x3f, 0xd0, 0x7f);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x5a, 0x4e);
pub const CYAN: Color32 = Color32::from_rgb(0x4c, 0xc3, 0xd9);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(18.0, FontFamily::Monospace)),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.0, FontFamily::Monospace)),
        ]
        .into();
        let v = &mut style.visuals;
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = BG;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = AMBER.gamma_multiply(0.35);
        v.widgets.noninteractive.bg_stroke.color = EDGE;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}

/// Small all-caps section label, like the silkscreen on an instrument panel.
pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).font(FontId::monospace(11.0)).color(DIM).strong());
    ui.add_space(2.0);
}

pub fn card(fill: Color32, stroke: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(6)
        .inner_margin(10)
}
