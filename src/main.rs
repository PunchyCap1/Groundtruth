mod app;
mod pipeline;
mod ui;
mod node_client;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Ground Truth")
            .with_inner_size([1360.0, 820.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "Groundtruth",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
