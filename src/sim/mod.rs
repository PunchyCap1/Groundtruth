//! circuit -> netlist -> ngspice -> rule checks. Runs on a worker thread on every edit.

pub mod checks;
pub mod netlist;
pub mod ngspice;

use crate::circuit::{Circuit, Hole, Kind};
use std::collections::BTreeMap;

/// Placeholder in a transient netlist for the data file ngspice writes; the runner fills in a temp path.
pub const TRAN_OUT: &str = "@TRAN_OUT@";

/// How far back an LED's brightest moment counts as "recently lit" for the checks.
pub const PEAK_WINDOW: f64 = 2.0;

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
    /// Transient only: simulated time at the end of this chunk, in seconds.
    pub time: Option<f64>,
    /// Transient only: every time point in this chunk.
    pub trace: Option<Trace>,
    /// Transient only: largest current into each leg over the last [`PEAK_WINDOW`] seconds.
    pub recent_peak: BTreeMap<(String, usize), f64>,
}

/// Waveforms from one transient chunk. Times start at 0 at the start of the chunk.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    pub time: Vec<f64>,
    pub net_voltage: BTreeMap<String, Vec<f64>>,
    pub leg_current: BTreeMap<(String, usize), Vec<f64>>,
}

/// What carries over from one transient chunk to the next, so time keeps running across edits.
#[derive(Clone, Debug, Default)]
pub struct TranState {
    /// Simulated seconds since the last reset.
    pub t: f64,
    /// Board net key -> volts at time `t`.
    pub net_v: BTreeMap<String, f64>,
    /// Capacitor id -> volts from leg 0 to leg 1.
    pub cap_v: BTreeMap<String, f64>,
    /// (part id, leg) -> (largest current seen recently, when).
    pub peak: BTreeMap<(String, usize), (f64, f64)>,
    /// Fingerprint of the wiring the peaks belong to; see [`layout_key`].
    pub layout: u64,
}

/// Fingerprint of everything except the controls a user plays with while it runs
/// (button presses, switch, pot wiper, LDR light). When it changes, old peaks no longer
/// describe this circuit, so an LED that was just unplugged reads as a fault right away.
fn layout_key(circuit: &Circuit) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in &circuit.parts {
        format!("{}|{:?}|{:?}|{:?}|{:?}", p.id, p.kind, p.legs, p.value, p.color).hash(&mut h);
    }
    h.finish()
}

