//! Talks to the Python node over HTTP.
//!
//! These structs mirror `node/groundtruth/schema.py`. If you change one side,
//! change the other, or the JSON won't parse.

use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct Component {
    #[serde(rename = "ref")] // `ref` is a Rust keyword, so the field gets another name
    pub reference: String,
    pub kind: String,
    pub value: Option<String>,
    pub color_bands: Option<Vec<String>>,
    pub markings: Option<String>,
    pub polarity_note: Option<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComponentLabels {
    pub components: Vec<Component>,
    pub notes: Option<String>,
    pub source: String,
}

/// Blocking call: run this on a background thread, never on the UI thread.
pub fn identify(node_url: &str, photo: &Path) -> Result<ComponentLabels, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120)) // vision calls can take a while
        .build()
        .map_err(|e| e.to_string())?;

    let form = reqwest::blocking::multipart::Form::new()
        .file("image", photo)
        .map_err(|e| format!("Couldn't read photo: {e}"))?;

    let url = format!("{}/identify", node_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .multipart(form)
        .send()
        .map_err(|e| format!("Couldn't reach the Python node at {url}. Is it running? ({e})"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!("Node returned {status}: {body}"));
    }

    resp.json::<ComponentLabels>()
        .map_err(|e| format!("Node sent JSON that doesn't match the schema: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the Python node running. Run with: cargo test -- --ignored
    #[test]
    #[ignore]
    fn identify_against_running_node() {
        let photo = std::env::temp_dir().join("groundtruth_link_test.jpg");
        std::fs::write(&photo, b"\xff\xd8\xff not a real photo").unwrap();
        let url = std::env::var("GROUNDTRUTH_NODE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8000".to_string());

        let labels = identify(&url, &photo).expect("node call failed");
        println!("{labels:#?}");
        assert!(!labels.components.is_empty());
    }
}