//! Turn simulated numbers into findings. Rules read only the circuit and
//! SimResult, so every number shown here comes from ngspice.

use super::{Checks, Finding, Fix, PEAK_WINDOW, Severity, SimResult};
use crate::circuit::{Circuit, Hole, Kind, Part, fmt_si};
use std::collections::BTreeMap;

const LED_MIN_A: f64 = 0.5e-3;
const LED_MAX_A: f64 = 25e-3;
const SHORT_A: f64 = 1.0;
const BASE_MAX_A: f64 = 10e-3;

/// Amps to milliamps, without printing "-0.00".
pub fn milliamps(i: f64) -> f64 {
    let ma = i * 1e3;
    if ma.abs() < 0.005 { 0.0 } else { ma }
}

pub fn analyze(circuit: &Circuit, sim: &SimResult) -> Checks {
    let mut findings = Vec::new();
    let nets = circuit.nets();
    let v = |h: &Hole| sim.voltage(h).unwrap_or(0.0);
    let finding = |part: &Part, severity, headline: String, detail: String, highlight: Vec<Hole>| Finding {
        part: part.id.clone(),
        severity,
        headline,
        detail,
        highlight,
        fix: None,
    };

    for part in &circuit.parts {
        let i = sim.current(&part.id);
        match part.kind {
            Kind::Led => {
                let Some(i) = i else { continue };
                let (anode, cathode) = (part.legs[0], part.legs[1]);
                let ma = milliamps(i);
                let peak = sim.recent_peak.get(&(part.id.clone(), 0)).copied().filter(|p| *p >= LED_MIN_A);
                if let (true, Some(peak)) = (i < LED_MIN_A, peak) {
                    // Over time, dark now but lit a moment ago: blinking or fading, not broken.
                    findings.push(finding(
                        part,
                        Severity::Info,
                        format!("{} is changing over time: {:.2} mA now, up to {:.1} mA in the last {:.0} s.", part.id, ma, milliamps(peak), PEAK_WINDOW),
                        "Watch it on the board, or select it to see its waveform on the scope.".into(),
                        vec![],
                    ));
                } else if i < LED_MIN_A {
                    findings.push(dark_led(circuit, part, ma, v(&anode), v(&cathode), &nets));
                } else if i > LED_MAX_A {
                    findings.push(finding(
                        part,
                        Severity::Fault,
                        format!("{} draws {:.1} mA, above its ~20 mA rating.", part.id, ma),
                        "There is no current-limiting resistor in its path, or the resistor is too small. A real LED would burn out.".into(),
                        vec![anode, cathode],
                    ));
                } else {
                    findings.push(finding(
                        part,
                        Severity::Ok,
                        format!("{} draws {:.1} mA at {:.2} V forward.", part.id, ma, v(&anode) - v(&cathode)),
                        "Within normal range.".into(),
                        vec![],
                    ));
                }
            }
            Kind::Battery => {
                // Branch current flows into + through the source, so a battery delivering power reads negative.
                let delivered = -i.unwrap_or(0.0);
                if delivered.abs() > SHORT_A {
                    findings.push(finding(
                        part,
                        Severity::Fault,
                        format!("Short circuit: {:.1} A out of {}.", delivered.abs(), part.id),
                        "A wire or part connects + straight to −. A real battery would get hot fast.".into(),
                        part.legs.clone(),
                    ));
                }
            }
            Kind::Resistor => {
                if let (Some(i), Some(ohms)) = (i, part.value) {
                    let watts = i * i * ohms;
                    if watts > 0.25 {
                        findings.push(finding(
                            part,
                            Severity::Fault,
                            format!("{} dissipates {:.2} W, over a 1/4 W rating.", part.id, watts),
                            format!("{:.1} mA through {}. It would overheat; use a bigger resistance.", milliamps(i), fmt_si(ohms, "Ω")),
                            part.legs.clone(),
                        ));
                    }
                }
            }
            Kind::Npn => {
                let key = (part.id.clone(), 1);
                // Over a transient run, judge the sustained current; switching spikes are normal.
                let ib = match sim.trace.as_ref().and_then(|t| t.leg_current.get(&key).map(|v| (&t.time, v))) {
                    Some((time, series)) => sustained(time, series),
                    None => sim.leg_current.get(&key).copied(),
                };
                if let Some(ib) = ib {
                    if ib > BASE_MAX_A {
                        findings.push(finding(
                            part,
                            Severity::Fault,
                            format!("{} base current is {:.1} mA.", part.id, milliamps(ib)),
                            "The base needs a resistor (1 kΩ–10 kΩ is typical) or the transistor will be damaged.".into(),
                            vec![part.legs[1]],
                        ));
                    }
                }
            }
            _ => {}
        }

        // Legs plugged into a row nothing else uses.
        if part.kind != Kind::Battery {
            for (leg, hole) in part.legs.iter().enumerate() {
                let alone = nets.get(&hole.net_key()).is_some_and(|v| v.len() == 1);
                if alone && part.kind != Kind::Led && !hole.col.is_rail() {
                    findings.push(finding(
                        part,
                        Severity::Warning,
                        format!("{}'s {} at {} isn't connected to anything.", part.id, part.kind.leg_names()[leg], hole.label()),
                        "Nothing else is plugged into that row, so no current can flow through this leg.".into(),
                        vec![*hole],
                    ));
                }
            }
        }
    }

    findings.sort_by_key(|f| f.severity);
    Checks { pass: !findings.iter().any(|f| f.severity == Severity::Fault), findings }
}

