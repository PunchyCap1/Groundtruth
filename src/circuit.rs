//! The circuit being edited: parts with their legs plugged into breadboard holes.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

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

    /// Short form for tables and prompts: `c14`, `L+5`.
    pub fn short(&self) -> String {
        format!("{}{}", self.col.name(), self.row)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Battery,
    Wire,
    Resistor,
    Led,
    Button,
    Pot,
    Capacitor,
    Diode,
    Npn,
    Ldr,
    Switch,
}

impl Kind {
    /// Palette order.
    pub const ALL: [Kind; 11] = [
        Kind::Battery,
        Kind::Wire,
        Kind::Resistor,
        Kind::Led,
        Kind::Button,
        Kind::Pot,
        Kind::Capacitor,
        Kind::Diode,
        Kind::Npn,
        Kind::Ldr,
        Kind::Switch,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Battery => "Battery",
            Kind::Wire => "Jumper wire",
            Kind::Resistor => "Resistor",
            Kind::Led => "LED",
            Kind::Button => "Push button",
            Kind::Pot => "Potentiometer",
            Kind::Capacitor => "Capacitor",
            Kind::Diode => "Diode",
            Kind::Npn => "NPN transistor",
            Kind::Ldr => "Photoresistor",
            Kind::Switch => "Slide switch",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Kind::Battery => "Powers the circuit. Plug + and − into the rails.",
            Kind::Wire => "Connects two holes. Tip: drag from any empty hole to draw one.",
            Kind::Resistor => "Limits current. Every LED needs one.",
            Kind::Led => "Lights up when current flows from + (long leg) to −.",
            Kind::Button => "Connects its legs while pressed. Select it and hold Space.",
            Kind::Pot => "Adjustable voltage divider. The middle leg is the wiper.",
            Kind::Capacitor => "Stores charge. Acts as an open circuit at DC.",
            Kind::Diode => "Lets current flow one way only (toward the stripe).",
            Kind::Npn => "A small base current switches a bigger collector current.",
            Kind::Ldr => "Resistance drops as light rises: about 1 MΩ in the dark, 1 kΩ in bright light.",
            Kind::Switch => "Stays on or off. Select it and tap Space to flip it.",
        }
    }

    pub fn id_prefix(self) -> &'static str {
        match self {
            Kind::Battery => "B",
            Kind::Wire => "W",
            Kind::Resistor => "R",
            Kind::Led => "LED",
            Kind::Button => "SW",
            Kind::Pot => "POT",
            Kind::Capacitor => "C",
            Kind::Diode => "D",
            Kind::Npn => "Q",
            Kind::Ldr => "LDR",
            Kind::Switch => "S",
        }
    }

    pub fn leg_names(self) -> &'static [&'static str] {
        match self {
            Kind::Battery => &["+", "−"],
            Kind::Led => &["anode +", "cathode −"],
            Kind::Diode => &["anode", "cathode"],
            Kind::Pot => &["end A", "wiper", "end B"],
            Kind::Npn => &["collector", "base", "emitter"],
            _ => &["leg 1", "leg 2"],
        }
    }

    pub fn unit(self) -> Option<&'static str> {
        match self {
            Kind::Battery => Some("V"),
            Kind::Resistor | Kind::Pot => Some("Ω"),
            Kind::Capacitor => Some("F"),
            _ => None,
        }
    }

    pub fn default_value(self) -> Option<f64> {
        match self {
            Kind::Battery => Some(5.0),
            Kind::Resistor => Some(220.0),
            Kind::Pot => Some(10_000.0),
            Kind::Capacitor => Some(10e-6),
            _ => None,
        }
    }

    /// Leg layout relative to the hole it is dropped on: (row offset, column).
    fn default_legs(self, at: Hole) -> Vec<Hole> {
        let down = |n: u8| Hole { row: at.row.saturating_add(n), col: at.col };
        match self {
            Kind::Battery if at.col.is_rail() => {
                let (plus, minus) = if matches!(at.col, Col::LPlus | Col::LMinus) {
                    (Col::LPlus, Col::LMinus)
                } else {
                    (Col::RPlus, Col::RMinus)
                };
                vec![Hole { row: at.row, col: plus }, Hole { row: at.row, col: minus }]
            }
            Kind::Battery | Kind::Capacitor | Kind::Button | Kind::Ldr | Kind::Switch => vec![at, down(2)],
            Kind::Wire | Kind::Diode => vec![at, down(3)],
            Kind::Resistor => vec![at, down(4)],
            Kind::Led => vec![at, down(1)],
            Kind::Pot | Kind::Npn => vec![at, down(1), down(2)],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub kind: Kind,
    /// For polar parts, leg 0 is + / anode / collector / pot end A.
    pub legs: Vec<Hole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// Wire or LED color name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Push button state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pressed: bool,
    /// Potentiometer wiper position, 0 (at end A) to 1 (at end B).
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub wiper: f64,
    /// Photoresistor light level, 0 (dark) to 1 (bright).
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub light: f64,
}

fn half() -> f64 {
    0.5
}

fn is_half(v: &f64) -> bool {
    *v == 0.5
}

impl Part {
    /// Photoresistor resistance for its light level, log-scaled from 1 MΩ (dark) to 1 kΩ (bright).
    pub fn ldr_ohms(&self) -> f64 {
        1e6 * 1e-3f64.powf(self.light.clamp(0.0, 1.0))
    }

    /// One-line description: "R1 · resistor 220 Ω".
    pub fn describe(&self) -> String {
        let mut s = format!("{} · {}", self.id, self.kind.name().to_lowercase());
        if let (Some(v), Some(unit)) = (self.value, self.kind.unit()) {
            s.push(' ');
            s.push_str(&fmt_si(v, unit));
        }
        if let Some(c) = &self.color {
            s.push_str(&format!(" ({c})"));
        }
        s
    }
}

pub const LED_COLORS: [&str; 6] = ["red", "yellow", "green", "blue", "white", "orange"];
pub const WIRE_COLORS: [&str; 6] = ["red", "black", "blue", "green", "yellow", "white"];

/// What a demo circuit should produce, checked by the tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Expect {
    pub pass: bool,
    #[serde(default)]
    pub fault_parts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Circuit {
    #[serde(default = "untitled")]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_rows")]
    pub rows: u8,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expect>,
}

fn untitled() -> String {
    "Untitled circuit".into()
}

fn default_rows() -> u8 {
    30
}

impl Default for Circuit {
    fn default() -> Self {
        Self { name: untitled(), description: String::new(), rows: default_rows(), parts: Vec::new(), expect: None }
    }
}

impl Circuit {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?).with_context(|| format!("writing {}", path.display()))
    }

    pub fn part(&self, id: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.id == id)
    }

    pub fn part_mut(&mut self, id: &str) -> Option<&mut Part> {
        self.parts.iter_mut().find(|p| p.id == id)
    }

    pub fn holes(&self) -> impl Iterator<Item = Hole> + '_ {
        (1..=self.rows).flat_map(|row| Col::STRIP.iter().chain(Col::RAILS.iter()).map(move |col| Hole { row, col: *col }))
    }

    pub fn on_board(&self, h: Hole) -> bool {
        h.row >= 1 && h.row <= self.rows
    }

    /// The part and leg plugged into `h`, if any.
    pub fn occupant(&self, h: Hole) -> Option<(&Part, usize)> {
        self.parts.iter().find_map(|p| p.legs.iter().position(|l| *l == h).map(|i| (p, i)))
    }

    /// True if every hole is on the board and free (ignoring part `except`), with no hole used twice.
    pub fn fits(&self, legs: &[Hole], except: Option<&str>) -> bool {
        legs.iter().enumerate().all(|(i, h)| {
            self.on_board(*h)
                && !legs[..i].contains(h)
                && self.occupant(*h).is_none_or(|(p, _)| Some(p.id.as_str()) == except)
        })
    }

    pub fn next_id(&self, kind: Kind) -> String {
        let prefix = kind.id_prefix();
        let n = (1..).find(|n| self.part(&format!("{prefix}{n}")).is_none()).unwrap();
        format!("{prefix}{n}")
    }

    /// Places a new part with its default leg layout at `at`, flipping upward
    /// if it would run off the bottom. Returns the new id, or None if it does not fit.
    pub fn add(&mut self, kind: Kind, at: Hole) -> Option<String> {
        let mut legs = kind.default_legs(at);
        if !self.fits(&legs, None) {
            let span = legs.iter().map(|h| h.row).max().unwrap() - at.row;
            if at.row <= span {
                return None;
            }
            // Extend upward instead, keeping leg 0 on the hole the user aimed at.
            legs = kind.default_legs(Hole { row: at.row - span, col: at.col });
            legs.reverse();
            if !self.fits(&legs, None) {
                return None;
            }
        }
        let id = self.next_id(kind);
        let color = match kind {
            Kind::Led => Some("red".to_string()),
            Kind::Wire => Some(WIRE_COLORS[self.parts.iter().filter(|p| p.kind == Kind::Wire).count() % WIRE_COLORS.len()].to_string()),
            _ => None,
        };
        self.parts.push(Part { id: id.clone(), kind, legs, value: kind.default_value(), color, pressed: false, wiper: 0.5, light: 0.5 });
        Some(id)
    }

    /// First free spot for a part, scanning down the left half of the board.
    pub fn add_anywhere(&mut self, kind: Kind) -> Option<String> {
        let cols: &[Col] = if kind == Kind::Battery { &[Col::LPlus] } else { &[Col::C, Col::H, Col::B, Col::G] };
        for col in cols {
            for row in 1..=self.rows {
                let probe = kind.default_legs(Hole { row, col: *col });
                if self.fits(&probe, None) && !self.under_a_part(&probe) {
                    return self.add(kind, Hole { row, col: *col });
                }
            }
        }
        None
    }

    /// True if `legs` (all in one column) would sit under the body of a part that
    /// already runs down that column, like an LED dropped between a resistor's legs.
    fn under_a_part(&self, legs: &[Hole]) -> bool {
        let span = |legs: &[Hole]| {
            let col = legs.first()?.col;
            let rows = legs.iter().map(|h| h.row);
            legs.iter().all(|h| h.col == col).then(|| (col, rows.clone().min().unwrap(), rows.max().unwrap()))
        };
        let Some((col, lo, hi)) = span(legs) else { return false };
        self.parts.iter().filter_map(|p| span(&p.legs)).any(|(c, a, b)| c == col && a <= hi && lo <= b)
    }

    pub fn remove(&mut self, id: &str) {
        self.parts.retain(|p| p.id != id);
    }

    /// Board net key -> every (part id, leg index) plugged into it.
    pub fn nets(&self) -> BTreeMap<String, Vec<(String, usize)>> {
        let mut map: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
        for part in &self.parts {
            for (i, hole) in part.legs.iter().enumerate() {
                map.entry(hole.net_key()).or_default().push((part.id.clone(), i));
            }
        }
        map
    }
}

