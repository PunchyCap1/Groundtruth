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

const HOLE: Color32 = Color32::from_gray(0x55);
const LEAD: Color32 = theme::DIM;
/// Positive side of rails, battery leads and symbols.
pub const PLUS: Color32 = Color32::from_rgb(0xd8, 0x4a, 0x3e);
/// Negative side.
pub const MINUS: Color32 = Color32::from_rgb(0x44, 0x78, 0xd8);
/// Flat body colors, close to the real parts.
const RESISTOR: Color32 = Color32::from_rgb(0xc9, 0xa8, 0x7c);
const CERAMIC: Color32 = Color32::from_rgb(0xd0, 0x8a, 0x3a);
const POT: Color32 = Color32::from_rgb(0x2f, 0x5f, 0xb8);
const BLACK_PART: Color32 = Color32::from_gray(0x22);
const PART_EDGE: Color32 = Color32::from_gray(0x66);

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
        Kind::Led | Kind::Capacitor | Kind::Ldr => (mid + normal * p * 0.6, p * 0.6),
        Kind::Npn => (legs[1] + normal * p * 0.55, p * 1.2),
        Kind::Pot => (legs[1] + normal * p * 0.7, p * 1.4),
        Kind::Button | Kind::Switch => (mid, p * 0.9),
        _ => (mid, p * 0.4),
    };
    Shape2 { legs, mid, dir, normal, body, body_r }
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
                if let Some(id) = circuit.add_with_legs(Kind::Wire, vec![*from, to]) {
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
    // The frame reports the rule checks: accent when passing, alert on a fault.
    let outline = match view.checks {
        Some(c) if !c.pass => theme::ALERT,
        Some(_) if !circuit.parts.is_empty() => theme::ACCENT,
        _ => theme::EDGE,
    };
    painter.rect_stroke(board_rect, 0, Stroke::new(1.0, outline), StrokeKind::Outside);

    // Rail rules and the center trench.
    let x0 = g.origin.x + p * 0.5;
    let x1 = g.origin.x + (circuit.rows as f32 + 0.5) * p;
    for (y, c) in [(-0.6, PLUS), (1.6, MINUS), (14.6, MINUS), (16.8, PLUS)] {
        let y = g.origin.y + y * p;
        painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.0, c));
    }
    let trench = g.origin.y + 8.1 * p;
    painter.extend(Shape::dashed_line(&[pos2(x0, trench), pos2(x1, trench)], Stroke::new(1.0, theme::EDGE), 3.0, 5.0));

    // Silkscreen.
    let silk = theme::LOW;
    let font = FontId::monospace((p * 0.4).clamp(8.0, 13.0));
    for c in Col::STRIP {
        let y = g.origin.y + slot(c) * p;
        painter.text(pos2(g.origin.x - p * 0.1, y), Align2::RIGHT_CENTER, c.name(), font.clone(), silk);
        painter.text(pos2(x1 + p * 0.6, y), Align2::LEFT_CENTER, c.name(), font.clone(), silk);
    }
    for (c, color) in [(Col::LPlus, PLUS), (Col::LMinus, MINUS), (Col::RMinus, MINUS), (Col::RPlus, PLUS)] {
        let sign = if matches!(c, Col::LPlus | Col::RPlus) { "+" } else { "−" };
        let y = g.origin.y + slot(c) * p;
        painter.text(pos2(x1 + p * 0.6, y), Align2::LEFT_CENTER, sign, font.clone(), color);
    }
    for row in (1..=circuit.rows).filter(|r| *r == 1 || r % 5 == 0) {
        let x = g.origin.x + row as f32 * p;
        painter.text(pos2(x, g.origin.y + 2.0 * p), Align2::CENTER_CENTER, row.to_string(), font.clone(), silk);
        painter.text(pos2(x, g.origin.y + 14.0 * p), Align2::CENTER_CENTER, row.to_string(), font.clone(), silk);
    }

    // Holes as a dot grid, with net tracing on hover.
    let hovered_hole = pointer.filter(|m| rect.contains(*m)).and_then(|m| g.nearest(m));
    let hovered_net = hovered_hole.map(|h| h.net_key());
    for h in circuit.holes() {
        let c = g.hole(h);
        if hovered_net.as_deref() == Some(h.net_key().as_str()) {
            painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(p * 0.6)), 0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Middle);
        }
        painter.circle_filled(c, (p * 0.07).max(1.0), HOLE);
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
            painter.rect_stroke(r, 0, Stroke::new(1.0, theme::HI.gamma_multiply(0.4 + 0.6 * pulse)), StrokeKind::Outside);
            ui.ctx().request_repaint_after(theme::PULSE_FRAME);
        }
    }

    // Rule-checker faults and suggested fixes.
    if let Some(checks) = view.checks {
        for f in checks.findings.iter().filter(|f| f.severity == Severity::Fault) {
            for h in &f.highlight {
                painter.circle_stroke(g.hole(*h), p * (0.5 + 0.15 * pulse), Stroke::new(1.5, theme::ALERT.gamma_multiply(0.4 + 0.6 * pulse)));
            }
        }
        if view.show_fix {
            for fix in checks.findings.iter().filter_map(|f| f.fix.as_ref()) {
                let Some(part) = circuit.part(&fix.part) else { continue };
                let mut ghost = part.clone();
                for (leg, to) in &fix.moves {
                    if let Some(l) = ghost.legs.get_mut(*leg) {
                        painter.extend(Shape::dashed_line(&[g.hole(*l), g.hole(*to)], Stroke::new(1.0, theme::ACCENT), 4.0, 4.0));
                        *l = *to;
                    }
                }
                draw_part(&painter, &g, &ghost, None, false, 0.45);
                for (_, to) in &fix.moves {
                    painter.circle_stroke(g.hole(*to), p * 0.45, Stroke::new(1.0, theme::ACCENT));
                }
            }
        }
        if checks.findings.iter().any(|f| f.severity == Severity::Fault) {
            ui.ctx().request_repaint_after(theme::PULSE_FRAME);
        }
    }

    // ---- Drag previews -------------------------------------------------------
    if let (Some(Drag::NewWire { from }), Some(m)) = (&ed.drag, pointer) {
        let target = g.nearest(m).filter(|h| h != from && circuit.occupant(*h).is_none());
        let end = target.map(|h| g.hole(h)).unwrap_or(m);
        painter.line_segment([g.hole(*from), end], Stroke::new(p * 0.13, theme::HI));
        painter.circle_filled(g.hole(*from), p * 0.13, theme::HI);
        if let Some(h) = target {
            painter.circle_stroke(g.hole(h), p * 0.4, Stroke::new(1.0, theme::HI));
        }
    }
    if let (Some(kind), Some(at)) = (palette_hover, hovered_hole) {
        let mut probe = circuit.clone();
        let fits = probe.add(kind, at);
        let color = if fits.is_some() { theme::ACCENT } else { theme::ALERT };
        match fits.and_then(|id| probe.part(&id).cloned()) {
            Some(ghost) => {
                draw_part(&painter, &g, &ghost, None, false, 0.55);
                for l in &ghost.legs {
                    painter.circle_stroke(g.hole(*l), p * 0.4, Stroke::new(1.0, color));
                }
            }
            None => {
                painter.circle_stroke(g.hole(at), p * 0.4, Stroke::new(1.0, color));
            }
        }
    }

    if circuit.parts.is_empty() && palette_hover.is_none() {
        empty_hint(&painter, board_rect, "drag a battery from the parts bin to start");
    }

    // ---- Hover info ----------------------------------------------------------
    if ed.drag.is_none() && palette_hover.is_none() {
        if let Some(m) = pointer.filter(|m| rect.contains(*m)) {
            let on_part = part_at(&g, circuit, m).or_else(|| hovered_hole.and_then(|h| circuit.occupant(h)).map(|(p, _)| p));
            if let Some(part) = on_part {
                let mut text = part.describe();
                if let Some(i) = view.sim.and_then(|s| s.current(&part.id)) {
                    text.push_str(&format!("\n{:.2} mA", milliamps(shown_current(part.kind, i))));
                }
                for (leg, h) in part.legs.iter().enumerate() {
                    let v = view.sim.and_then(|s| s.voltage(h)).map(|v| format!("  {v:.2} V")).unwrap_or_default();
                    text.push_str(&format!("\n{}: {}{v}", part.kind.leg_names()[leg], h.short()));
                }
                resp.clone().on_hover_text_at_pointer(text);
            } else if let Some(h) = hovered_hole {
                let mut text = format!("{}  ·  net {}", h.label(), h.net_key());
                if let Some(v) = view.sim.and_then(|s| s.voltage(&h)) {
                    text.push_str(&format!("\n{v:.2} V"));
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
    painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::monospace(12.0), theme::DIM);
}

fn draw_part(painter: &Painter, g: &Geom, part: &Part, sim: Option<&SimResult>, selected: bool, alpha: f32) {
    let p = g.p;
    let a = |c: Color32| c.gamma_multiply(alpha);
    let s = layout(g, part);
    let current = sim.and_then(|s| s.current(&part.id));
    let (p0, p1) = (s.legs[0], *s.legs.last().unwrap());
    let (mid, dir, normal) = (s.mid, s.dir, s.normal);
    let tag_font = FontId::monospace((p * 0.4).clamp(9.0, 12.0));
    let small = FontId::monospace((p * 0.32).clamp(8.0, 10.0));
    // Flat bodies in the parts' real colors; dark ones get a grey edge, the selected part a white one.
    let ink = a(if selected { theme::HI } else { theme::TEXT });
    let edge = |dark: bool| match (selected, dark) {
        (true, _) => Stroke::new(1.0, theme::HI),
        (false, true) => Stroke::new(1.0, a(PART_EDGE)),
        (false, false) => Stroke::NONE,
    };

    // Leads.
    let lead = Stroke::new(1.0, a(LEAD));
    match part.kind {
        Kind::Wire => {}
        Kind::Battery => {
            for (l, c) in s.legs.iter().zip([PLUS, Color32::from_gray(0x77)]) {
                painter.line_segment([*l, s.body], Stroke::new(1.0, a(c)));
                painter.circle_filled(*l, p * 0.12, a(c));
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
            let color = a(wire_color(part.color.as_deref()));
            let w = if selected { p * 0.2 } else { p * 0.13 };
            painter.line_segment([p0, p1], Stroke::new(w, color));
            for q in [p0, p1] {
                painter.circle_filled(q, p * 0.13, color);
            }
        }
        Kind::Battery => {
            let r = Rect::from_center_size(s.body, vec2(p * 2.2, p * 2.4));
            painter.rect(r, 0, a(BLACK_PART), edge(true), StrokeKind::Inside);
            let v = part.value.map(|v| fmt_si(v, "V")).unwrap_or("V?".into());
            painter.text(r.center(), Align2::CENTER_CENTER, v.replace(' ', ""), tag_font.clone(), ink);
            painter.text(r.center_top() + vec2(0.0, p * 0.35), Align2::CENTER_CENTER, "+ −", small.clone(), a(theme::DIM));
        }
        Kind::Resistor | Kind::Diode => {
            let len = ((p1 - p0).length() * 0.55).min(p * 2.4);
            let w = p * 0.5;
            let (b0, b1) = (mid - dir * len * 0.5, mid + dir * len * 0.5);
            let body = vec![b0 + normal * w * 0.5, b1 + normal * w * 0.5, b1 - normal * w * 0.5, b0 - normal * w * 0.5];
            let resistor = part.kind == Kind::Resistor;
            painter.add(Shape::convex_polygon(body, a(if resistor { RESISTOR } else { BLACK_PART }), edge(!resistor)));
            if resistor {
                if let Some(ohms) = part.value {
                    for (i, c) in bands(ohms).iter().enumerate() {
                        let at = b0 + dir * len * (0.22 + 0.2 * i as f32);
                        painter.line_segment([at - normal * w * 0.5, at + normal * w * 0.5], Stroke::new(p * 0.12, a(*c)));
                    }
                }
            } else {
                // Cathode stripe toward leg 1.
                let at = b1 - dir * len * 0.2;
                painter.line_segment([at - normal * w * 0.5, at + normal * w * 0.5], Stroke::new(p * 0.12, a(Color32::from_gray(0xcc))));
            }
        }
        Kind::Led => {
            let lit = brightness(current);
            let (_, bright) = led_colors(part.color.as_deref());
            let r = p * 0.5;
            if lit > 0.05 {
                painter.circle_stroke(s.body, r + p * 0.3, Stroke::new(1.0, a(bright.gamma_multiply(lit))));
            }
            let edge = if lit > 0.05 { bright } else { bright.gamma_multiply(0.55) };
            painter.circle(s.body, r, a(lerp(theme::BG, bright, lit)), Stroke::new(1.0, a(edge)));
            // Long leg (+) marker.
            painter.text(p0 - dir * p * 0.55, Align2::CENTER_CENTER, "+", small.clone(), a(theme::DIM));
        }
        Kind::Capacitor => {
            painter.circle(s.body, p * 0.5, a(CERAMIC), edge(false));
        }
        Kind::Ldr => {
            // Face brightens with the light level; the serpentine track is the sensor.
            let face = lerp(Color32::from_rgb(0x6a, 0x48, 0x28), Color32::from_rgb(0xf0, 0xd0, 0x90), part.light as f32);
            painter.circle(s.body, p * 0.55, a(face), edge(false));
            let pts: Vec<Pos2> = (0..=5)
                .map(|k| s.body + dir * p * (k as f32 * 0.16 - 0.4) + normal * p * if k % 2 == 0 { 0.25 } else { -0.25 })
                .collect();
            painter.add(Shape::line(pts, Stroke::new(1.5, a(Color32::from_rgb(0xa8, 0x32, 0x22)))));
        }
        Kind::Button => {
            let r = Rect::from_center_size(s.body, Vec2::splat(p * 1.5));
            painter.rect(r, 0, a(BLACK_PART), edge(true), StrokeKind::Inside);
            let cap = if part.pressed { theme::ACCENT } else { Color32::from_gray(0x88) };
            painter.circle_filled(s.body, p * if part.pressed { 0.32 } else { 0.4 }, a(cap));
        }
        Kind::Switch => {
            let along = if dir.x.abs() > dir.y.abs() { vec2(p * 2.0, p * 1.1) } else { vec2(p * 1.1, p * 2.0) };
            painter.rect(Rect::from_center_size(s.body, along), 0, a(BLACK_PART), edge(true), StrokeKind::Inside);
            // Knob sits toward leg 1 when on.
            let knob = Rect::from_center_size(s.body + dir * p * if part.pressed { 0.4 } else { -0.4 }, Vec2::splat(p * 0.6));
            painter.rect_filled(knob, 0, a(if part.pressed { theme::ACCENT } else { Color32::from_gray(0x99) }));
        }
        Kind::Pot => {
            let size = if dir.x.abs() > dir.y.abs() { vec2(p * 2.8, p * 2.0) } else { vec2(p * 2.0, p * 2.8) };
            painter.rect(Rect::from_center_size(s.body, size), 0, a(POT), edge(false), StrokeKind::Inside);
            painter.circle_filled(s.body, p * 0.7, a(Color32::from_gray(0xdd)));
            let ang = (-225.0 + 270.0 * part.wiper as f32).to_radians();
            painter.line_segment([s.body, s.body + vec2(ang.cos(), ang.sin()) * p * 0.7], Stroke::new(1.5, a(BLACK_PART)));
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
            painter.add(Shape::convex_polygon(pts, a(BLACK_PART), edge(true)));
            for (l, n) in s.legs.iter().zip(["C", "B", "E"]) {
                painter.text(*l + normal * p * 0.7, Align2::CENTER_CENTER, n, small.clone(), a(theme::DIM));
            }
        }
    }

    // Leg handles on the selected part.
    if selected {
        for l in &s.legs {
            painter.rect_stroke(Rect::from_center_size(*l, Vec2::splat(p * 0.5)), 0, Stroke::new(1.0, theme::HI), StrokeKind::Middle);
        }
    }

    // Id tag (with the value, for passives) and live current, kept on opposite sides of the part.
    // Ghosts (fix previews, drag previews) skip the tag so it doesn't overprint the real one.
    if part.kind != Kind::Wire && alpha >= 1.0 {
        let tag_at = match part.kind {
            Kind::Battery => s.body + vec2(0.0, p * 1.6),
            Kind::Led | Kind::Capacitor | Kind::Ldr | Kind::Button => s.body + normal * (s.body_r + p * 0.45),
            Kind::Npn => s.legs[1] + normal * p * 1.75,
            _ => mid - normal * p * 0.75,
        };
        let tag = match (part.kind, part.kind.unit(), part.value) {
            (Kind::Resistor | Kind::Capacitor | Kind::Pot, Some(unit), Some(v)) => format!("{} {}", part.id, fmt_si(v, unit).replace(' ', "")),
            _ => part.id.clone(),
        };
        painter.text(tag_at, Align2::CENTER_CENTER, tag, tag_font.clone(), a(if selected { theme::HI } else { theme::DIM }));
    }
}

/// Live current tag. Drawn in a pass after every part so a neighbouring part's
/// body can't cover it.
fn draw_readout(painter: &Painter, g: &Geom, part: &Part, sim: Option<&SimResult>) {
    let Some(i) = sim.and_then(|s| s.current(&part.id)) else { return };
    let show = matches!(part.kind, Kind::Resistor | Kind::Led | Kind::Diode | Kind::Npn | Kind::Battery | Kind::Ldr | Kind::Capacitor)
        || (matches!(part.kind, Kind::Button | Kind::Switch) && part.pressed);
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
    let small = FontId::monospace((p * 0.34).clamp(8.0, 11.0));
    let galley = painter.layout_no_wrap(format!("{:.2}mA", milliamps(shown_current(part.kind, i))), small, theme::ACCENT);
    let r = Rect::from_center_size(at, galley.size() + vec2(4.0, 0.0));
    painter.rect_filled(r, 0, theme::BG);
    painter.galley(r.min + vec2(2.0, 0.0), galley, theme::ACCENT);
}

/// Current as a learner should read it: what a battery supplies, and plain magnitude
/// for parts with no polarity (a resistor's sign only reflects which leg is leg 0).
pub fn shown_current(kind: Kind, i: f64) -> f64 {
    match kind {
        Kind::Battery => -i,
        Kind::Resistor | Kind::Button | Kind::Switch | Kind::Wire | Kind::Pot | Kind::Capacitor | Kind::Ldr => i.abs(),
        _ => i,
    }
}

pub fn wire_color(name: Option<&str>) -> Color32 {
    match name.unwrap_or("") {
        // A "black" wire is drawn dark grey so it shows on the black board.
        "red" => Color32::from_rgb(0xd8, 0x3a, 0x30),
        "black" => Color32::from_gray(0x55),
        "blue" => Color32::from_rgb(0x2f, 0x6c, 0xd8),
        "green" => Color32::from_rgb(0x2f, 0xa8, 0x5a),
        "yellow" => Color32::from_rgb(0xe8, 0xc2, 0x2e),
        "white" => Color32::from_gray(0xeb),
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

/// Small line drawing of a part, shown under the pointer while dragging from the parts bin.
pub fn icon(painter: &Painter, r: Rect, kind: Kind) {
    let c = r.center();
    let w = r.width();
    let ink = Stroke::new(1.0, theme::TEXT);
    let lead = Stroke::new(1.0, LEAD);
    let fill = theme::BG;
    let legs = |xs: &[f32], len: f32| {
        for x in xs {
            painter.line_segment([c + vec2(w * x, 0.0), c + vec2(w * x, w * len)], lead);
        }
    };
    match kind {
        Kind::Battery => {
            let b = Rect::from_center_size(c, vec2(w * 0.5, w * 0.62));
            painter.rect(b, 0, fill, ink, StrokeKind::Inside);
            painter.text(c, Align2::CENTER_CENTER, "5V", FontId::monospace(9.0), theme::TEXT);
        }
        Kind::Wire => {
            let color = wire_color(Some("red"));
            let (a, b) = (c - vec2(w * 0.35, -w * 0.2), c + vec2(w * 0.35, -w * 0.2));
            painter.line_segment([a, b], Stroke::new(2.0, color));
            painter.circle_filled(a, 2.5, color);
            painter.circle_filled(b, 2.5, color);
        }
        Kind::Resistor | Kind::Diode => {
            painter.line_segment([c - vec2(w * 0.42, 0.0), c + vec2(w * 0.42, 0.0)], lead);
            let body = if kind == Kind::Resistor { RESISTOR } else { BLACK_PART };
            painter.rect_filled(Rect::from_center_size(c, vec2(w * 0.42, w * 0.18)), 0, body);
            if kind == Kind::Diode {
                painter.line_segment([c + vec2(w * 0.12, -w * 0.09), c + vec2(w * 0.12, w * 0.09)], Stroke::new(2.0, theme::TEXT));
            }
        }
        Kind::Led => {
            legs(&[-0.1, 0.1], 0.4);
            painter.circle(c - vec2(0.0, w * 0.05), w * 0.2, fill, Stroke::new(1.0, led_colors(Some("red")).1));
        }
        Kind::Capacitor => {
            legs(&[-0.1, 0.1], 0.4);
            painter.circle_filled(c - vec2(0.0, w * 0.05), w * 0.2, CERAMIC);
        }
        Kind::Ldr => {
            legs(&[-0.1, 0.1], 0.4);
            let face = c - vec2(0.0, w * 0.05);
            painter.circle(face, w * 0.22, fill, ink);
            let pts: Vec<Pos2> =
                (0..=5).map(|k| face + vec2(w * (k as f32 * 0.06 - 0.15), if k % 2 == 0 { -w * 0.08 } else { w * 0.08 })).collect();
            painter.add(Shape::line(pts, ink));
        }
        Kind::Button => {
            painter.rect(Rect::from_center_size(c, Vec2::splat(w * 0.5)), 0, fill, ink, StrokeKind::Inside);
            painter.circle_stroke(c, w * 0.13, ink);
        }
        Kind::Switch => {
            painter.rect(Rect::from_center_size(c, vec2(w * 0.62, w * 0.32)), 0, fill, ink, StrokeKind::Inside);
            painter.rect_filled(Rect::from_center_size(c + vec2(w * 0.12, 0.0), Vec2::splat(w * 0.18)), 0, theme::TEXT);
        }
        Kind::Pot => {
            painter.rect_filled(Rect::from_center_size(c, vec2(w * 0.6, w * 0.5)), 0, POT);
            painter.circle_filled(c, w * 0.15, Color32::from_gray(0xdd));
            painter.line_segment([c, c + vec2(-w * 0.1, -w * 0.12)], Stroke::new(1.5, BLACK_PART));
        }
        Kind::Npn => {
            legs(&[-0.15, 0.0, 0.15], 0.4);
            let n = 12;
            let pts: Vec<Pos2> = (0..=n)
                .map(|k| {
                    let th = std::f32::consts::PI * k as f32 / n as f32;
                    c + vec2(th.cos() * w * 0.27, -th.sin() * w * 0.27 + w * 0.05)
                })
                .collect();
            painter.add(Shape::convex_polygon(pts, fill, ink));
        }
    }
}
