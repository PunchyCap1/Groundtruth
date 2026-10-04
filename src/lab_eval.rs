//! Lab-partner evaluation: asks Gemini about every example circuit with the same
//! context the app sends, then flags weak answers and any number Gemini states that
//! the simulator didn't give. Writes `lab_partner_report_<model>.md` in the project folder.
//!
//! Run:  cargo test lab_partner_eval -- --ignored --nocapture
//! More questions per circuit:  $env:LAB_EVAL_ALL=1 (3 questions instead of 1).
//! Only some circuits:  $env:LAB_EVAL_ONLY="Pot dimmer;LED blinker" (names as in the Examples menu, separated by ;).
//! Spacing between calls:  $env:LAB_EVAL_DELAY_SECS=13 (the default; the free tier allows 5 calls a minute).
//! The model comes from GEMINI_MODEL in .env; the report is named after it.
//! Stops early after two quota errors in a row, so it never burns the demo's quota.

use crate::assistant::{Gemini, Reply, context};
use crate::circuit::{Circuit, examples};
use crate::sim::{self, Output, ngspice::Ngspice};
use std::fmt::Write;

/// A physical quantity found in text: value in base units (V, A, Ω, W, F) and its kind.
#[derive(Clone, Debug, PartialEq)]
struct Quantity {
    value: f64,
    kind: char,
    text: String,
}

fn prefix(p: &str) -> Option<f64> {
    Some(match p {
        "" => 1.0,
        "m" => 1e-3,
        "µ" | "u" => 1e-6,
        "n" => 1e-9,
        "p" => 1e-12,
        "k" | "K" => 1e3,
        "M" => 1e6,
        _ => return None,
    })
}

/// Unit word or symbol -> (scale, kind). Kinds: V volts, A amps, R ohms, W watts, F farads.
fn unit(token: &str) -> Option<(f64, char)> {
    let words: &[(&str, f64, char)] = &[
        ("volt", 1.0, 'V'), ("volts", 1.0, 'V'), ("millivolts", 1e-3, 'V'),
        ("amp", 1.0, 'A'), ("amps", 1.0, 'A'), ("milliamp", 1e-3, 'A'), ("milliamps", 1e-3, 'A'),
        ("ohm", 1.0, 'R'), ("ohms", 1.0, 'R'), ("kilohm", 1e3, 'R'), ("kilohms", 1e3, 'R'),
        ("watt", 1.0, 'W'), ("watts", 1.0, 'W'), ("milliwatts", 1e-3, 'W'),
    ];
    let lower = token.to_lowercase();
    if let Some((_, s, k)) = words.iter().find(|(w, _, _)| *w == lower) {
        return Some((*s, *k));
    }
    for (sym, kind) in [("ohms", 'R'), ("ohm", 'R'), ("Ω", 'R'), ("V", 'V'), ("A", 'A'), ("W", 'W'), ("F", 'F')] {
        if let Some(p) = token.strip_suffix(sym) {
            if let Some(scale) = prefix(p) {
                return Some((scale, kind));
            }
        }
    }
    None
}

/// Every "number + unit" in `text`, like "14.7 mA", "1.77V", "4.7 kΩ" or "220 ohms".
/// Numbers glued to letters (R1, LED2, row c14) are skipped.
fn quantities(text: &str) -> Vec<Quantity> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts = chars[i].is_ascii_digit() && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '.' || chars[i - 1] == '_'));
        if !starts {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()))) {
            i += 1;
        }
        let number: String = chars[start..i].iter().collect();
        let mut j = i;
        if j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        let u_start = j;
        while j < chars.len() && (chars[j].is_alphabetic() || chars[j] == 'Ω' || chars[j] == 'µ') {
            j += 1;
        }
        let token: String = chars[u_start..j].iter().collect();
        if let (Ok(v), Some((scale, kind))) = (number.parse::<f64>(), unit(&token)) {
            out.push(Quantity { value: v * scale, kind, text: format!("{number} {token}") });
            i = j;
        }
    }
    out
}

