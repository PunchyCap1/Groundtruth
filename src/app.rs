use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Color32, FontId, Key, Modifiers, RichText, Sense, Stroke, vec2};

use crate::assistant::{self, Gemini, Issue, Reply, Turn};
use crate::circuit::{self, Circuit, Kind, LED_COLORS, WIRE_COLORS, fmt_si, parse_si};
use crate::sim::checks::milliamps;
use crate::sim::{self, Severity, ngspice::Ngspice};
use crate::ui::{board, schematic, theme};
use crate::{Config, GEMINI_MODELS};

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Board,
    Schematic,
    Split,
}

enum Cmd {
    Undo,
    Redo,
    New,
    Open,
    Load(Circuit),
}

/// A finished simulation pass. `tran` is the state to continue from, for transient chunks.
struct SimDone {
    circuit: Circuit,
    out: sim::Output,
    tran: Option<(u64, sim::TranState)>,
}

/// Seconds of history the scope keeps.
const SCOPE_WINDOW: f64 = 5.0;

/// One scope point, decimated from the transient traces.
struct Sample {
    t: f64,
    v: BTreeMap<String, f64>,
    i: BTreeMap<(String, usize), f64>,
}

enum Entry {
    User(String),
    Reply(Reply),
    Error(String),
}

pub struct App {
    config: Config,
    ngspice_version: Option<String>,

    circuit: Circuit,
    path: Option<PathBuf>,
    saved: Circuit,
    undo: Vec<Circuit>,
    redo: Vec<Circuit>,
    edit_base: Option<Circuit>,
    pending: Option<Cmd>,

    editor: board::Editor,
    /// Push button held down with Space (momentary, not part of the saved circuit).
    held: Option<String>,
    view: View,
    show_fix: bool,
    show_readouts: bool,
    value_text: (String, String),
    notice: Option<(String, f64)>,

    sim_rx: Option<Receiver<SimDone>>,
    sim_for: Option<Circuit>,
    out: sim::Output,
    /// Transient time state; chunks from before the last reset carry an older `tran_gen` and are dropped.
    tran: sim::TranState,
    tran_gen: u64,
    tran_paused: bool,
    tran_speed: f64,
    /// Wall-clock time the last chunk was started, which sets how much simulated time the next one covers.
    tran_wall: f64,
    scope: VecDeque<Sample>,

