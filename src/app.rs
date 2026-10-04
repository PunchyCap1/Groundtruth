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

    sim_rx: Option<Receiver<(Circuit, sim::Output)>>,
    sim_for: Option<Circuit>,
    out: sim::Output,

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
                Ok((c, out)) => {
                    self.sim_for = Some(c);
                    self.out = out;
                    self.sim_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.sim_rx = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
            }
        }
        let live = self.live();
        if self.sim_for.as_ref() == Some(&live) {
            return;
        }
        let (tx, rx) = channel();
        self.sim_rx = Some(rx);
        let ng = Ngspice { binary: self.config.ngspice.clone() };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let out = sim::run(&live, &ng);
            let _ = tx.send((live, out));
            ctx.request_repaint();
        });
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
        let (undo, redo, save, save_as, open, new) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::Z),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y),
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::COMMAND, Key::N),
            )
        });
        if redo {
            self.pending = Some(Cmd::Redo);
        } else if undo && !typing {
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
        let (del, flip, esc) = ctx.input(|i| {
            (i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace), i.key_pressed(Key::R), i.key_pressed(Key::Escape))
        });
        if let Some(id) = self.editor.selected.clone() {
            if del {
                self.circuit.remove(&id);
                self.editor.selected = None;
            } else if flip {
                if let Some(p) = self.circuit.part_mut(&id) {
                    p.legs.reverse();
                }
            }
        }
        if esc {
            self.editor.selected = None;
        }
        // 1–9 drop the matching palette part into the first free spot.
        let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (k, kind) in keys.iter().zip(Kind::ALL) {
            if ctx.input(|i| i.key_pressed(*k) && i.modifiers.is_none()) {
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
            ui.label(RichText::new("GROUND TRUTH").font(FontId::monospace(17.0)).color(theme::AMBER).strong());
            ui.label(RichText::new("breadboard playground").color(theme::DIM).font(FontId::monospace(11.0)));
            ui.add_space(14.0);

            if tool_button(ui, "New", "Ctrl+N").clicked() {
                self.pending = Some(Cmd::New);
            }
            if tool_button(ui, "Open", "Ctrl+O").clicked() {
                self.pending = Some(Cmd::Open);
            }
            if tool_button(ui, "Save", "Ctrl+S").clicked() {
                self.save(ui.ctx(), false);
            }
            ui.menu_button(RichText::new("Examples ▾").font(FontId::monospace(12.0)), |ui| {
                for ex in circuit::examples() {
                    let r = ui.button(&ex.name).on_hover_text(&ex.description);
                    if r.clicked() {
                        self.pending = Some(Cmd::Load(ex));
                        ui.close();
                    }
                }
            });
            ui.add_space(8.0);
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new(RichText::new("Undo").font(FontId::monospace(12.0)))).on_hover_text("Undo (Ctrl+Z)").clicked() {
                self.pending = Some(Cmd::Undo);
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new(RichText::new("Redo").font(FontId::monospace(12.0)))).on_hover_text("Redo (Ctrl+Shift+Z)").clicked() {
                self.pending = Some(Cmd::Redo);
            }
            ui.add_space(14.0);
            for (v, name) in [(View::Board, "BREADBOARD"), (View::Split, "SPLIT"), (View::Schematic, "SCHEMATIC")] {
                let on = self.view == v;
                let text = RichText::new(name).font(FontId::monospace(11.5)).color(if on { theme::BG } else { theme::DIM });
                let b = egui::Button::new(text).fill(if on { theme::AMBER } else { Color32::TRANSPARENT }).corner_radius(4);
                if ui.add(b).clicked() {
                    self.view = v;
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match &self.ngspice_version {
                    Some(v) => chip(ui, "SIM", v, theme::GREEN),
                    None => chip(ui, "SIM", "ngspice missing", theme::RED),
                }
                if self.ai_online() {
                    chip(ui, "AI", &self.config.gemini_model, theme::GREEN);
                } else {
                    chip(ui, "AI", "no GEMINI_API_KEY", theme::AMBER);
                }
                let dirty = if self.circuit != self.saved { " •" } else { "" };
                ui.label(RichText::new(format!("{}{dirty}", self.circuit.name)).font(FontId::monospace(12.0)).color(theme::TEXT));
            });
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "PARTS BIN");
        ui.label(RichText::new("Drag onto the board, or click to drop in a free spot.").color(theme::DIM).size(12.0));
        ui.add_space(4.0);
        let tile = vec2((ui.available_width() - 8.0) / 2.0, 64.0);
        egui::Grid::new("palette").spacing(vec2(8.0, 8.0)).show(ui, |ui| {
            for (i, kind) in Kind::ALL.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(tile, Sense::click_and_drag());
                resp.dnd_set_drag_payload(*kind);
                let hot = resp.hovered() || resp.dragged();
                ui.painter().rect_filled(rect, 6, if hot { theme::EDGE } else { theme::BG });
                ui.painter().rect_stroke(rect, 6, Stroke::new(1.0, if hot { theme::AMBER } else { theme::EDGE }), egui::StrokeKind::Inside);
                board::icon(ui.painter(), egui::Rect::from_center_size(rect.center() - vec2(0.0, 8.0), egui::Vec2::splat(36.0)), *kind);
                ui.painter().text(rect.center_bottom() - vec2(0.0, 10.0), egui::Align2::CENTER_CENTER, kind.name(), FontId::proportional(11.5), theme::TEXT);
                ui.painter().text(rect.left_top() + vec2(6.0, 4.0), egui::Align2::LEFT_TOP, (i + 1).to_string(), FontId::monospace(9.5), theme::DIM);
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
                let key = i + 1;
                resp.on_hover_ui(|ui| part_tooltip(ui, *kind, key));
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });

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
            ("Space", "hold a button"),
            ("1–9", "quick-add part"),
            ("Ctrl+Z", "undo"),
        ] {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{k:>9}")).font(FontId::monospace(11.0)).color(theme::AMBER));
                ui.label(RichText::new(what).size(12.0).color(theme::DIM));
            });
        }
    }

    fn circuit_props(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "CIRCUIT");
        ui.add(egui::TextEdit::singleline(&mut self.circuit.name).desired_width(f32::INFINITY));
        ui.add(egui::TextEdit::multiline(&mut self.circuit.description).hint_text("Notes about this circuit…").desired_rows(2).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            ui.label(RichText::new("Board rows").color(theme::DIM));
            for rows in [30u8, 45, 60] {
                let fits = self.circuit.parts.iter().all(|p| p.legs.iter().all(|h| h.row <= rows));
                if ui.add_enabled(fits, egui::RadioButton::new(self.circuit.rows == rows, rows.to_string())).clicked() {
                    self.circuit.rows = rows;
                }
            }
        });
        ui.label(RichText::new("Click a part to edit it.").color(theme::DIM).size(12.0));
    }

    fn inspector(&mut self, ui: &mut egui::Ui, id: &str) {
        let sim = self.out.sim.clone();
        let held = self.held.clone();
        let Some(part) = self.circuit.part_mut(id) else { return };
        theme::section(ui, "INSPECTOR");
        ui.horizontal(|ui| {
            ui.label(RichText::new(&part.id).font(FontId::monospace(16.0)).color(theme::AMBER).strong());
            ui.label(RichText::new(part.kind.name()).size(14.0));
        });
        ui.label(RichText::new(part.kind.blurb()).color(theme::DIM).size(12.0));
        ui.add_space(4.0);

        let presets: &[f64] = match part.kind {
            Kind::Battery => &[1.5, 3.0, 3.3, 5.0, 9.0, 12.0],
            Kind::Resistor => &[10.0, 100.0, 220.0, 330.0, 470.0, 1e3, 2.2e3, 4.7e3, 10e3, 47e3, 100e3, 1e6],
            Kind::Pot => &[1e3, 10e3, 100e3],
            Kind::Capacitor => &[100e-12, 10e-9, 100e-9, 1e-6, 10e-6, 100e-6],
            _ => &[],
        };
        if let (Some(unit), Some(v)) = (part.kind.unit(), part.value) {
            // Free-typed value with SI suffixes, plus common presets.
            if self.value_text.0 != part.id {
                self.value_text = (part.id.clone(), fmt_si(v, unit));
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Value").color(theme::DIM));
                let r = ui.add(egui::TextEdit::singleline(&mut self.value_text.1).desired_width(90.0).font(FontId::monospace(13.0)));
                if r.lost_focus() || ui.input(|i| i.key_pressed(Key::Enter)) && r.has_focus() {
                    match parse_si(&self.value_text.1).filter(|x| *x > 0.0) {
                        Some(x) => part.value = Some(x),
                        None => {}
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
                    ui.label(RichText::new("Color").color(theme::DIM));
                    for c in colors {
                        let swatch = if part.kind == Kind::Led { board::led_colors(Some(c)).1 } else { board::wire_color(Some(c)) };
                        let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 7.0, swatch);
                        if part.color.as_deref() == Some(*c) {
                            ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(2.0, theme::AMBER));
                        }
                        if resp.on_hover_text(*c).clicked() {
                            part.color = Some(c.to_string());
                        }
                    }
                });
            }
            Kind::Button => {
                ui.checkbox(&mut part.pressed, "Latched pressed");
                let down = held.as_deref() == Some(part.id.as_str());
                ui.label(RichText::new(if down { "HELD (Space)" } else { "Hold Space to press it momentarily." }).color(if down { theme::AMBER } else { theme::DIM }).size(12.0));
            }
            Kind::Pot => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Wiper").color(theme::DIM));
                    ui.add(egui::Slider::new(&mut part.wiper, 0.0..=1.0).show_value(false));
                    ui.label(RichText::new(format!("{:.0}%", part.wiper * 100.0)).monospace());
                });
            }
            _ => {}
        }

        ui.add_space(4.0);
        egui::Grid::new("legs").num_columns(3).spacing(vec2(10.0, 3.0)).show(ui, |ui| {
            for (leg, h) in part.legs.iter().enumerate() {
                ui.label(RichText::new(part.kind.leg_names()[leg]).color(theme::DIM).size(12.0));
                ui.label(RichText::new(h.short()).monospace());
                match sim.as_ref().and_then(|s| s.voltage(h)) {
                    Some(v) => ui.label(RichText::new(format!("{v:.2} V")).monospace().color(theme::CYAN)),
                    None => ui.label(""),
                };
                ui.end_row();
            }
        });
        if let Some(i) = sim.as_ref().and_then(|s| s.current(&part.id)) {
            let i = board::shown_current(part.kind, i);
            let what = match part.kind {
                Kind::Npn => "collector current",
                Kind::Battery => "supplying",
                _ => "current",
            };
            ui.label(RichText::new(format!("{what}: {:.3} mA", milliamps(i))).monospace().color(theme::CYAN));
        }

        ui.add_space(4.0);
        let mut delete = false;
        ui.horizontal(|ui| {
            if ui.button("⟲ Flip  (R)").on_hover_text("Reverse the legs, e.g. to turn an LED around").clicked() {
                part.legs.reverse();
            }
            delete = ui.button(RichText::new("✕ Delete").color(theme::RED)).clicked();
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
                (Some(_), _) => theme::badge_outline(ui, "NOT SIMULATED", theme::DIM),
                (None, Some(c)) if c.pass => theme::badge(ui, "✔ PASS", theme::GREEN),
                (None, Some(_)) => theme::badge(ui, "✖ FAULT", theme::RED),
                _ => theme::badge_outline(ui, "…", theme::DIM),
            };
            if let (None, Some(c)) = (&self.out.error, &self.out.checks) {
                let count = |sev| c.findings.iter().filter(|f| f.severity == sev).count();
                let (faults, warns) = (count(Severity::Fault), count(Severity::Warning));
                let mut tally = Vec::new();
                if faults > 0 {
                    tally.push(RichText::new(format!("{faults} fault{}", if faults == 1 { "" } else { "s" })).color(theme::RED));
                }
                if warns > 0 {
                    tally.push(RichText::new(format!("{warns} warning{}", if warns == 1 { "" } else { "s" })).color(theme::AMBER));
                }
                for t in tally {
                    ui.label(t.font(FontId::monospace(11.0)).strong());
                }
            }
            ui.label(RichText::new("from ngspice + rule checks").font(FontId::monospace(10.5)).color(theme::DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut self.show_readouts, "readouts");
                ui.checkbox(&mut self.show_fix, "fix ghosts");
            });
        });
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if let Some(e) = &self.out.error {
                if !self.circuit.parts.is_empty() {
                    let first = e.lines().next().unwrap_or("").to_string();
                    ui.label(RichText::new(first).color(theme::AMBER));
                }
            }
            let mut apply = None;
            if let Some(checks) = &self.out.checks {
                // Worst first, so a fault is never buried under OK lines.
                let mut findings: Vec<_> = checks.findings.iter().collect();
                findings.sort_by_key(|f| f.severity);
                for f in findings {
                    let (color, tag) = match f.severity {
                        Severity::Fault => (theme::RED, "✖ FAULT"),
                        Severity::Warning => (theme::AMBER, "⚠ WARN "),
                        Severity::Info => (theme::CYAN, "ℹ INFO "),
                        Severity::Ok => (theme::GREEN, "✔ OK   "),
                    };
                    let quiet = matches!(f.severity, Severity::Info | Severity::Ok);
                    let fill = if quiet { Color32::TRANSPARENT } else { color.gamma_multiply(0.12) };
                    let stroke = if quiet { Color32::TRANSPARENT } else { color.gamma_multiply(0.6) };
                    egui::Frame::new()
                        .fill(fill)
                        .stroke(Stroke::new(1.0, stroke))
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if quiet {
                                    ui.label(RichText::new(tag).font(FontId::monospace(11.0)).color(color));
                                } else {
                                    theme::badge(ui, tag.trim_end(), color);
                                }
                                let r = ui.add(egui::Label::new(RichText::new(&f.headline).size(13.5)).sense(Sense::click()));
                                if r.clicked() {
                                    self.editor.selected = Some(f.part.clone());
                                }
                                r.on_hover_text(&f.detail);
                                if let Some(fix) = &f.fix {
                                    if ui.small_button(RichText::new("Apply fix").color(theme::GREEN)).clicked() {
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
                if let Some(p) = self.circuit.part_mut(&fix.part) {
                    for (leg, to) in fix.moves {
                        p.legs[leg] = to;
                    }
                }
            }
            if let Some(nl) = &self.out.netlist {
                egui::CollapsingHeader::new(RichText::new("SPICE NETLIST").font(FontId::monospace(11.0)).color(theme::DIM)).show(ui, |ui| {
                    code_block(ui, nl.spice.trim());
                    for n in &nl.notes {
                        ui.colored_label(theme::AMBER, n);
                    }
                });
            }
            if let Some(s) = &self.out.sim {
                egui::CollapsingHeader::new(RichText::new("NGSPICE LOG").font(FontId::monospace(11.0)).color(theme::DIM)).show(ui, |ui| code_block(ui, s.log.trim()));
            }
        });
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
                ("Review my circuit", "Review my circuit. What's wrong, what's risky, and what could be better?"),
                ("Why isn't it working?", "Why isn't my circuit doing what I expect? Walk me through it."),
                ("Explain it", "Explain how this circuit works, step by step, following the current."),
                ("Ideas to try", "Suggest a few experiments or next circuits I could build from here."),
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
        egui::ScrollArea::vertical().max_height(log_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            if self.chat.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Ask anything about your circuit: why an LED is dark, what resistor to use, how a transistor switch works, or what to build next.").color(theme::DIM));
                ui.add_space(4.0);
                ui.label(RichText::new("Gemini sees the board, the nets and the simulated numbers. It can still be wrong, so check its claims against the readouts.").color(theme::DIM).size(11.5));
            }
            for entry in &self.chat {
                ui.add_space(6.0);
                match entry {
                    Entry::User(q) => {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                            theme::card(theme::EDGE, theme::EDGE).show(ui, |ui| {
                                ui.set_max_width(ui.available_width() * 0.85);
                                ui.label(q);
                            });
                        });
                    }
                    Entry::Error(e) => {
                        ui.colored_label(theme::RED, e);
                    }
                    Entry::Reply(r) => {
                        ui.label(&r.message);
                        for issue in &r.issues {
                            if issue_card(ui, issue, &mut self.editor.selected) {
                                hovered_parts = issue.parts.clone();
                            }
                        }
                        for idea in &r.ideas {
                            theme::card(theme::BG, theme::CYAN.gamma_multiply(0.5)).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(format!("💡 {}", idea.title)).color(theme::CYAN).strong());
                                ui.label(RichText::new(&idea.description).size(12.5));
                            });
                        }
                    }
                }
            }
            if busy {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().color(theme::AMBER));
                    ui.label(RichText::new(format!("{} is thinking…", self.config.gemini_model)).color(theme::DIM));
                });
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let r = ui.add_sized(
                vec2(ui.available_width() - 60.0, 54.0),
                egui::TextEdit::multiline(&mut self.input).hint_text("Ask your lab partner…  (Enter to send)").desired_rows(2),
            );
            let enter = r.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift);
            if enter {
                // The multiline edit already inserted the newline.
                self.input = self.input.trim_end_matches('\n').to_string();
            }
            let send = ui.add_enabled(!busy && !self.input.trim().is_empty(), egui::Button::new("Send").min_size(vec2(52.0, 54.0))).clicked();
            if (enter || send) && !busy {
                ask = Some(std::mem::take(&mut self.input));
            }
        });
        if let Some(q) = ask {
            self.ask(ui.ctx(), q);
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
        egui::Panel::bottom("checks").frame(side).default_size(170.0).min_size(90.0).resizable(true).show(ui, |ui| self.checks_panel(ui));

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
                let galley = painter.layout_no_wrap(text.clone(), FontId::monospace(12.5), theme::BG);
                let r = egui::Rect::from_center_size(egui::pos2(screen.center().x, screen.top() + 70.0), galley.size() + vec2(24.0, 12.0));
                painter.rect_filled(r, 6, theme::AMBER);
                painter.galley(r.min + vec2(12.0, 6.0), galley, theme::BG);
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
        if self.sim_rx.is_some() || self.ai_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let title = format!("Ground Truth — {}{}", self.circuit.name, if self.circuit != self.saved { " •" } else { "" });
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

/// Renders a lab-partner issue; returns true while hovered.
fn issue_card(ui: &mut egui::Ui, issue: &Issue, selected: &mut Option<String>) -> bool {
    let color = match issue.severity.as_str() {
        "error" => theme::RED,
        "warning" => theme::AMBER,
        _ => theme::GREEN,
    };
    let r = theme::card(color.gamma_multiply(0.08), color.gamma_multiply(0.7)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(issue.severity.to_uppercase()).font(FontId::monospace(10.5)).color(color));
            ui.label(RichText::new(&issue.title).strong());
        });
        ui.label(RichText::new(&issue.explanation).size(12.5));
        if !issue.suggestion.is_empty() {
            ui.label(RichText::new(format!("→ {}", issue.suggestion)).size(12.5).color(theme::GREEN));
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

fn tool_button(ui: &mut egui::Ui, text: &str, hint: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).font(FontId::monospace(12.0)))).on_hover_text(hint)
}

