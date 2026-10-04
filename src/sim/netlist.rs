//! Circuit -> SPICE netlist. Every leg we want a current for gets a 0 V source
//! in series, which ngspice reports as a branch current.

use super::{Netlist, TRAN_OUT, TranState};
use crate::circuit::{Circuit, Kind};
use anyhow::{Result, anyhow, bail};
use std::fmt::Write;

/// LED models tuned so forward voltage at ~10 mA lands near the datasheet value for each color.
fn led_model(color: &str) -> (&'static str, &'static str) {
    match color {
        "green" => ("DLED_GREEN", ".model DLED_GREEN D(IS=1e-22 N=1.8 RS=2)"),
        "blue" | "white" => ("DLED_BLUE", ".model DLED_BLUE D(IS=1e-28 N=2.0 RS=5)"),
        "yellow" | "orange" => ("DLED_YELLOW", ".model DLED_YELLOW D(IS=1e-20 N=1.75 RS=2)"),
        _ => ("DLED_RED", ".model DLED_RED D(IS=1e-19 N=1.7 RS=2)"),
    }
}

/// Internal resistance of every battery, in ohms. Small enough not to change normal readings.
const BATTERY_OHMS: f64 = 0.1;

const DIODE_MODEL: &str = ".model D1N4148 D(IS=2.52n RS=0.568 N=1.752 BV=100 IBV=100u)";
const NPN_MODEL: &str = ".model Q2N3904 NPN(IS=6.734f XTI=3 EG=1.11 VAF=74.03 BF=416.4 NE=1.259 ISE=6.734f IKF=66.78m \
XTB=1.5 BR=.7371 NC=2 ISC=0 IKR=0 RC=1 CJC=3.638p MJC=.3085 VJC=.75 FC=.5 CJE=4.493p MJE=.2593 VJE=.75 TR=239.5n TF=301.2p \
ITF=.4 VTF=4 XTF=2 RB=10)";

/// DC operating point.
pub fn build(circuit: &Circuit) -> Result<Netlist> {
    build_with(circuit, None)
}

/// A transient chunk of `dt` seconds starting from `state` (capacitor charge and node voltages).
pub fn build_tran(circuit: &Circuit, state: &TranState, dt: f64) -> Result<Netlist> {
    build_with(circuit, Some((state, dt)))
}