/// Quantities in `reply` that match nothing in `ctx` (same kind, within 3%, or rounding near zero).
fn ungrounded(reply: &str, ctx: &[Quantity]) -> Vec<String> {
    let near = |a: f64, b: f64, kind: char| {
        let floor = match kind {
            'V' => 0.015,
            'A' => 2e-5,
            _ => 1e-12,
        };
        (a.abs() - b.abs()).abs() <= (0.03 * b.abs()).max(floor)
    };
    let mut bad: Vec<String> = quantities(reply)
        .into_iter()
        .filter(|q| !ctx.iter().any(|c| c.kind == q.kind && near(q.value, c.value, q.kind)))
        .map(|q| q.text)
        .collect();
    bad.dedup();
    bad
}

/// Simulates the way the app does: one DC pass, or 1 s of transient in 50 ms chunks.
fn simulate(c: &Circuit, ng: &Ngspice) -> Output {
    if !sim::is_dynamic(c) {
        return sim::run(c, ng);
    }
    let mut state = sim::TranState::default();
    let mut last = Output::default();
    for _ in 0..20 {
        let (out, next) = sim::run_tran(c, ng, &state, 0.05);
        if out.error.is_some() {
            return out;
        }
        state = next;
        last = out;
    }
    last
}

/// Things a reviewer should look at, beyond ungrounded numbers.
fn weak_signs(c: &Circuit, out: &Output, r: &Reply) -> Vec<String> {
    let mut flags = Vec::new();
    let words = r.message.split_whitespace().count();
    if words > 120 {
        flags.push(format!("long: {words} words in the message (aim for under 120)"));
    }
    if r.message.contains("**") || r.message.contains('#') || r.message.lines().any(|l| l.trim_start().starts_with("- ") || l.trim_start().starts_with("* ")) {
        flags.push("markdown in the message (the panel shows it as raw symbols)".into());
    }
    let faults: Vec<&str> = out
        .checks
        .iter()
        .flat_map(|k| k.findings.iter())
        .filter(|f| f.severity == sim::Severity::Fault)
        .map(|f| f.part.as_str())
        .collect();
    if !faults.is_empty() && r.issues.is_empty() {
        flags.push(format!("missed the fault: the checker flags {} but the reply lists no issues", faults.join(", ")));
    }
    for f in &faults {
        if !r.issues.iter().any(|i| i.parts.iter().any(|p| p == f)) && !r.issues.is_empty() {
            flags.push(format!("no issue mentions faulted part {f}"));
        }
    }
    if faults.is_empty() && r.issues.iter().any(|i| i.severity == "error") {
        flags.push("reports an error on a circuit the checker passes".into());
    }
    for i in &r.issues {
        for p in &i.parts {
            if c.part(p).is_none() {
                flags.push(format!("issue names part {p:?}, which isn't in the circuit"));
            }
        }
    }
    if out.error.is_some() && !r.message.to_lowercase().contains("simulat") {
        flags.push("simulation failed, but the reply doesn't say the numbers are missing".into());
    }
    flags
}

