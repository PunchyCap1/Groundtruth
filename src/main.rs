mod app;
mod assistant;
mod circuit;
mod sim;
mod ui;

// Uncomment when you are reviewing, but we finished this
//#[cfg(test)]
//mod lab_eval;

/// Models offered in the lab-partner picker. First entry is the default.
pub const GEMINI_MODELS: [&str; 4] = ["gemini-3.8-flash", "gemini-3.5-flash", "gemini-3.7-flash", "gemini-3.1-pro-preview"];

/// Settings read from the environment / `.env`.
#[derive(Clone)]
pub struct Config {
    pub gemini_api_key: Option<String>,
    pub gemini_model: String,
    pub ngspice: String,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self {
            gemini_api_key: var("GEMINI_API_KEY"),
            gemini_model: var("GEMINI_MODEL").unwrap_or_else(|| GEMINI_MODELS[0].into()),
            ngspice: var("NGSPICE_PATH").unwrap_or_else(|| "ngspice".into()),
        }
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Ground Truth")
            .with_inner_size([1440.0, 860.0])
            .with_min_inner_size([1100.0, 680.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native("Groundtruth", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
