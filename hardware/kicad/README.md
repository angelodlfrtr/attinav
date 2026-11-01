# Attinav hardware — design notes

Design review, error budget and Rev B architecture for the Attinav AHRS module.

**Application:** precise AHRS for cave survey (speleology). Static "point and shoot"
station measurement — azimuth + inclination — not dynamic attitude tracking.
This distinction drives almost every decision below.

**Status:** Rev A reviewed 2026-09-20. Three functional bugs found (see
[Rev A defects](#rev-a-defects)). Rev B architecture proposed but not implemented.

---

## Table of contents

- [Rev A defects](#rev-a-defects)
- [Tooling caveats](#tooling-caveats)
- [Requirements and error budget](#requirements-and-error-budget)
- [Sensor selection rationale](#sensor-selection-rationale)
- [Rev B architecture](#rev-b-architecture)
- [Magnetic hygiene rules](#magnetic-hygiene-rules)
- [System integration](#system-integration)
- [Calibration](#calibration)
- [References](#references)

---

## Rev A defects

Verified against a kicad-cli generated netlist and PCB geometry, not from audit tools.

### Critical — functional bugs

**1. D1/D2 Schottky pair has a floating common node**

`Net-(D1-K)` connects only `D1.K` and `D2.K` — nothing else. D1 is A=GND/K=node,
D2 is A=+3.3V_EXT/K=node. As drawn the pair conducts nothing and protects nothing.

Placement (D1 85.9,138.4; D2 83.3,138.4; near Q1 80.0,134.4 and R4) suggests it was
intended as a rail clamp on `/EN`, which is otherwise a bare MOSFET gate exposed on
J3.12 and J4.2. Note D2's orientation (anode on +3.3V_EXT) is a feed diode, not a
clamp — a positive clamp needs anode on the signal, cathode on the rail.

**2. BOOT0 boot-select jumper is defeated**

NT1 (`NetTie-2_SMD_Pad0.5mm`) hard-shorts `Net-(U1-BOOT0)` to GND with 0 Ω copper.
J1 (pin 2 = +3.3V) feeds BOOT0 through R1 4.7k. Shorting J1 puts 4.7k across a dead
short; BOOT0 stays at ~0 V and never reaches VIH. DFU/system-bootloader entry is
impossible.

*Fix:* replace NT1 with a 10k pull-down. Then check the divider — 4.7k/10k gives only
~2.24 V, so drop R1 to ~1k for margin.

**3. Crystal load caps wrong**

Y1 = YXC **X50328MSB2GI**, 8 MHz, **CL = 20 pF** (confirmed via JLCPCB DB, LCSC
C115962). Required load caps:

```
C = 2 x (CL - Cstray) = 2 x (20 - 4) ~= 33 pF
```

Fitted: **20 pF** (C15/C16) → effective CL ≈ 14 pF. An under-loaded crystal runs
fast, roughly **+60 ppm**, far outside the crystal's own ±10/±20 ppm spec.

*Fix:* C15/C16 → 33 pF.

### Warnings

**4. STM32 (U1) has no local decoupling.** Measured pad-to-pad on the PCB:

| Pin | Nearest same-net cap |
|---|---|
| U1.24 VDD | C11 @ **16.7 mm** |
| U1.36 VDD | C11 @ **10.6 mm** |
| U1.48 VDD | C9 @ **11.9 mm** |
| U1.9 VDDA/VREF+ | C13 @ **11.2 mm** |
| U1.22 VCAP1 | C2 @ 2.26 mm — OK |

C9/C12/C13/C14 sit at x≈78 mm on the left board edge; U1 is at x=93.3 mm — opposite
edges of a 30.5 mm-wide board. Partly mitigated by the stackup (In2 = +3.3V plane,
In3 = GND at 0.1 mm → good plane capacitance), but 100 nF per VDD pin within ~3 mm
is standard practice.

By contrast **U2 (SCHA63T) is done correctly** — all 8 supply pins have caps at
~3.2 mm. The oversight is on U1/U3/U5 only.

VDDA/VREF+ (pin 9) also has no dedicated filtering (no ferrite, no 1 µF + 10 nF).
PB0 is an ADC input reading the LM94021, so this matters.

**5. U3 (RM3100) has no decoupling and no plane.** Nearest +3.3V cap is **24 mm**.
U3 sits at (91.4, 90.4), inside a copper-pour keepout spanning Y 81.28–102.362 on
**all six layers** — deliberate for magnetic cleanliness, but see
[Magnetic hygiene rules](#magnetic-hygiene-rules): this is probably backwards.
Its supply is thin traces only (+3.3V 12.3 mm, GND 9.1 mm, both single-layer F.Cu)
with no local charge reservoir, and the RM3100 is magneto-inductive — it actively
pulses current through its coils.

**6. U5 (TPL5010) 17.3 mm, U4 (LM94021) 5.8 mm** from nearest cap. Both datasheets
call for a local 100 nF.

**7. U1 VBAT (pin 1) floating**, flagged no-connect. ST guidance for STM32F4 is to
tie VBAT to VDD with a 100 nF when no backup battery is used.

**8. No bulk capacitance or protection on +3.3V_EXT.** It arrives on J2.2/J3.3 and
feeds only Q1.S, R4, D2.A — no capacitor at all. C14 is downstream of the PMOS, so
Q1 turn-on dumps inrush into ~13 µF with no gate RC for soft-start.

**9. `/EN` is a bare MOSFET gate on two connectors** (J3.12, J4.2) with only a 10k
pull-up — no series resistor, no ESD clamp (and see defect 1).

### Minor

- **C14 value string wrong**: reads `10uF/50V/20%/XYR/0603`; LCSC C96446 is actually
  Samsung CL10A106MA8NRNC = **10 µF 25 V X5R 0603**. Ordered part is correct and fine
  for 3.3 V; the text has a wrong voltage and a typo ("XYR"). Check C9/C12/C13
  ("50V" → C14663) the same way.
- **U3 has no LCSC/MPN**, and its Datasheet property points to a witmotion.com store
  page for a JY-RM3100 *module*, not the PNI RM3100 datasheet. Also behind the one
  real DRC warning: *"Footprint 'WIT-RM3100' does not match copy in library
  'WIT-rm3100'"*. Resolve before fab.
- **U2 uses property name `MP` not `MPN`** — breaks BOM tooling.
- **Mounting holes inconsistent**: H3 pad tied to GND; H1/H2/H4 explicitly no-connect
  (floating copper).
- **D4 power LED draws ~0.75 mA continuously** on a board built around a 35 nA TPL5010.
- Global `min_clearance`/`min_track_width` are 0.0 in the project, though the Default
  netclass sets 0.2 mm / 0.25 mm so DRC is still meaningful.

### Rev A verified good

- 0 ERC errors, 0 DRC errors, fully routed. No shorted nets. All 12 single-node nets
  are explicit no-connects.
- Geometry fab-friendly: 0.25/0.7 mm tracks, 0.3/0.5 mm via drills, 0.25 mm min pad
  (QFN48 0.5 mm pitch), ~1.6 mm 6-layer stackup.
- Q1 PMOS correctly oriented as high-side switch (S=input, D=load, gate pulled to
  source → default off).
- NRST: 100 nF (C1) + button + internal pull-up per ST guidance; TPL5010 reset gated
  by open solder jumper JP1 (intentional).
- Stackup well considered: In2 (+3.3V) / In3 (GND) 0.1 mm apart for plane capacitance.

---

## Tooling caveats

Konnect MCP audit results on this project are unreliable. Do not trust them without
verification:

- **`run_design_review` verdict "NOT READY"** rests entirely on 7 false decoupling
  errors. It reads **0 pads** from the board and finds only 7 of ~20 power pins.
- **`audit_decoupling`** reports "0/7 power pins have decoupling" while the netlist
  shows 5× 100 nF + 2× 1 µF + 10 µF on +3.3V. It needs board pads for its proximity
  check and silently degrades to "no cap found".
- **`find_orphan_items`: 261 "dangling wire ends"** — all false. Verified
  (81.28, 45.72) is U1 pin 7 (NRST) with a wire correctly landing on it. The tool
  does not resolve pin positions.
- **`get_component_nets` / `trace_from_point` return `net: null`** for pins whose net
  comes from a power symbol rather than a label. They resolve nets from labels only.
- **155 ERC + 45 DRC warnings** were all "library not found / not enabled". Root cause
  (found 2026-11-01): the Konnect MCP server entry in `~/.claude.json` hardcodes
  `KICAD10_SYMBOL_DIR` / `KICAD10_FOOTPRINT_DIR` as Nix store paths, which go stale at
  every rebuild that bumps `kicad-symbols` / `kicad-footprints`. Konnect also does not
  follow KiCad 10's nested `(type "Table")` entry in the user's global lib tables, so
  it had no fallback. Fixed by re-adding the server with the current paths (plus
  `KICAD10_TEMPLATE_DIR`); the durable fix is to generate that entry from the Nix
  package outputs. Symptom when it recurs: every stock `lib_id` fails with "no library
  'Device' in the project or global sym-lib-table". Symbols still resolve from the
  embedded cache, so 0-error results on existing sheets remain genuine.

**Reliable method:** generate a real netlist with `kicad-cli` and parse it, and parse
`.kicad_pcb` directly for geometry. Note KiCad 10 writes pad nets as `(net "NAME")`
with no net number — index `[1]`, not `[2]`.

### Konnect schematic-building notes (v0.13.0, learned on the mag sheet)

- **Subagents bind to the Konnect server process alive at spawn.** After an MCP
  reconnect, an already-spawned agent keeps talking to the old process (stale env, old
  toolsets). Respawn agents after any reconnect; never let two agents write one sheet.
- `register_*_library` with global scope **writes into `~/.config/kicad/10.0/*-lib-table`**
  and there is no unregister tool. Do not let agents probe library paths this way.
- `annotate_schematic` must run on the **file that owns the symbols** (child sheet); on
  the root it no-ops. Place with `?` references — explicit numbers are kept and defeat
  per-instance numbering on multi-instanced sheets.
- `move_schematic_component` refuses symbols once they carry multi-instance paths; use
  `move_region`. `edit_sheet` resize does not move sheet pins — re-place them with
  `edit_sheet_pin`, then `validate_sheet_pins`.
- Sheet boxes and pins must land on the 1.27 mm grid or every pin raises
  `endpoint_off_grid`; `batch_delete` by UUID is the only way to remove free text.
- **`kicad-cli sch erc` does not report duplicate reference designators across sheets.**
  A sheet annotated in isolation (MCU sheet, 2026-11-01: U1, R1, C1… colliding with the mag
  and power sheets) passed ERC clean while the netlist merged pins of different parts.
  After every annotation, parse the netlist `(components …)` block and assert every `ref`
  is unique; `annotate_schematic` must be run with the project context so other sheets'
  numbers are reserved.
- `render_schematic_png` without `output` drops a PNG next to the schematic; always pass
  a scratch path. A wire routed straight through a foreign pin silently creates a
  junction (`find_shorted_nets` cannot see unnamed nets) — check coil nodes with
  `trace_from_point` and the `kicad-cli` netlist.

---

## Requirements and error budget

**Target:** 0.1° azimuth, 0.05° inclination. Roughly 2–5× better than a DistoX2.

### The binding constraint

For a tilt-compensated compass, **tilt error is amplified into azimuth error by
tan(magnetic dip)**. At European dip (~63°), tan = **1.96**.

> **The accelerometer must be ~2× better than the azimuth target.**
> For 0.1° azimuth you need ~0.035° of tilt = **0.6 mg**.

This makes the accelerometer, not the magnetometer, the hard problem.

### Field error to heading error

```
heading error (rad) ~= dB / H        H ~= 20 uT horizontal (mid-latitude Europe)

0.1 deg  =>  dB < 35 nT
0.5 deg  =>  dB < 175 nT
```

### Budget

| Term | Contribution |
|---|---|
| Mag noise (3× RM3100, averaged ~5 nT) | 0.014° |
| Mag calibration residual (~20 nT) | 0.057° |
| Tilt error 0.035° × tan 63° | 0.069° |
| Mag↔accel frame misalignment residual | 0.030° |
| Sight-axis alignment | 0.030° |
| **RSS azimuth** | **≈ 0.10°** |
| SCA3300 Mode 4 noise (0.15 mg) | 0.009° |
| Accel offset after temp cal (~1 mg) | 0.057° |
| **RSS inclination** | **≈ 0.05°** |

Nothing here works on factory specs. Per-unit calibration is mandatory.

---

## Sensor selection rationale

### Why the gyro does not buy azimuth

A gyro only replaces the compass if it can **gyrocompass** — resolve Earth's rotation
(15.04°/hr). At 45° latitude the horizontal Earth rate is ~10.6°/hr, so 0.5° of
azimuth needs gyro bias stability around **0.1°/hr**. MEMS parts, SCHA63T included,
are one to two orders of magnitude away.

What a gyro *does* buy:
- Motion/stillness gating during a shot
- **Short attitude transfer** through a magnetically disturbed station. At ~0.28°/√hr
  ARW, carrying azimuth for 1 minute costs ~0.036° — genuinely useful, and a cheap
  part does it. **Do not pay €200 for SCHA63T to do this.**

### Why 3× sensors — and why not for precision

Averaging N sensors cuts *uncorrelated random* error by √N. Three gives 1.73×.
But the design is **not noise-limited**: the RM3100 detects 2.7 nT at 1 Hz, which at
20 µT horizontal is under 0.05° of azimuth noise — already 10× below target.

And **time-averaging is free**: the measurement is static, so 100 samples at 100 Hz
buys √100 = 10× with zero hardware, versus √3 = 1.7× for three chips. Or raise the
RM3100 cycle count.

**The real reasons for redundancy** (and what BRIC5 does — 2× SCA3300-D01 plus
redundant magnetometers, explicitly *"to detect calibration drift, malfunctions, and
magnetic gradients"*):

- **Magnetic gradiometry — of *nearby* iron.** A uniform Earth field reads identically at
  all sensors. Nearby iron — magnetite in the rock beside you, a bolt, a steel carabiner —
  creates a *gradient*. Spatially separated magnetometers that disagree after calibration
  flag the station as disturbed. DistoX users guess at this; measuring it is a real
  capability upgrade.
  **Blind spot:** a large *distant* mass — a steel gate 3 m away, an iron-bearing stratum —
  produces a nearly uniform offset with negligible gradient, and gradiometry cannot see it.
  That case is caught only by the **|B|-vs-IGRF and dip-vs-IGRF** checks. Gradiometry is
  one of three anomaly detectors, not *the* detector.
- **Blunder detection.** You cannot redo a trip.
- **2 detects disagreement; 3 lets you vote** and identify which sensor is wrong.

### Why 3× cheap sensors will NOT work

Averaging fixes *random* error. Cheap sensors fail on *systematic* error:

- Cheap magnetometers (AMR: QMC/HMC5883, LIS3MDL; Hall: BMM150/BMM350) suffer
  hysteresis, nonlinearity and temperature-dependent offset. Three identical parts on
  one board at one temperature drift **the same way** — correlated, so √N does nothing.
- AMR parts need set/reset degauss pulses; miss them and one strong field leaves a
  permanent bias.
- The RM3100's advantage was never noise — it's **linearity, no hysteresis, and gain
  stability**. Averaging cannot recover those.

**3× cheap ≠ 1× good.**

### Why not the DRV425 fluxgate

Verified against the DRV425 datasheet (Rev B, Oct 2025):

| Parameter | Value |
|---|---|
| Noise, **0.1–10 Hz** (compass band) | **17 nT RMS** |
| Noise density @ 1 kHz | 1.5 nT/√Hz |
| Offset | ±2 µT typ, ±8 µT max |
| **Offset drift** | **±5 nT/°C** |
| Gain drift | ±7 ppm/°C |
| Gain error | ±0.04% |
| Linearity error | 0.1% |
| Hysteresis (after ±10 mT sweep) | 1.4 µT |
| Quiescent current | 6 mA typ, 8 mA max @ 3.3 V |
| Compensation loop active after power-on | 70 µs |

The headline 1.5 nT/√Hz is **at 1 kHz** and irrelevant to a compass. The
compass-band figure of 17 nT RMS essentially **ties** the RM3100.

**Power is not the obstacle** — the 70 µs startup makes duty-cycling trivial
(~120 µA average for 3 axes at one shot/minute).

**Offset drift is the obstacle.** ±5 nT/°C gives 0.14° per 10 °C, or 0.27° across a
surface-to-cave transient (25 °C → 6 °C). It is a *magnetic* offset, so no
improvement in electronics touches it. And what closed-loop buys — superb gain
stability — is not what limits a compass, since heading comes from `atan2` of ratios
where common gain error largely cancels.

To beat the RM3100 you would need anti-parallel sensor pairs (6 parts, back-to-back
across the PCB so the sensitive axis reverses), a high-value shunt (~440 Ω instead of
22 Ω, trading the unused ±2 mT range for 20× lower amplifier drift contribution), a
ratiometric multiplexed 24-bit ADC chain, a temperature-stable perpendicular Z mount,
and sequenced duty-cycled power. Not worth it.

**Where the DRV425 would win:** as a dedicated gradiometer, because ±0.04% gain error
and ±7 ppm/°C gain drift mean two parts subtract very cleanly, and a gradiometer only
cares about the difference *changing*. Not needed here — 3× RM3100 already gives
gradiometry.

### RM3100 form factor — important

The RM3100 is a **discrete chipset, not a single package**:

| Component | Size | Qty per suite |
|---|---|---|
| SEN-XY | 6 × 2.1 × 2.21 mm | 2 |
| SEN-Z | 3.88 × 2.98 × **6.35 mm tall** | 1 |
| MagI2C ASIC | 4 × 4 × 0.75 mm, 28-pin MLF | 1 |

- MagI2C must sit **within 10 cm** of the coils.
- **You place the coils, so orthogonality is your responsibility.** PNI specifies a
  5 mil stencil with paste area much smaller than the pad, specifically to limit
  SEN-Z tilt — tilt maps directly to axis misalignment error.
- **SEN-Z is a vertical solenoid that senses the board-normal axis while being
  reflow-soldered flat.** No perpendicular daughtercard. This is the decisive
  advantage over any single-axis in-plane sensor such as the DRV425.
- No successor exists — there is no RM3200. The only variant is **RM3100-CB**, a
  board-mounted vibration-hardened version (50 g RMS, MIL-STD-810H 514.8).

---

## Rev B architecture

| Role | Part | Qty | Rationale |
|---|---|---|---|
| Magnetometer | **PNI RM3100 sensor suite** | **3 sets** | No better option exists. 3 sets = spatial gradiometer + voting |
| Accelerometer | **Murata SCA3300-D01**, Mode 4 | **2** | 0.15 mg RMS, damped element, self-diagnostics. `\|a\|=1g` is a built-in third vote |
| Gyro (optional) | **TDK ICM-42688-P** | 1 | Short attitude transfer + motion gating only |
| Temp at mag coils | **Pt1000 RTD, non-magnetic package** | 3 | The platinum *element* is non-magnetic; **SMD thin-film RTDs often have Ni-barrier terminations** — specify verified non-magnetic terminations or a leaded element. Compensates **gain** tempco only: the MagI2C has no temperature sensor, and PNI states offset is drift-free |
| Temp at accel | **SCA3300 internal die sensor** (TMP117 ×0–1 as board reference only) | — | 18.9 LSB/°C → **0.053 °C resolution**, read in the same SPI frame. Its ±10 °C *absolute* offset is irrelevant — compensation uses a per-unit curve against the reported value. Die temperature is the **correct** variable; an external sensor lags and misses package gradients |
| MCU | **STM32U575RGT6** (LQFP-64, non-Q: LDO only, VCAP; LCSC C5270980). **STM32U585RIT6 is pin-identical** (adds crypto accelerators and 1 MB flash) and stays the drop-in upgrade | 1 | see below — the section is written for the U585; everything applies to the U575 except the crypto peripherals |
| Regulator | **TPS7A2033PDBVR** (300 mA, 7 µVrms, high PSRR; LCSC C2862740, genuine TI only), **EN pin = the FFC `EN` line, high = on**, 100 k pull-down, bench jumper to `+5V` | 1 | **Never a buck** — switching current near a magnetometer is a field source. Replaces Rev A's PMOS gate. Analog rail `+3.3VA` by an **RC split (4.7 Ω + 22 µF), not a ferrite bead** (no magnetic material); the mag AVDD switches stay on `+3V3`. Inputs: `5V_FFC` and USB `VBUS` **Schottky OR-ed** (PMEG2010AEH). One regulator only — the MagI2C's 0.1 V AVDD/DVDD tracking rule forbids separate regulators |
| Protection | **USBLC6-2SC6** on the USB data pair (the port is a JST SH pigtail header, not a steel-shelled USB-C receptacle) + **TVS on both 5 V inputs** (SMF5V0A on `5V_FFC` and on `VBUS`) | 1 + 2 | Both at the rear, both silicon — no magnetic concern. Previously missing |
| Decoupling at coils | **Non-magnetic MLCC** (Cu/Ag termination) | — | Standard MLCCs have ferromagnetic Ni barrier terminations |

**Dropped:** SCHA63T (€200 for gyro performance that contributes nothing to static
precision, and its accelerometer is superseded by the SCA3300). DRV425 (see above).

### Accelerometer: SCA3300 over ADXL355

Verified from the SCA3300-D01 datasheet Table 2 (Doc. 3165 Rev 3):

| | **SCA3300-D01 (Mode 4)** | ADXL355 |
|---|---|---|
| Noise | 0.15 mg RMS ≈ **0.0086°** | 25 µg/√Hz ≈ 0.0045° |
| Offset temp dependency, −40…+125 °C | **±10 mg (±0.57°) X/Y**, ±15 mg Z — bounded total | ±0.15 mg/°C ≈ 0.008°/°C |
| Total offset incl. lifetime drift | ±20 mg (±1.15°) | 10-yr repeatability ±3.5 mg (0.2°) X/Y |
| Sensitivity error / tempco | ±0.7% / ±0.3% | — |
| Linearity | ±1 mg over ±1 g | — |
| Cross-axis | ±1% | ~±1% |
| Mechanical damping | **Yes** | No |
| Self-diagnostics | **Extensive** | Minimal |
| Range | **±1.5 g** (Mode 4) — right-sized for tilt | ±2/4/8 g |
| Bandwidth / settling (Mode 4) | 10 Hz / 100 ms | — |

Reasons SCA3300 wins here:

1. **Mechanically damped element.** Caving means knocks; an undamped MEMS rings after
   a jolt. Mode 4's 10 Hz / 100 ms settling matches the "stop moving, take the shot"
   workflow.
2. **±1.5 g range** — you only ever measure ±1 g, so full scale is on the useful signal.
3. **Bounded offset-drift spec** over the whole range rather than a per-°C slope.
4. **The ADXL355 is sensitive to PCB stress and thermal gradients** — one reported
   measurement showed 3.4–4.2 mg/°C on Z with a gradient across the package, over 20×
   the datasheet figure. A handheld going from a warm pack into a 6 °C cave is exactly
   that scenario.
5. It is what BRIC4/BRIC5 ship, in this exact application.

The ADXL355's lower noise density buys nothing, because the design is not noise-limited.

**Orientation rule:** the SCA3300's offset temperature dependency is ±10 mg on X/Y but
**±15 mg on Z**. Inclination of the sight axis is asin(a_sight / |a|), dominated by the
error on the axis *along* the sight line. So mount each SCA3300 with **the sight axis on
X or Y, never Z** — a free 1.5× on the budget's largest term.

**Refinement — rotate the pair.** That rule is right for normal legs, but on a steep pitch
shot the *perpendicular* axes dominate the inclination error, and one of them must be Z.
Mount the two SCA3300s **rotated 90° about the sight axis** relative to each other: every
shot geometry then has at least one sensor with good axes in the critical direction, and
any common-mode bias becomes a visible disagreement instead of a shared error. Costs
nothing; calibration absorbs the frames.

**Temperature comes from the die.** The SCA3300's internal sensor (18.9 LSB/°C,
0.053 °C resolution) is read in the same SPI frame and measures what the MEMS actually
experiences. Use it for compensation; an external TMP117 is redundant for this purpose.

### MCU: STM32U585

> **Fitted part (decided 2026-11-01): STM32U575RGT6**, LCSC C5270980 — the same die family,
> package and non-Q (LDO-only) pinout, minus the crypto accelerators and with 1 MB flash
> instead of 2 MB. Chosen because the U585RIT6 is out of stock at LCSC and would need
> consignment. Everything below applies to the U575 except the cryptographic peripherals;
> the U585RIT6 remains a zero-change upgrade.

The F401 is functionally adequate but wrong for this instrument.

| | STM32F401CE (Rev A) | **STM32U585 (Rev B)** |
|---|---|---|
| Core | Cortex-M4F @ 84 MHz | Cortex-M33 @ 160 MHz, DSP, FPU, TrustZone |
| Flash | 512 KB | **2 MB, dual-bank** |
| SRAM | 96 KB | **786 KB** |
| Power class | performance line | **ultra-low-power, <30 µA/MHz**. **LDO mode, no SMPS inductor on the board** — the "Q" part's SMPS is a switching regulator, which the no-buck rule forbids, and its run-mode saving is moot because the core sleeps (LPBAM) during integration. Prefer a non-Q part; if only a Q variant is in stock, run it LDO-only per AN5373 (VDDSMPS/VLXSMPS/VSSSMPS → VSS, 2.2 µF + 100 nF on VDD11, no inductor, `REGSEL = 0`). The rule is about the inductor, not the silicon |
| Autonomous DMA | — | **LPBAM** |

- **786 KB SRAM** makes the full 56-shot nonlinear least-squares calibration solve
  feasible on-device; impossible in 96 KB.
- **LPBAM (Low Power Background Autonomous Mode)** lets LPDMA clock SPI transactions
  and collect hundreds of averaged samples *while the M33 sleeps*. Precision here
  comes from averaging, so this makes averaging nearly free.
- **Dual-bank flash** — keep calibration in the bank you are not reflashing.
- LQFP64 comfortably fits ~30 GPIO: shared SPI + 3× MagI2C CS/DRDY + 2× SCA3300 CS +
  I2C + host interface + SWD.

> **Design decision: on-device calibration is a requirement, not a fallback.**
> A phone could host the solve instead (see
> [Calibration with a smartphone](#calibration-with-a-smartphone)), which would let the
> MCU shrink to a U535/U545 or even stay on the F401 — the DistoX2 does its whole job on
> a Cortex-M0. **We are deliberately not doing that.** The module solves its own
> calibration so it can be recalibrated and self-checked underground with no phone, no
> app version dependency, and no reliance on a BLE link. The phone becomes an optional
> UI and reference-data source rather than a required part of the loop. The U585's
> 786 KB SRAM and 160 MHz are sized for that, and the resulting firmware is the main
> consumer of both.

**TPL5010:** keep only if the module wakes itself. The carrier board already owns power
gating via `/EN` (now the LDO's EN pin), so if the carrier owns the duty cycle, drop the timer.

---

## Magnetic hygiene rules

Scale reference: **10 mA at 10 mm ≈ 200 nT**. Against 20 µT horizontal that is
**0.6° of azimuth** — the entire error budget from one stray milliamp-scale loop.

1. **Reverse the pour keepout.** Copper is diamagnetic (χ ≈ −10⁻⁵) and does not
   distort a DC field. The reason to fear planes near a magnetometer is *current in
   them*, not the metal — and removing the plane forced Rev A's supply and return into
   long open loops. Keep a ground plane under the magnetometers; route each MagI2C's
   supply/return as a tight pair with decoupling **at the MagI2C ASIC, not at the coils**
   (PNI: *"keep capacitors, especially tantalum capacitors, far away from the sensor
   coils"*); keep
   unrelated return currents out of the region.
2. **Surface finish: OSP. NOT ENIG.** Electroless *Nickel* Immersion Gold deposits
   3–6 µm of ferromagnetic nickel on every exposed pad. Rev A has `copper_finish "None"`
   — unspecified, so the fab chooses, and for 6-layer that is usually ENIG. TI states the
   same requirement for the DRV425: *"select non-magnetizable materials for the PCB and
   passives in the direct vicinity"*.
   **At JLCPCB the choice is forced and favourable:** 6-layer boards are offered only in
   **OSP or ENIG** — no HASL (planarity), no immersion silver. OSP is an organic coating
   with no metal at all, the flattest finish for the 0.4 mm-pitch MLF, and carries no
   surcharge. Caveats: ~6-month shelf life and bare copper on any later hand rework, so
   **order fab and assembly together** and don't stockpile bare boards.
3. **Non-magnetic hardware**: brass, titanium or nylon M2. Rev A's H1/H2 sit 12.3 mm
   from the magnetometer — a steel screw there is a large hard-iron source *and* a
   removable one that voids calibration when retorqued or swapped.
4. **Spread the 3 coil sets as widely as the board allows** — simultaneously the
   gradiometer baseline and the mutual-coupling mitigation.
5. **Time-multiplex the three RM3100s.** They actively drive their coils. At CC=200 a
   3-axis read is ~5 ms, so three sequential sets is ~15 ms; hand tremor over 15 ms is
   negligible, so the gradient measurement stays valid.
6. **LDO, never a switching regulator.**
7. **No crystal at all.** The can lid is typically ferromagnetic, and nothing needs one:
   SPI runs from the internal oscillator, **USB FS runs crystal-free on HSI48 + CRS**, and
   the **carrier owns wall-clock time** (it has BLE/phone) and sends it at power-up — the
   module keeps relative time only, so no RTC crystal either.
8. **Quiesce the board during the read** — LEDs off, no SPI traffic at the sampling
   instant. A constant current is calibratable; a blinking one is not.

### Calibratable vs. not

| Calibratable (hard/soft iron) | NOT calibratable |
|---|---|
| MLCC nickel barrier terminations | Time-varying currents |
| ENIG pads | Temperature-dependent drift |
| A permanently-fitted screw | Anything that moves relative to the sensor |
| Coil non-orthogonality (3×3 matrix absorbs it) | Mechanical instability of the mount |

Design so the uncalibratable set is minimised.

### Mechanical

- **SEN-Z is 6.35 mm tall** — taller than anything on Rev A (U2 at 4.8 mm). Check the
  enclosure.
- **Board area**: 9 coils + 3 ASICs + 2× SCA3300 (8.6 × 7.6 mm each) is a lot for
  30.48 × 58.42 mm. Expect to grow the board or go to a two-board stack.
- **Alignment to the sighting axis** is a precision mechanical problem. Everything
  above yields the attitude of the *PCB*; what you survey is the laser axis. If the
  module is removable, you need a kinematic mount or a documented re-calibration policy.
- **Conformal coat** — 100% humidity and mud. Costs nothing magnetically.

---

## System integration

The instrument is a **three-board stack**:

| Board | Contents |
|---|---|
| **AHRS module** (this design) | Magnetometers, accelerometers, gyro, MCU |
| **Carrier / UI board** | BLE radio, screen, buttons, battery, power management |
| **Laser board** | Laser rangefinder for distance |

Once the sensors sit in a real instrument, **magnetic hygiene stops being a PCB problem
and becomes a system mechanical problem.** This is the largest remaining risk in the
design.

### Cross-board magnetic threats

Same scale reference as above — 10 mA at 10 mm ≈ 200 nT ≈ 0.6° of azimuth.

| Source | Mechanism | Rough impact |
|---|---|---|
| **Speaker / buzzer / vibration motor** | **Permanent magnet** | Catastrophic — mT at cm range. Must be absent or very far |
| **Laser module** | Metal can + 30–50 mA pulsed drive + APD bias boost converter | ~270 nT at 30 mm → **0.8°** |
| **Battery (Li-ion)** | Steel case (hard iron) + large discharge current | Calibratable *if fixed* — see below |
| **Screen** | Steel frame + backlight boost converter | Hard iron, plus in-band envelope whenever brightness changes |
| **BLE radio** | RF itself is far out of band, but **supply current pulses at the packet rate** (~10 mA for ~1 ms) | ~100 nT at 20 mm → **0.3°**, transient |
| **Buttons** | Steel domes | Hard iron, calibratable if fixed |
| **Inter-board cable** | Supply/return loop area | Route as a tight pair, or flex with adjacent ground |

**Stack ordering is a first-class design decision.** Put the AHRS board at the far end
from battery, screen and laser, with the magnetometer coils at the far end of the AHRS
board.

### The magnetic quiet window

A **system-level** requirement, not a board-level one. The AHRS module must be able to
demand silence from the rest of the instrument — either a hardware `MEASURE` line it
asserts, or a protocol message the carrier honours.

During the window:

- BLE **not transmitting**
- Backlight off, or held in a fixed state
- Laser off
- No buzzer or vibration motor
- Converters in a fixed load state

Sequence one shot as:

1. User aims, presses the button
2. **Quiet window** → magnetometer + accelerometer capture *(the precision-critical step)*
3. Laser fires for distance
4. Display update + BLE transmit

Steps 2 and 3 must not overlap. Verify the instrument did not move between them using
the gyro — **this is the clearest justification for carrying a gyro at all.**

### Calibrate the assembled instrument, never the bare board

Every piece of ferromagnetic material in the assembly — screen frame, battery case,
laser can, screws — is hard/soft iron **fixed in the sensor frame**. That makes it
calibratable, but only *as an assembly*.

- Any disassembly potentially voids calibration.
- **A removable battery is a calibration hazard.** If the cell can shift position or be
  swapped for a different one, its hard-iron signature changes. Prefer a captive cell,
  or characterise the variation and fold it into the error budget.
- Store calibration in the AHRS module's flash but tag it with the **assembly serial**.
  Firmware should warn or refuse if the module finds itself in a different carrier.

### Module interface requirements

- Expose **both raw (uncalibrated) counts and calibrated output**. Calibration — whether
  solved on-device or on a phone — operates on raw data.
- Honour the quiet-window signal.
- Allow settling time after `/EN` power-up before measuring. The SCA3300 in Mode 4 needs
  100 ms output settling regardless.

---

## Calibration

This is where the accuracy actually comes from. The parts get you a platform capable
of 0.1°; only calibration realises it.

- Implement the **full Heeb/DistoX model**: magnetometer offset + 3×3 matrix (the
  matrix absorbs coil non-orthogonality), accelerometer offset + 3×3, **and the
  rotation between the mag and accel frames**. That last term is mandatory with
  separate packages — die-to-package rotation is often ±1–3° per part.
- Procedure: ~56 shots in 14 groups of 4 roll angles (DistoX2 convention).
- **Calibrate at 2–3 temperatures and interpolate.** Neither sensor hits target on
  factory calibration — the SCA3300's total offset error including lifetime drift is
  ±1.15°.
- **Calibrate at cave temperature with thermal soak**, not in a warm room. Caves are
  thermally benign (4–12 °C, very stable); the enemy is the surface→cave transient.
- Store with **checksum + calibration ID + date** in a dedicated flash bank, so every
  survey records which calibration produced it.

### Use the redundancy actively in firmware

Flag a shot when:

- the 3 magnetometers disagree after calibration → **magnetic gradient → *nearby* iron**
- `|a| != 1 g` → accelerometer fault or motion
- `|B| !=` the WMM/IGRF value for the cave's location → **uniform disturbance → *distant*
  iron or regional anomaly** — invisible to the gradiometer
- computed dip ≠ IGRF dip → same, and a cross-check on the mag↔accel calibration
- the gyro reports motion during the integration window

The first and third are **complementary, not redundant**: gradiometry sees what is near
you; the IGRF magnitude check sees what is far away. Shipping one without the other
leaves a class of anomaly undetected.

### Calibration with a smartphone

The solve runs **on-device** by design (see [MCU: STM32U585](#mcu-stm32u585)). The phone
is an optional complement, not part of the required loop.

**What a phone genuinely contributes:**

| Capability | Value |
|---|---|
| **GPS position + date → IGRF/WMM** | Expected \|B\|, dip I, declination D at the cave |
| **\|B\| check** | Validates magnetometer **scale factor** absolutely, no Helmholtz coil — to ~±150 nT ≈ 0.3% |
| **Dip check** | After calibration, computed dip must match IGRF. The classic DistoX calibration quality check |
| **Declination** | Magnetic → true north for the survey. Essential, not optional |
| **Precise UTC** | Logging, and the IGRF epoch |
| **Guidance UI** | Walk the user through 56 shots / 14 groups / 4 roll angles, show live residuals, flag under-sampled orientations |
| **Validation** | Display residual and solve conditioning, and **refuse a bad calibration** |
| **Temp-cal logging** | Log sensors + temperature over hours as the instrument warms and cools |

Guidance and validation matter more than they sound: the dominant calibration failure
mode is poor orientation coverage, and calibration *checking* is a known real-world gap
(see Perne in [References](#references)).

For better than IGRF on \|B\|, pull same-day data from the nearest INTERMAGNET
observatory — Chambon-la-Forêt (CLF) for France. That gets to tens of nT rather than ~150.

**What a phone can NOT do: be a reference.** Phone magnetometers are consumer AMR/Hall
parts (AK09918, BMM150/350) inside a case full of steel, speaker magnets, MagSafe
magnets, vibration motors and switching converters. Typical heading accuracy 3–15° —
30–150× worse than the 0.1° target. Its accelerometer is likewise ±30–50 mg. **The phone
is a host and a data source, never a standard.**

### Absolute references

None of these come from a phone alone.

- **Azimuth — surveyed baseline.** Two permanent marks whose azimuth is determined once
  with survey-grade GNSS or a total station, then reused indefinitely as the check
  baseline.
- **Azimuth — solar sighting.** With precise time and position from the phone, the sun's
  azimuth is computable to arcseconds. Sight through a filter or use shadow projection on
  a levelled mount. Cheap, needs no survey equipment, and you are outdoors at the
  entrance anyway.
- **Inclination — the flip method.** Measure, rotate 180° about the vertical, measure
  again. The readings must be equal and opposite; any common component is offset. Fully
  self-referencing, no equipment.
- **Magnetometer offset vs. gain — also the flip principle.** Rotating 180° flips the
  field in the sensor frame but not the offset, which separates the two. This is exactly
  what the 4-roll-angle DistoX procedure exploits.
- **Temperature coefficients.** A cooler with ice packs plus a low oven gives 3 points
  across 0–30 °C, with the SCA3300 die sensor / Pt1000 logging actual temperature. Crude but
  sufficient.

---

## References

**Parts**
- [PNI RM3100](https://www.pnisensor.com/rm3100/) ·
  [RM3100-CB](https://www.pnisensor.com/rm3100-cb/) ·
  [RM3100/RM2100 Sensor Suite User Manual V11.0](https://www.pnicorp.com/wp-content/uploads/RM3100-RM2100-Sensor-Suite-User-Manual-V11.0.pdf) ·
  [PNI magnetic sensor comparison chart](https://www.pnicorp.com/wp-content/uploads/0123_Mag_Sensor_Chart.pdf)
- [Murata SCA3300-D01 datasheet (Doc 3165 Rev 3)](https://www.murata.com/-/media/webrenewal/products/sensor/pdf/datasheet/datasheet_sca3300-d01.ashx?la=en-us&cvid=20190620010315610400)
- [Murata SCL3300 (inclinometer variant)](https://www.murata.com/en-us/products/sensor/inclinometer/overview/lineup/scl3300)
- [Analog Devices ADXL355](https://www.analog.com/en/products/adxl355.html) ·
  [ADXL355 thermal-gradient drift discussion](https://ez.analog.com/mems/f/q-a/109322/adxl355-strong-temperature-drift-on-z-axis-with-heater)
- [TI DRV425 datasheet (Rev B, Oct 2025)](https://www.ti.com/lit/ds/symlink/drv425.pdf) ·
  [DRV425 independent offset-stability testing, CTU Prague](https://www.researchgate.net/publication/330255637_Testing_and_application_of_an_integrated_fluxgate_sensor_DRV425)
- [STM32U585 datasheet (DS13086)](https://www.mouser.com/pdfDocs/stm32u585ai.pdf)

**Cave survey instruments and calibration**
- [Beat Heeb — The Next Generation of the DistoX Cave Surveying Instrument](https://paperless.bheeb.ch/download/DistoX2.pdf)
- [Marco Corvi — DistoX2, SAP5 and BRIC4 comparison](https://ia801800.us.archive.org/32/items/disto-xsap-bric-20200302/DistoXSapBric-20210329.pdf)
- [BRIC survey — design history](https://www.bricsurvey.com/about)
- [Matija Perne — DistoX calibration tools and the need for calibration checking (arXiv)](https://arxiv.org/pdf/2102.09891)

**Geomagnetic reference data**
- [NOAA/NCEI World Magnetic Model (WMM)](https://www.ncei.noaa.gov/products/world-magnetic-model) — field model, coefficients and source code
- [IAGA IGRF](https://www.ncei.noaa.gov/products/international-geomagnetic-reference-field) — higher-order reference field model
- [INTERMAGNET](https://intermagnet.org/) — observatory data; Chambon-la-Forêt (CLF) is the French station
