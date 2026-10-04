//! Data passed between pipeline stages.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A breadboard column. `a`–`e` and `f`–`j` are the two terminal-strip halves;
/// the four rails run the full length of the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Col {
    #[serde(rename = "a")] A,
    #[serde(rename = "b")] B,
    #[serde(rename = "c")] C,
    #[serde(rename = "d")] D,
    #[serde(rename = "e")] E,
    #[serde(rename = "f")] F,
    #[serde(rename = "g")] G,
    #[serde(rename = "h")] H,
    #[serde(rename = "i")] I,
    #[serde(rename = "j")] J,
    #[serde(rename = "L+")] LPlus,
    #[serde(rename = "L-")] LMinus,
    #[serde(rename = "R+")] RPlus,
    #[serde(rename = "R-")] RMinus,
}

impl Col {
    pub const STRIP: [Col; 10] = [Col::A, Col::B, Col::C, Col::D, Col::E, Col::F, Col::G, Col::H, Col::I, Col::J];
    pub const RAILS: [Col; 4] = [Col::LPlus, Col::LMinus, Col::RPlus, Col::RMinus];

    pub fn is_rail(self) -> bool {
        Col::RAILS.contains(&self)
    }

    pub fn name(self) -> &'static str {
        match self {
            Col::A => "a", Col::B => "b", Col::C => "c", Col::D => "d", Col::E => "e",
            Col::F => "f", Col::G => "g", Col::H => "h", Col::I => "i", Col::J => "j",
            Col::LPlus => "L+", Col::LMinus => "L-", Col::RPlus => "R+", Col::RMinus => "R-",
        }
    }
}

/// One hole on the board. Rows are 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hole {
    pub row: u8,
    pub col: Col,
}

impl Hole {
    /// Holes with the same key are electrically connected by the board itself.
    pub fn net_key(&self) -> String {
        match self.col {
            Col::A | Col::B | Col::C | Col::D | Col::E => format!("r{}L", self.row),
            Col::F | Col::G | Col::H | Col::I | Col::J => format!("r{}R", self.row),
            rail => rail.name().to_string(),
        }
    }

    pub fn label(&self) -> String {
        if self.col.is_rail() {
            format!("{} rail, row {}", self.col.name(), self.row)
        } else {
            format!("{}{}", self.col.name(), self.row)
        }
    }

    /// True if both holes sit in the same half (a–e, f–j) or on the same rail.
    pub fn same_side(&self, other: &Hole) -> bool {
        let side = |c: Col| match c {
            Col::A | Col::B | Col::C | Col::D | Col::E => 0,
            Col::F | Col::G | Col::H | Col::I | Col::J => 1,
            Col::LPlus => 2, Col::LMinus => 3, Col::RPlus => 4, Col::RMinus => 5,
        };
        side(self.col) == side(other.col)
    }
}

/// Stage 1 output: where every part's legs are, from OpenCV. No labels yet.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GridMap {
    pub rows: u8,
    pub parts: Vec<PlacedPart>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlacedPart {
    pub id: String,
    pub legs: Vec<Hole>,
    /// Bounding box in the photo, normalized `[x, y, w, h]`. Sent to the labeler as a hint.
    #[serde(default)]
    pub bbox: Option<[f32; 4]>,
}

/// Stage 2 output. Mirrors `schema/component_labels.schema.json`, which is the
/// contract for both the Gemini and the YOLO path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentLabels {
    pub components: Vec<ComponentLabel>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentLabel {
    pub id: String,
    pub kind: Kind,
    pub value: Option<f64>,
    pub markings: Option<String>,
    pub positive_leg: Option<u8>,
    pub confidence: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Resistor,
    Led,
    Wire,
    Supply,
    Capacitor,
    Button,
    Ic,
    Unknown,
}

/// Stage 3 output.
#[derive(Clone, Debug, Default)]
pub struct Netlist {
    pub spice: String,
    /// SPICE node name -> board net key (see [`Hole::net_key`]).
    pub node_to_net: BTreeMap<String, String>,
    /// Part id -> name of the 0 V source in series with it, for reading its current.
    pub ammeters: BTreeMap<String, String>,
    pub notes: Vec<String>,
}

/// Stage 4 output: raw numbers from ngspice.
#[derive(Clone, Debug, Default)]
pub struct SimResult {
    /// Board net key -> volts.
    pub net_voltage: BTreeMap<String, f64>,
    /// Part id -> amps, positive flowing into the part's first (or positive) leg.
    pub part_current: BTreeMap<String, f64>,
    pub log: String,
}

/// Stage 5 output. Every number in here is copied from [`SimResult`].
#[derive(Clone, Debug)]
pub struct Verdict {
    pub pass: bool,
    pub findings: Vec<Finding>,
}

#[derive(Clone, Debug)]
pub struct Finding {
    #[cfg_attr(not(test), allow(dead_code))]
    pub part: String,
    pub fault: bool,
    pub headline: String,
    pub detail: String,
    pub highlight: Vec<Hole>,
    pub fix: Option<Fix>,
}

/// A suggested corrected placement, rendered as a ghost on the virtual board.
#[derive(Clone, Debug)]
pub struct Fix {
    pub part: String,
    /// (leg index, destination hole)
    pub moves: Vec<(usize, Hole)>,
}
