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
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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