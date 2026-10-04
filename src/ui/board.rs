//! The breadboard editor: drag parts from the palette, move parts and single
//! legs, draw wires from any empty hole, and see live simulated numbers.

use super::theme;
use crate::circuit::{Circuit, Col, Hole, Kind, Part, fmt_si};
use crate::sim::checks::milliamps;
use crate::sim::{Checks, Severity, SimResult};
use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

#[derive(Default)]
pub struct Editor {
    pub selected: Option<String>,
    drag: Option<Drag>,
}

impl Editor {
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

enum Drag {
    Leg { id: String, leg: usize },
    Body { id: String, grab: Pos2, start: Vec<Hole> },
    NewWire { from: Hole },
}

pub struct View<'a> {
    pub sim: Option<&'a SimResult>,
    pub checks: Option<&'a Checks>,
    pub show_fix: bool,
    pub show_readouts: bool,
    /// Parts to call out, e.g. while the user hovers a lab-partner issue.
    pub flagged: &'a [String],
    /// Push button currently held down with Space.
    pub held: Option<&'a str>,
}

#[derive(Default)]
pub struct Out {
    pub notice: Option<String>,
}

pub const BOARD: Color32 = Color32::from_rgb(0xec, 0xe8, 0xdd);
const HOLE: Color32 = Color32::from_rgb(0x4a, 0x48, 0x44);
const LEAD: Color32 = Color32::from_rgb(0xa8, 0xad, 0xb3);
pub const RAIL_RED: Color32 = Color32::from_rgb(0xd6, 0x45, 0x3d);
pub const RAIL_BLUE: Color32 = Color32::from_rgb(0x3d, 0x6f, 0xd6);
const INK: Color32 = Color32::from_gray(50);

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
    rows: u8,
}

impl Geom {
    fn hole(&self, h: Hole) -> Pos2 {
        self.origin + vec2(h.row as f32 * self.p, slot(h.col) * self.p)
    }

    fn nearest(&self, m: Pos2) -> Option<Hole> {
        (1..=self.rows)
            .flat_map(|row| Col::STRIP.iter().chain(Col::RAILS.iter()).map(move |col| Hole { row, col: *col }))
            .map(|h| (h, self.hole(h).distance(m)))
            .filter(|(_, d)| *d < self.p * 0.55)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    }
}

/// Screen layout of one part, shared by drawing and hit-testing.
struct Shape2 {
    legs: Vec<Pos2>,
    mid: Pos2,
    dir: Vec2,
    /// Points left or up, away from the board's bottom-right.
    normal: Vec2,
    body: Pos2,
    body_r: f32,
}

fn layout(g: &Geom, part: &Part) -> Shape2 {
    let p = g.p;
    let legs: Vec<Pos2> = part.legs.iter().map(|h| g.hole(*h)).collect();
    let (first, last) = (legs[0], *legs.last().unwrap());
    let mid = first + (last - first) * 0.5;
    let dir = if first.distance(last) < 1.0 { vec2(1.0, 0.0) } else { (last - first).normalized() };
    let mut normal = vec2(-dir.y, dir.x);
    if normal.x > 0.01 || (normal.x.abs() <= 0.01 && normal.y > 0.0) {
        normal = -normal;
    }
    let (body, body_r) = match part.kind {
        // A pack near the start of the rails parks in the margin left of the board.
        Kind::Battery if part.legs.iter().all(|h| h.col.is_rail() && h.row <= 3) => (pos2(g.origin.x - p * 3.0, mid.y), p * 1.2),
        Kind::Battery => (mid + normal * p * 1.9, p * 1.2),
        Kind::Led | Kind::Capacitor => (mid + normal * p * 0.6, p * 0.6),
        Kind::Npn => (legs[1] + normal * p * 0.55, p * 1.2),
        Kind::Pot => (legs[1] + normal * p * 0.7, p * 1.4),
        Kind::Button => (mid, p * 0.9),
        _ => (mid, p * 0.4),
    };
    Shape2 { legs, mid, dir, normal, body, body_r }
}

/// True if `at` lies over the board body rather than the dark margin.
fn board_contains(g: &Geom, at: Pos2) -> bool {
    at.x > g.origin.x - g.p * 1.6 && at.y > g.origin.y - g.p * 1.6
}

