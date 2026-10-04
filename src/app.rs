<<<<<<< HEAD
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Color32, FontId, RichText, Sense, Stroke, StrokeKind, vec2};

use crate::pipeline::fixtures::{self, Fixture};
use crate::pipeline::grid::FixtureGridMapper;
use crate::pipeline::identify::{ComponentIdentifier, GeminiIdentifier, MockIdentifier};
use crate::pipeline::sim::Ngspice;
use crate::pipeline::verdict::milliamps;
use crate::pipeline::types::{ComponentLabels, GridMap, Netlist, SimResult, Verdict};
use crate::pipeline::{self, Config, Event, Photo, Stage, Stages, Status};
use crate::ui::{board, theme};

struct LoadedPhoto {
    path: PathBuf,
    texture: egui::TextureHandle,
    aspect: f32,
}

pub struct App {
    config: Config,
    ngspice_version: Option<String>,
    fixtures: Vec<(String, Fixture)>,
    fixture_idx: usize,
    photo: Option<LoadedPhoto>,
    notice: Option<String>,

    status: [Status; 5],
    grid: Option<GridMap>,
    labels: Option<ComponentLabels>,
    netlist: Option<Netlist>,
    sim: Option<SimResult>,
    verdict: Option<Verdict>,
    rx: Option<Receiver<Event>>,
    show_fix: bool,
    autoscan: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let config = Config::from_env();
        let ngspice_version = Ngspice { binary: config.ngspice.clone() }.probe();
        let (fixtures, notice) = match fixtures::load_all() {
            Ok(f) => (f, None),
            Err(e) => (Vec::new(), Some(format!("Could not load demo circuits: {e:#}"))),
        };
        let fixture_idx = fixtures.iter().position(|(n, _)| n == "led_wrong_row").unwrap_or(0);
        let mut app = Self {
            config,
            ngspice_version,
            fixtures,
            fixture_idx,
            photo: None,
            notice,
            status: std::array::from_fn(|_| Status::Waiting),
            grid: None,
            labels: None,
            netlist: None,
            sim: None,
            verdict: None,
            rx: None,
            show_fix: true,
            autoscan: false,
        };
        // Dev shortcut: `cargo run -- [photo] [--scan]`
        for arg in std::env::args().skip(1) {
            if arg == "--scan" {
                app.autoscan = true;
            } else {
                app.load_photo(&cc.egui_ctx, PathBuf::from(arg));
            }
        }
        app
    }

    fn online(&self) -> bool {
        self.config.gemini_api_key.is_some()
    }

    fn scanning(&self) -> bool {
        self.rx.is_some()
    }

    fn load_photo(&mut self, ctx: &egui::Context, path: PathBuf) {
        let img = match image::open(&path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.notice = Some(format!("Could not open {}: {e}", path.display()));
                return;
            }
        };
        let (w, h) = img.dimensions();
        let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
        let texture = ctx.load_texture("specimen", color, egui::TextureOptions::LINEAR);
        // A photo named after a demo circuit selects that circuit's grid stub.
        if let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
            if let Some(i) = self.fixtures.iter().position(|(n, _)| *n == stem) {
                self.fixture_idx = i;
            }
        }
        self.photo = Some(LoadedPhoto { path, texture, aspect: w as f32 / h.max(1) as f32 });
        self.notice = None;
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let Some((_, fixture)) = self.fixtures.get(self.fixture_idx).cloned() else {
            self.notice = Some("No demo circuit to stand in for the grid mapper.".into());
            return;
        };
        let photo = match &self.photo {
            Some(p) => match Photo::load(p.path.clone()) {
                Ok(p) => p,
                Err(e) => {
                    self.notice = Some(format!("{e:#}"));
                    return;
                }
            },
            None => Photo { bytes: Vec::new(), mime: "image/jpeg".into() },
        };
        let identify: Box<dyn ComponentIdentifier> = match &self.config.gemini_api_key {
            Some(key) => Box::new(GeminiIdentifier { api_key: key.clone(), model: self.config.gemini_model.clone() }),
            None => Box::new(MockIdentifier { fixture: fixture.clone() }),
        };
        let stages = Stages {
            grid: Box::new(FixtureGridMapper { fixture }),
            identify,
            ngspice: Ngspice { binary: self.config.ngspice.clone() },
        };

        self.status = std::array::from_fn(|_| Status::Waiting);
        self.grid = None;
        self.labels = None;
        self.netlist = None;
        self.sim = None;
        self.verdict = None;
        self.notice = None;

        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || pipeline::run(photo, stages, tx, || ctx.request_repaint()));
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut finished = false;
        loop {
            match rx.try_recv() {
                Ok(Event::Stage(s, st)) => {
                    let i = Stage::ALL.iter().position(|x| *x == s).unwrap();
                    self.status[i] = st;
                }
                Ok(Event::Grid(g)) => self.grid = Some(g),
                Ok(Event::Labels(l)) => self.labels = Some(l),
                Ok(Event::Netlist(n)) => self.netlist = Some(n),
                Ok(Event::Sim(s)) => self.sim = Some(s),
                Ok(Event::Verdict(v)) => self.verdict = Some(v),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.rx = None;
        }
    }

    fn top_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new("GROUND TRUTH").font(FontId::monospace(17.0)).color(theme::AMBER).strong());
            ui.label(RichText::new("the model sees · the simulator decides").color(theme::DIM).font(FontId::monospace(11.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match &self.ngspice_version {
                    Some(v) => chip(ui, "SIM", v, theme::GREEN),
                    None => chip(ui, "SIM", "ngspice missing", theme::RED),
                }
                if self.online() {
                    chip(ui, "ID", &format!("Gemini · {}", self.config.gemini_model), theme::GREEN);
                } else {
                    chip(ui, "ID", "mock labels · no GEMINI_API_KEY", theme::AMBER);
                }
                chip(ui, "GRID", "fixture stub", theme::AMBER);
                chip(ui, "NODE", "local", theme::DIM);
            });
        });
    }

    fn intake(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "SPECIMEN");
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let w = ui.available_width();
        let h = match &self.photo {
            Some(p) => (w / p.aspect).min(260.0),
            None => 190.0,
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6, theme::BG);
        if let Some(p) = &self.photo {
            let img_rect = egui::Rect::from_center_size(rect.center(), vec2(h * p.aspect, h).min(rect.size()));
            painter.image(p.texture.id(), img_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            // Viewfinder corners.
            for (c, dx, dy) in [
                (img_rect.left_top(), 1.0, 1.0),
                (img_rect.right_top(), -1.0, 1.0),
                (img_rect.left_bottom(), 1.0, -1.0),
                (img_rect.right_bottom(), -1.0, -1.0),
            ] {
                let s = Stroke::new(2.0, theme::AMBER);
                painter.line_segment([c, c + vec2(16.0 * dx, 0.0)], s);
                painter.line_segment([c, c + vec2(0.0, 16.0 * dy)], s);
            }
        } else {
            let edge = if hovering || resp.hovered() { theme::AMBER } else { theme::EDGE };
            let r = rect.shrink(6.0);
            for side in [
                [r.left_top(), r.right_top()],
                [r.right_top(), r.right_bottom()],
                [r.right_bottom(), r.left_bottom()],
                [r.left_bottom(), r.left_top()],
            ] {
                painter.extend(egui::Shape::dashed_line(&side, Stroke::new(1.5, edge), 6.0, 5.0));
            }
            painter.text(rect.center() - vec2(0.0, 22.0), egui::Align2::CENTER_CENTER, "⊞", FontId::proportional(30.0), edge);
            painter.text(rect.center() + vec2(0.0, 12.0), egui::Align2::CENTER_CENTER, "Drop a breadboard photo", FontId::proportional(14.0), theme::TEXT);
            painter.text(rect.center() + vec2(0.0, 32.0), egui::Align2::CENTER_CENTER, "or click to browse", FontId::proportional(12.0), theme::DIM);
        }
        if hovering {
            painter.rect_stroke(rect, 6, Stroke::new(2.0, theme::AMBER), StrokeKind::Inside);
        }
        if resp.clicked() && !self.scanning() {
            if let Some(path) = rfd::FileDialog::new().add_filter("Image", &["jpg", "jpeg", "png", "webp"]).pick_file() {
                self.load_photo(ui.ctx(), path);
            }
        }
        if let Some(p) = &self.photo {
            let name = p.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            ui.label(RichText::new(format!("{name}  ·  click to replace")).font(FontId::monospace(11.0)).color(theme::DIM));
        }

        theme::section(ui, "GRID STUB");
        ui.label(RichText::new("OpenCV mapper not wired yet. Leg positions come from this demo circuit.").color(theme::DIM).size(12.0));
        let current = self.fixtures.get(self.fixture_idx).map(|(_, f)| f.name.clone()).unwrap_or("none".into());
        egui::ComboBox::from_id_salt("fixture").width(ui.available_width()).selected_text(current).show_ui(ui, |ui| {
            for (i, (_, f)) in self.fixtures.iter().enumerate() {
                ui.selectable_value(&mut self.fixture_idx, i, &f.name).on_hover_text(&f.description);
            }
        });

        ui.add_space(14.0);
        let can_scan = !self.scanning() && (self.photo.is_some() || !self.online()) && !self.fixtures.is_empty();
        let label = if self.scanning() { "SCANNING…" } else { "▶  SCAN BOARD" };
        let btn = egui::Button::new(RichText::new(label).font(FontId::monospace(15.0)).color(theme::BG).strong())
            .fill(if can_scan { theme::AMBER } else { theme::EDGE })
            .corner_radius(6);
        if ui.add_enabled(can_scan, btn.min_size(vec2(ui.available_width(), 46.0))).clicked() {
            self.start_scan(ui.ctx());
        }
        if self.online() && self.photo.is_none() {
            ui.label(RichText::new("Gemini mode needs a photo.").color(theme::DIM).size(12.0));
        }
        if let Some(n) = &self.notice {
            ui.colored_label(theme::RED, n);
        }

        theme::section(ui, "PIPELINE");
        let t = ui.input(|i| i.time);
        for (i, stage) in Stage::ALL.iter().enumerate() {
            stage_row(ui, i + 1, stage.title(), &self.status[i], t);
        }
    }

    fn readout(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "VERDICT");
        match &self.verdict {
            None => {
                let failed = self.status.iter().find_map(|s| if let Status::Failed(e) = s { Some(e.clone()) } else { None });
                theme::card(theme::BG, theme::EDGE).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match failed {
                        Some(_) => ui.label(RichText::new("NO VERDICT").font(FontId::monospace(16.0)).color(theme::RED)),
                        None => ui.label(RichText::new("—").font(FontId::monospace(16.0)).color(theme::DIM)),
                    };
                    ui.label(RichText::new("Verdicts come only from ngspice output.").color(theme::DIM).size(12.0));
                });
            }
            Some(v) => {
                let (color, word) = if v.pass { (theme::GREEN, "PASS") } else { (theme::RED, "FAULT") };
                theme::card(color.gamma_multiply(0.12), color).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(word).font(FontId::monospace(22.0)).color(color).strong());
                    for f in v.findings.iter().filter(|f| f.fault == !v.pass) {
                        ui.add_space(4.0);
                        ui.label(RichText::new(&f.headline).size(14.5).strong());
                        ui.label(RichText::new(&f.detail).color(theme::DIM).size(12.5));
                    }
                });
                if v.findings.iter().any(|f| f.fix.is_some()) {
                    ui.checkbox(&mut self.show_fix, "Show corrected placement");
                }
            }
        }

        theme::section(ui, "PARTS");
        if let (Some(grid), Some(labels)) = (&self.grid, &self.labels) {
            egui::Grid::new("parts").striped(true).num_columns(4).spacing(vec2(10.0, 4.0)).show(ui, |ui| {
                for h in ["ID", "PART", "AT", "I"] {
                    ui.label(RichText::new(h).font(FontId::monospace(10.5)).color(theme::DIM));
                }
                ui.end_row();
                for part in &grid.parts {
                    let label = labels.components.iter().find(|c| c.id == part.id);
                    ui.label(RichText::new(&part.id).monospace());
                    ui.label(label.map(describe).unwrap_or("?".into()));
                    let at: Vec<String> = part.legs.iter().map(|h| h.label().replace(" rail, row ", "")).collect();
                    ui.label(RichText::new(at.join("→")).monospace().color(theme::DIM));
                    match self.sim.as_ref().and_then(|s| s.part_current.get(&part.id)) {
                        Some(i) => ui.label(RichText::new(format!("{:.2} mA", milliamps(i.abs()))).monospace().color(theme::CYAN)),
                        None => ui.label(RichText::new("—").color(theme::DIM)),
                    };
                    ui.end_row();
                }
            });
        } else {
            ui.label(RichText::new("No parts yet.").color(theme::DIM));
        }

        ui.add_space(8.0);
        if let Some(nl) = &self.netlist {
            egui::CollapsingHeader::new(RichText::new("SPICE NETLIST").font(FontId::monospace(11.0)).color(theme::DIM)).show(ui, |ui| {
                code_block(ui, &nl.spice);
                for n in &nl.notes {
                    ui.colored_label(theme::AMBER, n);
                }
            });
        }
        if let Some(sim) = &self.sim {
            egui::CollapsingHeader::new(RichText::new("NGSPICE LOG").font(FontId::monospace(11.0)).color(theme::DIM)).show(ui, |ui| {
                code_block(ui, sim.log.trim());
            });
        }
        if let Some(Status::Failed(e)) = self.status.iter().find(|s| matches!(s, Status::Failed(_))) {
            egui::CollapsingHeader::new(RichText::new("ERROR").font(FontId::monospace(11.0)).color(theme::RED))
                .default_open(true)
                .show(ui, |ui| code_block(ui, e));
        }
