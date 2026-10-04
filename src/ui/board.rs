//! The virtual breadboard: a digital twin of the scanned board with simulated
//! numbers, fault highlights and the suggested fix drawn on top.

use super::theme;
use crate::pipeline::verdict::milliamps;
use crate::pipeline::types::{Col, ComponentLabel, ComponentLabels, GridMap, Hole, Kind, PlacedPart, SimResult, Verdict};
use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

pub struct BoardView<'a> {
    pub rows: u8,
    pub grid: Option<&'a GridMap>,
    pub labels: Option<&'a ComponentLabels>,
    pub sim: Option<&'a SimResult>,
    pub verdict: Option<&'a Verdict>,
    pub show_fix: bool,
}

const BOARD: Color32 = Color32::from_rgb(0xec, 0xe8, 0xdd);
const HOLE: Color32 = Color32::from_rgb(0x4a, 0x48, 0x44);
const LEAD: Color32 = Color32::from_rgb(0xa8, 0xad, 0xb3);
const RAIL_RED: Color32 = Color32::from_rgb(0xd6, 0x45, 0x3d);
const RAIL_BLUE: Color32 = Color32::from_rgb(0x3d, 0x6f, 0xd6);

/// Vertical slot (in hole pitches) for each column. Board is drawn landscape:
/// rows run left to right, the L rails sit on top.
fn slot(col: Col) -> f32 {
    match col {
        Col::LPlus => 0.0,
        Col::LMinus => 1.0,
        Col::RMinus => 15.2,
        Col::RPlus => 16.2,
        c => {
            let i = Col::STRIP.iter().position(|s| *s == c).unwrap() as f32;
            if i < 5.0 { 2.8 + i } else { 4.4 + i }
        }
    }
}

struct Geom {
    origin: Pos2,
    p: f32,
}

impl Geom {
    fn hole(&self, h: Hole) -> Pos2 {
        self.origin + vec2(h.row as f32 * self.p, slot(h.col) * self.p)
    }
}

