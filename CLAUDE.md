# Ground Truth

A drag-and-drop breadboard playground for learning electronics. Users build circuits on a virtual breadboard, ngspice simulates them live, and a Gemini "lab partner" reviews the circuit, explains errors and suggests experiments. Friendly enough for a first LED, with enough depth (schematic view, SPICE netlist, transistors, pots) for experienced builders. This is a hackathon project. Ship the demo path first, and polish later.

**Core principle:** the simulator supplies the numbers; Gemini supplies the teaching. Every voltage and current shown in the UI comes from ngspice. The rule checker (`src/sim/checks.rs`) reports faults from those numbers. Gemini is free to review, critique and suggest, but it gets the simulated numbers as context and is labeled as AI output in the UI. No image processing: the photo/OpenCV/YOLO pipeline was removed on 2026-10-03.

## How it works

1. **Editor (`src/ui/board.rs`):** parts are dragged from the parts bin onto holes. Parts and single legs can be dragged; dragging from an empty hole draws a wire. Breadboard connectivity comes from `Hole::net_key` (a–e and f–j per row, plus four rails).
2. **Simulation (`src/sim/`):** every edit rebuilds a SPICE netlist (`netlist.rs`), runs `ngspice -b` on a worker thread (`ngspice.rs`), and runs the rule checks (`checks.rs`). 0 V sources in series act as ammeters on legs.
3. **Schematic (`src/ui/schematic.rs`):** generated from the same circuit. Wires merge nets, nets are stacked by simulated voltage, and selection syncs with the breadboard.
4. **Lab partner (`src/assistant.rs`):** sends the circuit, nets, sim results, rule findings and netlist to Gemini, and gets structured JSON back (`message`, `issues` tied to part ids, `ideas`). Hovering an issue highlights its parts.

## Architecture

- **Desktop app:** Rust + egui/eframe, a single binary. Keep the amber-on-dark "bench instrument" look (`src/ui/theme.rs`).
- **Circuit files:** JSON (`src/circuit.rs` `Circuit`). Polar parts list leg 0 first (battery +, LED anode, NPN collector, pot end A).
- **Hardware (later, tier 2):** Arduino UNO Q probing to compare a physical build against the simulation. Not started.

## Scope (in priority order)

1. **Must have:** drag-and-drop breadboard editor, live ngspice readouts and rule checks, Gemini review/chat, schematic view, example circuits, save/open.
2. **Should have:** UNO Q probe readings vs. simulation, more parts (photoresistor, switch, 555), transient sim (blinking).
3. **Stretch:** Gemini proposes circuits that load onto the board, voice via the Gemini Live API.

Don't start tier 2 or 3 work while tier 1 is broken. When unsure, ask which tier a task belongs to.

## Demo script (keep this working)

1. Open Examples → "LED, wrong row". The board outline is red, CHECKS shows the fault, and a green ghost shows the fix.
2. Click "Why isn't it working?" Gemini explains the row gap and highlights R1/LED1.
3. Click "Apply fix". The LED lights and the board turns green.
4. Start from New: drag a battery, resistor, LED and wires. Readouts update live.
5. Open "Transistor switch", select SW1, hold Space. The LED lights. Switch to SCHEMATIC.

## Repo layout

- `src/circuit.rs`: holes, parts, circuits, SI value parsing
- `src/sim/`: netlist builder, ngspice runner, rule checks
- `src/assistant.rs`: Gemini lab partner
- `src/ui/`: breadboard editor, schematic, theme
- `src/app.rs`: panels, undo/redo, save/open, background sim and Gemini threads
- `circuits/`: example circuits (compiled in), each with an `expect` block checked by tests

## Commands

- App: `cargo run` (dev flags: `cargo run -- circuits/pot_dimmer.json --split`)
- Tests: `cargo test` (needs `ngspice` on PATH for the simulation checks)
- Live Gemini check: `cargo test gemini_live -- --ignored --nocapture`

## Conventions

- Never commit API keys. The Gemini key comes from `GEMINI_API_KEY` in `.env`, which is gitignored. `GEMINI_MODEL` and `NGSPICE_PATH` are optional.
- Every number in the UI comes from `SimResult`. Never display a value that Gemini produced as if it were measured or simulated.
- Keep example circuits in `circuits/` as regression tests. A change that breaks an example's `expect` isn't done.
