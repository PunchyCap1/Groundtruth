//! Run ngspice in batch mode and read back node voltages and branch currents.

use super::{Netlist, SimResult, TRAN_OUT, Trace};
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Ngspice {
    pub binary: String,
}

impl Ngspice {
    /// Returns the version banner if the binary runs.
    pub fn probe(&self) -> Option<String> {
        let out = Command::new(&self.binary).arg("--version").output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .find(|l| l.contains("ngspice-"))
            .map(|l| l.trim_matches(['*', ' ']).split(" : ").next().unwrap_or("").to_string())
    }

    pub fn run(&self, netlist: &Netlist) -> Result<SimResult> {
        static RUN: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("groundtruth-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let n = RUN.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("circuit-{n}.cir"));
        let data = dir.join(format!("circuit-{n}.dat"));
        let tran = netlist.spice.contains(TRAN_OUT);
        std::fs::write(&path, netlist.spice.replace(TRAN_OUT, &data.to_string_lossy()))?;

        let out = Command::new(&self.binary)
            .arg("-b")
            .arg(&path)
            .output()
            .with_context(|| format!("could not run `{}`; is ngspice installed?", self.binary));
        let _ = std::fs::remove_file(&path);
        let table = std::fs::read_to_string(&data);
        let _ = std::fs::remove_file(&data);
        let out = out?;
        let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() || log.contains("Error") {
            bail!("{}\n{log}", failure_reason(&log));
        }
        if tran {
            parse_trace(&table.context("ngspice wrote no transient data")?, log, netlist)
        } else {
            parse(&log, netlist)
        }
    }
}

/// One line a person can act on, for the top of the error (the app shows only the first line).
fn failure_reason(log: &str) -> String {
    if log.contains("singular matrix") {
        return "ngspice could not solve this circuit: a loop has no resistance in it (check for batteries or wires connected straight across each other).".into();
    }
    match log.lines().map(str::trim).find(|l| l.starts_with("Error")) {
        Some(l) => format!("ngspice could not solve this circuit: {l}"),
        None => "ngspice could not solve this circuit (see the NGSPICE LOG below).".into(),
    }
}

/// Parses a `wrdata ... all` table: a header of vector names, then one row per time point.
/// The last row becomes the instantaneous result; every row goes into the trace.
pub fn parse_trace(table: &str, log: String, netlist: &Netlist) -> Result<SimResult> {
    let mut lines = table.lines();
    let header: Vec<String> = lines.next().unwrap_or("").split_whitespace().map(str::to_lowercase).collect();
    let mut trace = Trace::default();
    for line in lines {
        let row: Vec<f64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if row.len() != header.len() {
            continue;
        }
        trace.time.push(row[0]);
        for (name, value) in header.iter().zip(&row).skip(1) {
            if let Some(src) = name.strip_suffix("#branch") {
                if let Some(key) = netlist.ammeters.get(src) {
                    trace.leg_current.entry(key.clone()).or_default().push(*value);
                }
            } else if let Some(net) = netlist.node_to_net.get(name) {
                trace.net_voltage.entry(net.clone()).or_default().push(*value);
            }
        }
    }
    if trace.time.is_empty() || trace.leg_current.is_empty() {
        bail!("ngspice ran but wrote no transient data:\n{log}");
    }
    let n = trace.time.len();
    if let Some(net) = netlist.node_to_net.get("0") {
        trace.net_voltage.insert(net.clone(), vec![0.0; n]);
    }
    Ok(SimResult { net_voltage: last(&trace.net_voltage), leg_current: last(&trace.leg_current), log, trace: Some(trace), ..Default::default() })
}

fn last<K: Clone + Ord>(series: &BTreeMap<K, Vec<f64>>) -> BTreeMap<K, f64> {
    series.iter().filter_map(|(k, v)| Some((k.clone(), *v.last()?))).collect()
}

/// Parses `print all` output: lines of the form `name = 1.234e+00`.
pub fn parse(log: &str, netlist: &Netlist) -> Result<SimResult> {
    let mut res = SimResult { log: log.to_string(), ..Default::default() };
    for line in log.lines() {
        let Some((name, value)) = line.split_once(" = ") else { continue };
        let Ok(value) = value.trim().parse::<f64>() else { continue };
        let name = name.trim().to_lowercase();

        if let Some(src) = name.strip_suffix("#branch") {
            if let Some((part, leg)) = netlist.ammeters.get(src) {
                res.leg_current.insert((part.clone(), *leg), value);
            }
        } else if let Some(net) = netlist.node_to_net.get(&name) {
            res.net_voltage.insert(net.clone(), value);
        }
    }
    // Ground is never printed.
    if let Some(net) = netlist.node_to_net.get("0") {
        res.net_voltage.insert(net.clone(), 0.0);
    }
    if res.leg_current.is_empty() {
        bail!("ngspice ran but printed no currents:\n{log}");
    }
    Ok(res)
}
