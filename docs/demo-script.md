# Live demo script

Click-by-click script for the Ground Truth live demo (about 3 minutes). It follows the five demo steps in `CLAUDE.md`.

Every number below was read off the running app on branch `demo-with-bugbash` (ngspice-42, 5 V battery). If you see a different number on stage, trust the screen: the numbers come from ngspice, not from this file and not from Gemini.

Steps marked **GEMINI** need a working `GEMINI_API_KEY` in `.env`. Each has a fallback line in case Gemini is slow or offline.

## Before you go on stage

1. Open the terminal in the repo folder and start the app:

   ```
   cargo run
   ```

2. Check the top-right corner of the window:
   - `sim ngspice-42` (any version is fine). If it says ngspice is missing, set `NGSPICE_PATH` in `.env`.
   - `ai gemini-3.8-flash`. If it says `no GEMINI_API_KEY` in orange, the key is not loaded: fix `.env` and restart, or plan to use the fallback lines.
3. Click **examples → LED, wrong row**, then **why isn't it working?** once, so Gemini is warmed up and you know the key works. Then click **new** to clear the board.
4. Make the window full screen. Close Slack, notifications and other windows.

## Step 1: a broken circuit (about 30 s)

| Click | What the audience sees |
|---|---|
| **examples** (top bar) → **LED, wrong row** | The board outline turns orange. CHECKS shows `FAULT 1 fault 1 warning`. |
| Point at the CHECKS panel | `FAULT LED1 draws 0.00 mA because R1 is in row 14, not row 15.` |
| Point at the board | The battery readout says `0.00mA`. A green ghost LED shows where it should go. |

**Say:** "Here's a classic beginner mistake. Everything looks plugged in, but the LED is dark. Ground Truth runs the circuit through ngspice, a real circuit simulator, and it says zero milliamps. The checker knows why: the resistor ends in row 14 and the LED starts in row 15. Breadboard rows don't connect across."

## Step 2: ask the lab partner (about 30 s) — GEMINI

| Click | What the audience sees |
|---|---|
| **why isn't it working?** (right panel, LAB PARTNER) | After a few seconds Gemini answers about the row gap, with issues listed under the answer. |
| Hover over an issue in the answer | R1 and LED1 light up on the board. |

**Say:** "The simulator supplies the numbers. Gemini supplies the teaching. It gets the simulated voltages and the rule checks as context, so it explains the real problem instead of guessing."

**Fallback if Gemini is slow (more than about 8 s) or errors:** "The lab partner is thinking, but notice we don't need it to know what's wrong. The checker already told us, straight from the simulation." Point at the FAULT line in CHECKS and go on to step 3. Come back to the Gemini answer if it arrives.

Never read a number out of Gemini's answer as if it were measured. Only read numbers from the board, the inspector and CHECKS.

## Step 3: apply the fix (about 15 s)

| Click | What the audience sees |
|---|---|
| **apply fix** (green button on the FAULT line in CHECKS) | The LED moves one row and lights up red. The board outline turns green. |
| Point at CHECKS | `PASS` and `OK LED1 draws 14.7 mA at 1.77 V forward.` |
| Point at the board | The battery, R1 and LED1 readouts all say `14.69mA`. |

**Say:** "One click, the LED moves into the right row, and the simulator says 14.7 milliamps at 1.77 volts across the LED. Green board, passing checks."

## Step 4: build one from scratch (about 60 s)

Click **new** first. The board shows "drag a battery from the parts bin to start".

Holes are named by column letter and row number, like `c5`. The rails are the two lines at the top: red `+` and blue `−`.

| Do this | Result on screen |
|---|---|
| Drag **battery** from the parts bin onto the top-left of the board | B1 snaps onto the rails (`L+1` and `L−1`), 5 V. |
| Drag from the red `+` rail hole above row 5 down to hole `a5` | A red jumper wire W1. |
| Drag **resistor** onto hole `c5` | R1, 220 Ω, from `c5` to `c9`. CHECKS shows a WARN: leg 2 at c9 isn't connected. |
| Drag **led** onto hole `d9` | LED1 from `d9` to `d10`. CHECKS shows a FAULT because nothing connects row 10 yet. **Don't click apply fix here.** |
| Drag from hole `a10` up to the blue `−` rail | A black wire W2. The LED lights, the board turns green, `PASS LED1 draws 14.7 mA at 1.77 V forward.` |
| Click R1, then in the INSPECTOR on the left open **common** → **1 kΩ** | The LED dims. Readouts change to `3.32mA`. CHECKS: `LED1 draws 3.3 mA at 1.68 V forward.` |

**Say (while dragging):** "Drag parts in, drag from a hole to draw a wire. Every edit re-runs the simulation, so the checks follow along: right now the LED has no path back to the battery." After the ground wire: "And it lights." After 1 kΩ: "Bigger resistor, less current, dimmer LED, and the numbers update live."

**If a drag lands in the wrong hole:** press **Ctrl+Z** (Cmd+Z on Mac) and drag again. Don't explain it, just redo it.

## Step 5: transistor switch and schematic (about 40 s)

| Click | What the audience sees |
|---|---|
| **examples** → **Transistor switch** | Green board, LED1 (blue) is dark. CHECKS: `PASS` and `INFO LED1 is off while SW1 is open.` |
| Click the push button **SW1** (right side of the board, rows 18 to 22) | The INSPECTOR shows SW1 Push button. |
| Hold **Space** | The LED lights. The inspector says `held (space)`. CHECKS: `OK LED1 draws 5.5 mA at 3.09 V forward.` Base current through R2 and SW1: `0.43mA`. |
| Let go of Space | The LED goes dark again. |
| Tick **latched** in the INSPECTOR | The button stays pressed, so you can talk with your hands free. |
| Click **schematic** (top bar) | The same circuit as a schematic: SW1 0.43 mA, R1 330 Ω 5.51 mA, R2 10 kΩ 0.43 mA, LED1 5.51 mA, B1 5.94 mA, node voltages 5.00 V, 3.18 V, 0.72 V, 0.09 V, GND 0.00 V. |

**Say:** "A tiny current into the base, 0.43 milliamps, switches over 5 milliamps through the LED. That's what a transistor does. And it's the same circuit as a schematic, generated from the breadboard, with every voltage straight from the simulator."

**Fallback if Space does nothing:** the board probably lost keyboard focus. Click SW1 again and hold Space. Or skip Space and tick **latched**: the numbers are the same.

Use **schematic** rather than **split** for this step: in split view the bottom of the schematic (Q1 and ground) is cut off on a laptop screen.

## Closing line

"Ground Truth: a breadboard that tells you the truth, with a lab partner that explains it."

## Known quirks

- Without a key, the Gemini buttons show `Add GEMINI_API_KEY to .env to enable the lab partner.` in orange. Nothing else breaks.
- In step 4, the FAULT that appears after dropping the LED suggests moving R1. Ignore it and add the ground wire.
