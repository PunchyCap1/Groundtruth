mod app;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Groundtruth")
            .with_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Groundtruth",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