    chat: Vec<Entry>,
    input: String,
    ai_rx: Option<Receiver<Result<Reply, String>>>,
    flagged: Vec<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let config = Config::from_env();
        let ngspice_version = Ngspice { binary: config.ngspice.clone() }.probe();
        let mut app = Self {
            config,
            ngspice_version,
            circuit: Circuit::default(),
            path: None,
            saved: Circuit::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            edit_base: None,
            pending: None,
            editor: board::Editor::default(),
            held: None,
            view: View::Board,
            show_fix: true,
            show_readouts: true,
            value_text: Default::default(),
            notice: None,
            sim_rx: None,
            sim_for: None,
            out: sim::Output::default(),
            tran: sim::TranState::default(),
            tran_gen: 0,
            tran_paused: false,
            tran_speed: 1.0,
            tran_wall: 0.0,
            scope: VecDeque::new(),
            chat: Vec::new(),
            input: String::new(),
            ai_rx: None,
            flagged: Vec::new(),
        };
        // Dev shortcut: `cargo run -- [circuit.json] [--split | --schematic]`
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--split" => app.view = View::Split,
                "--schematic" => app.view = View::Schematic,
                _ => app.open_path(PathBuf::from(arg)),
            }
        }
        app
    }

    fn ai_online(&self) -> bool {
        self.config.gemini_api_key.is_some()
    }

    /// The circuit as simulated: the saved one plus any button held with Space.
    fn live(&self) -> Circuit {
        let mut c = self.circuit.clone();
        if let Some(p) = self.held.as_deref().and_then(|id| c.part_mut(id)) {
            p.pressed = true;
        }
        c
    }

    fn say(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        self.notice = Some((text.into(), ctx.input(|i| i.time)));
    }

    fn load(&mut self, c: Circuit, path: Option<PathBuf>) {
        self.undo.push(std::mem::replace(&mut self.circuit, c));
        self.redo.clear();
        self.saved = self.circuit.clone();
        self.path = path;
        self.editor.selected = None;
        self.held = None;
        self.reset_time();
    }

    /// Back to t = 0 with every capacitor discharged.
    fn reset_time(&mut self) {
        self.tran = sim::TranState::default();
        self.tran_gen += 1;
        self.scope.clear();
        self.sim_for = None;
    }

    fn open_path(&mut self, path: PathBuf) {
        match Circuit::load(&path) {
            Ok(c) => self.load(c, Some(path)),
            Err(e) => self.notice = Some((format!("{e:#}"), 0.0)),
        }
    }

    fn save(&mut self, ctx: &egui::Context, save_as: bool) {
        let path = match (&self.path, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new()
                .add_filter("Ground Truth circuit", &["json"])
                .set_file_name(format!("{}.json", self.circuit.name.to_lowercase().replace(' ', "_")))
                .save_file(),
        };
        let Some(path) = path else { return };
        let mut c = self.circuit.clone();
        c.expect = None;
        match c.save(&path) {
            Ok(()) => {
                self.saved = self.circuit.clone();
                self.say(ctx, format!("Saved {}", path.display()));
                self.path = Some(path);
            }
            Err(e) => self.say(ctx, format!("{e:#}")),
        }
    }

    fn run_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Undo => {
                if let Some(prev) = self.undo.pop() {
                    self.redo.push(std::mem::replace(&mut self.circuit, prev));
                }
            }
            Cmd::Redo => {
                if let Some(next) = self.redo.pop() {
                    self.undo.push(std::mem::replace(&mut self.circuit, next));
                }
            }
            Cmd::New => self.load(Circuit::default(), None),
            Cmd::Open => {
                if let Some(p) = rfd::FileDialog::new().add_filter("Ground Truth circuit", &["json"]).pick_file() {
                    self.open_path(p);
                }
            }
            Cmd::Load(c) => self.load(c, None),
        }
        if self.editor.selected.as_deref().is_some_and(|id| self.circuit.part(id).is_none()) {
            self.editor.selected = None;
        }
    }

    // ---- Simulation & lab partner -------------------------------------------

    fn pump_sim(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.sim_rx {
            match rx.try_recv() {
                Ok(done) => {
                    self.sim_rx = None;
                    match done.tran {
                        Some((generation, _)) if generation != self.tran_gen => {}
                        Some((_, next)) => {
                            self.record_scope(&done.out);
                            self.tran = next;
                            self.sim_for = Some(done.circuit);
                            self.out = done.out;
                        }
                        None => {
                            self.sim_for = Some(done.circuit);
                            self.out = done.out;
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.sim_rx = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
            }
        }
        let live = self.live();
        let now = ctx.input(|i| i.time);
        let ng = Ngspice { binary: self.config.ngspice.clone() };
        let (tx, rx) = channel();
        let repaint = ctx.clone();

        if !sim::is_dynamic(&live) {
            if self.sim_for.as_ref() == Some(&live) {
                return;
            }
            std::thread::spawn(move || {
                let out = sim::run(&live, &ng);
                let _ = tx.send(SimDone { circuit: live, out, tran: None });
                repaint.request_repaint();
            });
        } else {
            // Time keeps running: each chunk covers the wall time since the last one.
            // Paused, a 1 µs chunk still re-solves edits without moving the clock noticeably.
            let dt = if self.tran_paused {
                if self.sim_for.as_ref() == Some(&live) {
                    return;
                }
                1e-6
            } else {
                ((now - self.tran_wall) * self.tran_speed).clamp(1e-3, 0.2)
            };
            self.tran_wall = now;
            let (state, generation) = (self.tran.clone(), self.tran_gen);
            std::thread::spawn(move || {
                let (out, next) = sim::run_tran(&live, &ng, &state, dt);
                let _ = tx.send(SimDone { circuit: live, out, tran: Some((generation, next)) });
                repaint.request_repaint();
            });
        }
        self.sim_rx = Some(rx);
    }

    /// Appends a chunk's waveforms to the scope, one point per 5 ms of simulated time.
    fn record_scope(&mut self, out: &sim::Output) {
        let Some(trace) = out.sim.as_ref().and_then(|s| s.trace.as_ref()) else { return };
        let start = self.tran.t;
        let mut last = self.scope.back().map_or(f64::MIN, |s| s.t);
        for (k, t) in trace.time.iter().enumerate() {
            let t = start + t;
            if t - last < 0.005 && k + 1 < trace.time.len() {
                continue;
            }
            last = t;
            self.scope.push_back(Sample {
                t,
                v: trace.net_voltage.iter().map(|(n, s)| (n.clone(), s[k])).collect(),
                i: trace.leg_current.iter().map(|(n, s)| (n.clone(), s[k])).collect(),
            });
        }
        while self.scope.front().is_some_and(|s| s.t < last - SCOPE_WINDOW) {
            self.scope.pop_front();
        }
    }

    fn ask(&mut self, ctx: &egui::Context, question: String) {
        let question = question.trim().to_string();
        if question.is_empty() || self.ai_rx.is_some() {
            return;
        }
        let Some(api_key) = self.config.gemini_api_key.clone() else {
            self.chat.push(Entry::Error("Add GEMINI_API_KEY to .env to enable the lab partner.".into()));
            return;
        };
        let mut history = Vec::new();
        for pair in self.chat.windows(2) {
            if let [Entry::User(q), Entry::Reply(r)] = pair {
                history.push(Turn { user: q.clone(), model: r.message.clone() });
            }
        }
        let history: Vec<Turn> = history.into_iter().rev().take(6).rev().collect();
        let context = assistant::context(&self.live(), &self.out);
        self.chat.push(Entry::User(question.clone()));

        let gemini = Gemini { api_key, model: self.config.gemini_model.clone() };
        let (tx, rx) = channel();
        self.ai_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(gemini.ask(&history, &context, &question).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
    }

    fn pump_ai(&mut self) {
        let Some(rx) = &self.ai_rx else { return };
        if let Ok(res) = rx.try_recv() {
            self.chat.push(match res {
                Ok(r) => Entry::Reply(r),
                Err(e) => Entry::Error(e),
            });
            self.ai_rx = None;
        }
    }

    // ---- Keyboard ------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.text_edit_focused();
        let (redo, undo, save_as, save, open, new) = ctx.input_mut(|i| {
            (
                !typing && (i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y)),
                !typing && i.consume_key(Modifiers::COMMAND, Key::Z),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::COMMAND, Key::N),
            )
        });
        if redo {
            self.pending = Some(Cmd::Redo);
        } else if undo {
            self.pending = Some(Cmd::Undo);
        }
        if save_as {
            self.save(ctx, true);
        } else if save {
            self.save(ctx, false);
        }
        if open {
            self.pending = Some(Cmd::Open);
        }
        if new {
            self.pending = Some(Cmd::New);
        }

        // Momentary press: hold Space on a selected button.
        let space = !typing && ctx.input(|i| i.key_down(Key::Space));
        let sel_button = self.editor.selected.clone().filter(|id| self.circuit.part(id).is_some_and(|p| p.kind == Kind::Button));
        self.held = if space { sel_button } else { None };
    }

    /// Edits that change the circuit; run after the undo snapshot is taken.
    fn edit_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.text_edit_focused() {
            return;
        }
        let (del, flip, esc, toggle) = ctx.input(|i| {
            (
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                pressed_once(i, Key::R),
                i.key_pressed(Key::Escape),
                pressed_once(i, Key::Space),
            )
        });
        if let Some(id) = self.editor.selected.clone() {
            if del {
                self.circuit.remove(&id);
                self.editor.selected = None;
            } else if let Some(p) = self.circuit.part_mut(&id) {
                if flip {
                    p.legs.reverse();
                }
                // Slide switches latch: tap Space to flip.
                if toggle && p.kind == Kind::Switch {
                    p.pressed = !p.pressed;
                }
            }
        }
        if esc {
            self.editor.selected = None;
        }
        // 1–9 drop the matching palette part into the first free spot.
        let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (k, kind) in keys.iter().zip(Kind::ALL) {
            if ctx.input(|i| pressed_once(i, *k)) {
                match self.circuit.add_anywhere(kind) {
                    Some(id) => self.editor.selected = Some(id),
                    None => self.say(ctx, "The board is full."),
                }
            }
        }
    }

    // ---- Panels --------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new("ground truth").font(FontId::monospace(13.0)).color(theme::HI));
            ui.add_space(18.0);

            if menu_item(ui, "new", "Ctrl+N").clicked() {
                self.pending = Some(Cmd::New);
            }
            if menu_item(ui, "open", "Ctrl+O").clicked() {
                self.pending = Some(Cmd::Open);
            }
            if menu_item(ui, "save", "Ctrl+S").clicked() {
                self.save(ui.ctx(), false);
            }
            ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
            ui.menu_button(RichText::new("examples").font(FontId::monospace(12.0)), |ui| {
                for ex in circuit::examples() {
                    let r = ui.button(&ex.name).on_hover_text(&ex.description);
                    if r.clicked() {
                        self.pending = Some(Cmd::Load(ex));
                        ui.close();
                    }
                }
            });
            ui.add_space(10.0);
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new(RichText::new("undo").font(FontId::monospace(12.0))).frame(false)).on_hover_text("Ctrl+Z").clicked() {
                self.pending = Some(Cmd::Undo);
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new(RichText::new("redo").font(FontId::monospace(12.0))).frame(false)).on_hover_text("Ctrl+Shift+Z").clicked() {
                self.pending = Some(Cmd::Redo);
            }
            ui.add_space(18.0);
            // The current view is shown inverted.
            for (v, name) in [(View::Board, "board"), (View::Split, "split"), (View::Schematic, "schematic")] {
                let on = self.view == v;
                let text = RichText::new(name).font(FontId::monospace(12.0)).color(if on { theme::BG } else { theme::DIM });
                let b = egui::Button::new(text).fill(if on { theme::HI } else { Color32::TRANSPARENT }).stroke(Stroke::NONE);
                if ui.add(b).clicked() {
                    self.view = v;
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match &self.ngspice_version {
                    Some(v) => status(ui, "sim", v, theme::DIM),
                    None => status(ui, "sim", "ngspice missing", theme::ALERT),
                }
                ui.add_space(10.0);
                if self.ai_online() {
                    status(ui, "ai", &self.config.gemini_model, theme::DIM);
                } else {
                    status(ui, "ai", "no GEMINI_API_KEY", theme::ALERT);
                }
                ui.add_space(18.0);
                let dirty = if self.circuit != self.saved { " *" } else { "" };
                ui.label(RichText::new(format!("{}{dirty}", self.circuit.name.to_lowercase())).font(FontId::monospace(12.0)).color(theme::TEXT));
            });
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "PARTS");
        ui.label(RichText::new("drag onto the board, or click").color(theme::DIM).size(11.0));
        ui.add_space(4.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, kind) in Kind::ALL.iter().enumerate() {
            {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click_and_drag());
                resp.dnd_set_drag_payload(*kind);
                let hot = resp.hovered() || resp.dragged();
                if hot {
                    ui.painter().rect_filled(rect, 0, theme::HI);
                }
                let (key_color, name_color) = if hot { (theme::BG, theme::BG) } else { (theme::LOW, theme::TEXT) };
                let key = if i < 9 { (i + 1).to_string() } else { " ".into() };
                let font = FontId::monospace(12.0);
                ui.painter().text(rect.left_center() + vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, key, font.clone(), key_color);
                ui.painter().text(rect.left_center() + vec2(24.0, 0.0), egui::Align2::LEFT_CENTER, kind.name().to_lowercase(), font, name_color);
                if resp.dragged() {
                    if let Some(m) = ui.ctx().pointer_interact_pos() {
                        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("dnd_ghost")));
                        board::icon(&painter, egui::Rect::from_center_size(m, egui::Vec2::splat(40.0)), *kind);
                    }
                }
                if resp.clicked() {
                    match self.circuit.add_anywhere(*kind) {
                        Some(id) => self.editor.selected = Some(id),
                        None => self.say(ui.ctx(), "The board is full."),
                    }
                }
                resp.on_hover_ui(|ui| part_tooltip(ui, *kind, (i < 9).then_some(i + 1)));
            }
        }
        ui.spacing_mut().item_spacing.y = 6.0;

        ui.add_space(6.0);
        match self.editor.selected.clone() {
            Some(id) if self.circuit.part(&id).is_some() => self.inspector(ui, &id),
            _ => self.circuit_props(ui),
        }

        theme::section(ui, "SHORTCUTS");
        for (k, what) in [
            ("drag hole", "draw a wire"),
            ("R", "flip / reverse part"),
            ("Del", "delete part"),
            ("Space", "press button / flip switch"),
            ("1–9", "quick-add part"),
            ("Ctrl+Z", "undo"),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{k:>9}")).font(FontId::monospace(11.0)).color(theme::TEXT));
                ui.label(RichText::new(what).size(11.0).color(theme::DIM));
            });
        }
    }

    fn circuit_props(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "CIRCUIT");
        ui.add(egui::TextEdit::singleline(&mut self.circuit.name).desired_width(f32::INFINITY));
        ui.add(egui::TextEdit::multiline(&mut self.circuit.description).hint_text("notes…").desired_rows(2).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            ui.label(RichText::new("rows").color(theme::DIM));
            for rows in [30u8, 45, 60] {
                let fits = self.circuit.parts.iter().all(|p| p.legs.iter().all(|h| h.row <= rows));
                if ui.add_enabled(fits, egui::RadioButton::new(self.circuit.rows == rows, rows.to_string())).clicked() {
                    self.circuit.rows = rows;
                }
            }
        });
        ui.label(RichText::new("click a part to edit it").color(theme::DIM).size(12.0));
    }

    fn inspector(&mut self, ui: &mut egui::Ui, id: &str) {
        let sim = self.out.sim.clone();
        let held = self.held.clone();
        let Some(part) = self.circuit.part_mut(id) else { return };
        theme::section(ui, "INSPECTOR");
        ui.horizontal(|ui| {
            ui.label(RichText::new(&part.id).font(FontId::monospace(16.0)).color(theme::HI).strong());
            ui.label(RichText::new(part.kind.name()).size(14.0));
        });
        ui.label(RichText::new(part.kind.blurb()).color(theme::DIM).size(12.0));
        ui.add_space(8.0);

        let presets: &[f64] = match part.kind {
            Kind::Battery => &[1.5, 3.0, 3.3, 5.0, 9.0, 12.0],
            Kind::Resistor => &[10.0, 100.0, 220.0, 330.0, 470.0, 1e3, 2.2e3, 4.7e3, 10e3, 47e3, 100e3, 1e6],
            Kind::Pot => &[1e3, 10e3, 100e3],
            Kind::Capacitor => &[100e-12, 10e-9, 100e-9, 1e-6, 10e-6, 100e-6],
            _ => &[],
        };
        if let (Some(unit), Some(v)) = (part.kind.unit(), part.value) {
            // Free-typed value with SI suffixes, plus common presets.
            let key = format!("{}@{v}", part.id);
            if self.value_text.0 != key {
                self.value_text = (key, fmt_si(v, unit));
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("value").color(theme::DIM));
                let r = ui.add(egui::TextEdit::singleline(&mut self.value_text.1).desired_width(90.0).font(FontId::monospace(13.0)));
                if r.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter)) && r.has_focus() {
                    // Only commit real edits: re-parsing the rounded display would change the value.
                    if self.value_text.1.trim() != fmt_si(v, unit) {
                        if let Some(x) = parse_si(&self.value_text.1).filter(|x| x.is_finite() && *x > 0.0) {
                            part.value = Some(x);
                        }
                    }
                    self.value_text.1 = fmt_si(part.value.unwrap_or(v), unit);
                }
                egui::ComboBox::from_id_salt("presets").selected_text("common").width(80.0).show_ui(ui, |ui| {
                    for pv in presets {
                        if ui.selectable_label(part.value == Some(*pv), fmt_si(*pv, unit)).clicked() {
                            part.value = Some(*pv);
                            self.value_text.1 = fmt_si(*pv, unit);
                        }
                    }
                });
            });
        }
        match part.kind {
            Kind::Led | Kind::Wire => {
                let colors: &[&str] = if part.kind == Kind::Led { &LED_COLORS } else { &WIRE_COLORS };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("color").color(theme::DIM));
                    for c in colors {
                        let swatch = if part.kind == Kind::Led { board::led_colors(Some(c)).1 } else { board::wire_color(Some(c)) };
                        let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 7.0, swatch);
                        if part.color.as_deref() == Some(*c) {
                            ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(2.0, theme::HI));
                        }
                        if resp.on_hover_text(*c).clicked() {
                            part.color = Some(c.to_string());
                        }
                    }
                });
            }
            Kind::Button => {
                ui.checkbox(&mut part.pressed, "latched");
                let down = held.as_deref() == Some(part.id.as_str());
                ui.label(RichText::new(if down { "held (space)" } else { "hold space to press it" }).color(if down { theme::HI } else { theme::DIM }).size(12.0));
            }
            Kind::Switch => {
                ui.checkbox(&mut part.pressed, "on");
                ui.label(RichText::new("tap space to flip it").color(theme::DIM).size(12.0));
            }
            Kind::Ldr => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("light").color(theme::DIM));
                    ui.add(egui::Slider::new(&mut part.light, 0.0..=1.0).show_value(false));
                    ui.label(RichText::new(format!("{:.0}%", part.light * 100.0)).monospace());
                });
                ui.label(RichText::new(format!("model resistance {}", fmt_si(part.ldr_ohms(), "Ω"))).color(theme::DIM).size(12.0));
            }
            Kind::Pot => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("wiper").color(theme::DIM));
                    ui.add(egui::Slider::new(&mut part.wiper, 0.0..=1.0).show_value(false));
                    ui.label(RichText::new(format!("{:.0}%", part.wiper * 100.0)).monospace());
                });
            }
            _ => {}
        }

        ui.add_space(8.0);
        let dim = |t: &str| RichText::new(t).color(theme::DIM).size(12.0);
        let reading = |t: String| RichText::new(t).monospace().color(theme::ACCENT);
        egui::Grid::new("legs").num_columns(3).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
            ui.label(RichText::new("LEG").font(FontId::monospace(10.0)).color(theme::DIM));
            ui.label(RichText::new("HOLE").font(FontId::monospace(10.0)).color(theme::DIM));
            ui.label(RichText::new("NGSPICE").font(FontId::monospace(10.0)).color(theme::DIM));
            ui.end_row();
            for (leg, h) in part.legs.iter().enumerate() {
                ui.label(dim(part.kind.leg_names()[leg]));
                ui.label(RichText::new(h.short()).monospace());
                match sim.as_ref().and_then(|s| s.voltage(h)) {
                    Some(v) => ui.label(reading(format!("{v:.2} V"))),
                    None => ui.label(dim("—")),
                };
                ui.end_row();
            }
            if let Some(i) = sim.as_ref().and_then(|s| s.current(&part.id)) {
                let what = match part.kind {
                    Kind::Npn => "collector",
                    Kind::Battery => "supplying",
                    _ => "current",
                };
                ui.label(dim(what));
                ui.label("");
                ui.label(reading(format!("{:.2} mA", milliamps(board::shown_current(part.kind, i)))));
                ui.end_row();
            }
        });

        ui.add_space(8.0);
        let mut delete = false;
        ui.horizontal(|ui| {
            if ui.button("flip (r)").on_hover_text("Reverse the legs, e.g. to turn an LED around").clicked() {
                part.legs.reverse();
            }
            delete = ui.button(RichText::new("delete").color(theme::ALERT)).clicked();
        });
        if delete {
            self.circuit.remove(id);
            self.editor.selected = None;
        }
    }

    fn checks_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            theme::section(ui, "CHECKS");
            match (&self.out.error, &self.out.checks) {
                _ if self.circuit.parts.is_empty() => theme::badge_outline(ui, "EMPTY BOARD", theme::DIM),
                (Some(_), _) => theme::badge_outline(ui, "NOT SIMULATED", theme::HI),
                (None, Some(c)) if c.pass => theme::badge(ui, "✔ PASS", theme::ACCENT),
                (None, Some(_)) => theme::badge(ui, "✖ FAULT", theme::ALERT),
                _ => theme::badge_outline(ui, "…", theme::DIM),
            };
            if let (None, Some(c)) = (&self.out.error, &self.out.checks) {
                let count = |sev| c.findings.iter().filter(|f| f.severity == sev).count();
                let (faults, warns) = (count(Severity::Fault), count(Severity::Warning));
                if faults > 0 {
                    ui.label(RichText::new(format!("{faults} fault{}", if faults == 1 { "" } else { "s" })).font(FontId::monospace(11.0)).color(theme::ALERT));
                }
                if warns > 0 {
                    ui.label(RichText::new(format!("{warns} warning{}", if warns == 1 { "" } else { "s" })).font(FontId::monospace(11.0)).color(theme::HI));
                }
            }
            // Shares the panel with the scope over time, so skip the caption there.
            if !sim::is_dynamic(&self.circuit) {
                ui.label(RichText::new("from ngspice + rule checks").font(FontId::monospace(10.5)).color(theme::DIM));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut self.show_readouts, "readouts");
                ui.checkbox(&mut self.show_fix, "fix ghosts");
            });
        });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if let Some(e) = &self.out.error {
                if !self.circuit.parts.is_empty() {
                    let first = e.lines().next().unwrap_or("").to_string();
                    ui.label(RichText::new(first).color(theme::HI));
                }
            }
            let mut apply = None;
            if let Some(checks) = &self.out.checks {
                // Worst first, so a fault is never buried under OK lines.
                let mut findings: Vec<_> = checks.findings.iter().collect();
                findings.sort_by_key(|f| f.severity);
                for f in findings {
                    let (color, tag) = match f.severity {
                        Severity::Fault => (theme::ALERT, "✖ FAULT"),
                        Severity::Warning => (theme::HI, "⚠ WARN "),
                        Severity::Info => (theme::ACCENT, "ℹ INFO "),
                        Severity::Ok => (theme::ACCENT, "✔ OK   "),
                    };
                    let quiet = matches!(f.severity, Severity::Info | Severity::Ok);
                    let stroke = if quiet { Color32::TRANSPARENT } else { color };
                    ui.add_space(2.0);
                    egui::Frame::new()
                        .stroke(Stroke::new(1.0, stroke))
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_wrapped(|ui| {
                                match f.severity {
                                    Severity::Fault => theme::badge(ui, tag, color),
                                    Severity::Warning => theme::badge_outline(ui, tag.trim_end(), color),
                                    _ => ui.label(RichText::new(tag).font(FontId::monospace(11.0)).color(color)),
                                };
                                let r = ui.add(egui::Label::new(RichText::new(&f.headline).size(13.5)).sense(Sense::click()));
                                if r.clicked() {
                                    self.editor.selected = Some(f.part.clone());
                                }
                                r.on_hover_text(&f.detail);
                                if let Some(fix) = &f.fix {
                                    if ui.small_button(RichText::new("apply fix").color(theme::ACCENT)).clicked() {
                                        apply = Some(fix.clone());
                                    }
                                }
                            });
                            if f.severity != Severity::Ok {
                                ui.label(RichText::new(&f.detail).color(theme::DIM).size(12.0));
                            }
                        });
                }
            }
            if let Some(fix) = apply {
                if let Some(p) = self.circuit.part(&fix.part) {
                    let mut legs = p.legs.clone();
                    for (leg, to) in fix.moves {
                        if let Some(l) = legs.get_mut(leg) {
                            *l = to;
                        }
                    }
                    if self.circuit.fits(&legs, Some(&fix.part)) {
                        self.circuit.part_mut(&fix.part).unwrap().legs = legs;
                    }
                }
            }
            if let Some(nl) = &self.out.netlist {
                egui::CollapsingHeader::new(RichText::new("SPICE NETLIST").font(FontId::monospace(11.0)).color(theme::DIM)).show(ui, |ui| {
                    code_block(ui, nl.spice.trim());
                    for n in &nl.notes {
                        ui.colored_label(theme::HI, n);
                    }
                });
            }
            if let Some(s) = &self.out.sim {
                egui::CollapsingHeader::new(RichText::new("NGSPICE LOG").font(FontId::monospace(11.0)).color(theme::DIM)).show(ui, |ui| code_block(ui, s.log.trim()));
            }
        });
    }

    /// Waveforms of the selected part, or every capacitor's voltage, over the last few seconds.
    fn scope_panel(&mut self, ui: &mut egui::Ui) {
        let selected = self.editor.selected.as_deref().and_then(|id| self.circuit.part(id)).filter(|p| p.kind != Kind::Wire).cloned();
        ui.horizontal(|ui| {
            theme::section(ui, "SCOPE");
            ui.label(RichText::new(format!("t = {:.2} s", self.tran.t)).font(FontId::monospace(12.5)).color(theme::ACCENT))
                .on_hover_text("Simulated time. Circuits with capacitors run as a live ngspice transient simulation.");
            if tool_button(ui, if self.tran_paused { "▶ Run" } else { "Pause" }, "Freeze time; edits still re-solve").clicked() {
                self.tran_paused = !self.tran_paused;
                self.tran_wall = ui.input(|i| i.time);
            }
            if tool_button(ui, "Reset", "Back to t = 0 with every capacitor discharged").clicked() {
                self.reset_time();
            }
            let slow = self.tran_speed < 1.0;
            if tool_button(ui, if slow { "0.1×" } else { "1×" }, "Simulation speed: real time or slow motion").clicked() {
                self.tran_speed = if slow { 1.0 } else { 0.1 };
            }
            let what = match &selected {
                Some(p) => format!("{} · last {SCOPE_WINDOW:.0} s", p.id),
                None => format!("capacitors · select a part to probe it · last {SCOPE_WINDOW:.0} s"),
            };
            ui.add(egui::Label::new(RichText::new(what).font(FontId::monospace(10.5)).color(theme::DIM)).truncate());
        });
        let colors = [theme::ACCENT, theme::HI, theme::ACCENT];
        let mut volts: Vec<(String, Color32, Vec<(f64, f64)>)> = Vec::new();
        let mut amps = Vec::new();
        match &selected {
            Some(p) => {
                for (leg, h) in p.legs.iter().enumerate() {
                    let net = h.net_key();
                    let pts = self.scope.iter().filter_map(|s| Some((s.t, *s.v.get(&net)?))).collect();
                    volts.push((format!("{} {}", p.id, p.kind.leg_names()[leg]), colors[leg % 3], pts));
                }
                let sign = if p.kind == Kind::Battery { -1e3 } else { 1e3 };
                let key = (p.id.clone(), 0);
                let pts: Vec<(f64, f64)> = self.scope.iter().filter_map(|s| Some((s.t, s.i.get(&key)? * sign))).collect();
                if !pts.is_empty() {
                    amps.push((format!("{} current", p.id), theme::ALERT, pts));
                }
            }
            None => {
                for (k, p) in self.circuit.parts.iter().filter(|p| p.kind == Kind::Capacitor).enumerate() {
                    let (a, b) = (p.legs[0].net_key(), p.legs[1].net_key());
                    let pts = self.scope.iter().filter_map(|s| Some((s.t, s.v.get(&a)? - s.v.get(&b)?))).collect();
                    volts.push((format!("{} across", p.id), colors[k % 3], pts));
                }
            }
        }
        let t_end = self.scope.back().map_or(0.0, |s| s.t);
        let h = ui.available_height();
        if amps.is_empty() {
            strip(ui, h, "V", t_end, &volts);
        } else {
            strip(ui, h * 0.58, "V", t_end, &volts);
            ui.add_space(4.0);
            strip(ui, ui.available_height(), "mA", t_end, &amps);
        }
    }

    fn lab_partner(&mut self, ui: &mut egui::Ui) -> Vec<String> {
        let mut hovered_parts = Vec::new();
        ui.horizontal(|ui| {
            theme::section(ui, "LAB PARTNER");
            ui.label(RichText::new("Gemini").font(FontId::monospace(10.5)).color(theme::DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut models: Vec<String> = GEMINI_MODELS.iter().map(|m| m.to_string()).collect();
                if !models.contains(&self.config.gemini_model) {
                    models.insert(0, self.config.gemini_model.clone());
                }
                ui.add_enabled_ui(self.ai_rx.is_none(), |ui| {
                    egui::ComboBox::from_id_salt("gemini_model").selected_text(RichText::new(&self.config.gemini_model).font(FontId::monospace(11.0))).show_ui(ui, |ui| {
                        for m in models {
                            ui.selectable_value(&mut self.config.gemini_model, m.clone(), m);
                        }
                    });
                });
            });
        });

        let busy = self.ai_rx.is_some();
        let mut ask = None;
        ui.horizontal_wrapped(|ui| {
            for (label, q) in [
                ("review", "Review my circuit. What's wrong, what's risky, and what could be better?"),
                ("why isn't it working?", "Why isn't my circuit doing what I expect? Walk me through it."),
                ("explain", "Explain how this circuit works, step by step, following the current."),
                ("ideas", "Suggest a few experiments or next circuits I could build from here."),
                ("build one for me", "Propose a small working circuit I could build next, using the parts in the bin, and fill in `circuit` so it loads onto the board."),
            ] {
                if ui.add_enabled(!busy, egui::Button::new(RichText::new(label).size(12.5))).clicked() {
                    ask = Some(q.to_string());
                }
            }
        });
        ui.add_space(4.0);

        // Input pinned to the bottom, conversation above it.
        let input_h = 74.0;
        let log_h = (ui.available_height() - input_h).max(80.0);
        let mut load_proposal = None;
        egui::ScrollArea::vertical().max_height(log_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            if self.chat.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("ask anything about your circuit: why an led is dark, what resistor to use, how a transistor switch works, or what to build next.").color(theme::DIM));
                ui.add_space(4.0);
                ui.label(RichText::new("gemini sees the board, the nets and the simulated numbers. it can still be wrong, so check its claims against the readouts.").color(theme::DIM).size(11.5));
            }
            for entry in &self.chat {
                ui.add_space(6.0);
                match entry {
                    Entry::User(q) => {
                        ui.label(RichText::new(format!("> {q}")).color(theme::DIM));
                    }
                    Entry::Error(e) => {
                        ui.colored_label(theme::ALERT, e);
                    }
                    Entry::Reply(r) => {
                        ui.label(RichText::new("GEMINI · AI, not simulated").font(FontId::monospace(10.0)).color(theme::DIM));
                        ui.label(&r.message);
                        for issue in &r.issues {
                            if issue_card(ui, issue, &mut self.editor.selected) {
                                hovered_parts = issue.parts.clone();
                            }
                        }
                        for idea in &r.ideas {
                            ui.add_space(2.0);
                            ui.label(RichText::new(format!("+ {}", idea.title.to_lowercase())).color(theme::ACCENT));
                            ui.label(RichText::new(&idea.description).color(theme::TEXT));
                        }
                        if let Some(c) = &r.circuit {
                            theme::card(theme::BG, theme::EDGE).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new("PROPOSED CIRCUIT").font(FontId::monospace(10.5)).color(theme::HI));
                                ui.label(RichText::new(&c.name).strong());
                                if !c.description.is_empty() {
                                    ui.label(RichText::new(&c.description).size(12.5));
                                }
                                let ids: Vec<&str> = c.parts.iter().map(|p| p.id.as_str()).collect();
                                ui.label(RichText::new(ids.join(", ")).font(FontId::monospace(10.5)).color(theme::DIM));
                                if ui.add_enabled(!busy, egui::Button::new(RichText::new("load onto board").color(theme::ACCENT))).on_hover_text("Replaces the board (Ctrl+Z undoes it). ngspice simulates it like any other circuit.").clicked() {
                                    load_proposal = Some(c.clone());
                                }
                            });
                        }
                    }
                }
            }
            if busy {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().color(theme::HI));
                    ui.label(RichText::new(format!("{} is thinking…", self.config.gemini_model)).color(theme::DIM));
                });
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let r = ui.add_sized(
                vec2(ui.available_width() - 60.0, 54.0),
                egui::TextEdit::multiline(&mut self.input).hint_text("ask your lab partner…").desired_rows(2),
            );
            let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
            if enter {
                // The multiline edit already inserted the newline.
                self.input = self.input.trim_end_matches('\n').to_string();
            }
            let send = ui.add_enabled(!busy && !self.input.trim().is_empty(), egui::Button::new("send").min_size(vec2(52.0, 54.0))).clicked();
            if (enter || send) && !busy {
                ask = Some(std::mem::take(&mut self.input));
            }
        });
        if let Some(q) = ask {
            self.ask(ui.ctx(), q);
        }
        if let Some(c) = load_proposal {
            match assistant::check_proposal(&c) {
                Ok(c) => {
                    self.pending = Some(Cmd::Load(c));
                    self.say(ui.ctx(), "Loaded Gemini's circuit. Ctrl+Z to undo.");
                }
                Err(e) => self.chat.push(Entry::Error(format!("Couldn't load that circuit: {e}"))),
            }
        }
        hovered_parts
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump_ai();

        // Opening a circuit by dropping its file on the window.
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()).filter(|p| p.extension().is_some_and(|e| e == "json"))) {
            self.open_path(path);
        }

        self.shortcuts(&ctx);
        if let Some(cmd) = self.pending.take() {
            self.run_cmd(cmd);
        }
        // Anything that changes the circuit from here on becomes one undo step.
        let before = self.circuit.clone();
        self.edit_shortcuts(&ctx);

        let bar = egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(14, 8));
        egui::Panel::top("top").frame(bar).exact_size(46.0).show(ui, |ui| self.top_bar(ui));

        let side = egui::Frame::new().fill(theme::PANEL).inner_margin(14).stroke(Stroke::new(1.0, theme::EDGE));
        egui::Panel::left("bin").frame(side).exact_size(270.0).resizable(false).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.left_panel(ui));
        });
        let mut hovered_issue = Vec::new();
        egui::Panel::right("partner").frame(side).default_size(380.0).min_size(320.0).show(ui, |ui| {
            hovered_issue = self.lab_partner(ui);
        });
        egui::Panel::bottom("checks").frame(side).default_size(190.0).min_size(90.0).resizable(true).show(ui, |ui| {
            if sim::is_dynamic(&self.circuit) {
                ui.columns(2, |cols| {
                    cols[0].vertical(|ui| self.checks_panel(ui));
                    cols[1].vertical(|ui| self.scope_panel(ui));
                });
            } else {
                self.checks_panel(ui);
            }
        });

        let flagged = std::mem::take(&mut self.flagged);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG).inner_margin(12)).show(ui, |ui| {
            let live = self.live();
            let sim = self.out.sim.clone();
            let checks = self.out.checks.clone();
            let board_view = |ui: &mut egui::Ui, app: &mut App| {
                let out = board::show(
                    ui,
                    &mut app.circuit,
                    &mut app.editor,
                    &board::View {
                        sim: sim.as_ref(),
                        checks: checks.as_ref(),
                        show_fix: app.show_fix,
                        show_readouts: app.show_readouts,
                        flagged: &flagged,
                        held: app.held.as_deref(),
                    },
                );
                if let Some(n) = out.notice {
                    app.say(ui.ctx(), n);
                }
            };
            match self.view {
                View::Board => board_view(ui, self),
                View::Schematic => schematic::show(ui, &live, sim.as_ref(), &mut self.editor.selected, &flagged),
                View::Split => {
                    let h = ui.available_height() * 0.55;
                    ui.allocate_ui(vec2(ui.available_width(), h), |ui| board_view(ui, self));
                    ui.separator();
                    schematic::show(ui, &live, sim.as_ref(), &mut self.editor.selected, &flagged);
                }
            }
        });

        // Status line.
        if let Some((text, at)) = &self.notice {
            let age = ctx.input(|i| i.time) - at;
            if *at > 0.0 && age > 4.0 {
                self.notice = None;
            } else {
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("notice")));
                let screen = ctx.content_rect();
                let galley = painter.layout_no_wrap(text.clone(), FontId::monospace(12.0), theme::BG);
                let r = egui::Rect::from_center_size(egui::pos2(screen.center().x, screen.top() + 66.0), galley.size() + vec2(16.0, 8.0));
                painter.rect_filled(r, 0, theme::HI);
                painter.galley(r.min + vec2(8.0, 4.0), galley, theme::BG);
                ctx.request_repaint();
            }
        }

        // Fold this frame's edits into the undo history once the gesture is over.
        if self.circuit != before && self.edit_base.is_none() {
            self.edit_base = Some(before);
        }
        let settled = !self.editor.dragging() && !ctx.input(|i| i.pointer.any_down()) && !ctx.text_edit_focused();
        if settled {
            if let Some(base) = self.edit_base.take() {
                if base != self.circuit {
                    self.undo.push(base);
                    self.redo.clear();
                    if self.undo.len() > 200 {
                        self.undo.remove(0);
                    }
                }
            }
        }

        self.flagged = hovered_issue;
        self.pump_sim(&ctx);
        if sim::is_dynamic(&self.circuit) && !self.tran_paused {
            ctx.request_repaint();
        } else if self.sim_rx.is_some() || self.ai_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let title = format!("Ground Truth — {}{}", self.circuit.name, if self.circuit != self.saved { " •" } else { "" });
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