/// Built-in example circuits (from `circuits/`), compiled into the binary.
pub fn examples() -> Vec<Circuit> {
    [
        include_str!("../circuits/led_fixed.json"),
        include_str!("../circuits/led_wrong_row.json"),
        include_str!("../circuits/led_reversed.json"),
        include_str!("../circuits/button_led.json"),
        include_str!("../circuits/pot_dimmer.json"),
        include_str!("../circuits/transistor_switch.json"),
        include_str!("../circuits/night_light.json"),
        include_str!("../circuits/fading_led.json"),
        include_str!("../circuits/blinker.json"),
    ]
    .iter()
    .map(|s| serde_json::from_str(s).expect("built-in example parses"))
    .collect()
}

/// 4700.0, "Ω" -> "4.7 kΩ".
pub fn fmt_si(v: f64, unit: &str) -> String {
    let a = v.abs();
    let (scale, prefix) = match a {
        a if a == 0.0 => (1.0, ""),
        a if a >= 1e6 => (1e6, "M"),
        a if a >= 1e3 => (1e3, "k"),
        a if a >= 1.0 => (1.0, ""),
        a if a >= 1e-3 => (1e-3, "m"),
        a if a >= 1e-6 => (1e-6, "µ"),
        a if a >= 1e-9 => (1e-9, "n"),
        _ => (1e-12, "p"),
    };
    let x = v / scale;
    let num = format!("{x:.2}");
    let num = num.trim_end_matches('0').trim_end_matches('.');
    format!("{num} {prefix}{unit}")
}

