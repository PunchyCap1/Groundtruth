//! Stage 2: label each grid-mapped part. Labels only, never verdicts or geometry.

use super::Photo;
use super::fixtures::Fixture;
use super::types::{ComponentLabels, GridMap};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde_json::{Value, json};

pub const SCHEMA: &str = include_str!("../../schema/component_labels.schema.json");

pub trait ComponentIdentifier: Send {
    fn name(&self) -> String;
    fn identify(&self, photo: &Photo, grid: &GridMap) -> Result<ComponentLabels>;
}

/// Used when no `GEMINI_API_KEY` is set: returns the demo circuit's labels.
pub struct MockIdentifier {
    pub fixture: Fixture,
}

impl ComponentIdentifier for MockIdentifier {
    fn name(&self) -> String {
        "mock (no GEMINI_API_KEY)".into()
    }

    fn identify(&self, _photo: &Photo, _grid: &GridMap) -> Result<ComponentLabels> {
        Ok(self.fixture.labels.clone())
    }
}

pub struct GeminiIdentifier {
    pub api_key: String,
    pub model: String,
}

impl ComponentIdentifier for GeminiIdentifier {
    fn name(&self) -> String {
        format!("Gemini ({})", self.model)
    }

    fn identify(&self, photo: &Photo, grid: &GridMap) -> Result<ComponentLabels> {
        let parts: Vec<Value> = grid
            .parts
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "legs": p.legs.iter().map(|h| h.label()).collect::<Vec<_>>(),
                    "bbox_normalized_xywh": p.bbox,
                })
            })
            .collect();

        let prompt = format!(
            "You are labeling parts on a solderless breadboard photo.\n\
             The parts below were already located by a separate vision step. For each id, \
             identify what the part is and read its value.\n\
             - Resistors: read the color bands; value in ohms.\n\
             - Supplies: value in volts.\n\
             - LEDs and other polar parts: positive_leg is the index into that part's `legs` \
               list of the anode / + terminal (longer leg, flat edge is the cathode).\n\
             - Do not judge whether the circuit is correct. Do not move or add parts.\n\
             - Return exactly one entry per id below, with null for anything you cannot read.\n\n\
             Parts:\n{}",
            serde_json::to_string_pretty(&parts)?
        );

        let body = json!({
            "contents": [{
                "parts": [
                    { "text": prompt },
                    { "inline_data": {
                        "mime_type": photo.mime,
                        "data": base64::engine::general_purpose::STANDARD.encode(&photo.bytes),
                    }}
                ]
            }],
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseSchema": to_gemini_schema(&serde_json::from_str(SCHEMA)?),
                "temperature": 0.0,
            }
        });

        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            self.model
        );
        let mut resp = ureq::post(&url)
            .header("x-goog-api-key", &self.api_key)
            .config()
            .http_status_as_error(false)
            .build()
            .send_json(&body)
            .context("calling Gemini")?;
        let status = resp.status();
        let reply: Value = resp.body_mut().read_json().context("reading Gemini response")?;
        if !status.is_success() {
            let msg = reply["error"]["message"].as_str().unwrap_or("unknown error");
            bail!("Gemini returned {status}: {msg}");
        }

        let text = reply["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .ok_or_else(|| anyhow!("Gemini response had no text part: {reply}"))?;
        let labels: ComponentLabels =
            serde_json::from_str(text).with_context(|| format!("Gemini JSON did not match schema: {text}"))?;
        check_ids(&labels, grid)?;
        Ok(labels)
    }
}

/// Every grid part must get exactly one label, and no extras.
pub fn check_ids(labels: &ComponentLabels, grid: &GridMap) -> Result<()> {
    for p in &grid.parts {
        let n = labels.components.iter().filter(|c| c.id == p.id).count();
        if n != 1 {
            bail!("labeler returned {n} entries for {}", p.id);
        }
    }
    if let Some(extra) = labels.components.iter().find(|c| !grid.parts.iter().any(|p| p.id == c.id)) {
        bail!("labeler invented part {}", extra.id);
    }
    Ok(())
}

/// Gemini's `responseSchema` takes an OpenAPI-style subset of JSON Schema.
/// Convert the shared schema file rather than keeping a second copy.
fn to_gemini_schema(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                match k.as_str() {
                    "$schema" | "title" | "additionalProperties" => {}
                    "type" => match val {
                        Value::Array(types) => {
                            let non_null: Vec<&Value> = types.iter().filter(|t| *t != "null").collect();
                            if let Some(t) = non_null.first() {
                                out.insert("type".into(), (*t).clone());
                            }
                            if non_null.len() < types.len() {
                                out.insert("nullable".into(), Value::Bool(true));
                            }
                        }
                        other => {
                            out.insert("type".into(), other.clone());
                        }
                    },
                    _ => {
                        out.insert(k.clone(), to_gemini_schema(val));
                    }
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(to_gemini_schema).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_schema_has_no_null_types() {
        let s = to_gemini_schema(&serde_json::from_str(SCHEMA).unwrap());
        let value = &s["properties"]["components"]["items"]["properties"]["value"];
        assert_eq!(value["type"], "number");
        assert_eq!(value["nullable"], true);
        assert!(s.get("$schema").is_none());
    }
}
