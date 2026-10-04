//! Stage 3: grid map + labels -> SPICE netlist.

use super::types::{ComponentLabels, GridMap, Kind, Netlist};
use anyhow::{Result, anyhow, bail};
use std::collections::BTreeMap;
use std::fmt::Write;

/// Generic red LED. Good enough to tell 0 mA from 15 mA from 80 mA.
const LED_MODEL: &str = ".model DLED D(IS=1e-19 N=1.7 RS=2)";

pub fn build(grid: &GridMap, labels: &ComponentLabels) -> Result<Netlist> {
    let label = |id: &str| {
        labels
            .components
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow!("no label for {id}"))
    };

    // The supply's negative net is SPICE ground.
    let supply = grid
        .parts
        .iter()
        .find(|p| label(&p.id).is_ok_and(|l| l.kind == Kind::Supply))
        .ok_or_else(|| anyhow!("no power supply found on the board"))?;
    let supply_pos = label(&supply.id)?.positive_leg.unwrap_or(0) as usize;
    let ground_net = supply.legs[1 - supply_pos.min(1)].net_key();

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

    let mut spice = String::from("* Ground Truth generated netlist\n.option rshunt=1e12\n");
    let mut needs_led_model = false;

    for part in &grid.parts {
        let l = label(&part.id)?;
        if part.legs.len() != 2 {
            nl.notes.push(format!("{} has {} legs; only 2-leg parts are simulated so far", part.id, part.legs.len()));
            continue;
        }
        let pos = l.positive_leg.unwrap_or(0) as usize;
        if pos > 1 {
            bail!("{}: positive_leg {} out of range", part.id, pos);
        }
        let a = node(part.legs[pos].net_key(), &mut nl);
        let b = node(part.legs[1 - pos].net_key(), &mut nl);
        let id = &part.id;

        let element = match l.kind {
            Kind::Supply => {
                let v = l.value.unwrap_or(5.0);
                writeln!(spice, "V_{id} {a} {b} DC {v}")?;
                nl.ammeters.insert(id.clone(), format!("V_{id}"));
                continue;
            }
            Kind::Resistor => {
                let ohms = l.value.ok_or_else(|| anyhow!("{id}: resistor value unreadable"))?;
                format!("R_{id} {{mid}} {b} {ohms}")
            }
            Kind::Wire => format!("R_{id} {{mid}} {b} 1m"),
            Kind::Led => {
                needs_led_model = true;
                format!("D_{id} {{mid}} {b} DLED")
            }
            Kind::Capacitor => format!("C_{id} {{mid}} {b} {}", l.value.unwrap_or(1e-6)),
            Kind::Button => {
                nl.notes.push(format!("{id}: button treated as open (not pressed)"));
                continue;
            }
            Kind::Ic | Kind::Unknown => {
                nl.notes.push(format!("{id}: {:?} not simulated yet", l.kind));
                continue;
            }
        };

        // 0 V source in series acts as an ammeter.
        let mid = format!("{}_a", id.to_lowercase());
        let am = format!("V_am_{id}");
        writeln!(spice, "{am} {a} {mid} 0")?;
        writeln!(spice, "{}", element.replace("{mid}", &mid))?;
        nl.ammeters.insert(id.clone(), am);
    }

    if needs_led_model {
        writeln!(spice, "{LED_MODEL}")?;
    }
    spice.push_str(".control\nop\nprint all\n.endc\n.end\n");
    nl.spice = spice;
    Ok(nl)
}

/// Which board holes share a net, for parts-per-net checks.
pub fn legs_by_net(grid: &GridMap) -> BTreeMap<String, Vec<(String, usize)>> {
    let mut map: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for part in &grid.parts {
        for (i, hole) in part.legs.iter().enumerate() {
            map.entry(hole.net_key()).or_default().push((part.id.clone(), i));
        }
    }
    map
}
