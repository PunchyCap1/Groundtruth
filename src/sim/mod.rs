//! circuit -> netlist -> ngspice -> rule checks. Runs on a worker thread on every edit.

pub mod checks;
pub mod netlist;
pub mod ngspice;

use crate::circuit::{Circuit, Hole};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Netlist {
    pub spice: String,
    /// SPICE node name -> board net key (see [`Hole::net_key`]).
    pub node_to_net: BTreeMap<String, String>,
    /// Lowercase name of a 0 V source -> the (part id, leg) it measures.
    pub ammeters: BTreeMap<String, (String, usize)>,
    pub notes: Vec<String>,
}

/// Raw numbers from ngspice.
#[derive(Clone, Debug, Default)]
pub struct SimResult {
    /// Board net key -> volts.
    pub net_voltage: BTreeMap<String, f64>,
    /// (part id, leg) -> amps flowing into the part at that leg.
    pub leg_current: BTreeMap<(String, usize), f64>,
    pub log: String,
}

impl SimResult {
    /// Current into leg 0: through a 2-leg part, into a pot's end A, or a transistor's collector.
    pub fn current(&self, id: &str) -> Option<f64> {
        self.leg_current.get(&(id.to_string(), 0)).copied()
    }

    pub fn voltage(&self, h: &Hole) -> Option<f64> {
        self.net_voltage.get(&h.net_key()).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Fault,
    Warning,
    Info,
    Ok,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub part: String,
    pub severity: Severity,
    pub headline: String,
    pub detail: String,
    pub highlight: Vec<Hole>,
    pub fix: Option<Fix>,
}

/// A suggested corrected placement, drawn as a ghost and applied with one click.
#[derive(Clone, Debug)]
pub struct Fix {
    pub part: String,
    /// (leg index, destination hole)
    pub moves: Vec<(usize, Hole)>,
}

#[derive(Clone, Debug)]
pub struct Checks {
    pub pass: bool,
    pub findings: Vec<Finding>,
}

/// Everything one simulation pass produced. `error` is set when no numbers could be computed.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub netlist: Option<Netlist>,
    pub sim: Option<SimResult>,
    pub checks: Option<Checks>,
    pub error: Option<String>,
}

pub fn run(circuit: &Circuit, ng: &ngspice::Ngspice) -> Output {
    let mut out = Output::default();
    let nl = match netlist::build(circuit) {
        Ok(n) => n,
        Err(e) => {
            out.error = Some(format!("{e:#}"));
            return out;
        }
    };
    out.netlist = Some(nl.clone());
    match ng.run(&nl) {
        Ok(sim) => {
            out.checks = Some(checks::analyze(circuit, &sim));
            out.sim = Some(sim);
        }
        Err(e) => out.error = Some(format!("{e:#}")),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::examples;

    /// Every example builds a netlist; if ngspice is installed, it also reaches the expected result.
    #[test]
    fn example_circuits() {
        let ng = ngspice::Ngspice { binary: std::env::var("NGSPICE_PATH").unwrap_or("ngspice".into()) };
        let have_ngspice = ng.probe().is_some();
        if !have_ngspice {
            eprintln!("ngspice not found; checking netlists only");
        }
        for c in examples() {
            netlist::build(&c).unwrap();
            if !have_ngspice {
                continue;
            }
            let out = run(&c, &ng);
            let checks = out.checks.unwrap_or_else(|| panic!("{}: {:?}", c.name, out.error));
            let expect = c.expect.as_ref().unwrap();
            assert_eq!(checks.pass, expect.pass, "{}: {:#?}", c.name, checks.findings);
            for part in &expect.fault_parts {
                assert!(
                    checks.findings.iter().any(|f| f.severity == Severity::Fault && &f.part == part),
                    "{}: expected fault on {part}",
                    c.name
                );
            }
        }
    }

    #[test]
    fn pressing_the_button_lights_the_led() {
        let ng = ngspice::Ngspice { binary: "ngspice".into() };
        if ng.probe().is_none() {
            return;
        }
        for name in ["Button + LED", "Transistor switch"] {
            let mut c = examples().into_iter().find(|c| c.name == name).unwrap();
            let off = run(&c, &ng).sim.unwrap().current("LED1").unwrap();
            c.part_mut("SW1").unwrap().pressed = true;
            let on = run(&c, &ng).sim.unwrap().current("LED1").unwrap();
            assert!(off < 1e-6 && on > 3e-3, "{name}: off {off} A, on {on} A");
        }
    }
}