fn seg_dist(m: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((m - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    m.distance(a + ab * t)
}

/// Topmost part whose body or lead is under `m`.
fn part_at<'c>(g: &Geom, circuit: &'c Circuit, m: Pos2) -> Option<&'c Part> {
    circuit.parts.iter().rev().find(|part| {
        let s = layout(g, part);
        let on_body = s.body.distance(m) < s.body_r;
        let on_lead = s.legs.windows(2).any(|w| seg_dist(m, w[0], w[1]) < g.p * 0.3)
            || s.legs.iter().any(|l| seg_dist(m, *l, s.body) < g.p * 0.2);
        on_body || on_lead
    })
}

pub fn show(ui: &mut egui::Ui, circuit: &mut Circuit, ed: &mut Editor, view: &View) -> Out {
    let mut out = Out::default();
    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(avail, Sense::click_and_drag());
    let rect = resp.rect;

    // 3 extra pitches on the left hold off-board battery packs.
    let cols_units = circuit.rows as f32 + 5.0 + 3.0;
    let rows_units = 16.2 + 4.0;
    let p = (rect.width() / cols_units).min(rect.height() / rows_units).max(4.0);
    let size = vec2(cols_units * p, rows_units * p);
    let mut board_rect = Rect::from_center_size(rect.center(), size).shrink(p * 0.4);
    board_rect.min.x += 3.0 * p;
    let g = Geom { origin: board_rect.min + vec2(p * 1.6, p * 1.6), p, rows: circuit.rows };
    let t = ui.input(|i| i.time);
    let pointer = resp.hover_pos().or_else(|| ui.input(|i| i.pointer.hover_pos()));

    // ---- Interaction -------------------------------------------------------
    if resp.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
        let hole = g.nearest(origin);
        let leg = hole
            .filter(|h| g.hole(*h).distance(origin) < p * 0.32)
            .and_then(|h| circuit.occupant(h))
            .map(|(part, leg)| (part.id.clone(), leg));
        if let Some((id, leg)) = leg {
            ed.selected = Some(id.clone());
            ed.drag = Some(Drag::Leg { id, leg });
        } else if let Some(part) = part_at(&g, circuit, origin) {
            ed.selected = Some(part.id.clone());
            ed.drag = Some(Drag::Body { id: part.id.clone(), grab: origin, start: part.legs.clone() });
        } else if let Some(from) = hole.filter(|h| circuit.occupant(*h).is_none()) {
            ed.drag = Some(Drag::NewWire { from });
        }
    }
    if let (Some(drag), Some(m)) = (&ed.drag, pointer) {
        match drag {
            Drag::Leg { id, leg } => {
                if let (Some(target), Some(part)) = (g.nearest(m), circuit.part(id)) {
                    let mut legs = part.legs.clone();
                    legs[*leg] = target;
                    if legs != part.legs && circuit.fits(&legs, Some(id)) {
                        circuit.part_mut(id).unwrap().legs = legs;
                    }
                }
            }
            Drag::Body { id, grab, start } => {
                let delta = m - *grab;
                let moved: Option<Vec<Hole>> = start.iter().map(|h| g.nearest(g.hole(*h) + delta)).collect();
                if let (Some(legs), Some(part)) = (moved, circuit.part(id)) {
                    if legs != part.legs && circuit.fits(&legs, Some(id)) {
                        circuit.part_mut(id).unwrap().legs = legs;
                    }
                }
            }
            Drag::NewWire { .. } => {}
        }
    }
    if resp.drag_stopped() {
        if let (Some(Drag::NewWire { from }), Some(m)) = (&ed.drag, pointer) {
            if let Some(to) = g.nearest(m).filter(|to| to != from && circuit.occupant(*to).is_none()) {
                if let Some(id) = circuit.add(Kind::Wire, *from) {
                    circuit.part_mut(&id).unwrap().legs = vec![*from, to];
                    ed.selected = Some(id);
                }
            }
        }
        ed.drag = None;
    }
    if resp.clicked() {
        let m = resp.interact_pointer_pos().unwrap_or_default();
        let leg_part = g.nearest(m).and_then(|h| circuit.occupant(h)).map(|(p, _)| p.id.clone());
        ed.selected = leg_part.or_else(|| part_at(&g, circuit, m).map(|p| p.id.clone()));
    }
    if let Some(kind) = resp.dnd_release_payload::<Kind>() {
        match pointer.and_then(|m| g.nearest(m)) {
            Some(at) => match circuit.add(*kind, at) {
                Some(id) => ed.selected = Some(id),
                None => out.notice = Some(format!("No room for a {} there.", kind.name().to_lowercase())),
            },
            None => out.notice = Some("Drop parts onto a hole.".into()),
        }
    }
    let palette_hover = resp.dnd_hover_payload::<Kind>().map(|k| *k);

    // ---- Board ---------------------------------------------------------------
    let outline = match view.checks {
        Some(c) if c.pass => theme::GREEN,
        Some(_) => theme::RED,
        None => theme::EDGE,
    };
    if view.checks.is_some() && !circuit.parts.is_empty() {
        painter.rect_filled(board_rect.expand(6.0), 14, outline.gamma_multiply(0.14));
    }
    painter.rect_filled(board_rect, 10, BOARD);
    painter.rect_stroke(board_rect, 10, Stroke::new(3.0, outline), StrokeKind::Outside);

    // Rail stripes and the center trench.
    let x0 = g.origin.x + p * 0.5;
    let x1 = g.origin.x + (circuit.rows as f32 + 0.5) * p;
    for (y, c) in [(-0.6, RAIL_RED), (1.6, RAIL_BLUE), (14.6, RAIL_BLUE), (16.8, RAIL_RED)] {
        let y = g.origin.y + y * p;
        painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(p * 0.08, c));
    }
    let trench = Rect::from_min_max(pos2(x0, g.origin.y + 7.4 * p), pos2(x1, g.origin.y + 8.6 * p));
    painter.rect_filled(trench, 2, Color32::from_rgb(0xdb, 0xd6, 0xca));

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
        painter.text(pos2(x1 + p * 0.6, y), Align2::LEFT_CENTER, sign, font.clone(), color);
    }
    for row in (1..=circuit.rows).filter(|r| *r == 1 || r % 5 == 0) {
        let x = g.origin.x + row as f32 * p;
        painter.text(pos2(x, g.origin.y + 2.0 * p), Align2::CENTER_CENTER, row.to_string(), font.clone(), silk);
        painter.text(pos2(x, g.origin.y + 14.0 * p), Align2::CENTER_CENTER, row.to_string(), font.clone(), silk);
    }

    // Holes, with net tracing on hover.
    let hovered_hole = pointer.filter(|m| rect.contains(*m)).and_then(|m| g.nearest(m));
    let hovered_net = hovered_hole.map(|h| h.net_key());
    for h in circuit.holes() {
        let c = g.hole(h);
        if hovered_net.as_deref() == Some(h.net_key().as_str()) {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(p * 0.8)), 2, theme::CYAN.gamma_multiply(0.35));
        }
        painter.rect_filled(Rect::from_center_size(c, Vec2::splat(p * 0.36)), 1, HOLE);
    }

    // ---- Parts ---------------------------------------------------------------
    let selected = ed.selected.clone();
    let order = circuit.parts.iter().filter(|p| Some(&p.id) != selected.as_ref()).chain(circuit.parts.iter().filter(|p| Some(&p.id) == selected.as_ref()));
    for part in order {
        let is_sel = Some(&part.id) == selected.as_ref();
        let mut shown = part.clone();
        shown.pressed |= view.held == Some(part.id.as_str());
        draw_part(&painter, &g, &shown, view.sim, is_sel, 1.0);
    }
    if view.show_readouts {
        for part in &circuit.parts {
            let mut shown = part.clone();
            shown.pressed |= view.held == Some(part.id.as_str());
            draw_readout(&painter, &g, &shown, view.sim);
        }
    }

    // Lab-partner callouts.
    let pulse = 0.5 + 0.5 * (t * 4.0).sin() as f32;
    for id in view.flagged {
        if let Some(part) = circuit.part(id) {
            let s = layout(&g, part);
            let r = Rect::from_points(&[s.legs.clone(), vec![s.body]].concat()).expand(p * 0.7);
            painter.rect_stroke(r, 6, Stroke::new(2.0, theme::AMBER.gamma_multiply(0.5 + 0.5 * pulse)), StrokeKind::Outside);
            ui.ctx().request_repaint();
        }
    }

    // Rule-checker faults and suggested fixes.
    if let Some(checks) = view.checks {
        for f in checks.findings.iter().filter(|f| f.severity == Severity::Fault) {
            for h in &f.highlight {
                painter.circle_stroke(g.hole(*h), p * (0.55 + 0.15 * pulse), Stroke::new(2.5, theme::RED.gamma_multiply(0.5 + 0.5 * pulse)));
            }
        }
        if view.show_fix {
            for fix in checks.findings.iter().filter_map(|f| f.fix.as_ref()) {
                let Some(part) = circuit.part(&fix.part) else { continue };
                let mut ghost = part.clone();
                for (leg, to) in &fix.moves {
                    if let Some(l) = ghost.legs.get_mut(*leg) {
                        painter.extend(Shape::dashed_line(&[g.hole(*l), g.hole(*to)], Stroke::new(2.0, theme::GREEN), 5.0, 4.0));
                        *l = *to;
                    }
                }
                draw_part(&painter, &g, &ghost, None, false, 0.45);
                for (_, to) in &fix.moves {
                    painter.circle_stroke(g.hole(*to), p * 0.5, Stroke::new(2.0, theme::GREEN));
                }
            }
        }
        if checks.findings.iter().any(|f| f.severity == Severity::Fault) {
            ui.ctx().request_repaint();
        }
    }

    // ---- Drag previews -------------------------------------------------------
    if let (Some(Drag::NewWire { from }), Some(m)) = (&ed.drag, pointer) {
        let target = g.nearest(m).filter(|h| h != from && circuit.occupant(*h).is_none());
        let end = target.map(|h| g.hole(h)).unwrap_or(m);
        painter.line_segment([g.hole(*from), end], Stroke::new(p * 0.26, theme::AMBER.gamma_multiply(0.8)));
        painter.circle_filled(g.hole(*from), p * 0.2, theme::AMBER);
        if let Some(h) = target {
            painter.circle_stroke(g.hole(h), p * 0.45, Stroke::new(2.0, theme::AMBER));
        }
    }
    if let (Some(kind), Some(at)) = (palette_hover, hovered_hole) {
        let mut probe = circuit.clone();
        let fits = probe.add(kind, at);
        let color = if fits.is_some() { theme::GREEN } else { theme::RED };
        match fits.and_then(|id| probe.part(&id).cloned()) {
            Some(ghost) => {
                draw_part(&painter, &g, &ghost, None, false, 0.55);
                for l in &ghost.legs {
                    painter.circle_stroke(g.hole(*l), p * 0.45, Stroke::new(2.0, color));
                }
            }
            None => {
                painter.circle_stroke(g.hole(at), p * 0.45, Stroke::new(2.0, color));
            }
        }
    }

    if circuit.parts.is_empty() && palette_hover.is_none() {
        empty_hint(&painter, board_rect, "Drag a battery from the parts bin to get started");
    }

    // ---- Hover info ----------------------------------------------------------
    if ed.drag.is_none() && palette_hover.is_none() {
        if let Some(m) = pointer.filter(|m| rect.contains(*m)) {
            let on_part = part_at(&g, circuit, m).or_else(|| hovered_hole.and_then(|h| circuit.occupant(h)).map(|(p, _)| p));
            if let Some(part) = on_part {
                let mut text = part.describe();
                if let Some(i) = view.sim.and_then(|s| s.current(&part.id)) {
                    let i = if part.kind == Kind::Battery { -i } else { i };
                    text.push_str(&format!("\n{:.2} mA", milliamps(i)));
                }
                for (leg, h) in part.legs.iter().enumerate() {
                    let v = view.sim.and_then(|s| s.voltage(h)).map(|v| format!("  {v:.2} V")).unwrap_or_default();
                    text.push_str(&format!("\n{}: {}{v}", part.kind.leg_names()[leg], h.short()));
                }
                resp.clone().on_hover_text_at_pointer(text);
            } else if let Some(h) = hovered_hole {
                let mut text = format!("{}  ·  net {}", h.label(), h.net_key());
                if let Some(v) = view.sim.and_then(|s| s.voltage(&h)) {
                    text.push_str(&format!("\n{v:.3} V"));
                }
                text.push_str("\ndrag from here to draw a wire");
                resp.clone().on_hover_text_at_pointer(text);
            }
        }
    }
    if ed.drag.is_some() || palette_hover.is_some() {
        ui.ctx().request_repaint();
    }
    out
}