/// "4.7k", "4k7", "220", "10u", "1M" -> value. Ignores a trailing unit.
pub fn parse_si(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches(['Ω', 'V', 'F', 'v', 'f']).trim().replace("ohm", "").replace('µ', "u");
    let s = s.trim();
    // Decimal exponent per prefix; parsing "4.7e3" avoids float error from 4.7 * 1e3.
    let exp = |c: char| match c {
        'p' => Some(-12),
        'n' => Some(-9),
        'u' => Some(-6),
        'm' => Some(-3),
        'k' | 'K' => Some(3),
        'M' => Some(6),
        _ => None,
    };
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }
    // Suffix form (4.7k) or infix form (4k7).
    let pos = s.find(|c: char| exp(c).is_some())?;
    let e = exp(s[pos..].chars().next()?)?;
    let (int, frac) = (s[..pos].trim(), &s[pos + 1..]);
    let num = if frac.is_empty() { int.to_string() } else { format!("{int}.{frac}") };
    num.parse::<f64>().ok()?;
    format!("{num}e{e}").parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn si_round_trip() {
        assert_eq!(fmt_si(4700.0, "Ω"), "4.7 kΩ");
        assert_eq!(fmt_si(220.0, "Ω"), "220 Ω");
        assert_eq!(fmt_si(10e-6, "F"), "10 µF");
        assert_eq!(parse_si("4.7k"), Some(4700.0));
        assert_eq!(parse_si("4k7"), Some(4700.0));
        assert_eq!(parse_si("10uF"), Some(10e-6));
        assert_eq!(parse_si("1M"), Some(1e6));
        assert_eq!(parse_si("abc"), None);
    }

    #[test]
    fn add_rejects_occupied_holes() {
        let mut c = Circuit::default();
        let at = Hole { row: 5, col: Col::C };
        assert_eq!(c.add(Kind::Resistor, at).as_deref(), Some("R1"));
        assert_eq!(c.add(Kind::Led, at), None);
        assert_eq!(c.add(Kind::Led, Hole { row: 5, col: Col::D }).as_deref(), Some("LED1"));
    }

    #[test]
    fn add_anywhere_skips_part_bodies() {
        let mut c = Circuit::default();
        let r = c.add_anywhere(Kind::Resistor).unwrap();
        let led = c.add_anywhere(Kind::Led).unwrap();
        let (r, led) = (c.part(&r).unwrap(), c.part(&led).unwrap());
        let r_rows = r.legs[0].row..=r.legs[1].row;
        assert!(led.legs.iter().all(|h| h.col != r.legs[0].col || !r_rows.contains(&h.row)), "LED placed under R1: {:?}", led.legs);
    }
}