fn build_with(circuit: &Circuit, tran: Option<(&TranState, f64)>) -> Result<Netlist> {
    let batteries: Vec<_> = circuit.parts.iter().filter(|p| p.kind == Kind::Battery).collect();
    let first = batteries.first().ok_or_else(|| anyhow!("Add a battery to power the circuit."))?;
    for b in &batteries {
        if b.legs[0].net_key() == b.legs[1].net_key() {
            bail!("{} is shorted: both of its leads are in the same row or rail.", b.id);
        }
    }
    // The first battery's − terminal is SPICE ground.
    let ground_net = first.legs[1].net_key();

    let mut nl = Netlist::default();
    let node = |net: String, nl: &mut Netlist| -> String {
        let name = if net == ground_net {
            "0".to_string()
        } else {
            format!("n_{}", net.replace('+', "p").replace('-', "m")).to_lowercase()
        };
        nl.node_to_net.insert(name.clone(), net);
        name
    };

    let mut spice = format!("* Ground Truth: {}\n.option rshunt=1e12\n", circuit.name);
    let mut models: Vec<&str> = Vec::new();

    for part in &circuit.parts {
        let id = &part.id;
        let nets: Vec<String> = part.legs.iter().map(|h| node(h.net_key(), &mut nl)).collect();

        if part.kind == Kind::Battery {
            let v = part.value.unwrap_or(5.0);
            // A small internal resistance, like a real cell. Without it two batteries in
            // parallel are two ideal sources fighting, and ngspice cannot solve the circuit.
            let inner = format!("{}_int", id.to_lowercase());
            writeln!(spice, "R_{id}_int {} {inner} {BATTERY_OHMS}", nets[0])?;
            writeln!(spice, "V_{id} {inner} {} DC {v}", nets[1])?;
            nl.ammeters.insert(format!("v_{id}").to_lowercase(), (id.clone(), 0));
            continue;
        }

        // Legs that get an ammeter connect through an internal node instead.
        let metered = if part.legs.len() == 3 { 2 } else { 1 };
        let mut pins = nets.clone();
        for (leg, pin) in pins.iter_mut().enumerate().take(metered) {
            let inner = format!("{}_{leg}", id.to_lowercase());
            let am = format!("V_am_{id}_{leg}");
            writeln!(spice, "{am} {pin} {inner} 0")?;
            nl.ammeters.insert(am.to_lowercase(), (id.clone(), leg));
            *pin = inner;
        }

        match part.kind {
            Kind::Battery => unreachable!(),
            Kind::Resistor => writeln!(spice, "R_{id} {} {} {}", pins[0], pins[1], part.value.unwrap_or(220.0).max(1e-3))?,
            Kind::Wire => writeln!(spice, "R_{id} {} {} 1m", pins[0], pins[1])?,
            Kind::Ldr => writeln!(spice, "R_{id} {} {} {}", pins[0], pins[1], part.ldr_ohms())?,
            Kind::Button | Kind::Switch => {
                let r = if part.pressed { "1m" } else { "1e12" };
                writeln!(spice, "R_{id} {} {} {r}", pins[0], pins[1])?
            }
            Kind::Capacitor => match tran {
                Some((state, _)) => {
                    let v0 = state.cap_v.get(id).copied().unwrap_or(0.0);
                    writeln!(spice, "C_{id} {} {} {} IC={v0}", pins[0], pins[1], part.value.unwrap_or(1e-6))?;
                }
                None => {
                    writeln!(spice, "C_{id} {} {} {}", pins[0], pins[1], part.value.unwrap_or(1e-6))?;
                    nl.notes.push(format!("{id}: capacitors are open circuits in a DC operating point."));
                }
            },
            Kind::Led => {
                let (name, model) = led_model(part.color.as_deref().unwrap_or("red"));
                writeln!(spice, "D_{id} {} {} {name}", pins[0], pins[1])?;
                models.push(model);
            }
            Kind::Diode => {
                writeln!(spice, "D_{id} {} {} D1N4148", pins[0], pins[1])?;
                models.push(DIODE_MODEL);
            }
            Kind::Pot => {
                let total = part.value.unwrap_or(10_000.0).max(1.0);
                let w = part.wiper.clamp(0.0, 1.0);
                writeln!(spice, "R_{id}_a {} {} {}", pins[0], pins[1], (total * w).max(0.1))?;
                writeln!(spice, "R_{id}_b {} {} {}", pins[1], pins[2], (total * (1.0 - w)).max(0.1))?;
            }
            Kind::Npn => {
                writeln!(spice, "Q_{id} {} {} {} Q2N3904", pins[0], pins[1], pins[2])?;
                models.push(NPN_MODEL);
            }
        }
    }

    models.sort();
    models.dedup();
    for m in models {
        writeln!(spice, "{m}")?;
    }
    match tran {
        None => spice.push_str(".control\nop\nprint all\n.endc\n.end\n"),
        Some((state, dt)) => {
            // Start from where the last chunk ended; nets that are new since then start at 0 V.
            for (node, net) in &nl.node_to_net {
                if let (false, Some(v)) = (node == "0", state.net_v.get(net)) {
                    writeln!(spice, ".ic v({node})={v}")?;
                }
            }
            let step = dt / 100.0;
            writeln!(spice, ".control\nset wr_singlescale\nset wr_vecnames\ntran {step} {dt} 0 {step} uic\nwrdata {TRAN_OUT} all\n.endc\n.end")?;
        }
    }
    nl.spice = spice;
    Ok(nl)
}