fn empty_hint(painter: &Painter, rect: Rect, text: &str) {
    let galley = painter.layout_no_wrap(text.into(), FontId::monospace(13.0), theme::TEXT);
    let r = Rect::from_center_size(rect.center(), galley.size() + vec2(28.0, 18.0));
    painter.rect_filled(r, 6, theme::BG.gamma_multiply(0.85));
    painter.galley(r.center() - galley.size() * 0.5, galley, theme::TEXT);
}

fn draw_part(painter: &Painter, g: &Geom, part: &Part, sim: Option<&SimResult>, selected: bool, alpha: f32) {
    let p = g.p;
    let a = |c: Color32| c.gamma_multiply(alpha);
    let s = layout(g, part);
    let current = sim.and_then(|s| s.current(&part.id));
    let (p0, p1) = (s.legs[0], *s.legs.last().unwrap());
    let (mid, dir, normal) = (s.mid, s.dir, s.normal);
    let tag_font = FontId::monospace((p * 0.42).clamp(9.0, 13.0));
    let small = FontId::monospace((p * 0.34).clamp(8.0, 11.0));

    if selected {
        let glow = Stroke::new(p * 0.5, theme::AMBER.gamma_multiply(0.35));
        for l in &s.legs {
            painter.line_segment([*l, s.body], glow);
        }
        painter.circle_filled(s.body, s.body_r + p * 0.25, theme::AMBER.gamma_multiply(0.25));
    }

    // Leads.
    let lead = Stroke::new(p * 0.09, a(LEAD));
    match part.kind {
        Kind::Wire => {}
        Kind::Battery => {
            for (l, c) in s.legs.iter().zip([RAIL_RED, Color32::from_gray(35)]) {
                painter.line_segment([*l, s.body], Stroke::new(p * 0.2, a(c)));
                painter.circle_filled(*l, p * 0.18, a(c));
            }
        }
        Kind::Pot | Kind::Npn => {
            for l in &s.legs {
                painter.line_segment([*l, *l + (s.body - s.legs[1])], lead);
            }
        }
        _ => {
            painter.line_segment([p0, p1], lead);
            if s.body != mid {
                painter.line_segment([p0, s.body], lead);
                painter.line_segment([p1, s.body], lead);
            }
        }
    }

    match part.kind {
        Kind::Wire => {
            let color = wire_color(part.color.as_deref());
            painter.line_segment([p0, p1], Stroke::new(p * 0.26, a(color)));
            for q in [p0, p1] {
                painter.circle_filled(q, p * 0.2, a(color));
            }
        }
        Kind::Battery => {
            let r = Rect::from_center_size(s.body, vec2(p * 2.2, p * 2.4));
            painter.rect_filled(r, 4, a(Color32::from_rgb(0x24, 0x28, 0x2e)));
            painter.rect_stroke(r, 4, Stroke::new(1.0, a(theme::AMBER)), StrokeKind::Inside);
            let v = part.value.map(|v| fmt_si(v, "V")).unwrap_or("V?".into());
            painter.text(r.center(), Align2::CENTER_CENTER, v.replace(' ', ""), tag_font.clone(), a(theme::AMBER));
            painter.text(r.center_top() + vec2(0.0, p * 0.35), Align2::CENTER_CENTER, "+ −", small.clone(), a(theme::DIM));
        }
        Kind::Resistor | Kind::Diode => {
            let len = ((p1 - p0).length() * 0.55).min(p * 2.6);
            let (b0, b1) = (mid - dir * len * 0.5, mid + dir * len * 0.5);
            let w = p * 0.6;
            let body = if part.kind == Kind::Resistor { Color32::from_rgb(0xd9, 0xb9, 0x8a) } else { Color32::from_gray(30) };
            painter.line_segment([b0, b1], Stroke::new(w, a(body)));
            painter.circle_filled(b0, w * 0.5, a(body));
            painter.circle_filled(b1, w * 0.5, a(body));
            if part.kind == Kind::Resistor {
                if let Some(ohms) = part.value {
                    for (i, c) in bands(ohms).iter().enumerate() {
                        let at = b0 + dir * len * (0.18 + 0.2 * i as f32);
                        painter.line_segment([at - normal * w * 0.5, at + normal * w * 0.5], Stroke::new(p * 0.14, a(*c)));
                    }
                }
            } else {
                // Cathode stripe toward leg 1.
                let at = b1 - dir * len * 0.15;
                painter.line_segment([at - normal * w * 0.5, at + normal * w * 0.5], Stroke::new(p * 0.16, a(Color32::from_gray(200))));
            }
        }
        Kind::Led => {
            let lit = brightness(current);
            let (dark, bright) = led_colors(part.color.as_deref());
            if lit > 0.05 {
                for k in 1..=4 {
                    painter.circle_filled(s.body, p * (0.6 + 0.35 * k as f32), a(bright.gamma_multiply(0.12 * lit)));
                }
            }
            painter.circle_filled(s.body, p * 0.55, a(lerp(dark, bright, lit)));
            painter.circle_stroke(s.body, p * 0.55, Stroke::new(1.0, a(Color32::from_gray(30))));
            // Long leg (+) marker.
            painter.text(p0 - dir * p * 0.55, Align2::CENTER_CENTER, "+", tag_font.clone(), a(RAIL_RED));
        }
        Kind::Capacitor => {
            painter.circle_filled(s.body, p * 0.55, a(Color32::from_rgb(0xd8, 0x8a, 0x2a)));
            painter.circle_stroke(s.body, p * 0.55, Stroke::new(1.0, a(Color32::from_gray(60))));
        }
        Kind::Button => {
            let r = Rect::from_center_size(s.body, Vec2::splat(p * 1.5));
            painter.rect_filled(r, 3, a(Color32::from_gray(40)));
            let cap = if part.pressed { theme::AMBER } else { Color32::from_gray(90) };
            painter.circle_filled(s.body, p * (if part.pressed { 0.42 } else { 0.5 }), a(cap));
        }
        Kind::Pot => {
            let r = Rect::from_center_size(s.body, vec2(p * 2.8, p * 2.0).abs().max(Vec2::splat(p)));
            let r = if dir.x.abs() > dir.y.abs() { r } else { Rect::from_center_size(s.body, vec2(p * 2.0, p * 2.8)) };
            painter.rect_filled(r, 4, a(Color32::from_rgb(0x2f, 0x5f, 0xb8)));
            painter.circle_filled(s.body, p * 0.75, a(Color32::from_gray(225)));
            let ang = (-225.0 + 270.0 * part.wiper as f32).to_radians();
            painter.line_segment([s.body, s.body + vec2(ang.cos(), ang.sin()) * p * 0.7], Stroke::new(p * 0.14, a(INK)));
        }
        Kind::Npn => {
            // TO-92: half disc with the flat face toward the legs.
            let n = 14;
            let mut pts: Vec<Pos2> = (0..=n)
                .map(|k| {
                    let th = std::f32::consts::PI * k as f32 / n as f32;
                    s.legs[1] - normal * p * 0.25 + (dir * th.cos() + normal * th.sin()) * p * 1.25
                })
                .collect();
            pts.push(s.legs[1] - normal * p * 0.25 - dir * p * 1.25);
            painter.add(Shape::convex_polygon(pts, a(Color32::from_gray(35)), Stroke::NONE));
            for (l, n) in s.legs.iter().zip(["C", "B", "E"]) {
                painter.text(*l + normal * p * 0.7, Align2::CENTER_CENTER, n, small.clone(), a(Color32::from_gray(200)));
            }
        }
    }

    // Leg handles on the selected part.
    if selected {
        for l in &s.legs {
            painter.circle_stroke(*l, p * 0.3, Stroke::new(2.0, theme::AMBER));
        }
    }

    // Id tag and live current, kept on opposite sides of the part. Ghosts (fix
    // previews, drag previews) skip the tag so it doesn't overprint the real one.
    if part.kind != Kind::Wire && alpha >= 1.0 {
        let tag_at = match part.kind {
            Kind::Battery => s.body + vec2(0.0, p * 1.6),
            Kind::Led | Kind::Capacitor | Kind::Button => s.body + normal * (s.body_r + p * 0.45),
            Kind::Npn => s.legs[1] + normal * p * 1.75,
            _ => mid - normal * p * 0.75,
        };
        let on_board = board_contains(g, tag_at);
        painter.text(tag_at, Align2::CENTER_CENTER, &part.id, tag_font.clone(), a(if on_board { INK } else { theme::TEXT }));
    }
}

