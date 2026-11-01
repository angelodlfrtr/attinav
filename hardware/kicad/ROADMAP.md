# Attinav Rev B — roadmap

Sequence, status and decisions for the Rev B redesign.
Technical rationale lives in [README.md](README.md) — this file is the plan.

**Scope: this repo covers the AHRS sensor module only.** Not a complete instrument.
The carrier (BLE, screen, buttons, battery), the laser board and the housing are
separate and **not designed here**. Where they affect the module they are recorded as
[Housing and system requirements](#housing-and-system-requirements) — an interface spec
the module depends on, not a design. See also
[Possible carrier roles](#possible-carrier-roles).

---

## Decisions taken

| Decision | Choice | Notes |
|---|---|---|
| Rev B approach | **Full 3× board directly**, no 1× prototype | Risk shifted into the design — see [Risk register](#risk-register) |
| Magnetometer | 3× PNI RM3100 sensor suite | Gradiometry + voting |
| Accelerometer | 2× Murata SCA3300-D01, Mode 4 | `\|a\|=1g` is a free third vote |
| MCU | STM32U585, "Q" suffix | **On-device calibration is a requirement, not a fallback** |
| Gyro | 1× TDK ICM-42688-P | Motion gating only, not azimuth |
| Dropped | SCHA63T, DRV425, ADXL355, LM94021, crystal | Reasons in README |
| iPhone / 3D scan integration | **Out of scope** | Parked |
| Surface finish | Immersion silver or OSP — **never ENIG** | Nickel is ferromagnetic |

---

## Phase 1 — Resolved

> **Assumed instrument context** (not designed here, see
> [Housing and system requirements](#housing-and-system-requirements)): a self-contained
> handheld, ~175 × 48 × 28 mm, zoned front-to-rear as laser → magnetometers →
> module electronics → carrier → battery. **If that zoning changes, revisit 1.1 and 1.2.**

| # | Decision | Resolution |
|---|---|---|
| 1.1 | Board outline | **110 × 40 mm, 6 layers, 1.6 mm** |
| 1.1b | Mechanical datum | **Flat seating face + 2 dowels at opposite ends of the long axis** (L ≥ 30 mm), plus clearance holes for clamping screws |
| 1.2 | Coil cluster placement | **Non-collinear triangle in the forward zone**, ~40–50 mm long baselines |
| 1.3 | Carrier interface | **UART over 8-pin FFC**; USB-C direct to the MCU, separate |
| 1.4 | Supply from carrier | **5 V**, local low-noise LDO to 3.3 V |
| 1.5 | Accelerometer count | **2** |
| 1.6 | Gyro | **Keep** — 1× ICM-42688-P |

### 1.1 / 1.2 — Floorplan

One rigid PCB, 110 × 40 mm, spanning the non-conductive nose into the machined chassis.

```
  x=0                                                        110 mm
    ┌──────────────────────────────┬──────────────────────────────┐
    │                              │                              │
    │   [A]                        │                              │  40
    │          (o)                 │   [G1][G2]          (o)      │  mm
    │   [B]              [C]       │   [U585]   [LDO] [USB-C][FFC]│
    │                              │                              │
    └──────────────────────────────┴──────────────────────────────┘
      POM/PEEK nose (non-conductive)   machined aluminium chassis
      x = 0 … 55  magnetometers        x = 55 … 110  electronics
      A–B 24mm, A–C / B–C ~40mm        datum dowels L ≈ 43 mm
                                                          carrier end →
```

**Keep it one board.** The coils' orientation relative to the datum is then fixed by the
FR4 itself, with no mechanical interface to drift.

Place `[G1] [G2]` adjacent so they share temperature and mechanical stress — that makes
their disagreement a meaningful fault signal rather than a thermal artefact.

### Why 110 × 40 — and why it is not a minimum

**The accuracy-limited minimum is ~70 × 35 mm.** Nothing in the error budget improves
above roughly 75 × 35 mm:

| Error term | Contribution | Scales with board size? |
|---|---|---|
| Mag noise (averaged) | 0.014° | **No** — sensor + averaging only |
| Mag calibration residual | 0.057° | **No** — calibration quality |
| Tilt error × tan 63° | 0.069° | **No** — accel temperature calibration |
| Mag↔accel misalignment | 0.030° | **No** — PCB rigidity |
| Sight-axis alignment | 0.030° | **Partly** — via dowel separation *L* |

**You cannot buy accuracy with board area.** The accuracy levers are multi-temperature
calibration (biggest), the housing datum and thermal mass, distance from the battery (a
*housing* length, not a board dimension), and layout hygiene.

**What does scale is anomaly detection** — a capability, not an accuracy term, and the
module's distinguishing feature. Averaged differential noise between two clusters is
~2.1 nT, so the resolvable gradient is 2.1 nT / baseline:

| Baseline | Gradient floor |
|---|---|
| 20 mm | 105 nT/m |
| 35 mm | 60 nT/m |
| 50 mm | 42 nT/m |

An anomaly producing **35 nT is exactly 0.1°** — the whole azimuth budget. From 0.5 m
that is a gradient of ~210 nT/m: only **2× margin at a 20 mm baseline, 5× at 50 mm**.
So 40–50 mm baselines are worth paying board length for.

Caveat: on a 40 mm-wide board the **transverse baseline is capped at ~25 mm**, so the
gradiometer is anisotropic — gradients across the short axis are detected less sensitively.

Also note this supersedes rule 4 ("spread as widely as the board allows") in
[README → Magnetic hygiene rules](README.md#magnetic-hygiene-rules). Averaging makes
baseline a soft constraint; **distance from the battery and laser is the hard one.**

If the housing forces a shrink: ~85 × 35 mm costs detection sensitivity and nothing in
the error budget. Below ~70 × 35 mm real compromises begin.

### 1.1b — Datum (corrected)

The error budget allocates **0.03° to sight-axis alignment**. Two locating features
separated by *L*, each with radial clearance δ, permit a rotation of **θ ≈ 2δ/L**:

| Fit | δ | L = 43 mm | L = 25 mm |
|---|---|---|---|
| Reamed + ground dowel (H7/h6) | 0.006 mm | **0.014°** | 0.024° |
| Machined, loose | 0.02 mm | 0.047° | 0.092° |
| M2 clearance hole only | 0.15 mm | 0.35° | 0.60° |

**So: dowels locate, screws only clamp.** And — correcting the earlier floorplan — the
dowels go at **opposite ends of the long axis**, not straddling the accelerometers.
Straddling them across the 40 mm width gives L ≈ 25 mm, which even with a machined fit
lands at 0.024°, consuming most of the allocation for no reason.

Plus a **flat seating face** against the chassis for out-of-plane constraint, which
dowels cannot provide.

Dowels must be **non-magnetic** — brass or ceramic.

### 1.3 — Interface

| Signal | Direction | Purpose |
|---|---|---|
| 5 V | carrier → AHRS | power |
| GND | — | — |
| UART TX / RX | bidirectional | commands + shot data |
| `/EN` | carrier → AHRS | power gate (keep Rev A's PMOS) |
| `QUIET` | **AHRS → carrier** | open-drain, asserted during magnetic integration |

Six signals on an **8-pin FFC** (two spares). A flex with adjacent ground also fixes the
inter-board loop-area problem; pin headers do not.

**UART, not SPI** — a shot is tens of bytes, so bandwidth is irrelevant. UART needs fewer
pins, survives a cable better, and is trivially debuggable. SPI would make the carrier
poll as master for no benefit.

`QUIET` is an **output** from this board: only the module knows when it is integrating.

**USB-C goes direct to the U585's USB OTG**, not through the carrier — the
calibration raw-stream path and standalone bench interface.

### 1.4 — Supply

**PSRR decides it.** The carrier rail will be noisy: backlight boost, BLE current bursts,
laser firing. An LDO from 5 V rejects that; an LDO in dropout from 3.3 V passes it
straight to the sensors.

Secondary: TPS7A20 dropout ~200 mV, so 3.3 V in delivers ~3.1 V — marginal against the
SCA3300's 3.0–3.6 V window.

The carrier boosts from Li-ion to make 5 V. That boost is a magnetic source but sits on
the far board and switches at MHz (out of band), and the LDO rejects its in-band envelope.
**Document the AHRS current draw** so the carrier can size it.

Changed from Rev A, which took 3.3 V_EXT directly.

### 1.6 — Why keep the gyro

**Accelerometers cannot detect yaw rotation about the gravity vector** — and yaw is
exactly what corrupts azimuth. If the instrument twists during magnetic integration, only
the gyro sees it. €8 and 2.5 × 3 mm for a motion-validity flag on every shot.

### Stackup — keep Rev A's

| Layer | Assignment |
|---|---|
| F.Cu | signal + components |
| In1.Cu | GND |
| **In2.Cu** | **+3.3 V** |
| **In3.Cu** | **GND** |
| In4.Cu | GND |
| B.Cu | GND + spill signals |

Keep the asymmetric dielectric with **In2/In3 at 0.1 mm** — one of Rev A's genuinely good
decisions. **No pour keepout this time**: keep GND under the coils and manage return paths
instead.

---

## Housing and system requirements

**Not designed in this repo.** This is the interface spec the module depends on — what
the housing, carrier and laser must provide for the error budget to hold. Record it here
so the constraints are not lost, and hand it to whoever builds the instrument.

### Hard requirements

| # | Requirement | Why |
|---|---|---|
| H1 | **Non-conductive section (POM/PEEK) over the magnetometer zone**, ≥10 mm clearance to any aluminium | **Eddy currents.** See [below](#h1--the-eddy-current-problem) |
| H2 | **Machined** chassis providing a flat seating face + 2 dowel bosses, L ≥ 30 mm, reamed | Datum error scales as 2δ/L; a printed housing caps you at ~0.25° |
| H3 | **Non-magnetic dowels** — brass or ceramic | Hard iron 50 mm from the coils |
| H4 | **No speaker, no vibration motor, anywhere.** Piezo buzzer only | Permanent magnets, and the motor's **moves** — not calibratable |
| H5 | **Captive LiPo pouch cell, not an 18650** | 18650s have **steel cans**. Pouch cells use aluminium laminate |
| H6 | Battery **rearmost**, ≥100 mm from the coil zone | Its current varies with load |
| H7 | Battery **not user-removable** | Swapping changes its hard-iron signature → voids calibration |
| H8 | **Laser doweled to the same chassis** as the module, optical axis inline | One interface each; the budget multiplies interfaces |
| H9 | Laser can ≥10 mm from the nearest coil | Static hard iron is calibratable, but minimise it |
| H10 | Carrier honours the **`QUIET` line**: no BLE TX, backlight fixed, laser off, converters at fixed load | Pulsed currents are *not* calibratable |
| H11 | Tripod boss (if fitted) **at the rear**, brass or direct-in-aluminium, **never a steel insert** | And note the tripod's own screw is usually steel — a *variable* source |
| H12 | Module **bonded or captive** after calibration, not removable | Calibration is assembly-specific |

### H1 — the eddy-current problem

The RM3100 is magneto-**inductive**: it measures LC oscillation frequency, which depends
on coil inductance. A nearby conductor loads the coil through eddy currents and shifts
that inductance, i.e. **shifts the gain**. Skin depth in aluminium at ~100 kHz is
~0.26 mm, so a 2 mm wall is many skin depths and couples well. Worse, aluminium
conductivity varies ~0.4%/°C, so a fixed geometry becomes a **temperature-dependent gain
drift** — the class of error calibration handles worst.

**Status: unverified.** This is reasoning from physics, not a cited spec. **Rev A was
never fabricated**, so there is no existing hardware to test against. Three ways to
resolve it:

- [ ] **(a) Cheapest — buy one assembled RM3100 breakout** alongside the Phase 2 parts
      order (GNSS.store ELT0327 ~€35, or a cavedevice kit on a scrap board). Log gain vs.
      distance to an aluminium plate, and gain vs. temperature with the plate fixed. ~€35
      and no design work, and the same board doubles as a target for early calibration-
      firmware development.
- [ ] **(b) Check the PNI sensor suite manual** for guidance on nearby non-ferrous metal.
      Free, and may settle it outright.
- [ ] **(c) Design around it — the conservative default.** Build the hybrid housing
      (non-conductive nose) regardless. The POM/PEEK nose is a non-structural cover, so
      it costs little and removes the risk without needing the answer.

**Recommended: (b), then (c), with (a) if you want the number.** Do not defer this to
Rev B bring-up *and* commit to a one-piece aluminium housing — if H1 is real, a
single-billet chassis is unfixable, whereas the hybrid is cheap insurance either way.

### Why machined, quantitatively

| Housing | δ (radial) | Datum error at L = 43 mm | Azimuth budget total |
|---|---|---|---|
| Machined, reamed + ground dowel | 0.006 mm | 0.014° | **0.10°** |
| SLA / MJF print | ~0.1 mm | 0.16° | 0.19° |
| FDM print | ~0.2 mm | 0.25° | **0.27°** |

A printed housing makes itself the single largest error term — larger than all the
sensors combined — and caps the instrument around 0.25–0.3°, i.e. Cavway X1 territory.

**Escape hatch:** calibrate the sensor-to-sight-axis transform against a known azimuth
(surveyed baseline or solar sighting). That converts the requirement from *precise* to
*stable*, so a printed housing with a **potted** board could still reach target. Do this
anyway even with a machined chassis — machining buys precision, calibration buys
insensitivity to what is left.

### Assumed zoning

```
  0        30   40                95        135              175 mm
  ├─────────┤   ├──────────────────┤─────────┤────────────────┤
  │ LASER   │gap│  MAGNETOMETERS   │  AHRS   │ CARRIER + BATT │
  │         │   │  (POM/PEEK nose) │ (alu)   │  (alu)         │
  └─────────┴───┴──────────────────┴─────────┴────────────────┘
```

Self-contained handheld, ~175 × 48 × 28 mm, est. 200–250 g. Longer than the Cavway X1's
131 mm because **separating the coils from the battery is the dominant constraint** and
length is cheap. Coil-to-battery ≈ 100 mm.

Keep the laser **inline**, not offset: an offset creates parallax error scaling inversely
with shot distance — 20 mm offset on a 2 m shot is 0.57°. Correctable using the measured
distance, but inline avoids the complexity.

---

## Phase 2 — Parts

- [ ] **Order RM3100 parts.** Primary: [cavedevice.com](https://cavedevice.com/products/magnetometer-sensor-rm3100-set-by-pni)
      — €20/kit, individual parts from €4, **no MOQ**, EU shipping. Confirm stock first.
- [ ] **Check whether LCSC C464558 is available for JLCPCB assembly.** If yes this is
      significant: it hands the SEN-Z placement-tilt risk to a professional pick-and-place
      instead of your own assembly.
- [ ] Order for **3+ board builds**, not 1. Murata lead times are the schedule risk, and
      multiple units let you characterise part-to-part spread for the error budget.
- [ ] Source **non-magnetic MLCCs** (Cu/Ag termination) for coil decoupling.
- [ ] Source **non-magnetic M2 hardware** — brass, titanium or nylon.
- [ ] Consider buying a **Cavway X1 as a reference instrument** (±0.4° / ±0.2°) for
      cross-checking. Note: never place two instruments side by side — they interfere
      magnetically. Shoot the same legs sequentially.

Alternative suppliers: Solsta UK (authorized, post-Brexit customs), PNI direct
(`sales@pnisensor.com`, $250/10 suites). **Avoid brokers** — counterfeit risk.

---

## Phase 3 — Library (highest-risk step)

With no prototype, footprint correctness is only proven after fab. Treat accordingly.

- [ ] SEN-XY symbol + footprint (PN 13104)
- [ ] **SEN-Z symbol + footprint (PN 13101) — precision-critical.** Pad, paste and mask
      geometry straight from the PNI manual: 5 mil stencil, paste area much smaller than
      pad. Coil tilt → axis misalignment → azimuth error.
- [ ] MagI2C symbol + footprint (PN 13156, 28-pin MLF 4×4)
- [ ] SCA3300-D01 symbol + footprint (12-pin pre-molded, 8.6 × 7.6 × 3.3 mm)
- [ ] Pt1000 symbol (standard resistor footprint)
- [ ] Verify STM32U585 in `MCU_ST_STM32U5`; check ICM-42688-P and TMP117
- [ ] **Verify every footprint against the physical parts in hand** before releasing
      gerbers — 1:1 paper printouts, parts laid on top, calipers on the MLF. This is the
      step a prototype would have caught for you.

---

## Phase 4 — Schematic

- [ ] **Go hierarchical.** Rev A is one flat sheet with 113 symbols.
- [ ] Sheets: power, MCU, magnetometer (**instantiated 3×**), accel, interface
- [ ] Read the PNI manual for REXT value and cycle-count configuration
- [ ] Pin budget check against LQFP64 before committing to the package
- [ ] **Design for bring-up** — needed now that there is no prototype:
  - [ ] **USB-C on the U585's USB OTG** — streaming raw counts during calibration
        development is far faster than BLE through a carrier that does not exist
  - [ ] **Standalone bench power** — jumper or header so the board runs without a carrier
  - [ ] **Test points**: each supply rail, SPI bus, DRDY per MagI2C
  - [ ] **Raw-count path in the protocol from day one** — calibration needs it

---

## Phase 5 — Layout, in this order

1. [ ] Board outline + mounting/datum features from the enclosure
2. [ ] **Place the 3 coil clusters first, maximally separated.** Everything works around them
3. [ ] Per cluster: coils + MagI2C + tight non-magnetic decoupling, supply/return as a tight pair
4. [ ] Accelerometers near the mechanical datum, close together (same temperature, same stress)
5. [ ] MCU, LDO, interface last
6. [ ] Planes: **no pour keepout this time.** Keep GND under the coils, but route so no
       unrelated return current passes beneath them
7. [ ] Keep Rev A's tight In2(+3.3V)/In3(GND) 0.1 mm plane pair — it was a good decision

---

## Phase 6 — Verification

- [ ] Set real design rules — fix Rev A's `min_clearance: 0.0` and `min_track_width: 0.0`
      to the fab's actual capability
- [ ] **Specify surface finish explicitly.** Rev A has `copper_finish "None"`, which lets
      the fab choose ENIG and its 3–6 µm of ferromagnetic nickel
- [ ] Verify via `kicad-cli` netlist parsing, **not** the audit tools
      (see README → Tooling caveats)
- [ ] 3D check: SEN-Z is 6.35 mm tall, taller than anything on Rev A
- [ ] ERC + DRC clean

---

## Phase 7 — System

- [ ] Carrier interface spec: quiet-window protocol, raw + calibrated streams, shot sequencing
- [ ] Non-magnetic hardware specified in the BOM
- [ ] Alignment datum to the sight axis defined and toleranced

---

## Parallel track — start now

**Board-independent and on the critical path.** Neither sensor hits target on factory
calibration, so the error budget depends entirely on this.

- [ ] Implement the **full Heeb/DistoX calibration model**: mag offset + 3×3, accel
      offset + 3×3, and the mag↔accel frame rotation
- [ ] Multi-temperature calibration and interpolation — **this is where Rev B beats the
      commercial instruments**, and it is firmware, not hardware
- [ ] Temperature chamber procedure (cooler + ice packs, low oven, 3 points over 0–30 °C)
- [ ] Quality metrics: gradiometer residual, `\|a\|` vs 1 g, `\|B\|` vs IGRF, gyro motion flag
- [ ] Absolute verification method: surveyed baseline or solar sighting

Having the solver drafted and ready the day boards arrive is what keeps bring-up short.

---

## Risk register

Going straight to the full board moves two risks from "cheaply retired" to "managed".

| Risk | Impact | Mitigation |
|---|---|---|
| **SEN-Z placement tilt** | Non-orthogonality → azimuth error | Footprint from PNI manual; 3 clusters cross-check each other via gradiometer residual; prefer professional assembly; ask assembler for a coplanarity spec; build multiple units to characterise spread |
| **Footprint error found only after fab** | Respin + long lead time | Physical-part verification in Phase 3; order parts for 3+ builds |
| **Calibration firmware unproven on hardware** | Long bring-up | USB-C raw streaming, standalone bench power, generous test points; draft solver in parallel |
| **Board area insufficient for cluster separation** | Weak gradiometer baseline | Size generously in Phase 1 |
| **Murata / PNI lead times** | Schedule | Order early, order spares |
| **Scope creep into a full instrument** | Project never finishes | **Module is the deliverable.** Carrier and laser stay out of scope |

---

## Possible carrier roles

The sensor head is identical in all of these — none require Rev B changes, so none are
foreclosed by building the module first.

1. **DistoX-style paperless instrument** — own carrier with BLE, screen, buttons, laser.
   Most work; competes with the Cavway X1 (±0.4° / ±0.2°, RM3100 + SCA3300, IP67, shipping).
2. **Accuracy upgrade for the [Caveatron](https://www.caveatron.com/)** — open-source
   real-time centerline + lidar sweep instrument (compass <1°, clino <0.5°). Attinav's
   sensors are 5–10× better. Open module into an open project; far less work.
3. **Absolute attitude aid for a DIY lidar SLAM rig** — Livox MID-360 + GLIM/Voxel-SLAM.
   Orientation drift is SLAM's dominant error, and no cave rig has a drift-free absolute
   heading reference. **The 3× gradiometer is the enabling piece**: it tells the SLAM when
   the magnetic heading is trustworthy, so disturbed readings can be rejected. Arguably
   the most novel option.

**Why this project is worth doing** (recorded so the motivation does not drift):
gradiometric anomaly *measurement* rather than mere detection; open hardware resilience
in an ecosystem that is one person deep twice over (DistoX2 discontinued, Cavway X1 a
single developer); and temperature-compensated calibration, which appears unexploited
commercially. Raw accuracy is the *weakest* justification — the instrument stops being
the limiting factor somewhere around 0.2–0.3°.
