//! Stage 4: run ngspice in batch mode and read back node voltages and currents.

use super::types::{Netlist, SimResult};
use anyhow::{Context, Result, bail};
use std::process::Command;

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
        let dir = std::env::temp_dir().join(format!("groundtruth-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("circuit.cir");
        std::fs::write(&path, &netlist.spice)?;

        let out = Command::new(&self.binary)
            .arg("-b")
            .arg(&path)
            .output()
            .with_context(|| format!("could not run `{}`; is ngspice installed?", self.binary))?;
        let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if !out.status.success() || log.contains("Error") {
            bail!("ngspice failed:\n{log}");
        }
        parse(&log, netlist)
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
            if let Some((part, _)) = netlist.ammeters.iter().find(|(_, am)| am.to_lowercase() == src) {
                res.part_current.insert(part.clone(), value);
            }
        } else if let Some(net) = netlist.node_to_net.get(&name) {
            res.net_voltage.insert(net.clone(), value);
        }
    }
    // Ground is never printed.
    if let Some(net) = netlist.node_to_net.get("0") {
        res.net_voltage.insert(net.clone(), 0.0);
    }
    if res.part_current.is_empty() {
        bail!("ngspice ran but printed no currents:\n{log}");
    }
    Ok(res)
}