/// Renders a lab-partner issue; returns true while hovered.
fn issue_card(ui: &mut egui::Ui, issue: &Issue, selected: &mut Option<String>) -> bool {
    let color = match issue.severity.as_str() {
        "error" => theme::ALERT,
        "warning" => theme::ALERT,
        _ => theme::DIM,
    };
    let r = theme::card(theme::BG, theme::EDGE).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(issue.severity.to_lowercase()).font(FontId::monospace(11.0)).color(color));
            ui.label(RichText::new(&issue.title).color(theme::HI));
        });
        ui.label(RichText::new(&issue.explanation));
        if !issue.suggestion.is_empty() {
            ui.label(RichText::new(format!("fix: {}", issue.suggestion)).color(theme::ACCENT));
        }
        if !issue.parts.is_empty() {
            ui.label(RichText::new(format!("parts: {}  ·  click to select", issue.parts.join(", "))).font(FontId::monospace(10.5)).color(theme::DIM));
        }
    });
    let resp = r.response.interact(Sense::click());
    if resp.clicked() {
        *selected = issue.parts.first().cloned();
    }
    resp.hovered()
}

/// A strip chart of `series` (label, color, (time, value) points) over the last [`SCOPE_WINDOW`] seconds up to `t_end`.
fn strip(ui: &mut egui::Ui, height: f32, unit: &str, t_end: f64, series: &[(String, Color32, Vec<(f64, f64)>)]) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height.max(40.0)), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_stroke(rect, 0, Stroke::new(1.0, theme::EDGE), egui::StrokeKind::Inside);
    let values = series.iter().flat_map(|s| s.2.iter().map(|p| p.1));
    let (mut lo, mut hi) = values.fold((0.0f64, 0.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
    // At least one unit of span, so nanovolt drift doesn't fill the screen.
    if hi - lo < 1.0 {
        hi = lo + 1.0;
    }
    let pad = (hi - lo) * 0.08;
    (lo, hi) = (lo - pad, hi + pad);
    let plot = egui::Rect::from_min_max(rect.min + vec2(52.0, 6.0), rect.max - vec2(8.0, 6.0));
    let t0 = t_end - SCOPE_WINDOW;
    let at = |t: f64, v: f64| {
        egui::pos2(
            plot.left() + ((t - t0) / SCOPE_WINDOW) as f32 * plot.width(),
            plot.bottom() - ((v - lo) / (hi - lo)) as f32 * plot.height(),
        )
    };
    let small = FontId::monospace(10.0);
    let grid = Stroke::new(1.0, theme::EDGE);
    painter.line_segment([at(t0, 0.0), at(t_end, 0.0)], grid);
    for v in [hi - pad, lo + pad] {
        painter.text(egui::pos2(rect.left() + 6.0, at(t0, v).y), egui::Align2::LEFT_CENTER, format!("{v:.2} {unit}"), small.clone(), theme::DIM);
    }
    for (k, (label, color, pts)) in series.iter().enumerate() {
        let line: Vec<egui::Pos2> = pts.iter().filter(|p| p.0 >= t0).map(|p| at(p.0, p.1)).collect();
        if line.len() > 1 {
            painter.add(egui::Shape::line(line, Stroke::new(1.0, *color)));
        }
        let now = pts.last().map(|p| format!("{label}  {:.2} {unit}", p.1)).unwrap_or(label.clone());
        let galley = painter.layout_no_wrap(now, small.clone(), *color);
        let at = plot.right_top() + vec2(-4.0 - galley.size().x, 2.0 + 14.0 * k as f32);
        painter.rect_filled(egui::Rect::from_min_size(at, galley.size()).expand(2.0), 0, theme::BG);
        painter.galley(at, galley, *color);
    }
}

fn tool_button(ui: &mut egui::Ui, text: &str, hint: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).font(FontId::monospace(12.0)))).on_hover_text(hint)
}