fn chip(ui: &mut egui::Ui, key: &str, value: &str, color: Color32) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let (r, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, color);
                ui.label(RichText::new(key).font(FontId::monospace(10.5)).color(theme::DIM));
                ui.label(RichText::new(value).font(FontId::monospace(11.0)));
            });
        });
}

fn code_block(ui: &mut egui::Ui, text: &str) {
    theme::card(theme::BG, theme::EDGE).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(text).font(FontId::monospace(11.0))).wrap());
    });
}

/// Parts-bin hover card: what the part is, its legs (polarity), default value and shortcut.
fn part_tooltip(ui: &mut egui::Ui, kind: Kind, key: usize) {
    ui.set_max_width(260.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(kind.name()).strong().color(theme::AMBER));
        ui.label(RichText::new(format!("{}1, {}2…", kind.id_prefix(), kind.id_prefix())).font(FontId::monospace(10.5)).color(theme::DIM));
    });
    ui.label(kind.blurb());
    ui.add_space(2.0);
    let legs = kind.leg_names().join(" · ");
    ui.label(RichText::new(format!("Legs  {legs}")).font(FontId::monospace(11.0)).color(theme::DIM));
    if let (Some(v), Some(unit)) = (kind.default_value(), kind.unit()) {
        ui.label(RichText::new(format!("Default  {}", fmt_si(v, unit))).font(FontId::monospace(11.0)).color(theme::DIM));
    }
    ui.label(RichText::new(format!("Drag onto the board, click, or press {key}")).size(11.5).color(theme::DIM));
}
