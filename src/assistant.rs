//! Gemini lab partner: reviews the circuit, answers questions, suggests experiments.

use crate::circuit::{Circuit, Kind};
use crate::sim::checks::milliamps;
use crate::sim::{Output, Severity};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;

#[derive(Clone, Debug, Deserialize)]
pub struct Reply {
    pub message: String,
    #[serde(default)]
    pub issues: Vec<Issue>,
    #[serde(default)]
    pub ideas: Vec<Idea>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Issue {
    /// "error", "warning" or "tip".
    pub severity: String,
    #[serde(default)]
    pub parts: Vec<String>,
    pub title: String,
    pub explanation: String,
    #[serde(default)]
    pub suggestion: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Idea {
    pub title: String,
    pub description: String,
}

/// One past exchange, kept short: the user's text and Gemini's message only.
#[derive(Clone, Debug)]
pub struct Turn {
    pub user: String,
    pub model: String,
}

const SYSTEM: &str = "You are the lab partner inside Ground Truth, a drag-and-drop breadboard electronics playground. \
Users range from complete beginners to experienced engineers; match their level (default to friendly and beginner-clear, \
switch to precise technical language if they write that way).

Breadboard facts: rows are numbered 1..N. Holes a-e in the same row are connected to each other, and f-j in the same row \
are connected to each other, but the two halves are separate (a trench runs between e and f). The rails L+, L-, R+, R- each \
run the full length of the board. A hole is written like c14 (column c, row 14) or L+5 (rail L+, row 5).

Parts: polar parts list leg 0 first: battery +, LED anode (long leg), diode anode, NPN collector/base/emitter, pot end A/wiper/end B.

Each message includes the current circuit, its nets, an ngspice DC operating point and the app's rule-checker findings. \
When you quote voltages or currents, use the simulated numbers given; if something wasn't simulated, say so rather than guessing. \
Capacitors are open in a DC operating point. Push buttons are open unless marked pressed.

Reply as JSON. `message` is plain text (no markdown, no bullet characters), a few short paragraphs at most. \
`issues` lists concrete problems or risks, each tied to part ids, with a clear fix in `suggestion`; leave it empty if there are none. \
`ideas` lists experiments or extensions the user could build next with these parts; include ideas only when asked or when the circuit works.";

const REPLY_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "message": { "type": "string" },
    "issues": { "type": "array", "items": { "type": "object", "properties": {
      "severity": { "type": "string", "enum": ["error", "warning", "tip"] },
      "parts": { "type": "array", "items": { "type": "string" } },
      "title": { "type": "string" },
      "explanation": { "type": "string" },
      "suggestion": { "type": "string" }
    }, "required": ["severity", "parts", "title", "explanation", "suggestion"] } },
    "ideas": { "type": "array", "items": { "type": "object", "properties": {
      "title": { "type": "string" },
      "description": { "type": "string" }
    }, "required": ["title", "description"] } }
  },
  "required": ["message", "issues", "ideas"]
}"#;

pub struct Gemini {
    pub api_key: String,
    pub model: String,
}