/// A text-only top-bar command.
fn menu_item(ui: &mut egui::Ui, text: &str, hint: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).font(FontId::monospace(12.0))).frame(false)).on_hover_text(hint)
}

/// "key value" in the top bar's right corner; laid out right to left.
fn status(ui: &mut egui::Ui, key: &str, value: &str, color: Color32) {
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.label(RichText::new(value).font(FontId::monospace(11.0)).color(color));
    ui.label(RichText::new(key).font(FontId::monospace(11.0)).color(theme::LOW));
}

/// True on the initial press of `key` with no modifiers, ignoring key-repeat.
fn pressed_once(i: &egui::InputState, key: Key) -> bool {
    i.events.iter().any(|e| {
        matches!(e, egui::Event::Key { key: k, pressed: true, repeat: false, modifiers, .. }
            if *k == key && modifiers.is_none())
    })
}

fn code_block(ui: &mut egui::Ui, text: &str) {
    theme::card(theme::BG, theme::EDGE).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(text).font(FontId::monospace(11.0))).wrap());
    });
}

/// Parts-bin hover card: what the part is, its legs (polarity), default value and shortcut.
fn part_tooltip(ui: &mut egui::Ui, kind: Kind, key: Option<usize>) {
    ui.set_max_width(260.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(kind.name()).strong().color(theme::HI));
        ui.label(RichText::new(format!("{}1, {}2…", kind.id_prefix(), kind.id_prefix())).font(FontId::monospace(10.5)).color(theme::DIM));
    });
    ui.label(kind.blurb());
    ui.add_space(2.0);
    let legs = kind.leg_names().join(" · ");
    ui.label(RichText::new(format!("legs     {legs}")).font(FontId::monospace(11.0)).color(theme::DIM));
    if let (Some(v), Some(unit)) = (kind.default_value(), kind.unit()) {
        ui.label(RichText::new(format!("default  {}", fmt_si(v, unit))).font(FontId::monospace(11.0)).color(theme::DIM));
    }
    let how = match key {
        Some(k) => format!("drag onto the board, click, or press {k}"),
        None => "drag onto the board, or click".into(),
    };
    ui.label(RichText::new(how).size(11.5).color(theme::DIM));
}