=======
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use crate::node_client::{self, ComponentLabels};

pub struct App {
    node_url: String,
    photo: Option<PathBuf>,
    status: String,
    labels: Option<ComponentLabels>,
    pending: Option<Receiver<Result<ComponentLabels, String>>>,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            node_url: std::env::var("GROUNDTRUTH_NODE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8000".to_string()),
            photo: None,
            status: "Pick a breadboard photo to start.".to_string(),
            labels: None,
            pending: None,
        }
    }

    fn start_identify(&mut self, ctx: egui::Context) {
        let Some(photo) = self.photo.clone() else { return };
        let url = self.node_url.clone();
        let (tx, rx) = channel();
        self.pending = Some(rx);
        self.status = "Asking the node to identify components...".to_string();

        // The HTTP call runs on its own thread so the window doesn't freeze.
        std::thread::spawn(move || {
            let result = node_client::identify(&url, &photo);
            let _ = tx.send(result);
            ctx.request_repaint(); // wake the UI up to show the answer
        });
    }

    fn poll_result(&mut self) {
        let Some(rx) = &self.pending else { return };
        if let Ok(result) = rx.try_recv() {
            self.pending = None;
            match result {
                Ok(labels) => {
                    self.status = format!(
                        "Found {} component(s) (source: {}).",
                        labels.components.len(),
                        labels.source
                    );
                    self.labels = Some(labels);
                }
                Err(e) => {
                    self.status = format!("Error: {e}");
                    self.labels = None;
                }
            }
        }
>>>>>>> 92d53ccd88f61fa90ac6fe382ee49afa8f5769ef
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
<<<<<<< HEAD
        self.poll();
        if std::mem::take(&mut self.autoscan) {
            self.start_scan(ui.ctx());
        }

        let dropped = ui.ctx().input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            if !self.scanning() {
                self.load_photo(ui.ctx(), path);
            }
        }

        let bar = egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(14, 8));
        egui::Panel::top("top").frame(bar).exact_size(44.0).show(ui, |ui| self.top_bar(ui));

        let side = egui::Frame::new().fill(theme::PANEL).inner_margin(14).stroke(Stroke::new(1.0, theme::EDGE));
        egui::Panel::left("intake").frame(side).exact_size(280.0).resizable(false).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.intake(ui));
        });
        egui::Panel::right("readout").frame(side).default_size(350.0).min_size(300.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.readout(ui));
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG).inner_margin(16)).show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::section(ui, "VIRTUAL BOARD");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("hover a hole to trace its net").font(FontId::monospace(11.0)).color(theme::DIM));
                });
            });
            let rows = self.grid.as_ref().map(|g| g.rows).unwrap_or(30);
            board::show(
                ui,
                &board::BoardView {
                    rows,
                    grid: self.grid.as_ref(),
                    labels: self.labels.as_ref(),
                    sim: self.sim.as_ref(),
                    verdict: self.verdict.as_ref(),
                    show_fix: self.show_fix,
                },
            );
        });
    }
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