pub fn show(ui: &mut egui::Ui, view: &BoardView) {
    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(avail, Sense::hover());
    let rect = resp.rect;

    // 3 extra pitches on the left hold the off-board supply.
    let cols_units = view.rows as f32 + 5.0 + 3.0;
    let rows_units = 16.2 + 4.0;
    let p = (rect.width() / cols_units).min(rect.height() / rows_units).max(4.0);
    let size = vec2(cols_units * p, rows_units * p);
    let mut board_rect = Rect::from_center_size(rect.center(), size).shrink(p * 0.4);
    board_rect.min.x += 3.0 * p;
    let g = Geom { origin: board_rect.min + vec2(p * 1.6, p * 1.6), p };
    let t = ui.input(|i| i.time);

    // Board body; its outline is the overall verdict.
    let outline = match view.verdict {
        Some(v) if v.pass => theme::GREEN,
        Some(_) => theme::RED,
        None => theme::EDGE,
    };
    if view.verdict.is_some() {
        painter.rect_filled(board_rect.expand(6.0), 14, outline.gamma_multiply(0.18));
    }
    painter.rect_filled(board_rect, 10, BOARD);
    painter.rect_stroke(board_rect, 10, Stroke::new(3.0, outline), StrokeKind::Outside);

    // Rail stripes and the center trench.
    let x0 = g.origin.x + p * 0.5;
    let x1 = g.origin.x + (view.rows as f32 + 0.5) * p;
    for (y, c) in [(-0.6, RAIL_RED), (1.6, RAIL_BLUE), (14.6, RAIL_BLUE), (16.8, RAIL_RED)] {
        let y = g.origin.y + y * p;
        painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(p * 0.08, c));
    }
    let trench = Rect::from_min_max(pos2(x0, g.origin.y + 7.4 * p), pos2(x1, g.origin.y + 8.6 * p));
    painter.rect_filled(trench, 2, BOARD.gamma_multiply(0.9).to_opaque().linear_multiply(0.94));

    // Silkscreen.
    let silk = Color32::from_gray(0x8a);
    let font = FontId::monospace((p * 0.42).clamp(8.0, 14.0));
    for c in Col::STRIP {
        let y = g.origin.y + slot(c) * p;
        painter.text(pos2(g.origin.x - p * 0.1, y), Align2::RIGHT_CENTER, c.name(), font.clone(), silk);
        painter.text(pos2(x1 + p * 0.6, y), Align2::LEFT_CENTER, c.name(), font.clone(), silk);
    }
    for (c, color) in [(Col::LPlus, RAIL_RED), (Col::LMinus, RAIL_BLUE), (Col::RMinus, RAIL_BLUE), (Col::RPlus, RAIL_RED)] {
        let sign = if matches!(c, Col::LPlus | Col::RPlus) { "+" } else { "−" };
        let y = g.origin.y + slot(c) * p;
        painter.text(pos2(g.origin.x - p * 0.1, y), Align2::RIGHT_CENTER, sign, font.clone(), color);
    }
    for row in (1..=view.rows).filter(|r| *r == 1 || r % 5 == 0) {
        let x = g.origin.x + row as f32 * p;
        painter.text(pos2(x, g.origin.y + 2.0 * p), Align2::CENTER_CENTER, row.to_string(), font.clone(), silk);
        painter.text(pos2(x, g.origin.y + 14.0 * p), Align2::CENTER_CENTER, row.to_string(), font.clone(), silk);
    }

    // Holes, with net tracing on hover.
    let hovered = resp.hover_pos().and_then(|m| nearest_hole(&g, view.rows, m));
    let hovered_net = hovered.map(|h| h.net_key());
    for row in 1..=view.rows {
        for col in Col::STRIP.iter().chain(Col::RAILS.iter()) {
            let h = Hole { row, col: *col };
            let c = g.hole(h);
            let lit = hovered_net.as_deref() == Some(h.net_key().as_str());
            if lit {
                painter.rect_filled(Rect::from_center_size(c, Vec2::splat(p * 0.8)), 2, theme::CYAN.gamma_multiply(0.35));
            }
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(p * 0.36)), 1, HOLE);
        }
    }

    let Some(grid) = view.grid else {
        empty_hint(&painter, board_rect, "Scan a board to populate the replica");
        return;
    };
    let label_of = |id: &str| view.labels.and_then(|l| l.components.iter().find(|c| c.id == id));

    for part in &grid.parts {
        draw_part(&painter, &g, part, label_of(&part.id), view.sim, board_rect, 1.0);
    }

    if let Some(v) = view.verdict {
        let pulse = 0.5 + 0.5 * (t * 4.0).sin() as f32;
        for f in v.findings.iter().filter(|f| f.fault) {
            for h in &f.highlight {
                let c = g.hole(*h);
                painter.circle_stroke(c, p * (0.55 + 0.15 * pulse), Stroke::new(2.5, theme::RED.gamma_multiply(0.5 + 0.5 * pulse)));
            }
        }
        if view.show_fix {
            for fix in v.findings.iter().filter_map(|f| f.fix.as_ref()) {
                let Some(part) = grid.parts.iter().find(|p| p.id == fix.part) else { continue };
                let mut ghost = part.clone();
                for (leg, to) in &fix.moves {
                    if let Some(l) = ghost.legs.get_mut(*leg) {
                        painter.extend(Shape::dashed_line(&[g.hole(*l), g.hole(*to)], Stroke::new(2.0, theme::GREEN), 5.0, 4.0));
                        *l = *to;
                    }
                }
                draw_part(&painter, &g, &ghost, label_of(&ghost.id), None, board_rect, 0.45);
                for (_, to) in &fix.moves {
                    painter.circle_stroke(g.hole(*to), p * 0.5, Stroke::new(2.0, theme::GREEN));
                }
            }
        }
        if v.findings.iter().any(|f| f.fault) {
            ui.ctx().request_repaint();
        }
    }

    if let Some(h) = hovered {
        let mut text = format!("{}  net {}", h.label(), h.net_key());
        if let Some(volts) = view.sim.and_then(|s| s.net_voltage.get(&h.net_key())) {
            text.push_str(&format!("\n{volts:.3} V"));
        }
        resp.on_hover_text_at_pointer(text);
    }
}