/// Live calls to Gemini: run on purpose, not with every `cargo test`.
#[test]
#[ignore]
fn lab_partner_eval() {
    let config = crate::Config::from_env();
    let api_key = config.gemini_api_key.clone().expect("GEMINI_API_KEY not set in .env");
    let gemini = Gemini { api_key, model: config.gemini_model.clone() };
    let ng = Ngspice { binary: config.ngspice.clone() };
    assert!(ng.probe().is_some(), "ngspice not found; check NGSPICE_PATH in .env");

    let mut questions = vec![("Review my circuit", "Review my circuit. What's wrong, what's risky, and what could be better?")];
    if std::env::var("LAB_EVAL_ALL").is_ok() {
        questions.push(("Why isn't it working?", "Why isn't my circuit doing what I expect? Walk me through it."));
        questions.push(("Explain it", "Explain how this circuit works, step by step, following the current."));
    }

    let only: Vec<String> = std::env::var("LAB_EVAL_ONLY")
        .map(|s| s.split(';').map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()).collect())
        .unwrap_or_default();
    let delay = std::env::var("LAB_EVAL_DELAY_SECS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(13);
    let file = format!("lab_partner_report_{}.md", gemini.model);
    let mut quota_errors = 0;

    let mut report = format!("# Lab partner report\n\nModel: {}\n\nMark each answer GOOD or WEAK and write what was wrong.\n", gemini.model);
    let (mut asked, mut flagged, mut number_flags) = (0, 0, 0);
    'circuits: for c in examples() {
        if !only.is_empty() && !only.contains(&c.name.to_lowercase()) {
            continue;
        }
        let out = simulate(&c, &ng);
        let ctx = context(&c, &out);
        let known = quantities(&ctx);
        let verdict = match (&out.error, &out.checks) {
            (Some(_), _) => "NOT SIMULATED",
            (None, Some(k)) if k.pass => "PASS",
            _ => "FAULT",
        };
        for (label, q) in &questions {
            if asked > 0 {
                std::thread::sleep(std::time::Duration::from_secs(delay));
            }
            asked += 1;
            let _ = write!(report, "\n---\n\n## {} · {label}\n\nSimulator: **{verdict}**\n\n", c.name);
            match gemini.ask(&[], &ctx, q) {
                Err(e) => {
                    flagged += 1;
                    quota_errors = if format!("{e:#}").contains("429") { quota_errors + 1 } else { 0 };
                    let _ = writeln!(report, "**ERROR:** {e:#}\n");
                    println!("{:<24} {label:<22} ERROR {}", c.name, format!("{e:#}").lines().next().unwrap_or(""));
                    if quota_errors >= 2 {
                        let _ = writeln!(report, "**Stopped early:** two quota errors in a row. Wait, or switch GEMINI_MODEL, then rerun the rest with LAB_EVAL_ONLY.\n");
                        println!("Stopped early: out of quota for {}. Rerun the rest later with LAB_EVAL_ONLY.", gemini.model);
                        break 'circuits;
                    }
                }
                Ok(r) => {
                    quota_errors = 0;
                    let mut flags = weak_signs(&c, &out, &r);
                    let bad = ungrounded(&format!("{} {}", r.message, r.issues.iter().map(|i| format!("{} {}", i.explanation, i.suggestion)).collect::<Vec<_>>().join(" ")), &known);
                    if !bad.is_empty() {
                        number_flags += 1;
                        flags.insert(0, format!("numbers not in the simulator output: {} (fine only if clearly a suggestion, like a resistor to try)", bad.join(", ")));
                    }
                    if !flags.is_empty() {
                        flagged += 1;
                    }
                    let _ = writeln!(report, "{}\n", r.message);
                    for i in &r.issues {
                        let _ = writeln!(report, "- **{}** [{}] {}: {} → {}", i.severity.to_uppercase(), i.parts.join(", "), i.title, i.explanation, i.suggestion);
                    }
                    if r.circuit.is_some() {
                        let _ = writeln!(report, "- (proposed a circuit)");
                    }
                    let _ = writeln!(report, "\n**Flags:** {}\n\n**Your verdict:** GOOD / WEAK: ", if flags.is_empty() { "none".into() } else { flags.join("; ") });
                    println!("{:<24} {label:<22} {}", c.name, if flags.is_empty() { "ok".into() } else { flags.join(" | ") });
                }
            }
        }
    }
    let _ = write!(report, "\n---\n\n**Totals:** {asked} answers, {flagged} flagged, {number_flags} with numbers not in the simulator output.\n");
    std::fs::write(&file, &report).unwrap_or_else(|e| panic!("writing {file}: {e}"));
    println!("\n{asked} answers, {flagged} flagged, {number_flags} with ungrounded numbers. Full report: {file}");
}

#[test]
fn finds_quantities_and_skips_part_names() {
    let q = quantities("LED1 draws 14.7 mA at 1.77V, R1 is 4.7 kΩ in row c14; try 330 ohms or 2 milliamps.");
    let got: Vec<(char, String)> = q.iter().map(|q| (q.kind, q.text.clone())).collect();
    assert_eq!(
        got,
        vec![('A', "14.7 mA".into()), ('V', "1.77 V".into()), ('R', "4.7 kΩ".into()), ('R', "330 ohms".into()), ('A', "2 milliamps".into())]
    );
    assert!((q[0].value - 0.0147).abs() < 1e-12 && (q[2].value - 4700.0).abs() < 1e-9);
}

#[test]
fn flags_only_numbers_the_simulator_did_not_give() {
    let ctx = quantities("- LED1 · led (red): current 14.700 mA\n- r5L = 3.234 V\n- R1 · resistor 220 Ω\n- B1 · battery 5 V");
    let bad = ungrounded("Your LED gets 14.7 mA and 3.23 V; the 220 Ω resistor is fine, but 25 mA would burn it at 9 V.", &ctx);
    assert_eq!(bad, vec!["25 mA".to_string(), "9 V".to_string()]);
}