fn stage_row(ui: &mut egui::Ui, n: usize, title: &str, status: &Status, t: f64) {
    let (color, detail) = match status {
        Status::Waiting => (theme::EDGE, String::new()),
        Status::Running => (theme::AMBER.gamma_multiply(0.55 + 0.45 * (t * 6.0).sin().abs() as f32), "running…".into()),
        Status::Done(s) => (theme::GREEN, s.clone()),
        Status::Failed(e) => (theme::RED, e.lines().next().unwrap_or("").to_string()),
    };
    if matches!(status, Status::Running) {
        ui.ctx().request_repaint();
    }
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 9.0, color.gamma_multiply(0.25));
        ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.5, color));
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, n.to_string(), FontId::monospace(10.0), theme::TEXT);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(title).size(13.0));
            if !detail.is_empty() {
                let c = if matches!(status, Status::Failed(_)) { theme::RED } else { theme::DIM };
                ui.add(egui::Label::new(RichText::new(detail).font(FontId::monospace(10.5)).color(c)).truncate());
            }
        });
    });
}

fn describe(l: &crate::pipeline::types::ComponentLabel) -> String {
    use crate::pipeline::types::Kind::*;
    match (l.kind, l.value) {
        (Resistor, Some(v)) if v >= 1000.0 => format!("resistor {}k", v / 1000.0),
        (Resistor, Some(v)) => format!("resistor {v}Ω"),
        (Supply, Some(v)) => format!("supply {v} V"),
        (Led, _) => format!("LED {}", l.markings.as_deref().unwrap_or("")),
        (k, _) => format!("{k:?}").to_lowercase(),
    }
}