fn dark_led(circuit: &Circuit, part: &Part, ma: f64, va: f64, vc: f64, nets: &BTreeMap<String, Vec<(String, usize)>>) -> Finding {
    let id = part.id.as_str();
    let (anode, cathode) = (part.legs[0], part.legs[1]);
    let base = format!("{id} draws {ma:.2} mA");
    let fault = |headline: String, detail: String, highlight: Vec<Hole>, fix: Option<Fix>| Finding {
        part: id.into(),
        severity: Severity::Fault,
        headline,
        detail,
        highlight,
        fix,
    };

    if vc > va + 0.5 {
        return fault(
            format!("{base}: it is plugged in backwards."),
            format!(
                "The anode (+, long leg) at {} sits at {va:.2} V but the cathode at {} is at {vc:.2} V. Current can't flow backwards through an LED. Flip it.",
                anode.label(),
                cathode.label()
            ),
            vec![anode, cathode],
            Some(Fix { part: id.into(), moves: vec![(0, cathode), (1, anode)] }),
        );
    }

    // A leg alone in its net: look for the nearby leg it was probably meant to share a row with.
    for (leg_name, leg, hole) in [("anode", 0, anode), ("cathode", 1, cathode)] {
        let alone = nets.get(&hole.net_key()).is_some_and(|v| v.len() == 1);
        if !alone {
            continue;
        }
        let nearest = circuit
            .parts
            .iter()
            .filter(|p| p.id != id)
            .flat_map(|p| p.legs.iter().map(move |h| (p, *h)))
            .filter(|(_, h)| h.same_side(&hole) && !h.col.is_rail())
            .min_by_key(|(_, h)| (h.row as i32 - hole.row as i32).abs())
            .filter(|(_, h)| (h.row as i32 - hole.row as i32).abs() <= 3);

        return match nearest {
            Some((other, h)) => fault(
                format!("{base} because {} is in row {}, not row {}.", other.id, h.row, hole.row),
                format!(
                    "{id}'s {leg_name} at {} connects to nothing else, so no current can flow. Breadboard rows only connect holes in the same row. Move it to row {}.",
                    hole.label(),
                    h.row
                ),
                vec![hole, h],
                Some(Fix { part: id.into(), moves: vec![(leg, Hole { row: h.row, col: hole.col })] }),
            ),
            None => fault(
                format!("{base}: its {leg_name} is not connected."),
                format!("Nothing else is plugged into the row of {}.", hole.label()),
                vec![hole],
                None,
            ),
        };
    }

    // An open switch somewhere is the likely, and intended, reason.
    if let Some(sw) = circuit.parts.iter().find(|p| matches!(p.kind, Kind::Button | Kind::Switch) && !p.pressed) {
        let how = if sw.kind == Kind::Switch { "tap Space to flip it on" } else { "hold Space to press it" };
        return Finding {
            part: id.into(),
            severity: Severity::Info,
            headline: format!("{id} is off while {} is open.", sw.id),
            detail: format!("Select {} and {how}.", sw.id),
            highlight: vec![],
            fix: None,
        };
    }

    // A night-light style circuit is supposed to be off in bright light.
    if let Some(ldr) = circuit.parts.iter().find(|p| p.kind == Kind::Ldr && p.light > 0.5) {
        return Finding {
            part: id.into(),
            severity: Severity::Info,
            headline: format!("{id} is off while {} sees {:.0}% light.", ldr.id, ldr.light * 100.0),
            detail: format!("Select {} and drag its Light slider down to simulate covering it.", ldr.id),
            highlight: vec![],
            fix: None,
        };
    }

    fault(
        format!("{base}: no path from the battery."),
        format!("Anode at {va:.2} V, cathode at {vc:.2} V. Follow the current from + through {id} back to −; something breaks the loop."),
        vec![anode, cathode],
        None,
    )
}

/// The level a signal sits at or below for 80% of the run (time-weighted), so brief
/// spikes, like a transistor base during a switching edge, don't count.
fn sustained(time: &[f64], values: &[f64]) -> Option<f64> {
    let n = time.len().min(values.len());
    if n < 2 {
        return values.last().copied();
    }
    let mut pts: Vec<(f64, f64)> = (0..n - 1).map(|k| (values[k], (time[k + 1] - time[k]).max(0.0))).collect();
    let total: f64 = pts.iter().map(|p| p.1).sum();
    if total <= 0.0 {
        return values.last().copied();
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut acc = 0.0;
    for (v, dt) in &pts {
        acc += dt;
        if acc >= 0.8 * total {
            return Some(*v);
        }
    }
    pts.last().map(|p| p.0)
}