/// Live current tag. Drawn in a pass after every part so a neighbouring part's
/// body can't cover it.
fn draw_readout(painter: &Painter, g: &Geom, part: &Part, sim: Option<&SimResult>) {
    let Some(i) = sim.and_then(|s| s.current(&part.id)) else { return };
    let show = matches!(part.kind, Kind::Resistor | Kind::Led | Kind::Diode | Kind::Npn | Kind::Battery)
        || (part.kind == Kind::Button && part.pressed);
    if !show {
        return;
    }
    let p = g.p;
    let s = layout(g, part);
    let at = match part.kind {
        Kind::Battery => s.body - vec2(0.0, p * 1.7),
        Kind::Led | Kind::Npn => s.mid - s.normal * p * 0.9,
        _ => s.mid - s.normal * p * 1.6,
    };
    let font = FontId::monospace((p * 0.42).clamp(9.0, 13.0));
    let galley = painter.layout_no_wrap(format!("{:.2} mA", milliamps(shown_current(part.kind, i))), font, theme::CYAN);
    let r = Rect::from_center_size(at, galley.size() + vec2(8.0, 4.0));
    painter.rect_filled(r, 3, theme::BG.gamma_multiply(0.9));
    painter.galley(r.min + vec2(4.0, 2.0), galley, theme::CYAN);
}

