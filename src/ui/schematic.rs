//! Schematic view generated from the breadboard. Jumper wires collapse into
//! their nets; nets are stacked by simulated voltage (highest on top), and each
//! part hangs in its own column between the nets its legs touch. That makes a
//! reversed LED visibly point uphill.

use super::board::{MINUS, PLUS, brightness, led_colors, lerp};
use super::theme;
use crate::circuit::{Circuit, Kind, Part, fmt_si};
use crate::sim::SimResult;
use crate::sim::checks::milliamps;
use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};
use std::collections::BTreeMap;

/// Union-find over board nets, joining nets connected by jumper wires.
fn merged_nets(circuit: &Circuit) -> BTreeMap<String, String> {
    let mut parent: BTreeMap<String, String> = circuit.nets().keys().map(|k| (k.clone(), k.clone())).collect();
    fn find(parent: &mut BTreeMap<String, String>, k: &str) -> String {
        let p = parent.get(k).cloned().unwrap_or_else(|| k.to_string());
        if p == k {
            return p;
        }
        let root = find(parent, &p);
        parent.insert(k.to_string(), root.clone());
        root
    }
    for w in circuit.parts.iter().filter(|p| p.kind == Kind::Wire) {
        let a = find(&mut parent, &w.legs[0].net_key());
        let b = find(&mut parent, &w.legs[1].net_key());
        if a != b {
            // Prefer a rail name as the representative, it reads better.
            let (keep, drop) = if b.ends_with('+') || b.ends_with('-') { (b, a) } else { (a, b) };
            parent.insert(drop, keep);
        }
    }
    let keys: Vec<String> = parent.keys().cloned().collect();
    keys.into_iter().map(|k| (k.clone(), find(&mut parent, &k))).collect()
}

struct Placed<'a> {
    part: &'a Part,
    x: f32,
    /// Node per leg.
    nodes: Vec<String>,
}