fn code_block(ui: &mut egui::Ui, text: &str) {
    theme::card(theme::BG, theme::EDGE).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(text).font(FontId::monospace(11.0))).wrap());
    });
}
=======
        self.poll_result();
        let busy = self.pending.is_some();

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Groundtruth");

            ui.horizontal(|ui| {
                ui.label("Node URL:");
                ui.text_edit_singleline(&mut self.node_url);
            });

            ui.horizontal(|ui| {
                if ui.button("Choose photo...").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Images", &["jpg", "jpeg", "png", "webp"])
                        .pick_file()
                    {
                        self.photo = Some(path);
                        self.labels = None;
                        self.status = "Photo selected. Click Identify.".to_string();
                    }
                }
                if let Some(p) = &self.photo {
                    ui.label(p.display().to_string());
                }
            });

            let can_run = self.photo.is_some() && !busy;
            if ui
                .add_enabled(can_run, egui::Button::new("Identify components"))
                .clicked()
            {
                self.start_identify(ui.ctx().clone());
            }

            ui.horizontal(|ui| {
                if busy {
                    ui.spinner();
                }
                ui.label(&self.status);
            });

            ui.separator();

            if let Some(labels) = &self.labels {
                egui::Grid::new("components").striped(true).show(ui, |ui| {
                    ui.strong("Ref");
                    ui.strong("Kind");
                    ui.strong("Value");
                    ui.strong("Details");
                    ui.strong("Confidence");
                    ui.end_row();

                    for c in &labels.components {
                        ui.label(&c.reference);
                        ui.label(&c.kind);
                        ui.label(c.value.as_deref().unwrap_or("-"));
                        let mut details = Vec::new();
                        if let Some(b) = &c.color_bands {
                            details.push(format!("bands: {}", b.join("-")));
                        }
                        if let Some(m) = &c.markings {
                            details.push(format!("markings: {m}"));
                        }
                        if let Some(p) = &c.polarity_note {
                            details.push(p.clone());
                        }
                        ui.label(if details.is_empty() { "-".into() } else { details.join("; ") });
                        ui.label(format!("{:.0}%", c.confidence * 100.0));
                        ui.end_row();
                    }
                });

                if let Some(notes) = &labels.notes {
                    ui.add_space(8.0);
                    ui.label(format!("Notes: {notes}"));
                }
            }
        });
    }
}
>>>>>>> 92d53ccd88f61fa90ac6fe382ee49afa8f5769ef
