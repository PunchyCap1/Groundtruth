//! Demo circuits in `circuits/`. Each one carries a hand-made grid map and
//! labels, so the stub stages have something to return and tests have
//! something to check against.

use super::types::{ComponentLabels, GridMap};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
pub struct Fixture {
    pub name: String,
    pub description: String,
    pub grid: GridMap,
    pub labels: ComponentLabels,
    #[cfg_attr(not(test), allow(dead_code))]
    pub expect: Expect,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, Deserialize)]
pub struct Expect {
    pub pass: bool,
    #[serde(default)]
    pub fault_parts: Vec<String>,
}

pub fn circuits_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("circuits")
}

pub fn load_all() -> Result<Vec<(String, Fixture)>> {
    let dir = circuits_dir();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "json") {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            out.push((stem, load(&path)?));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub fn load(path: &Path) -> Result<Fixture> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}