/// Current as a learner should read it: what a battery supplies, and plain magnitude
/// for parts with no polarity (a resistor's sign only reflects which leg is leg 0).
pub fn shown_current(kind: Kind, i: f64) -> f64 {
    match kind {
        Kind::Battery => -i,
        Kind::Resistor | Kind::Button | Kind::Wire | Kind::Pot | Kind::Capacitor => i.abs(),
        _ => i,
    }
}

pub fn wire_color(name: Option<&str>) -> Color32 {
    match name.unwrap_or("") {
        "red" => Color32::from_rgb(0xd8, 0x3a, 0x30),
        "black" => Color32::from_gray(35),
        "blue" => Color32::from_rgb(0x2f, 0x6c, 0xd8),
        "green" => Color32::from_rgb(0x2f, 0xa8, 0x5a),
        "yellow" => Color32::from_rgb(0xe8, 0xc2, 0x2e),
        "white" => Color32::from_gray(235),
        _ => Color32::from_rgb(0xe0, 0x8a, 0x2a),
    }
}

/// 0..1 glow for an LED current; square root so a few mA already reads as on.
pub fn brightness(current: Option<f64>) -> f32 {
    (current.map(milliamps).unwrap_or(0.0).max(0.0) as f32 / 15.0).sqrt().clamp(0.0, 1.0)
}

