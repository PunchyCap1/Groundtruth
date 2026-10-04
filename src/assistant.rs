//! Gemini lab partner: reviews the circuit, answers questions, suggests experiments.

use crate::circuit::{Circuit, Kind, fmt_si};
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
    /// A circuit Gemini proposes, in the same JSON shape as `circuits/*.json`.
    /// Checked with `check_proposal` before it can be loaded onto the board.
    #[serde(default)]
    pub circuit: Option<Circuit>,
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
Without capacitors the numbers are a DC operating point. With capacitors the app runs a live transient simulation: \
the numbers are instantaneous values at simulated time t, capacitors carry charge from moment to moment, and LEDs may blink or fade. Push buttons and slide switches are open unless marked pressed/on. \
A photoresistor's resistance is set by its light level (given with the part), from about 1 MΩ dark to 1 kΩ bright.

Reply as JSON. `message` is plain text (no markdown, no bullet characters), a few short paragraphs at most. \
`issues` lists concrete problems or risks, each tied to part ids, with a clear fix in `suggestion`; leave it empty if there are none. \
`ideas` lists experiments or extensions the user could build next with these parts; include ideas only when asked or when the circuit works.

`circuit` is optional. Fill it only when the user asks you to build, design or propose a circuit; otherwise leave it out. \
It is loaded straight onto the board and then simulated by ngspice, so it must be physically valid: every part has exactly \
its legs (battery 2, wire 2, resistor 2, led 2, button 2, capacitor 2, diode 2, ldr 2, switch 2, pot 3, npn 3) in polar leg order, ids are \
unique (B1, R1, LED1, W1, SW1, POT1, C1, D1, Q1), no two legs share a hole, rows stay within 1..30, and every part is wired \
into a closed loop with the battery. Put the battery's legs on the rails (L+ and L- in row 1), connect rails to rows with \
wires, and always give an LED a series resistor. Give `value` for batteries (volts), resistors and pots (ohms) and \
capacitors (farads). Do not state simulated numbers for a proposed circuit; the app will simulate it.";

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
    }, "required": ["title", "description"] } },
    "circuit": { "type": "object", "properties": {
      "name": { "type": "string" },
      "description": { "type": "string" },
      "parts": { "type": "array", "items": { "type": "object", "properties": {
        "id": { "type": "string" },
        "kind": { "type": "string", "enum": ["battery", "wire", "resistor", "led", "button", "pot", "capacitor", "diode", "npn", "ldr", "switch"] },
        "legs": { "type": "array", "items": { "type": "object", "properties": {
          "row": { "type": "integer" },
          "col": { "type": "string", "enum": ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "L+", "L-", "R+", "R-"] }
        }, "required": ["row", "col"] } },
        "value": { "type": "number" },
        "color": { "type": "string" }
      }, "required": ["id", "kind", "legs"] } }
    }, "required": ["name", "description", "parts"] }
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

/// Checks a circuit Gemini proposed before it goes on the board: right leg count per
/// part, unique ids, every hole on a 30/45/60-row board and used once. Missing values
/// get the part's default. The result is only a layout; ngspice still supplies every number.
pub fn check_proposal(proposed: &Circuit) -> std::result::Result<Circuit, String> {
    if proposed.parts.is_empty() {
        return Err("The proposed circuit has no parts.".into());
    }
    let max_row = proposed.parts.iter().flat_map(|p| p.legs.iter().map(|h| h.row)).max().unwrap_or(1);
    let rows = [30u8, 45, 60].into_iter().find(|r| max_row <= *r).ok_or(format!("A leg is in row {max_row}; the biggest board has 60 rows."))?;
    let mut c = Circuit {
        name: proposed.name.trim().to_string(),
        description: format!("Proposed by Gemini. {}", proposed.description.trim()).trim().to_string(),
        rows,
        parts: Vec::new(),
        expect: None,
    };
    if c.name.is_empty() {
        c.name = "Gemini's circuit".into();
    }
    for p in &proposed.parts {
        let want = p.kind.leg_names().len();
        if p.legs.len() != want {
            return Err(format!("{} is a {} with {} legs; it needs {want}.", p.id, p.kind.name().to_lowercase(), p.legs.len()));
        }
        if p.id.trim().is_empty() || c.part(&p.id).is_some() {
            return Err(format!("Part id \"{}\" is empty or used twice.", p.id));
        }
        if let Some(h) = p.legs.iter().find(|h| h.row == 0) {
            return Err(format!("{} has a leg in row 0; rows start at 1 ({}).", p.id, h.short()));
        }
        if !c.fits(&p.legs, None) {
            return Err(format!("{} would share a hole with another leg.", p.id));
        }
        let mut part = p.clone();
        part.value = match p.kind.unit() {
            Some(_) => p.value.filter(|v| v.is_finite() && *v > 0.0).or(p.kind.default_value()),
            None => None,
        };
        part.pressed = false;
        c.parts.push(part);
    }
    // Same rules as opening a file (also catches ids ngspice can't use, like "R 1").
    c.validate().map_err(|e| format!("{e:#}"))?;
    Ok(c)
}