impl Gemini {
    pub fn ask(&self, history: &[Turn], context: &str, question: &str) -> Result<Reply> {
        let mut contents = Vec::new();
        for t in history {
            contents.push(json!({ "role": "user", "parts": [{ "text": t.user }] }));
            contents.push(json!({ "role": "model", "parts": [{ "text": t.model }] }));
        }
        contents.push(json!({ "role": "user", "parts": [{ "text": format!("{context}\n\n---\n{question}") }] }));

        let body = json!({
            "systemInstruction": { "parts": [{ "text": SYSTEM }] },
            "contents": contents,
            "generationConfig": {
                "responseMimeType": "application/json",
                "responseSchema": serde_json::from_str::<Value>(REPLY_SCHEMA)?,
                "temperature": 0.4,
            }
        });
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", self.model);

        // Overload errors (429/5xx) are usually brief, so back off and retry before giving up.
        const RETRY_DELAYS_SECS: [u64; 3] = [1, 2, 4];
        let mut attempt = 0;
        let reply = loop {
            let mut resp = ureq::post(&url)
                .header("x-goog-api-key", &self.api_key)
                .config()
                .http_status_as_error(false)
                // Without a timeout a dead connection leaves the lab partner spinning forever.
                .timeout_global(Some(std::time::Duration::from_secs(45)))
                .build()
                .send_json(&body)
                .context("calling Gemini")?;
            let status = resp.status();
            // Read as text first: 5xx errors are often HTML, and must still reach the retry below.
            let raw = resp.body_mut().read_to_string().context("reading Gemini response")?;
            let reply: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
            if status.is_success() {
                if reply.is_null() {
                    bail!("Gemini returned something that isn't JSON: {raw}");
                }
                break reply;
            }
            let transient = matches!(status.as_u16(), 429 | 500 | 503 | 504);
            if transient && attempt < RETRY_DELAYS_SECS.len() {
                std::thread::sleep(std::time::Duration::from_secs(RETRY_DELAYS_SECS[attempt]));
                attempt += 1;
                continue;
            }
            let msg = reply["error"]["message"].as_str().unwrap_or("unknown error");
            bail!("Gemini returned {status} after {} attempt(s): {msg}", attempt + 1);
        };

        let text = reply["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .ok_or_else(|| anyhow!("Gemini response had no text part: {reply}"))?;
        serde_json::from_str(text).with_context(|| format!("Gemini reply did not match the schema: {text}"))
    }
}

/// Plain-text snapshot of the circuit and its simulation for the prompt.
pub fn context(circuit: &Circuit, out: &Output) -> String {
    let mut s = String::new();
    let sim = out.sim.as_ref();
    let _ = writeln!(s, "CIRCUIT: {} ({} rows)", circuit.name, circuit.rows);
    if !circuit.description.is_empty() {
        let _ = writeln!(s, "{}", circuit.description);
    }

    let _ = writeln!(s, "\nPARTS:");
    if circuit.parts.is_empty() {
        let _ = writeln!(s, "(empty board)");
    }
    for p in &circuit.parts {
        let legs: Vec<String> =
            p.legs.iter().zip(p.kind.leg_names()).map(|(h, n)| format!("{n} at {}", h.short())).collect();
        let mut extra = String::new();
        match p.kind {
            Kind::Button => extra = if p.pressed { " [pressed]".into() } else { " [not pressed]".into() },
            Kind::Pot => extra = format!(" [wiper at {:.0}% from end A]", p.wiper * 100.0),
            _ => {}
        }
        let current = sim.and_then(|s| s.current(&p.id)).map(|i| format!(", current {:.3} mA", milliamps(i))).unwrap_or_default();
        let _ = writeln!(s, "- {}{extra}: {}{current}", p.describe(), legs.join(", "));
    }

    let _ = writeln!(s, "\nNETS (holes the breadboard connects, with what is plugged in):");
    for (net, legs) in circuit.nets() {
        let who: Vec<String> = legs
            .iter()
            .map(|(id, leg)| {
                let p = circuit.part(id).unwrap();
                format!("{id} {} ({})", p.kind.leg_names()[*leg], p.legs[*leg].short())
            })
            .collect();
        let volts = sim.and_then(|s| s.net_voltage.get(&net)).map(|v| format!(" = {v:.3} V")).unwrap_or_default();
        let _ = writeln!(s, "- {net}{volts}: {}", who.join(", "));
    }

    match (&out.error, &out.checks) {
        (Some(e), _) => {
            let _ = writeln!(s, "\nSIMULATION: could not run: {}", e.lines().next().unwrap_or(""));
        }
        (None, Some(checks)) => {
            let _ = writeln!(s, "\nRULE CHECKER ({}):", if checks.pass { "no faults" } else { "faults found" });
            for f in &checks.findings {
                let tag = match f.severity {
                    Severity::Fault => "FAULT",
                    Severity::Warning => "WARNING",
                    Severity::Info => "INFO",
                    Severity::Ok => "OK",
                };
                let _ = writeln!(s, "- {tag} {}: {} {}", f.part, f.headline, f.detail);
            }
        }
        _ => {}
    }
    if let Some(nl) = &out.netlist {
        let _ = writeln!(s, "\nSPICE NETLIST:\n{}", nl.spice.trim());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live call. `cargo test gemini_live -- --ignored --nocapture` (set GEMINI_MODEL to try another model)
    #[test]
    #[ignore]
    fn gemini_live() {
        let _ = dotenvy::dotenv();
        let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY not set");
        let model = std::env::var("GEMINI_MODEL").unwrap_or(crate::GEMINI_MODELS[0].into());
        let circuit = crate::circuit::examples().into_iter().find(|c| c.name == "LED, wrong row").unwrap();
        let out = crate::sim::run(&circuit, &crate::sim::ngspice::Ngspice { binary: crate::Config::from_env().ngspice });
        let reply = Gemini { api_key, model: model.clone() }
            .ask(&[], &context(&circuit, &out), "Review my circuit. Why isn't my LED lighting up?")
            .unwrap();
        println!("model: {model}\n{reply:#?}");
        assert!(!reply.message.is_empty());
    }
}
