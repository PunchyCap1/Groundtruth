//! photo -> grid map -> labels -> netlist -> ngspice -> verdict.
//!
//! Each stage is a plain function or trait object so it can later be handed to
//! a cluster node instead of running in-process.

pub mod fixtures;
pub mod grid;
pub mod identify;
pub mod netlist;
pub mod sim;
pub mod types;
pub mod verdict;

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use grid::GridMapper;
use identify::ComponentIdentifier;
use types::{ComponentLabels, GridMap, Netlist, SimResult, Verdict};

pub struct Photo {
    pub bytes: Vec<u8>,
    pub mime: String,
}

impl Photo {
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let bytes = std::fs::read(&path)?;
        let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
            Some("png") => "image/png",
            Some("webp") => "image/webp",
            _ => "image/jpeg",
        }
        .to_string();
        Ok(Self { bytes, mime })
    }
}

/// Settings read from the environment / `.env`.
#[derive(Clone)]
pub struct Config {
    pub gemini_api_key: Option<String>,
    pub gemini_model: String,
    pub ngspice: String,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Self {
            gemini_api_key: var("GEMINI_API_KEY"),
            gemini_model: var("GEMINI_MODEL").unwrap_or_else(|| "gemini-2.5-flash".into()),
            ngspice: var("NGSPICE_PATH").unwrap_or_else(|| "ngspice".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Grid,
    Identify,
    Netlist,
    Simulate,
    Verdict,
}

impl Stage {
    pub const ALL: [Stage; 5] = [Stage::Grid, Stage::Identify, Stage::Netlist, Stage::Simulate, Stage::Verdict];

    pub fn title(self) -> &'static str {
        match self {
            Stage::Grid => "Grid map",
            Stage::Identify => "Identify parts",
            Stage::Netlist => "Build netlist",
            Stage::Simulate => "Simulate",
            Stage::Verdict => "Verdict",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Status {
    Waiting,
    Running,
    Done(String),
    Failed(String),
}

pub enum Event {
    Stage(Stage, Status),
    Grid(GridMap),
    Labels(ComponentLabels),
    Netlist(Netlist),
    Sim(SimResult),
    Verdict(Verdict),
}

pub struct Stages {
    pub grid: Box<dyn GridMapper>,
    pub identify: Box<dyn ComponentIdentifier>,
    pub ngspice: sim::Ngspice,
}

/// Runs every stage in order, reporting progress. Stops at the first failure.
pub fn run(photo: Photo, stages: Stages, tx: Sender<Event>, wake: impl Fn()) {
    let send = |e: Event| {
        let _ = tx.send(e);
        wake();
    };
    let fail = |stage, err: anyhow::Error| send(Event::Stage(stage, Status::Failed(format!("{err:#}"))));

    send(Event::Stage(Stage::Grid, Status::Running));
    let grid = match stages.grid.map(&photo) {
        Ok(g) => g,
        Err(e) => return fail(Stage::Grid, e),
    };
    send(Event::Stage(Stage::Grid, Status::Done(format!("{} parts via {}", grid.parts.len(), stages.grid.name()))));
    send(Event::Grid(grid.clone()));

    send(Event::Stage(Stage::Identify, Status::Running));
    let labels = match stages.identify.identify(&photo, &grid).and_then(|l| identify::check_ids(&l, &grid).map(|_| l)) {
        Ok(l) => l,
        Err(e) => return fail(Stage::Identify, e),
    };
    send(Event::Stage(Stage::Identify, Status::Done(format!("{} labels via {}", labels.components.len(), stages.identify.name()))));
    send(Event::Labels(labels.clone()));

    send(Event::Stage(Stage::Netlist, Status::Running));
    let nl = match netlist::build(&grid, &labels) {
        Ok(n) => n,
        Err(e) => return fail(Stage::Netlist, e),
    };
    send(Event::Stage(Stage::Netlist, Status::Done(format!("{} nets", nl.node_to_net.len()))));
    send(Event::Netlist(nl.clone()));

    send(Event::Stage(Stage::Simulate, Status::Running));
    let sim = match stages.ngspice.run(&nl) {
        Ok(s) => s,
        Err(e) => return fail(Stage::Simulate, e),
    };
    send(Event::Stage(Stage::Simulate, Status::Done("ngspice .op converged".into())));
    send(Event::Sim(sim.clone()));

    send(Event::Stage(Stage::Verdict, Status::Running));
    let v = verdict::analyze(&grid, &labels, &sim);
    let faults = v.findings.iter().filter(|f| f.fault).count();
    send(Event::Stage(Stage::Verdict, Status::Done(if v.pass { "pass".into() } else { format!("{faults} fault(s)") })));
    send(Event::Verdict(v));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every demo circuit builds a netlist; if ngspice is installed, it also
    /// reaches the expected verdict.
    #[test]
    fn demo_circuits() {
        let ng = sim::Ngspice { binary: Config::from_env().ngspice };
        let have_ngspice = ng.probe().is_some();
        if !have_ngspice {
            eprintln!("ngspice not found; checking netlists only");
        }
        for (name, fx) in fixtures::load_all().unwrap() {
            identify::check_ids(&fx.labels, &fx.grid).unwrap();
            let nl = netlist::build(&fx.grid, &fx.labels).unwrap();
            if !have_ngspice {
                continue;
            }
            let sim = ng.run(&nl).unwrap();
            let v = verdict::analyze(&fx.grid, &fx.labels, &sim);
            assert_eq!(v.pass, fx.expect.pass, "{name}: {:#?}", v.findings);
            for part in &fx.expect.fault_parts {
                assert!(v.findings.iter().any(|f| f.fault && &f.part == part), "{name}: expected fault on {part}");
            }
        }
    }
}