/// Plain-text snapshot of the circuit and its simulation for the prompt.
pub fn context(circuit: &Circuit, out: &Output) -> String {
    let mut s = String::new();
    let sim = out.sim.as_ref();
    let _ = writeln!(s, "CIRCUIT: {} ({} rows)", circuit.name, circuit.rows);
    if !circuit.description.is_empty() {
        let _ = writeln!(s, "{}", circuit.description);
    }

    if let Some(t) = sim.and_then(|s| s.time) {
        let _ = writeln!(s, "\nSIMULATION: live transient at t = {t:.3} s. Values below are instantaneous.");
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
            Kind::Switch => extra = if p.pressed { " [on]".into() } else { " [off]".into() },
            Kind::Pot => extra = format!(" [wiper at {:.0}% from end A]", p.wiper * 100.0),
            Kind::Capacitor => {
                if let Some((a, b)) = sim.and_then(|s| Some((s.voltage(&p.legs[0])?, s.voltage(&p.legs[1])?))) {
                    extra = format!(" [charged to {:.3} V]", a - b);
                }
            }
            Kind::Ldr => extra = format!(" [light {:.0}%, {}]", p.light * 100.0, fmt_si(p.ldr_ohms(), "Ω")),
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

    /// A canned Gemini reply with a proposed circuit loads and simulates without a key.
    #[test]
    fn proposed_circuit_loads_and_simulates() {
        let text = r#"{
          "message": "Here's a red LED with a current-limiting resistor.",
          "issues": [], "ideas": [],
          "circuit": { "name": "Red LED", "description": "5 V, 220 ohm, red LED.", "parts": [
            { "id": "B1", "kind": "battery", "legs": [{"row": 1, "col": "L+"}, {"row": 1, "col": "L-"}], "value": 5 },
            { "id": "W1", "kind": "wire", "legs": [{"row": 5, "col": "L+"}, {"row": 5, "col": "a"}], "color": "red" },
            { "id": "R1", "kind": "resistor", "legs": [{"row": 5, "col": "c"}, {"row": 10, "col": "c"}] },
            { "id": "LED1", "kind": "led", "legs": [{"row": 10, "col": "d"}, {"row": 11, "col": "d"}], "color": "red" },
            { "id": "W2", "kind": "wire", "legs": [{"row": 11, "col": "a"}, {"row": 11, "col": "L-"}], "color": "black" }
          ] }
        }"#;
        let reply: Reply = serde_json::from_str(text).unwrap();
        let c = check_proposal(reply.circuit.as_ref().unwrap()).unwrap();
        assert_eq!(c.part("R1").unwrap().value, Some(220.0), "missing value falls back to the default");
        assert!(c.description.starts_with("Proposed by Gemini."));

        let ng = crate::sim::ngspice::Ngspice { binary: crate::Config::from_env().ngspice };
        if ng.probe().is_none() {
            return;
        }
        let out = crate::sim::run(&c, &ng);
        assert!(out.checks.unwrap().pass);
        assert!(out.sim.unwrap().current("LED1").unwrap() > 5e-3);
    }

    #[test]
    fn bad_proposals_are_rejected() {
        let parse = |parts: &str| serde_json::from_str::<Circuit>(&format!(r#"{{"name": "x", "parts": {parts}}}"#)).unwrap();
        let shared = parse(r#"[{"id": "R1", "kind": "resistor", "legs": [{"row": 5, "col": "c"}, {"row": 9, "col": "c"}]},
                              {"id": "R2", "kind": "resistor", "legs": [{"row": 9, "col": "c"}, {"row": 12, "col": "c"}]}]"#);
        assert!(check_proposal(&shared).is_err());
        let three_legged_led = parse(r#"[{"id": "LED1", "kind": "led", "legs": [{"row": 1, "col": "a"}, {"row": 2, "col": "a"}, {"row": 3, "col": "a"}]}]"#);
        assert!(check_proposal(&three_legged_led).is_err());
        let off_board = parse(r#"[{"id": "R1", "kind": "resistor", "legs": [{"row": 70, "col": "c"}, {"row": 72, "col": "c"}]}]"#);
        assert!(check_proposal(&off_board).is_err());
        assert!(check_proposal(&parse("[]")).is_err());
    }

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
