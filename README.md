# Ground Truth

A drag-and-drop breadboard playground for learning electronics. Build a circuit on a virtual breadboard, ngspice simulates it live, and a Gemini lab partner explains what's going on and suggests what to try next.

The simulator supplies the numbers; Gemini supplies the teaching. Every voltage and current in the app comes from ngspice, and Gemini's replies are labeled as AI output.

![A wrong-row LED circuit flagged as a fault, with the fix shown as a ghost](docs/screenshots/02-fault-wrong-row.png)

**Demo video:** [docs/demo.mp4](docs/demo.mp4) (under 2 minutes: find a fault, fix it, build an LED circuit from scratch, switch a transistor, see the schematic).

## What it does

**Spot the mistake.** The rule checker reads the simulated numbers and flags faults on the board. Here, the resistor ends in row 14 but the LED's anode sits in row 15, so no current flows. A ghost marks where the leg should go, and **apply fix** moves it.

![After apply fix the LED lights at 14.69 mA and the checks pass](docs/screenshots/03-fix-applied.png)

**Watch it work.** Live readouts update on every edit. In the transistor switch, select SW1 and hold Space: a small base current through R2 switches the LED current through Q1.

![Transistor switch with SW1 held: the LED draws 5.51 mA](docs/screenshots/04-transistor-pressed.png)

**See the schematic.** The schematic is generated from the same circuit, stacked by simulated voltage, with selection synced to the breadboard.

![Split view with the breadboard above and the generated schematic below](docs/screenshots/05-split-schematic.png)

**Start from scratch.** Drag a battery, resistor, LED and wires from the parts bin. Hover a part to see its legs, default value and shortcut key.

<img src="docs/screenshots/06-parts-tooltip.png" alt="Parts-bin tooltip for the potentiometer" width="420">

**Ask the lab partner.** "review", "why isn't it working?", "explain" and "ideas" send the circuit, nets, simulation results and rule findings to Gemini. "build one for me" asks Gemini to propose a circuit that loads onto the board, where ngspice simulates it like any other.

## How it works

1. **Editor.** Parts snap onto breadboard holes. Holes a–e and f–j in the same row are connected, plus four power rails, just like a real breadboard.
2. **Simulation.** Every edit rebuilds a SPICE netlist and runs `ngspice` on a background thread. Zero-volt sources act as ammeters on each leg. Circuits with capacitors run a live transient simulation, plotted on the SCOPE panel.
3. **Rule checks.** A checker reads the simulated numbers and reports faults in plain language: an LED that draws no current, an LED plugged in backwards, too much current with no resistor, a shorted battery, a leg in the wrong row. Faults it can fix get an **apply fix** button.
4. **Lab partner.** Gemini receives the circuit, the nets, the ngspice results, the rule findings and the netlist, and replies with structured JSON: a message, issues tied to part ids (hover one to highlight the parts), and ideas to try.

## Running it

You need Rust and [ngspice](https://ngspice.sourceforge.io/) on your PATH (`brew install ngspice` on macOS, `sudo apt install ngspice` on Debian/Ubuntu).

```sh
cp .env.example .env   # then add your GEMINI_API_KEY
cargo run
```

Open **examples** for ready-made circuits (LED, transistor switch, pot dimmer, night light, fading LED, blinker, voltage divider, two LEDs in parallel, and broken ones to debug: wrong row, reversed LED, no resistor, battery short), or pass one on the command line: `cargo run -- circuits/pot_dimmer.json --split`.

The app works without a Gemini key; only the lab partner is disabled. `GEMINI_MODEL` and `NGSPICE_PATH` in `.env` are optional.

## Tests

```sh
cargo test                                         # needs ngspice for the simulation checks
cargo test gemini_live -- --ignored --nocapture    # live Gemini call, needs GEMINI_API_KEY
```

Every example in `circuits/` has an `expect` block that the tests check against the simulation, so the examples double as regression tests.

## Layout

- `src/circuit.rs`: holes, parts, circuits, SI value parsing
- `src/sim/`: SPICE netlist builder, ngspice runner, rule checks
- `src/assistant.rs`: Gemini lab partner
- `src/ui/`: breadboard editor, schematic, theme
- `src/app.rs`: panels, undo/redo, save/open
- `circuits/`: example circuits

## What's next

- Probe a physical breadboard with an Arduino UNO Q and compare the real readings with the simulation.
- More parts, starting with the 555 timer.
- Talk to the lab partner by voice with the Gemini Live API.