pub fn show(ui: &mut egui::Ui, circuit: &Circuit, sim: Option<&SimResult>, selected: &mut Option<String>, flagged: &[String]) {
    let root = merged_nets(circuit);
    let node_of = |part: &Part, leg: usize| root.get(&part.legs[leg].net_key()).cloned().unwrap_or_default();
    let parts: Vec<&Part> = circuit.parts.iter().filter(|p| p.kind != Kind::Wire).collect();

    if parts.is_empty() {
        let (r, painter) = ui.allocate_painter(ui.available_size(), Sense::hover());
        painter.text(r.rect.center(), Align2::CENTER_CENTER, "the schematic draws itself as you build on the breadboard", FontId::monospace(12.0), theme::DIM);
        return;
    }

    // Order nodes top to bottom by voltage (or by battery polarity before the first simulation).
    let ground = circuit.parts.iter().find(|p| p.kind == Kind::Battery).map(|b| node_of(b, 1));
    let positive: Vec<String> = circuit.parts.iter().filter(|p| p.kind == Kind::Battery).map(|b| node_of(b, 0)).collect();
    let mut node_v: BTreeMap<String, f64> = BTreeMap::new();
    for part in &parts {
        for leg in 0..part.legs.len() {
            let node = node_of(part, leg);
            let v = sim
                .and_then(|s| s.voltage(&part.legs[leg]))
                .unwrap_or(if positive.contains(&node) { 1e6 } else if Some(&node) == ground.as_ref() { -1e6 } else { 0.0 });
            node_v.entry(node).or_insert(v);
        }
    }
    let mut nodes: Vec<(String, f64)> = node_v.clone().into_iter().collect();
    nodes.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let rank: BTreeMap<String, usize> = nodes.iter().enumerate().map(|(i, (n, _))| (n.clone(), i)).collect();

    // Columns: batteries first, then by the span of nets each part bridges.
    let mut ordered = parts.clone();
    ordered.sort_by_key(|p| {
        let rs: Vec<usize> = (0..p.legs.len()).map(|l| rank[&node_of(p, l)]).collect();
        (p.kind != Kind::Battery, *rs.iter().min().unwrap(), *rs.iter().max().unwrap())
    });

    let avail = ui.available_size();
    let margin = vec2(110.0, 50.0);
    let dy = ((avail.y - 2.0 * margin.y) / (nodes.len().max(2) - 1) as f32).clamp(52.0, 120.0);
    let dx = ((avail.x - 2.0 * margin.x) / ordered.len() as f32).clamp(90.0, 150.0);
    let needed = vec2(margin.x * 2.0 + dx * ordered.len() as f32, margin.y * 2.0 + dy * (nodes.len().max(2) - 1) as f32);

    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        let (resp, painter) = ui.allocate_painter(needed.max(avail), Sense::click());
        let origin = resp.rect.min + margin;
        let y_of = |node: &str| origin.y + rank[node] as f32 * dy;
        let placed: Vec<Placed> = ordered
            .iter()
            .enumerate()
            .map(|(i, p)| Placed { part: p, x: origin.x + dx * (i as f32 + 0.5), nodes: (0..p.legs.len()).map(|l| node_of(p, l)).collect() })
            .collect();
        let sym = (dy * 0.55).min(52.0);
        let pointer = resp.hover_pos();
        let mut hovered: Option<&Part> = None;

        // Each part's attach points: (node, x).
        let mut attach: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        for pl in &placed {
            for (leg, node) in pl.nodes.iter().enumerate() {
                attach.entry(node.clone()).or_default().push(pl.x + side_offset(pl.part.kind, leg, dx, pl.nodes.iter().all(|n| n == node)));
            }
        }

        // Buses.
        for (node, xs) in &attach {
            let y = y_of(node);
            let (x0, x1) = xs.iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
            let wire = Stroke::new(1.0, theme::DIM);
            painter.line_segment([pos2(x0 - 18.0, y), pos2(x1, y)], wire);
            if xs.len() >= 3 {
                for x in xs {
                    painter.circle_filled(pos2(*x, y), 2.5, theme::TEXT);
                }
            }
            let is_ground = Some(node) == ground.as_ref();
            let v = sim.and_then(|_| node_v.get(node)).map(|v| format!("{v:.2} V")).unwrap_or_default();
            let label = if is_ground { format!("GND {v}") } else { v };
            painter.text(pos2(x0 - 24.0, y), Align2::RIGHT_CENTER, label, FontId::monospace(11.0), if is_ground { theme::DIM } else { theme::ACCENT });
            if is_ground {
                let g = pos2(x0 - 18.0, y);
                painter.line_segment([g, g + vec2(0.0, 10.0)], wire);
                for (k, w) in [(0.0, 10.0), (4.0, 6.0), (8.0, 2.5)] {
                    painter.line_segment([g + vec2(-w, 10.0 + k), g + vec2(w, 10.0 + k)], wire);
                }
            }
        }

        // Parts.
        for pl in &placed {
            let part = pl.part;
            let is_sel = selected.as_deref() == Some(part.id.as_str());
            let ink = if is_sel { theme::HI } else { theme::TEXT };
            let stroke = Stroke::new(if is_sel { 1.6 } else { 1.0 }, ink);
            let current = sim.and_then(|s| s.current(&part.id));
            let (y0, y1) = (y_of(&pl.nodes[0]), y_of(pl.nodes.last().unwrap()));
            let shorted = pl.nodes.iter().all(|n| n == &pl.nodes[0]);

            let bounds = if shorted && part.legs.len() == 2 {
                // Both legs on one net: hang the part under its bus, sideways.
                let c = pos2(pl.x, y0 + dy * 0.38);
                let half = dx * 0.28;
                for sx in [-half, half] {
                    painter.line_segment([pos2(pl.x + sx, y0), pos2(pl.x + sx, c.y)], stroke);
                }
                painter.line_segment([pos2(pl.x - half, c.y), c - vec2(sym * 0.5, 0.0)], stroke);
                painter.line_segment([pos2(pl.x + half, c.y), c + vec2(sym * 0.5, 0.0)], stroke);
                symbol(&painter, part, c, vec2(1.0, 0.0), sym, stroke, current);
                Rect::from_center_size(c, vec2(dx * 0.6, sym * 0.8))
            } else {
                let (top, bottom) = (y0.min(y1), y0.max(y1));
                let c = pos2(pl.x, (y0 + y1) * 0.5);
                let u = if y1 >= y0 { vec2(0.0, 1.0) } else { vec2(0.0, -1.0) };
                match part.kind {
                    Kind::Pot => {
                        painter.line_segment([pos2(pl.x, y0), c - u * sym * 0.5], stroke);
                        painter.line_segment([c + u * sym * 0.5, pos2(pl.x, y1)], stroke);
                        let xw = pl.x + side_offset(Kind::Pot, 1, dx, false);
                        let yw = y_of(&pl.nodes[1]);
                        painter.line_segment([pos2(xw, c.y), pos2(xw, yw)], stroke);
                        painter.line_segment([pos2(xw, c.y), c + vec2(sym * 0.32, 0.0)], stroke);
                    }
                    Kind::Npn => {
                        let xb = pl.x + side_offset(Kind::Npn, 1, dx, false);
                        let yb = y_of(&pl.nodes[1]);
                        painter.line_segment([pos2(xb, c.y), pos2(xb, yb)], stroke);
                        painter.line_segment([pos2(xb, c.y), c - vec2(sym * 0.2, 0.0)], stroke);
                        let r = sym * 0.45;
                        painter.line_segment([pos2(pl.x, y0), pos2(pl.x, c.y + (y0 - c.y).signum() * r * 0.75)], stroke);
                        painter.line_segment([pos2(pl.x, y1), pos2(pl.x, c.y + (y1 - c.y).signum() * r * 0.75)], stroke);
                    }
                    _ => {
                        let half = (sym * 0.5).min((bottom - top) * 0.5);
                        painter.line_segment([pos2(pl.x, y0), c - u * half], stroke);
                        painter.line_segment([c + u * half, pos2(pl.x, y1)], stroke);
                    }
                }
                let len = sym.min((bottom - top).max(sym * 0.6));
                symbol(&painter, part, c, u, len, stroke, current);
                Rect::from_center_size(c, vec2(dx * 0.6, (bottom - top).max(sym)))
            };

            // Labels to the right of the symbol.
            let mut lines = vec![part.id.clone()];
            if let (Some(v), Some(unit)) = (part.value, part.kind.unit()) {
                lines.push(fmt_si(v, unit));
            }
            let label_at = pos2(bounds.center().x + sym * 0.45, bounds.center().y - 8.0);
            painter.text(label_at, Align2::LEFT_BOTTOM, lines.join(" "), FontId::monospace(11.0), ink);
            if let Some(i) = current {
                let i = if part.kind == Kind::Battery { -i } else { i };
                painter.text(label_at + vec2(0.0, 4.0), Align2::LEFT_TOP, format!("{:.2} mA", milliamps(i)), FontId::monospace(11.0), theme::ACCENT);
            }
            if flagged.contains(&part.id) {
                painter.rect_stroke(bounds.expand(6.0), 0, Stroke::new(1.0, theme::HI), StrokeKind::Outside);
            }
            if pointer.is_some_and(|m| bounds.contains(m)) {
                hovered = Some(part);
                painter.rect_stroke(bounds.expand(4.0), 0, Stroke::new(1.0, theme::EDGE), StrokeKind::Outside);
            }
        }

        if resp.clicked() {
            *selected = hovered.map(|p| p.id.clone());
        }
        if let Some(p) = hovered {
            resp.on_hover_text_at_pointer(p.describe());
        }
        if !flagged.is_empty() {
            ui.ctx().request_repaint();
        }
    });
}