fn nearest_hole(g: &Geom, rows: u8, m: Pos2) -> Option<Hole> {
    (1..=rows)
        .flat_map(|row| Col::STRIP.iter().chain(Col::RAILS.iter()).map(move |col| Hole { row, col: *col }))
        .map(|h| (h, g.hole(h).distance(m)))
        .filter(|(_, d)| *d < g.p * 0.5)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(h, _)| h)
}

fn empty_hint(painter: &Painter, rect: Rect, text: &str) {
    let r = Rect::from_center_size(rect.center(), vec2(320.0, 34.0));
    painter.rect_filled(r, 6, theme::BG.gamma_multiply(0.85));
    painter.text(r.center(), Align2::CENTER_CENTER, text, FontId::monospace(13.0), theme::TEXT);
}

fn draw_part(painter: &Painter, g: &Geom, part: &PlacedPart, label: Option<&ComponentLabel>, sim: Option<&SimResult>, board: Rect, alpha: f32) {
    let p = g.p;
    let a = |c: Color32| c.gamma_multiply(alpha);
    let kind = label.map(|l| l.kind).unwrap_or(Kind::Unknown);
    let current = sim.and_then(|s| s.part_current.get(&part.id)).copied();
    let legs: Vec<Pos2> = part.legs.iter().map(|h| g.hole(*h)).collect();
    if legs.len() < 2 {
        return;
    }
    let (p0, p1) = (legs[0], legs[1]);
    let mid = p0 + (p1 - p0) * 0.5;
    let dir = (p1 - p0).normalized();
    let normal = vec2(-dir.y, dir.x);
    let tag_font = FontId::monospace((p * 0.42).clamp(9.0, 13.0));
    let readout = |text: String, at: Pos2| {
        let galley = painter.layout_no_wrap(text, tag_font.clone(), a(theme::CYAN));
        let r = Rect::from_center_size(at, galley.size() + vec2(8.0, 4.0));
        painter.rect_filled(r, 3, a(theme::BG.gamma_multiply(0.9)));
        painter.galley(r.min + vec2(4.0, 2.0), galley, theme::CYAN);
    };

    match kind {
        Kind::Wire => {
            let color = wire_color(label.and_then(|l| l.markings.as_deref()));
            painter.line_segment([p0, p1], Stroke::new(p * 0.26, a(color)));
            for q in [p0, p1] {
                painter.circle_filled(q, p * 0.2, a(color));
            }
        }
        Kind::Supply => {
            // Off-board source, drawn in the left margin with leads to the rails.
            let box_c = pos2(board.min.x - p * 1.6, (p0.y + p1.y) * 0.5);
            let r = Rect::from_center_size(box_c, vec2(p * 2.2, p * 2.4));
            for (q, c) in [(p0, RAIL_RED), (p1, Color32::from_gray(40))] {
                painter.line_segment([pos2(r.max.x, q.y), q], Stroke::new(p * 0.2, a(c)));
                painter.circle_filled(q, p * 0.18, a(c));
            }
            painter.rect_filled(r, 4, a(Color32::from_rgb(0x24, 0x28, 0x2e)));
            painter.rect_stroke(r, 4, Stroke::new(1.0, a(theme::AMBER)), StrokeKind::Inside);
            let v = label.and_then(|l| l.value).map(|v| format!("{v:.0}V")).unwrap_or("V?".into());
            painter.text(r.center(), Align2::CENTER_CENTER, v, tag_font.clone(), a(theme::AMBER));
            return;
        }
        Kind::Resistor => {
            painter.line_segment([p0, p1], Stroke::new(p * 0.09, a(LEAD)));
            let len = ((p1 - p0).length() * 0.55).min(p * 2.6);
            let (b0, b1) = (mid - dir * len * 0.5, mid + dir * len * 0.5);
            let w = p * 0.6;
            painter.line_segment([b0, b1], Stroke::new(w, a(Color32::from_rgb(0xd9, 0xb9, 0x8a))));
            painter.circle_filled(b0, w * 0.5, a(Color32::from_rgb(0xd9, 0xb9, 0x8a)));
            painter.circle_filled(b1, w * 0.5, a(Color32::from_rgb(0xd9, 0xb9, 0x8a)));
            if let Some(ohms) = label.and_then(|l| l.value) {
                for (i, c) in bands(ohms).iter().enumerate() {
                    let at = b0 + dir * len * (0.18 + 0.2 * i as f32);
                    painter.line_segment([at - normal * w * 0.5, at + normal * w * 0.5], Stroke::new(p * 0.14, a(*c)));
                }
            }
        }
        Kind::Led => {
            painter.line_segment([p0, p1], Stroke::new(p * 0.09, a(LEAD)));
            let ma = current.map(milliamps).unwrap_or(0.0);
            let lit = (ma as f32 / 15.0).clamp(0.0, 1.0);
            let body = mid + normal * p * 0.6;
            if lit > 0.05 {
                for k in 1..=4 {
                    painter.circle_filled(body, p * (0.6 + 0.35 * k as f32), a(Color32::from_rgb(255, 60, 40).gamma_multiply(0.12 * lit)));
                }
            }
            let base = Color32::from_rgb(0x8a, 0x1c, 0x18);
            let fill = lerp(base, Color32::from_rgb(0xff, 0x4a, 0x38), lit);
            painter.circle_filled(body, p * 0.55, a(fill));
            painter.circle_stroke(body, p * 0.55, Stroke::new(1.0, a(Color32::from_gray(30))));
            if let Some(pos) = label.and_then(|l| l.positive_leg) {
                if let Some(q) = legs.get(pos as usize) {
                    let out = (*q - mid).normalized() * p * 0.6;
                    painter.text(*q + out, Align2::CENTER_CENTER, "+", tag_font.clone(), a(RAIL_RED));
                }
            }
        }
        _ => {
            painter.line_segment([p0, p1], Stroke::new(p * 0.09, a(LEAD)));
            painter.rect_filled(Rect::from_center_size(mid, Vec2::splat(p * 0.9)), 2, a(Color32::from_gray(60)));
        }
    }

    // Part id and, after simulation, its current.
    let tag_at = mid - normal * p * 0.75;
    painter.text(tag_at, Align2::CENTER_CENTER, &part.id, tag_font.clone(), a(Color32::from_gray(50)));
    if let (Some(i), Kind::Resistor | Kind::Led) = (current, kind) {
        readout(format!("{:.2} mA", milliamps(i)), mid + normal * p * 1.6);
    }
}