/// Circuits with something that stores energy are simulated over time instead of at a DC operating point.
pub fn is_dynamic(circuit: &Circuit) -> bool {
    circuit.parts.iter().any(|p| p.kind == Kind::Capacitor)
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

/// Simulates `dt` more seconds from `state`. On failure the state is returned unchanged.
pub fn run_tran(circuit: &Circuit, ng: &ngspice::Ngspice, state: &TranState, dt: f64) -> (Output, TranState) {
    let mut out = Output::default();
    let nl = match netlist::build_tran(circuit, state, dt) {
        Ok(n) => n,
        Err(e) => {
            out.error = Some(format!("{e:#}"));
            return (out, state.clone());
        }
    };
    out.netlist = Some(nl.clone());
    let mut sim = match ng.run(&nl) {
        Ok(s) => s,
        Err(e) => {
            out.error = Some(format!("{e:#}"));
            return (out, state.clone());
        }
    };

    let end = state.t + dt;
    let layout = layout_key(circuit);
    let peak = if state.layout == layout { state.peak.clone() } else { BTreeMap::new() };
    let mut next = TranState { t: end, net_v: sim.net_voltage.clone(), cap_v: BTreeMap::new(), peak, layout };
    for p in circuit.parts.iter().filter(|p| p.kind == Kind::Capacitor) {
        if let (Some(a), Some(b)) = (sim.voltage(&p.legs[0]), sim.voltage(&p.legs[1])) {
            next.cap_v.insert(p.id.clone(), a - b);
        }
    }
    if let Some(trace) = &sim.trace {
        for (key, series) in &trace.leg_current {
            let Some((k, max)) = series.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
            let at = state.t + trace.time[k];
            let stale = next.peak.get(key).is_none_or(|(v, when)| *when < end - PEAK_WINDOW || max >= *v);
            if stale {
                next.peak.insert(key.clone(), (max, at));
            }
        }
    }
    next.peak.retain(|(id, _), _| circuit.part(id).is_some());
    sim.time = Some(end);
    sim.recent_peak = next.peak.iter().map(|(k, (v, _))| (k.clone(), *v)).collect();
    out.checks = Some(checks::analyze(circuit, &sim));
    out.sim = Some(sim);
    (out, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::examples;

    /// Every example builds a netlist; if ngspice is installed, it also reaches the expected result.
    #[test]
    fn example_circuits() {
        let ng = ngspice::Ngspice { binary: crate::Config::from_env().ngspice };
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
        let ng = ngspice::Ngspice { binary: crate::Config::from_env().ngspice };
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

    /// Runs `seconds` of transient in 50 ms chunks, calling `each` after every chunk.
    fn run_for(c: &Circuit, ng: &ngspice::Ngspice, state: &mut TranState, seconds: f64, mut each: impl FnMut(&Output)) {
        for _ in 0..(seconds / 0.05).round() as usize {
            let (out, next) = run_tran(c, ng, state, 0.05);
            assert!(out.error.is_none(), "{}: {:?}", c.name, out.error);
            each(&out);
            *state = next;
        }
    }

    #[test]
    fn transient_chunks_follow_the_rc_curve() {
        let ng = ngspice::Ngspice { binary: crate::Config::from_env().ngspice };
        if ng.probe().is_none() {
            return;
        }
        // 5 V through 10 kΩ into 10 µF: τ = 0.1 s, so V(t) = 5 (1 − e^(−t/τ)).
        let c: Circuit = serde_json::from_str(
            r#"{"parts": [
                {"id": "B1", "kind": "battery", "legs": [{"row": 1, "col": "L+"}, {"row": 1, "col": "L-"}], "value": 5.0},
                {"id": "R1", "kind": "resistor", "legs": [{"row": 3, "col": "L+"}, {"row": 3, "col": "a"}], "value": 10000.0},
                {"id": "C1", "kind": "capacitor", "legs": [{"row": 3, "col": "c"}, {"row": 3, "col": "L-"}], "value": 1e-5}
            ]}"#,
        )
        .unwrap();
        let mut state = TranState::default();
        run_for(&c, &ng, &mut state, 0.2, |_| {});
        let want = 5.0 * (1.0 - (-2.0f64).exp());
        let got = state.cap_v["C1"];
        assert!((got - want).abs() < 0.03, "after 0.2 s: {got} V, want {want} V");
    }

    #[test]
    fn blinker_blinks_without_faults() {
        let ng = ngspice::Ngspice { binary: crate::Config::from_env().ngspice };
        if ng.probe().is_none() {
            return;
        }
        let c = examples().into_iter().find(|c| c.name == "LED blinker").unwrap();
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        let mut state = TranState::default();
        run_for(&c, &ng, &mut state, 3.0, |out| {
            let i = out.sim.as_ref().unwrap().current("LED1").unwrap();
            (lo, hi) = (lo.min(i), hi.max(i));
            let checks = out.checks.as_ref().unwrap();
            assert!(checks.pass, "{:#?}", checks.findings);
        });
        assert!(hi > 3e-3 && lo < 0.5e-3, "LED1 never blinked: {lo} .. {hi} A");
    }

    #[test]
    fn fading_led_fades_after_the_button() {
        let ng = ngspice::Ngspice { binary: crate::Config::from_env().ngspice };
        if ng.probe().is_none() {
            return;
        }
        let mut c = examples().into_iter().find(|c| c.name == "Fading LED").unwrap();
        let mut state = TranState::default();
        c.part_mut("SW1").unwrap().pressed = true;
        run_for(&c, &ng, &mut state, 1.0, |_| {});
        c.part_mut("SW1").unwrap().pressed = false;
        let mut led = Vec::new();
        run_for(&c, &ng, &mut state, 1.5, |out| led.push(out.sim.as_ref().unwrap().current("LED1").unwrap()));
        assert!(led[0] > 3e-3, "lit right after release: {} A", led[0]);
        assert!(led.windows(2).all(|w| w[1] <= w[0] + 1e-6), "should only dim: {led:?}");
        assert!(led[led.len() - 1] < led[0] * 0.5, "should fade: {led:?}");
    }

    #[test]
    fn night_light_follows_the_ldr_and_switch() {
        let ng = ngspice::Ngspice { binary: crate::Config::from_env().ngspice };
        if ng.probe().is_none() {
            return;
        }
        let dark = examples().into_iter().find(|c| c.name == "Night light").unwrap();
        let led = |c: &Circuit| {
            let out = run(c, &ng);
            (out.sim.unwrap().current("LED1").unwrap(), out.checks.unwrap().pass)
        };
        let mut bright = dark.clone();
        bright.part_mut("LDR1").unwrap().light = 1.0;
        let mut switched_off = dark.clone();
        switched_off.part_mut("S1").unwrap().pressed = false;
        let (on, (lit, _), (off, ok)) = (led(&dark).0, led(&bright), led(&switched_off));
        assert!(on > 3e-3, "dark: {on} A");
        assert!(lit < 0.5e-3, "bright: {lit} A");
        assert!(off < 1e-6 && ok, "switched off: {off} A");
        assert!(led(&bright).1, "bright light should be an info, not a fault");
    }
}