/// Horizontal offset of a leg's connection point from the part's column.
fn side_offset(kind: Kind, leg: usize, dx: f32, shorted: bool) -> f32 {
    match (kind, leg) {
        (Kind::Pot, 1) => dx * 0.32,
        (Kind::Npn, 1) => -dx * 0.32,
        (_, 0) if shorted => -dx * 0.28,
        (_, 1) if shorted => dx * 0.28,
        _ => 0.0,
    }
}

/// Draws a part's symbol centered on `c`, running along `u` from leg 0 to the last leg.
fn symbol(painter: &Painter, part: &Part, c: Pos2, u: Vec2, len: f32, stroke: Stroke, current: Option<f64>) {
    // Side of the symbol that decorations (arrows, wiper) sit on: right, or up for sideways parts.
    let n = if u.y.abs() > 0.5 { vec2(1.0, 0.0) } else { vec2(0.0, -1.0) };
    let (a, b) = (c - u * len * 0.5, c + u * len * 0.5);
    match part.kind {
        Kind::Wire => {}
        Kind::Resistor | Kind::Pot | Kind::Ldr => {
            let steps = 6;
            let mut pts = vec![a];
            for k in 0..steps {
                let t = (k as f32 + 0.5) / steps as f32;
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                pts.push(a + u * len * t + n * len * 0.16 * side);
            }
            pts.push(b);
            painter.add(Shape::line(pts, stroke));
            if part.kind == Kind::Pot {
                // Wiper arrow from the right.
                let tip = c + n * len * 0.2;
                painter.line_segment([c + n * len * 0.32, tip], stroke);
                arrow_head(painter, tip, -n, len * 0.12, stroke);
            }
            if part.kind == Kind::Ldr {
                // Incoming light arrows.
                let thin = Stroke::new(1.2, stroke.color);
                let d = (-n + u * 0.3).normalized();
                for k in [-0.1, 0.08] {
                    let e = c + n * len * 0.22 + u * len * k;
                    painter.line_segment([e - d * len * 0.22, e], thin);
                    arrow_head(painter, e, d, len * 0.08, thin);
                }
            }
        }
        Kind::Switch => {
            let (d0, d1) = (c - u * len * 0.25, c + u * len * 0.25);
            painter.line_segment([a, d0], stroke);
            painter.line_segment([d1, b], stroke);
            painter.circle_stroke(d0, 3.0, stroke);
            painter.circle_stroke(d1, 3.0, stroke);
            let end = if part.pressed { d1 } else { d1 - n * len * 0.3 };
            painter.line_segment([d0, end], stroke);
        }
        Kind::Battery => {
            painter.line_segment([a, c - u * len * 0.08], stroke);
            painter.line_segment([b, c + u * len * 0.08], stroke);
            let long = c - u * len * 0.08;
            let short = c + u * len * 0.08;
            painter.line_segment([long - n * len * 0.38, long + n * len * 0.38], Stroke::new(stroke.width, PLUS));
            painter.line_segment([short - n * len * 0.2, short + n * len * 0.2], Stroke::new(stroke.width * 2.2, MINUS));
            painter.text(long + n * len * 0.5 - u * len * 0.12, Align2::CENTER_CENTER, "+", FontId::monospace(12.0), PLUS);
        }
        Kind::Capacitor => {
            let gap = len * 0.08;
            painter.line_segment([a, c - u * gap], stroke);
            painter.line_segment([b, c + u * gap], stroke);
            for s in [-gap, gap] {
                let m = c + u * s;
                painter.line_segment([m - n * len * 0.32, m + n * len * 0.32], stroke);
            }
        }
        Kind::Led | Kind::Diode => {
            let base = c - u * len * 0.22;
            let tip = c + u * len * 0.22;
            painter.line_segment([a, base], stroke);
            painter.line_segment([tip, b], stroke);
            let w = len * 0.26;
            let fill = if part.kind == Kind::Led {
                let lit = brightness(current);
                let (dark, bright) = led_colors(part.color.as_deref());
                lerp(dark, bright, lit).gamma_multiply(0.3 + 0.7 * lit)
            } else {
                Color32::TRANSPARENT
            };
            painter.add(Shape::convex_polygon(vec![base - n * w, base + n * w, tip], fill, stroke));
            painter.line_segment([tip - n * w, tip + n * w], stroke);
            if part.kind == Kind::Led {
                for k in [0.0, 0.18] {
                    let s = c + n * len * 0.3 + u * len * (k - 0.1);
                    let e = s + (n + u * -0.3).normalized() * len * 0.22;
                    painter.line_segment([s, e], Stroke::new(1.2, stroke.color));
                    arrow_head(painter, e, (n + u * -0.3).normalized(), len * 0.08, Stroke::new(1.2, stroke.color));
                }
            }
        }
        Kind::Button => {
            let (d0, d1) = (c - u * len * 0.25, c + u * len * 0.25);
            painter.line_segment([a, d0], stroke);
            painter.line_segment([d1, b], stroke);
            painter.circle_stroke(d0, 3.0, stroke);
            painter.circle_stroke(d1, 3.0, stroke);
            let bar = if part.pressed { d1 - n * len * 0.12 } else { d1 - n * len * 0.38 };
            painter.line_segment([d0 - n * len * 0.12, bar], stroke);
            let mid = d0 + (bar - d0) * 0.5 - n * len * 0.06;
            painter.line_segment([mid, mid - n * len * 0.28], stroke);
            painter.line_segment([mid - n * len * 0.28 - u * len * 0.1, mid - n * len * 0.28 + u * len * 0.1], stroke);
        }
        Kind::Npn => {
            let r = len * 0.45;
            painter.circle_stroke(c, r, stroke);
            let bx = c.x - r * 0.25;
            painter.line_segment([pos2(bx, c.y - r * 0.55), pos2(bx, c.y + r * 0.55)], Stroke::new(stroke.width * 1.6, stroke.color));
            painter.line_segment([pos2(c.x - r * 0.55, c.y), pos2(bx, c.y)], stroke);
            // Collector toward leg 0, emitter (with arrow) toward leg 2.
            let to_c = if u.y >= 0.0 { -1.0 } else { 1.0 };
            let cpt = pos2(c.x, c.y + to_c * r * 0.75);
            let ept = pos2(c.x, c.y - to_c * r * 0.75);
            painter.line_segment([pos2(bx, c.y + to_c * r * 0.25), cpt], stroke);
            painter.line_segment([pos2(bx, c.y - to_c * r * 0.25), ept], stroke);
            let dir = (ept - pos2(bx, c.y - to_c * r * 0.25)).normalized();
            arrow_head(painter, ept - dir * r * 0.15, dir, r * 0.25, stroke);
        }
    }
}

fn arrow_head(painter: &Painter, tip: Pos2, dir: Vec2, size: f32, stroke: Stroke) {
    let back = tip - dir * size;
    let side = vec2(-dir.y, dir.x) * size * 0.6;
    painter.add(Shape::convex_polygon(vec![tip, back + side, back - side], stroke.color, Stroke::NONE));
}
