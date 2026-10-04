//! Stage 5: turn simulated numbers into findings. Rules read only SimResult
//! and the grid map, so every number shown comes from ngspice.

use super::netlist::legs_by_net;
use super::types::{ComponentLabels, Finding, Fix, GridMap, Hole, Kind, PlacedPart, SimResult, Verdict};

const LED_MIN_A: f64 = 0.5e-3;
const LED_MAX_A: f64 = 25e-3;
const SHORT_A: f64 = 1.0;

/// Amps to milliamps, without printing "-0.00".
pub fn milliamps(i: f64) -> f64 {
    let ma = i * 1e3;
    if ma.abs() < 0.005 { 0.0 } else { ma }
}

pub fn analyze(grid: &GridMap, labels: &ComponentLabels, sim: &SimResult) -> Verdict {
    let mut findings = Vec::new();
    let nets = legs_by_net(grid);
    let v = |h: &Hole| sim.net_voltage.get(&h.net_key()).copied().unwrap_or(0.0);

    for part in &grid.parts {
        let Some(label) = labels.components.iter().find(|c| c.id == part.id) else { continue };
        let i = sim.part_current.get(&part.id).copied();

        match label.kind {
            Kind::Led => {
                let Some(i) = i else { continue };
                let pos = label.positive_leg.unwrap_or(0) as usize;
                let (anode, cathode) = (part.legs[pos], part.legs[1 - pos]);
                let ma = milliamps(i);

                if i < LED_MIN_A {
                    findings.push(dark_led(part, pos, ma, v(&anode), v(&cathode), grid, &nets));
                } else if i > LED_MAX_A {
                    findings.push(Finding {
                        part: part.id.clone(),
                        fault: true,
                        headline: format!("{} draws {:.1} mA, above the ~20 mA rating.", part.id, ma),
                        detail: "There is no current-limiting resistor in its path, or the resistor is too small.".into(),
                        highlight: vec![anode, cathode],
                        fix: None,
                    });
                } else {
                    findings.push(Finding {
                        part: part.id.clone(),
                        fault: false,
                        headline: format!("{} draws {:.1} mA at {:.2} V forward.", part.id, ma, v(&anode) - v(&cathode)),
                        detail: "Within normal range.".into(),
                        highlight: vec![],
                        fix: None,
                    });
                }
            }
            Kind::Supply => {
                let total = i.unwrap_or(0.0).abs();
                if total > SHORT_A {
                    findings.push(Finding {
                        part: part.id.clone(),
                        fault: true,
                        headline: format!("Short circuit: {:.1} A through the board.", total),
                        detail: "A wire or part connects the supply rails directly.".into(),
                        highlight: part.legs.clone(),
                        fix: None,
                    });
                }
            }
            Kind::Resistor => {
                if let (Some(i), Some(ohms)) = (i, label.value) {
                    let watts = i * i * ohms;
                    if watts > 0.25 {
                        findings.push(Finding {
                            part: part.id.clone(),
                            fault: true,
                            headline: format!("{} dissipates {:.2} W, over a 1/4 W rating.", part.id, watts),
                            detail: format!("{:.1} mA through {} ohms.", milliamps(i), ohms),
                            highlight: part.legs.clone(),
                            fix: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    Verdict { pass: !findings.iter().any(|f| f.fault), findings }
}

fn dark_led(
    part: &PlacedPart,
    pos: usize,
    ma: f64,
    va: f64,
    vc: f64,
    grid: &GridMap,
    nets: &std::collections::BTreeMap<String, Vec<(String, usize)>>,
) -> Finding {
    let id = part.id.as_str();
    let (anode, cathode) = (part.legs[pos], part.legs[1 - pos]);
    let base = format!("{id} draws {ma:.2} mA");

    if vc > va + 0.5 {
        return Finding {
            part: id.into(),
            fault: true,
            headline: format!("{base}: it is reversed."),
            detail: format!(
                "Anode at {} sits at {va:.2} V, cathode at {} at {vc:.2} V. Swap the legs.",
                anode.label(),
                cathode.label()
            ),
            highlight: vec![anode, cathode],
            fix: Some(Fix { part: id.into(), moves: vec![(pos, cathode), (1 - pos, anode)] }),
        };
    }

    // A leg alone in its net: look for the nearby leg it was probably meant to share a row with.
    for (leg_name, leg, hole) in [("anode", pos, anode), ("cathode", 1 - pos, cathode)] {
        let alone = nets.get(&hole.net_key()).is_some_and(|v| v.len() == 1);
        if !alone {
            continue;
        }
        let nearest = grid
            .parts
            .iter()
            .filter(|p| p.id != id)
            .flat_map(|p| p.legs.iter().map(move |h| (p, *h)))
            .filter(|(_, h)| h.same_side(&hole) && !h.col.is_rail())
            .min_by_key(|(_, h)| (h.row as i32 - hole.row as i32).abs())
            .filter(|(_, h)| (h.row as i32 - hole.row as i32).abs() <= 3);

        return match nearest {
            Some((other, h)) => Finding {
                part: id.into(),
                fault: true,
                headline: format!("{base} because {} is in row {}, not row {}.", other.id, h.row, hole.row),
                detail: format!(
                    "{id}'s {leg_name} at {} connects to nothing else, so no current can flow. Move it to row {}.",
                    hole.label(),
                    h.row
                ),
                highlight: vec![hole, h],
                fix: Some(Fix { part: id.into(), moves: vec![(leg, Hole { row: h.row, col: hole.col })] }),
            },
            None => Finding {
                part: id.into(),
                fault: true,
                headline: format!("{base}: its {leg_name} is not connected."),
                detail: format!("Nothing else is plugged into the row of {}.", hole.label()),
                highlight: vec![hole],
                fix: None,
            },
        };
    }

    Finding {
        part: id.into(),
        fault: true,
        headline: format!("{base}: no path from the supply."),
        detail: format!("Anode at {va:.2} V, cathode at {vc:.2} V."),
        highlight: vec![anode, cathode],
        fix: None,
    }
}
