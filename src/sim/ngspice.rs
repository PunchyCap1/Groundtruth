//! Run ngspice in batch mode and read back node voltages and branch currents.

use super::{Netlist, SimResult};
use anyhow::{Context, Result, bail};
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
        let path = dir.join(format!("circuit-{}.cir", RUN.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&path, &netlist.spice)?;

        let out = Command::new(&self.binary)
            .arg("-b")
            .arg(&path)
            .output()
            .with_context(|| format!("could not run `{}`; is ngspice installed?", self.binary));
        let _ = std::fs::remove_file(&path);
        let out = out?;
        let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() || log.contains("Error") {
            bail!("{}\n{log}", failure_reason(&log));
        }
        parse(&log, netlist)
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
