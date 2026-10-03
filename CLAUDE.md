# Ground Truth

An electronics bench assistant: photo of a breadboard → verified circuit model → fault shown on a virtual replica of the board. This is a hackathon project. Ship the demo path first, and polish later.

**Core principle:** the model sees, the simulator decides, the probes confirm. Vision models only label components. They never decide whether a circuit is correct, and they never handle geometry. Any verdict shown to the user must come from ngspice output or probe readings, never from model text.

## Pipeline

1. **Grid mapping (OpenCV):** detect the breadboard and hole grid, then map every component leg to an exact (row, column).
2. **Component ID:** labels parts (resistor value, LED polarity, chip markings). The output is structured JSON with the same schema in both modes:
   - *Online:* Gemini API
   - *Offline:* local YOLO model
3. **Netlist + simulation:** combine the grid map and labels into a SPICE netlist and simulate with ngspice/PySpice. The verdict cites concrete numbers, e.g. "LED1 draws 0 mA because R1 is in row 14, not row 15."
4. **Virtual board:** the desktop app renders a digital twin, highlights the fault, shows simulated currents and voltages, and previews the corrected placement.
5. **Probing:** the Arduino UNO Q's MCU side reads real voltages to confirm the predictions.

## Architecture

- **Desktop app:** Rust (Tauri or egui, TBD). Handles photo upload, the virtual board, and explanations. Talks to the cluster over the LAN.
- **Cluster:** one or many nodes. Each node joins the cluster and advertises its capabilities. A scheduler assigns pipeline stages to whatever is available. When a node drops, its tasks reroute to the others. A single node must always work on its own.
  - Demo hardware: Raspberry Pi 5, Raspberry Pi 4 B, Arduino UNO Q. Real-time probe readings go to the UNO Q's microcontroller.
- **Transport:** HTTP or MQTT (TBD).

## Stack

Rust, Python, OpenCV, Gemini API, YOLO (trained on Roboflow Universe breadboard datasets plus our own photos), ngspice/PySpice.

## Scope (in priority order)

1. **Must have:** photo → grid map → Gemini labels → netlist → ngspice verdict → virtual board, on a single node, for 2–3 demo circuits.
2. **Should have:** multi-node scheduling with failover, offline YOLO mode, UNO Q probe readings.
3. **Stretch:** live camera mode, voice explanations via the Gemini Live API.

Don't start tier 2 or 3 work while tier 1 is broken. When unsure, ask which tier a task belongs to.

## Demo script (keep this working)

1. Build an LED circuit with a deliberate mistake.
2. Scan it. Ground Truth flags the fault with simulated numbers and highlights it on the virtual board.
3. Pull the network cable. Offline mode still works.
4. Unplug a board mid-scan. The work reroutes without breaking.
5. Fix the circuit and rescan. The board turns green.

## Repo layout

<!-- TODO: fill in once the structure exists -->
- `app/`: Rust desktop app
- `node/`: Python cluster node (pipeline stages, scheduler client)
- `vision/`: OpenCV grid mapping, YOLO model and training scripts
- `sim/`: netlist builder and ngspice wrapper
- `firmware/`: UNO Q probe code
- `circuits/`: demo circuit photos and expected netlists

## Commands

<!-- TODO: replace with real commands -->
- Desktop app: `cd app && cargo run`
- Node: `cd node && python -m groundtruth.node`
- Tests: `cargo test` / `pytest`

## Conventions

- Never commit API keys. The Gemini key comes from `GEMINI_API_KEY` in `.env`, which is gitignored.
- Keep the online and offline component-ID outputs on one shared JSON schema. If you change it, update both paths.
- Every pipeline stage must run standalone on a single node, with no scheduler required.
- Keep demo circuits in `circuits/` as regression tests. A change that breaks a demo circuit isn't done.
