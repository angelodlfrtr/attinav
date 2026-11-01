# Attinav Rev B — roadmap

Sequence, status and decisions for the Rev B redesign.
Technical rationale lives in [README.md](README.md) — this file is the plan.

**Scope: the AHRS sensor module only.** Not a complete instrument. The carrier
(BLE, screen, buttons, battery) and laser boards are separate and out of scope here.
See [Possible carrier roles](#possible-carrier-roles).

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

> ### ⚠ Governing assumption — confirm before layout
> A handheld instrument roughly in Cavway X1 territory: **~130 × 55 × 33 mm**, with the
> AHRS board at one end, the carrier (battery, screen, BLE) at the other, and the laser
> between or alongside. **Decisions 1.1 and 1.2 hang off this.** If the housing differs,
> revisit both.

| # | Decision | Resolution |
|---|---|---|
| 1.1 | Mechanical envelope | **80 × 36 mm, 6 layers, 1.6 mm** |
| 1.1b | Mechanical datum | **2× reamed locating holes + non-magnetic dowel pins**, plus 2× clearance holes for screws |
| 1.2 | Coil cluster placement | **Compact non-collinear triangle at the far end**, 18–38 mm baselines |
| 1.3 | Carrier interface | **UART over 8-pin FFC**; USB-C direct to the MCU, separate |
| 1.4 | Supply from carrier | **5 V**, local low-noise LDO to 3.3 V |
| 1.5 | Accelerometer count | **2** |
| 1.6 | Gyro | **Keep** — 1× ICM-42688-P |

### 1.1 / 1.2 — Floorplan

```
                36 mm
    ┌───────────────────────────┐
    │                           │
    │   [A]               [C]   │   magnetometer zone
    │                           │   x = 0 … 50 mm
    │   [B]                     │   A–B 18mm,  A–C / B–C ~38mm
    │                           │
    ├───────────────────────────┤
    │  (o)   [G1] [G2]    (o)   │   accel pair + DATUM
    ├───────────────────────────┤   x = 50 … 64 mm
    │  [U585]        [LDO]      │   MCU / power / interface
    │  [USB-C]       [FFC]      │   x = 64 … 80 mm
    └───────────────────────────┘
                80 mm
                                    ↓ carrier board this end
```

Area 2880 mm², ~60% larger than Rev A's 1780 mm². Buys comfortable separation and
routing room.

**Why cluster the magnetometers rather than spread them along the board** — this reverses
the earlier "spread as widely as possible" guidance in
[README → Magnetic hygiene rules](README.md#magnetic-hygiene-rules), rule 4:

Gradient from a dipole at distance *r* is roughly `3|B|/r`. Steel 0.5 m away producing
100 nT gives **~600 nT/m**. At a 20 mm baseline that needs a 12 nT differential, against
~21 nT of raw differential sensor noise — marginal. **But hundreds of samples are averaged
per shot**, dropping noise to ~1.5 nT, which resolves 12 nT with 8× margin.

So baseline stops being the binding constraint and **distance from the carrier's magnetic
sources becomes it** — the laser (~0.8°) and BLE (~0.3°) dominate. Clustering all three at
the far end maximises that distance. A non-collinear triangle also yields 2D gradient
information, which a line along the board would not.

Place `[G1] [G2]` adjacent so they share temperature and mechanical stress — that makes
their disagreement a meaningful fault signal rather than a thermal artefact.

### 1.1b — Datum

The error budget allocates **0.03° to sight-axis alignment**. Clearance holes with M2
screws give ±0.1–0.2 mm of slop, which over a 60 mm lever is **0.1–0.2°** — it blows the
budget alone. So: **dowels locate, screws only clamp.** Non-magnetic dowels (brass or
ceramic). Marked `(o)` above, straddling the accelerometers so the datum sits close to
what it references.

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