fn wire_color(markings: Option<&str>) -> Color32 {
    let m = markings.unwrap_or("").to_lowercase();
    for (name, c) in [
        ("red", Color32::from_rgb(0xd8, 0x3a, 0x30)),
        ("black", Color32::from_gray(35)),
        ("blue", Color32::from_rgb(0x2f, 0x6c, 0xd8)),
        ("green", Color32::from_rgb(0x2f, 0xa8, 0x5a)),
        ("yellow", Color32::from_rgb(0xe8, 0xc2, 0x2e)),
        ("white", Color32::from_gray(235)),
    ] {
        if m.contains(name) {
            return c;
        }
    }
    Color32::from_rgb(0xe0, 0x8a, 0x2a)
}

/// First two digits and multiplier of the resistor color code.
fn bands(ohms: f64) -> [Color32; 3] {
    const C: [Color32; 10] = [
        Color32::from_gray(20),
        Color32::from_rgb(0x7a, 0x45, 0x1e),
        Color32::from_rgb(0xd0, 0x2a, 0x22),
        Color32::from_rgb(0xf0, 0x80, 0x20),
        Color32::from_rgb(0xf0, 0xd0, 0x20),
        Color32::from_rgb(0x30, 0xa0, 0x40),
        Color32::from_rgb(0x30, 0x60, 0xd0),
        Color32::from_rgb(0x80, 0x40, 0xc0),
        Color32::from_gray(128),
        Color32::from_gray(245),
    ];
    if ohms < 10.0 {
        return [C[0], C[(ohms.round() as usize).min(9)], C[0]];
    }
    let exp = ohms.log10().floor() as i32 - 1;
    let digits = (ohms / 10f64.powi(exp)).round() as usize;
    [C[(digits / 10).min(9)], C[digits % 10], C[exp.clamp(0, 9) as usize]]
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}