/// (unlit, lit) body colors.
pub fn led_colors(name: Option<&str>) -> (Color32, Color32) {
    match name.unwrap_or("red") {
        "green" => (Color32::from_rgb(0x1c, 0x5a, 0x2a), Color32::from_rgb(0x50, 0xff, 0x7a)),
        "blue" => (Color32::from_rgb(0x1a, 0x2c, 0x6a), Color32::from_rgb(0x5a, 0x8c, 0xff)),
        "yellow" => (Color32::from_rgb(0x6a, 0x5a, 0x14), Color32::from_rgb(0xff, 0xe0, 0x40)),
        "orange" => (Color32::from_rgb(0x6a, 0x38, 0x14), Color32::from_rgb(0xff, 0x9a, 0x30)),
        "white" => (Color32::from_gray(0x8a), Color32::from_rgb(0xf4, 0xf6, 0xff)),
        _ => (Color32::from_rgb(0x8a, 0x1c, 0x18), Color32::from_rgb(0xff, 0x4a, 0x38)),
    }
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

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Small icon of a part for the palette.
pub fn icon(painter: &Painter, r: Rect, kind: Kind) {
    let c = r.center();
    let w = r.width();
    let lead = Stroke::new(1.5, LEAD);
    match kind {
        Kind::Battery => {
            let b = Rect::from_center_size(c, vec2(w * 0.5, w * 0.62));
            painter.rect_filled(b, 3, Color32::from_rgb(0x24, 0x28, 0x2e));
            painter.rect_stroke(b, 3, Stroke::new(1.0, theme::AMBER), StrokeKind::Inside);
            painter.text(c, Align2::CENTER_CENTER, "5V", FontId::monospace(9.0), theme::AMBER);
        }
        Kind::Wire => {
            let (a, b) = (c - vec2(w * 0.35, -w * 0.2), c + vec2(w * 0.35, -w * 0.2));
            painter.line_segment([a, b], Stroke::new(3.5, wire_color(Some("red"))));
            painter.circle_filled(a, 3.0, wire_color(Some("red")));
            painter.circle_filled(b, 3.0, wire_color(Some("red")));
        }
        Kind::Resistor | Kind::Diode => {
            painter.line_segment([c - vec2(w * 0.42, 0.0), c + vec2(w * 0.42, 0.0)], lead);
            let body = if kind == Kind::Resistor { Color32::from_rgb(0xd9, 0xb9, 0x8a) } else { Color32::from_gray(30) };
            painter.line_segment([c - vec2(w * 0.2, 0.0), c + vec2(w * 0.2, 0.0)], Stroke::new(w * 0.2, body));
            let bands: &[(f32, Color32)] = if kind == Kind::Resistor {
                &[(-0.12, Color32::from_rgb(0xd0, 0x2a, 0x22)), (-0.02, Color32::from_rgb(0xd0, 0x2a, 0x22)), (0.08, Color32::from_rgb(0x7a, 0x45, 0x1e))]
            } else {
                &[(0.14, Color32::from_gray(200))]
            };
            for (x, col) in bands {
                painter.line_segment([c + vec2(w * x, -w * 0.1), c + vec2(w * x, w * 0.1)], Stroke::new(2.0, *col));
            }
        }
        Kind::Led => {
            painter.line_segment([c + vec2(-w * 0.1, 0.0), c + vec2(-w * 0.1, w * 0.4)], lead);
            painter.line_segment([c + vec2(w * 0.1, 0.0), c + vec2(w * 0.1, w * 0.3)], lead);
            painter.circle_filled(c - vec2(0.0, w * 0.05), w * 0.22, Color32::from_rgb(0xff, 0x4a, 0x38));
        }
        Kind::Button => {
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(w * 0.5)), 3, Color32::from_gray(40));
            painter.circle_filled(c, w * 0.16, Color32::from_gray(110));
        }
        Kind::Pot => {
            painter.rect_filled(Rect::from_center_size(c, vec2(w * 0.6, w * 0.5)), 3, Color32::from_rgb(0x2f, 0x5f, 0xb8));
            painter.circle_filled(c, w * 0.17, Color32::from_gray(225));
            painter.line_segment([c, c + vec2(-w * 0.1, -w * 0.12)], Stroke::new(2.0, INK));
        }
        Kind::Capacitor => {
            painter.line_segment([c + vec2(-w * 0.1, 0.0), c + vec2(-w * 0.1, w * 0.4)], lead);
            painter.line_segment([c + vec2(w * 0.1, 0.0), c + vec2(w * 0.1, w * 0.4)], lead);
            painter.circle_filled(c - vec2(0.0, w * 0.05), w * 0.2, Color32::from_rgb(0xd8, 0x8a, 0x2a));
        }
        Kind::Npn => {
            for x in [-0.15, 0.0, 0.15] {
                painter.line_segment([c + vec2(w * x, 0.0), c + vec2(w * x, w * 0.4)], lead);
            }
            let n = 12;
            let pts: Vec<Pos2> = (0..=n)
                .map(|k| {
                    let th = std::f32::consts::PI * k as f32 / n as f32;
                    c + vec2(th.cos() * w * 0.27, -th.sin() * w * 0.27 + w * 0.05)
                })
                .collect();
            painter.add(Shape::convex_polygon(pts, Color32::from_gray(35), Stroke::NONE));
        }
    }
}
