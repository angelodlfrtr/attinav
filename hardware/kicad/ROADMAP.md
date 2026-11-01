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
| MCU | STM32U585, **LDO mode — no SMPS inductor fitted** | **On-device calibration is a requirement, not a fallback.** The SMPS is a switching regulator — forbidden by the no-buck rule, and moot since the core sleeps during integration. Non-Q preferred; a Q variant is acceptable wired LDO-only per AN5373, since non-Q LQFP-64 (RIT6) is out of stock at LCSC |
| Gyro | 1× TDK ICM-42688-P | Motion gating only, not azimuth |
| Dropped | SCHA63T, DRV425, ADXL355, LM94021, crystal, **2× TMP117**, **PMOS `/EN` gate (Q1)** | Reasons in README. TMP117: the SCA3300 die sensor is the correct compensation source. PMOS: the LDO's EN pin does the job |
| 3D scan / LiDAR integration | **Parked** | Carrier-level feature, needs no module changes |
| Smartphone | **Bench tool + reference source only** | Magnetic screening and IGRF/solar lookup. **No app to be written** — on-device solving makes it redundant, and the cave ecosystem is Android |
| Surface finish | **OSP** — **never ENIG** | Nickel is ferromagnetic. JLCPCB 6-layer offers only OSP or ENIG, so OSP is both the rule and the only option |

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
| 1.3 | Carrier interface | **UART over 8-pin 1.0 mm latching FFC**, interleaved-ground pinout; USB-C direct to the MCU, separate. Compliant link is **required** by the bonded datum |
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
      A–B ~28 mm, A–C / B–C ~36 mm     datum dowels L ≈ 44 mm
                                                          carrier end →
```

**Keep it one board.** The coils' orientation relative to the datum is then fixed by the
FR4 itself, with no mechanical interface to drift.

**Orient every triad with its MagI2C on the chassis (+x) side of its coils.** Each triad is
an **8 × 20 mm block** — coils (8 × 8) + ≥5 mm gap (PNI: caps away from coils) + ASIC and
decoupling (7 × 7). With the ASIC toward the chassis, triad C's envelope can end exactly
at the nose boundary (x = 55) while its **coils end at x ≈ 43, leaving 12 mm of clearance
to the aluminium** — satisfying H1's ≥10 mm without moving the boundary. It is also the
right choice for SPI routing, since all three ASICs then face the MCU. A and B stacked at
the nose tip, C at mid-height on the right, gives the baselines above — verified by
placement calculation: A–B 28.0, A–C 35.8, dowel L 44.0 mm, θ 0.016°.

Place `[G1] [G2]` adjacent so they share temperature and mechanical stress — that makes
their disagreement a meaningful fault signal rather than a thermal artefact.

### Why 110 × 40 — and why it is not a minimum

**Two different floors apply, and an earlier version of this section conflated them.**
Each triad is an 8 × 20 mm block (see the orientation note above), and the hybrid housing
puts the dowels in the chassis zone only, so two *serial* length requirements stack:

- **Accuracy-only floor ≈ 65 × 40 mm** — dowel span L ≥ 30 mm in the chassis zone plus one
  triad length in the nose. Gradiometry would be crippled (A–C ≈ 15 mm, ~1.5× margin).
- **Useful-gradiometer floor ≈ 90 × 40 mm** — A–C ≥ 30 mm (3× margin on the 210 nT/m
  threat) needs a ~48 mm nose, and L ≥ 30 mm needs a ~41 mm chassis. **35 mm width does
  not work** at any reasonable length: A–B drops to 23 mm and L falls short.
- **110 × 40 chosen** — ~20 mm of length slack over the gradiometer floor, A–C ≈ 36 mm,
  3.6× margin, L = 44 mm. Raw component utilisation is only ~27 %, so *area* is not the
  constraint — *length* is, because the two requirements are serial.

A **one-piece aluminium housing** (acceptable since H1 was downgraded) lets the dowels
span the whole board, removing the chassis-zone constraint; the floor then drops to
roughly 75 × 40 mm.

Nothing in the error budget improves above the accuracy-only floor:

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

Three caveats on these numbers:

- **The 2.1 nT figure assumes averaging works cleanly.** At cycle count 200 the RM3100's
  LSB is ~13 nT, comparable to its noise, so quantisation limits how far averaging gets
  you. Use a **higher cycle count (400+)** during the shot so the LSB sits well below the
  noise. Treat 2.1 nT as optimistic by ~2× otherwise.
- **Gradiometry only sees *nearby* iron.** A distant large mass or iron-bearing stratum
  produces a near-uniform offset with negligible gradient — invisible at *any* baseline.
  That case is caught by **|B|-vs-IGRF and dip-vs-IGRF**, which must ship alongside. See
  [README → Use the redundancy actively](README.md#use-the-redundancy-actively-in-firmware).
- On a 40 mm-wide board the **transverse baseline is capped at ~25 mm**, so the gradiometer
  is anisotropic — gradients across the short axis are detected less sensitively.

Also note this supersedes rule 4 ("spread as widely as the board allows") in
[README → Magnetic hygiene rules](README.md#magnetic-hygiene-rules). Averaging makes
baseline a soft constraint; **distance from the battery and laser is the hard one.**

If the housing forces a shrink: down to ~90 × 40 mm costs only ~6 mm of A–C baseline
(3.6× → 3× margin) and nothing in the error budget. Below that, with the hybrid housing,
either the dowel span or the gradiometer baseline must give — and keep the width at 40 mm
either way.

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
| `/EN` | carrier → AHRS | power gate — **drives the LDO's EN pin**; Rev A's PMOS (Q1) is dropped |
| `QUIET` | **AHRS → carrier** | open-drain, asserted during magnetic integration |

Six signals on an **8-pin, 1.0 mm pitch FFC with flip-latch connectors on both boards**.
The two spare positions are spent as grounds:

```
 1    2    3    4    5    6    7    8
GND  5V  GND   TX   RX  /EN QUIET GND
```

5 V sits between two grounds so its return is adjacent on both sides and the supply
current loop is as small as the cable allows; the UART pair is flanked from the power pin.
1.0 mm over 0.5 mm: more robust, inspectable, and eight positions is all that is needed.
Basic parts at JLCPCB.

**Why a compliant link is required, not merely convenient.** Housing path 2 bonds the board
to the chassis and captures its position by calibration. Anything that afterwards moves or
stresses the board corrupts the sight-axis alignment — so the interconnect must not be a
rigid bridge through which carrier flex, screen presses, battery weight or assembly
tolerance reach the bonded board. That one requirement decides the options:

| Option | Verdict | Why |
|---|---|---|
| Castellated edges (module soldered onto the carrier) | ✗ | Carrier becomes the structural reference: laser→chassis→carrier→solder→AHRS through a flexing board. Also puts carrier copper and return currents under the coil zone |
| Pin headers / stacking sockets (Rev A's J2–J5) | ✗ | Rigid coupling; friction contacts go intermittent under knocks; mating force loads the bonded board; fixes the stack height; 8–11 mm tall. Bench SWD only |
| Mezzanine (Hirose DF40, Molex SlimStack) | ✗ | Same rigid coupling, plus ±0.1 mm parallelism between a bonded board and a separately mounted carrier. 0.4 mm pitch is unreworkable. Overkill for 6 signals |
| **FFC / FPC jumper** | **✓ primary** | Compliant; bends, so it works whether the carrier sits in-line or stacked above the electronics zone; adjacent conductors give a tight loop; latching variants hold under vibration; ~1.5 mm tall |
| **JST GH 1.25 mm latching harness** | **✓ alternative** | Equally compliant, more shock-tolerant, routes anywhere; bulkier (~4 mm), hand-crimped, and the 5 V/GND pair must be twisted to keep the loop small |

Rigid-flex (AHRS + carrier as one assembly) is the best answer if this ever becomes a
product, but it couples both board designs into one revision cycle and the carrier is out
of scope here.

Switch to JST GH only if the cable path turns out awkward or ruggedness matters more than
height — it is a legitimate choice, not a fallback.

**Connector metalwork:** some FFC connectors carry stainless-steel reinforcement nails;
JST GH has none. At 55+ mm from the coils and fixed, it is calibratable and nearly
negligible — but given a free choice, pick an all-plastic-body FFC part. Strain relief and
cable routing are housing requirement H13.

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

Accelerometers cannot detect rotation about the gravity vector — gravity is invariant under
it. But the **magnetometer time-series can**, since B rotates in the body frame, so the
gyro is *not* the only yaw-motion detector. Its genuine value is narrower but real: it
detects motion **when the magnetic field itself is disturbed or changing**, at higher
bandwidth, and independently of both other sensors — precisely the situation where the
magnetometer-based check is least trustworthy. €8 and 2.5 × 3 mm for an independent
motion-validity flag on every shot.

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
| H1 | *Preferred, not blocking:* non-conductive section (POM/PEEK) over the magnetometer zone, ≥10 mm clearance to any aluminium | Eddy currents — **downgraded to low risk**, PNI is silent on non-ferrous metal. See [below](#h1--the-eddy-current-problem) |
| H2 | **Machined aluminium chassis recommended** — flat seating face + 2 dowel bosses, L ≥ 30 mm. **The hard requirement is dimensional *stability*, not locating precision** | With the board bonded (H12) and the sensor→sight-axis transform calibrated, dowel clearance is irrelevant; only later movement matters. **JLCPCB drills to ±0.075 mm position, so the PCB's own holes cannot be precision locators** — bond + calibrate is the working path. See [below](#the-housing-requirement-corrected) |
| H3 | **Non-magnetic dowels** — brass or ceramic | Hard iron 50 mm from the coils |
| H4 | **No speaker, no vibration motor, anywhere.** Audible feedback only via a **piezoelectric element** — verify the part is genuinely piezo, not an electromagnetic buzzer sold as one | Permanent magnets, and the motor's **moves** — not calibratable |
| H5 | **Captive LiPo pouch cell, not an 18650** | 18650s have **steel cans**. Pouch cells use aluminium laminate |
| H6 | Battery **rearmost**, ≥100 mm from the coil zone | Its current varies with load |
| H7 | Battery **not user-removable** | Swapping changes its hard-iron signature → voids calibration |
| H8 | **Laser doweled to the same chassis** as the module, optical axis inline | One interface each; the budget multiplies interfaces |
| H9 | Laser can ≥10 mm from the nearest coil | Static hard iron is calibratable, but minimise it |
| H10 | Carrier honours the **`QUIET` line**: no BLE TX, backlight fixed, laser off, converters at fixed load | Pulsed currents are *not* calibratable |
| H11 | Tripod boss (if fitted) **at the rear**, brass or direct-in-aluminium, **never a steel insert** | And note the tripod's own screw is usually steel — a *variable* source |
| H12 | Module **bonded or captive** after calibration, not removable | Calibration is assembly-specific |
| H13 | **FFC strain-relieved at the AHRS end** (adhesive dab or chassis clamp) and **routed rearward only** | Residual cable tension pulls on the bonded board; the cable carries the 5 V supply current and must never cross the coil zone |

### H1 — the eddy-current problem

The RM3100 is magneto-**inductive**: it measures LC oscillation frequency, which depends
on coil inductance. A nearby conductor loads the coil through eddy currents and shifts
that inductance, i.e. **shifts the gain**. Skin depth in aluminium at ~100 kHz is
~0.26 mm, so a 2 mm wall is many skin depths and couples well. Worse, aluminium
conductivity varies ~0.4%/°C, so a fixed geometry becomes a **temperature-dependent gain
drift** — the class of error calibration handles worst.

### Status: downgraded to low risk — PNI is silent on non-ferrous metal

Checked against the **RM3100 & RM2100 Sensor Suite User Manual, Doc 1017252 R07**,
section 4.2.2 *Local Magnetic Field Considerations*. PNI lists only three hazards:

> large electric currents, large masses of **ferrous** material, or devices incorporating
> permanent magnets, such as speakers and electric motors

**Nothing about eddy currents or non-ferrous conductors.** A keyword search of the whole
manual for *eddy / aluminium / conductive / metal / enclosure / housing / mount* returns
nothing relevant.

That is weak but real evidence against the concern: the RM3100 ships in drone compasses,
marine and space applications, which routinely sit inside aluminium and CFRP structures.
PNI would very likely document it if aluminium proximity shifted gain materially.

**Revised position:**
- The hybrid housing (non-conductive nose) remains the **preferred** design — it is cheap
  insurance and the nose is a non-structural cover either way.
- But it is **no longer a blocker.** A one-piece aluminium chassis is an acceptable risk
  if it simplifies manufacture, provided H1 is tested at Rev B bring-up.
- Keeping ≥10 mm aluminium clearance remains sensible regardless.

PNI also documents the exact test method, should you want the number (manual §4.2.2):
place the eval board on a firm surface, gradually bring the component toward it, and note
when the field starts to change — holding board orientation constant throughout. Without
an eval board, bring the component toward one coil and watch the MagI2C reading.

### Related findings from the same manual

| Finding | Consequence |
|---|---|
| *"Position the sensor coils as close to each other as possible"* to minimise gradient error | **Within** a triad, group the three coils tightly. Separating the three *triads* for gradiometry is consistent with this — each triad still samples one clean field point |
| *"Keep capacitors, especially tantalum capacitors, far away from the sensor coils"* | **Corrects earlier guidance in this repo.** Decouple at the MagI2C, not at the coils |
| MagI2C within 10 cm of its coils; the two ASIC→coil traces as close together as possible | Trace-pair routing requirement |
| **AVDD may be switched off between measurements**; registers retained on DVDD | Per-triad AVDD gating gives power saving, time-multiplexing and de-energised idle coils in one mechanism |
| DVDD on whenever AVDD is on; DVDD first or simultaneous; AVDD within 0.1 V of DVDD | **Power-sequencing requirement** — needs a load switch or sequencer per MagI2C |
| AVSS/DVSS within 0.1 V; may share a plane if clean | Single ground plane is acceptable |
| Linear regime **±200 µT** (Earth ~50 µT) | 4× headroom |
| Sen-Z-f **polarity indicator is critical**; Sen-XY-f is insensitive to it | Footprint and placement care |
| Sen-Z-f **may be bottom-mounted** with ZDRVN/ZDRVP swapped | Option for managing its 6.35 mm height |
| **Halide-free solder paste required** for the coils, reflow *and* cleaning | Tell the assembler — easy to miss, and non-negotiable |
| Do **not** solder the MagI2C's exposed centre die pad | Footprint detail |
| MSL: Sen-XY-f / Sen-Z-f level 1, MagI2C MLF level 3 | Handling and bake-out |
| 5 mil stencil, paste area much smaller than pad, to reduce tilt and misalignment | The SEN-Z tilt mitigation, confirmed |
| Reflow per J-STD-020, peak <260 °C; opposite side may be wave-soldered afterwards | Process window |

### The housing requirement, corrected

An earlier version of this section argued from θ ≈ 2δ/L that a printed housing "caps you
at ~0.25°". **That argument was inconsistent with H12.** The 2δ/L analysis describes a
*removable* board relocated by dowels. With the board **bonded and non-removable** (H12)
and the sensor→sight-axis transform **calibrated** against a known azimuth (surveyed
baseline or solar sighting), dowel clearance is irrelevant — the board sits wherever it
sits, calibration measures that, and only *subsequent movement* matters.

So the real requirement is **dimensional stability**, not locating precision:

| Property | Machined aluminium | POM / PEEK | Printed polymer |
|---|---|---|---|
| CTE | **~23 ppm/°C** | ~100 ppm/°C | 50–100+ ppm/°C, anisotropic |
| Creep / moisture uptake | negligible | low | **significant** — SLA creeps, FDM absorbs water |
| Thermal mass, gradient spreading | **high** | low | low |

Uniform expansion over a 20 °C transient is a *scale* change and harmless; what hurts is
*asymmetric* movement, which polymers do and aluminium essentially does not. Aluminium
also spreads thermal gradients across the accelerometers — directly attacking the
budget's largest term. **Machined aluminium remains the recommendation**, but on
stability and thermal grounds, not the 2δ/L number.

**Two acceptable paths — and JLCPCB fabrication decides between them:**

1. **Machined chassis + precision dowels** — precision *and* stability. **Not available as
   drawn with JLCPCB-drilled holes**: hole position tolerance is ±0.075 mm and NPTH
   diameter ±0.05–0.08 mm, so the board-side holes alone give δ ≈ 0.1 mm radial and
   θ ≈ 0.26° at L = 44 mm. To use this path the PCB holes must be **drilled undersize at
   fab and match-drilled or reamed in the housing fixture** after assembly. Extra process;
   only worth it if the module must be removable.
2. **Stable housing + bonded board + calibrated transform** — **the default with JLCPCB.**
   Precision comes from calibration; the housing need only not move. The chassis dowels
   become assembly aids, not locators. A printed housing is *possible* on this path if
   stability is demonstrated over temperature and humidity; that demonstration is real
   work, and polymers make it harder. Machined aluminium is still recommended for
   stability and thermal mass.

**Where 2δ/L still applies:** if the module is ever **removed and refitted** (service,
carrier swap), dowel precision sets the repeatability — and H12 says recalibrate anyway.

| Fit | δ (radial) | θ at L = 43 mm |
|---|---|---|
| Reamed + ground dowel (H7/h6) | 0.006 mm | 0.014° |
| Machined, loose | 0.02 mm | 0.047° |
| Printed hole | ~0.15 mm | 0.40° |

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

## Manufacturing at JLCPCB

The board is drawn by hand and fabricated **and assembled** at JLCPCB. These are the fab's
constraints treated as design inputs, verified Nov 2026 — recheck the live quote tool at
order time; they move.

### Decisions this forces

| Item | JLCPCB reality | Consequence |
|---|---|---|
| **Surface finish** | 6-layer: **OSP or ENIG only**. No HASL (planarity), no immersion silver | **OSP.** Non-metallic, flattest for the 0.4 mm MLF, no surcharge. Order fab + assembly together (OSP shelf life ~6 months, bare copper on rework) |
| **Stackup** | **JLC06161H-3313**: 35 µm outer, **15.2 µm (0.5 oz) inner**; 3313 0.099 / core 0.55 / **2116 0.109** / core 0.55 / 3313 0.099 mm | Rev A's intent, confirmed. In2/In3 (L3/L4) sit across the 0.109 mm 2116 — the tight plane pair survives. Inner copper is 0.5 oz, not 1 oz |
| **Assembly tier** | Economic honours no process requests; 6-layer + fine pitch + special paste + consignment | **Standard PCBA** |
| **Datum** | Hole position ±0.075 mm; NPTH ±0.05–0.08 mm | PCB holes cannot be precision locators → **bond + calibrate** (housing path 2). See [H2](#housing-and-system-requirements) |
| **Edge clearance** | Component body ≥ **2.5 mm** from board edge | Floorplan used 2 mm: A–B becomes 27 mm, nothing else changes |
| **Footprint density** | IPC-7351B **medium or low** only | Courtyards accordingly; PNI's reduced-paste apertures go on the paste layer in your footprints |

### Design rules (6-layer)

| Rule | JLCPCB | Use |
|---|---|---|
| Min trace / space | 0.09 mm | 0.15 mm |
| Via drill / diameter | 0.15 / 0.25 mm | 0.3 / 0.5 mm (Rev A) |
| Drill size tolerance | +0.13 / −0.08 mm | — |
| Hole position tolerance | ±0.075 mm | drives the datum decision |
| SMD pad-to-pad (different nets) | 0.15 mm | — |
| Via / NPTH to track | 0.2 mm | — |
| Annular ring | via 0.05 (0.075 rec.), other 0.13, PTH 0.3 mm | — |
| Outline | ±0.1 mm precision / ±0.2 regular | request precision |

### KiCad Board Setup — values to enter

Ready for transcription into **Board Setup** (or a Konnect `set_design_rules` /
stackup call). Do this in the project Rev B is actually drawn in — board setup lives in
the `.kicad_pcb`/`.kicad_pro`, so a fresh project does not inherit it.

**Design Rules → Constraints** (fixes Rev A's `0.0` globals)

| Field | Value | Note |
|---|---|---|
| Minimum clearance | **0.15 mm** | JLCPCB 0.09; netclass 0.2 still governs normal routing |
| Minimum track width | **0.15 mm** | JLCPCB 0.09 |
| Minimum via diameter | **0.45 mm** | Rev A's smallest; JLCPCB 0.25 |
| Minimum via hole | **0.3 mm** | JLCPCB 0.15, but ≤0.25 mm drills carry a surcharge |
| Minimum annular width | 0.075 mm | keep Rev A; JLCPCB via 0.05 min / 0.075 rec. |
| Minimum through hole | 0.3 mm | keep |
| Copper to hole clearance | 0.25 mm | keep |
| Copper to edge clearance | 0.25 mm | keep |
| Hole to hole clearance | 0.25 mm | keep — but **check PTH-to-PTH ≥ 0.45 mm by eye**; KiCad has one field, JLCPCB has two limits (via 0.2, PTH pad 0.45) |

**Net Classes** — keep Rev A's, they are JLCPCB-safe

| Class | Clearance | Track | Via dia / drill |
|---|---|---|---|
| Default | 0.2 | 0.25 | 0.6 / 0.3 |
| Power | 0.2 | 0.7 | 1.0 / 0.5 |

**Physical Stackup** — 6 copper layers, 1.6 mm, **JLC06161H-3313**

| Layer | Type | Material | Thickness | Note |
|---|---|---|---|---|
| F.Cu | copper | — | **0.035** | 1 oz |
| — | prepreg | **3313**, εr 4.1 | **0.0994** | was 0.1 |
| In1.Cu | copper | — | **0.0152** | **0.5 oz** — was 0.035 |
| — | core | FR4 | **0.55** | was 0.535 |
| In2.Cu | copper | — | **0.0152** | 0.5 oz |
| — | prepreg | **2116** | **0.1088** | the In2/In3 plane pair gap; was 0.1 |
| In3.Cu | copper | — | **0.0152** | 0.5 oz |
| — | core | FR4 | **0.55** | |
| In4.Cu | copper | — | **0.0152** | 0.5 oz |
| — | prepreg | **3313**, εr 4.1 | **0.0994** | |
| B.Cu | copper | — | **0.035** | 1 oz |

Sums to ~1.54 mm copper + dielectric, ~1.56 mm with mask — JLCPCB's "1.6 mm" nominal.

**Copper finish: `OSP`** (KiCad's dropdown has it; Rev A is `None`). Solder mask colour
and silkscreen as you like — neither is magnetically relevant.

### Part sourcing status

| Part | JLCPCB / LCSC | Status (Nov 2026) | Action |
|---|---|---|---|
| **RM3100** (coils + MagI2C) | LCSC C464558 | Listed; **JLCPCB assembly status unconfirmed** — part page unreadable. Odd-form 6.35 mm solenoid risks *"large and difficult to process"* rejection | Check in parts library / pre-order. Fallback: **consign** |
| **SCA3300-D01-1** | JLCPCB C2655105 | **364 in stock**, SMT, Economic & Standard. *"PCB assembly fixture needed to protect and support this part"* | Order through JLCPCB; expect a fixture charge |
| **STM32U585** | LCSC | Non-Q LQFP-64 RIT6 **out of stock**; ZIT6 ×3; Q variants VIT6Q ×60, CIU6Q ×10 | Consign RIT6 from ST eStore, or VIT6Q wired LDO-only (Phase 4) |
| **ICM-42688-P** | JLCPCB C54308212 | **Tokmas clone only**; genuine TDK not stocked | Role is a motion flag, so the clone is tolerable — but **consign genuine** since you are consigning anyway |
| **TPS7A2033PDBVR** | LCSC C2862740 | 153 k in stock, Extended part | Order; accept the loading fee |
| Pt1000 non-magnetic, ESD array, TVS | — | Verify at order time | — |

### Consignment

Overseas parts ship to JLCPCB's **Hong Kong warehouse** under the compliant consignment
flow. Only parts with a JLC number (or engineer approval for a new part) can be consigned;
a consigned part number **cannot be mixed** with the JLCPCB-stocked same part in one order;
~**$45 customs handling** per parcel (<5000 CNY), duties on you; packaging instructions
appear in the order flow and are strict. Consigned parts are not tested. Budget one
consignment parcel per build covering the RM3100 sets, genuine gyro, and any non-stock MCU.

### Solder paste

No halide-free option exists as a checkbox. **Standard PCBA**, state **ROL0 / halide-free
required for PNI coils** in the PCBA Remark, and open a ticket before production for
written confirmation. Their Type 4 lead-free no-clean paste is *probably* ROL0; do not
assume.

---

## Phase 2 — Parts

- [ ] **Order RM3100 parts.** Primary: [cavedevice.com](https://cavedevice.com/products/magnetometer-sensor-rm3100-set-by-pni)
      — €20/kit, individual parts from €4, **no MOQ**, EU shipping. Confirm stock first.
- [ ] **Check whether LCSC C464558 is available for JLCPCB assembly** — in the JLCPCB
      parts library or via the "pre-order" flow; the public part page is a JS shell and
      could not be read. If yes this is significant: it hands the SEN-Z placement-tilt risk
      to a professional pick-and-place. **Risk:** JLCPCB's T&Cs reject *"parts that are
      large and difficult to process"*, and a 6.35 mm solenoid is an odd-form part — expect
      hand placement or refusal. **Fallback:** consign the coils and ASICs (see
      [Manufacturing at JLCPCB](#manufacturing-at-jlcpcb)).
- [ ] Order for **3+ board builds**, not 1. Murata lead times are the schedule risk, and
      multiple units let you characterise part-to-part spread for the error budget.
- [ ] Source **non-magnetic MLCCs** (Cu/Ag termination) **only where a capacitor must sit
      within ~10 mm of a coil.** PNI's rule is to keep capacitors — tantalums especially —
      *away* from the coils, so decoupling belongs at the MagI2C. Standard MLCCs are fine
      there.
- [ ] Confirm the assembler can use **halide-free solder paste** — PNI requires it for the
      sensor coils, for both reflow and cleaning. Non-negotiable and easy to miss.
      **At JLCPCB there is no checkbox for this.** Their line uses Type 4 lead-free
      paste; standard no-clean is usually ROL0 (halide-free) but is not guaranteed. Route:
      **Standard PCBA tier only** (Economic honours no process requests), state the ROL0
      requirement in the **PCBA Remark** field, and open a support ticket before
      production to get written confirmation.
- [ ] **Pt1000 ×3 in a verified non-magnetic package.** SMD thin-film RTDs typically carry
      Ni-barrier terminations like MLCCs — the platinum element is clean, the package may
      not be. Leaded element or confirmed Cu/Ag terminations.
- [ ] **TMP117: at most one**, as a board-level thermal-soak reference. The SCA3300's
      internal die sensor (18.9 LSB/°C, 0.053 °C) is the correct source for accelerometer
      compensation, so the two per-accel TMP117s are dropped.
- [ ] **USB-C ESD array** (TPD4E05U06 class) and **TVS on 5 V in** — previously missing.
- [ ] Source **non-magnetic M2 hardware** — brass, titanium or nylon.
- [ ] **Screen candidate hardware and materials with a smartphone magnetometer.** Any app
      exposing the raw sensor works; A/B against a known-non-magnetic reference rather
      than trusting absolute values, and work away from the phone's own MagSafe ring.
      Resolution is ~0.1–0.5 µT against a 35 nT budget, so it answers *"is this steel?"*
      and nothing finer — enough for dowels (H3), cells (H5) and tripod inserts (H11),
      but **not** for a single 0603's Ni barrier termination. Cannot resolve
      [H1](#h1--the-eddy-current-problem).
- [ ] Consider buying a **Cavway X1 as a reference instrument** (±0.4° / ±0.2°) for
      cross-checking. Note: never place two instruments side by side — they interfere
      magnetically. Shoot the same legs sequentially.

Alternative suppliers: Solsta UK (authorized, post-Brexit customs), PNI direct
(`sales@pnisensor.com`, $250/10 suites). **Avoid brokers** — counterfeit risk.

---

## Phase 3 — Library (highest-risk step)

With no prototype, footprint correctness is only proven after fab. Treat accordingly.

**Done 2026-11-01** — project library `attinav` (`components/attinav/`), registered in both
project tables. Only the physical-part check below remains.

| Part | Symbol | Footprint | Source / notes |
|---|---|---|---|
| Sen-XY-f | `attinav:RM3100_Sen-XY-f` | `attinav:PNI_Sen-XY-f` | UM Fig 3-3. **Lands at 5.80 c-c, paste at 5.30 c-c** — the 5.30 dimension is the *termination* pitch; lands sit 0.25 outward. Paste 0.60×1.20 on separate F.Paste-only pads |
| Sen-Z-f | `attinav:RM3100_Sen-Z-f` | `attinav:PNI_Sen-Z-f` | UM Fig 3-6. Lands 2.40×1.95, gap 0.40; paste 1.66×1.35 flush to the inner edge. **Pad 1 = polarity end; orient per UM Fig 4-4 (NED)** |
| MagI2C | `attinav:RM3100_MagI2C` (28 pins, Table 4-1) | `attinav:PNI_MagI2C_MLF-28_4x4mm_P0.4mm_NoEP` | Stock QFN-28 P0.4 lead geometry (0.25×0.825); EP omitted per UM Fig 3-7. Pin 1 top-left, CCW — matches PNI |
| SCA3300-D01 | `attinav:SCA3300-D01` (Table 6) | `attinav:Murata_SCA3300-D01` | Doc 3165 Fig 10: 0.61×0.98, pitch 1.0, rows 7.87 apart |
| ICM-42688-P | `attinav:ICM-42688-P` (DS v1.9 Table 10) | **stock** `Package_LGA:LGA-14_3x2.5mm_P0.5mm_LayoutBorder3x4y` | 4-3-4-3 CCW matches TDK D1=1.5 / E1=1.0; no centre pad (TDK: must not be soldered). **Verify the pin-1 corner against DS Fig 5 at placement** |
| Pt1000 | **stock** `Sensor_Temperature:PT1000` | stock 0603/0805 per the chosen non-magnetic package | — |
| STM32U585, TPS7A20, TMP117 | stock `STM32U585RITx` / `VITxQ`, `TPS7A20xxxDBV`, `TMP117xxDRV` | stock LQFP-64/-100, SOT-23-5, WSON-6 1.5×1.5 | verified present in the installed libraries |

Verified three ways: Konnect readback of pads and pins, `kicad-cli fp/sym export` (KiCad's own
parser accepts every file), and rendered inspection of all four footprints. Courtyards came
out pad-derived from `create_footprint` (SCA3300 was **1.5 mm short per side**) and were
corrected to body-or-pads + 0.25. Known cosmetic: silk boxes on Sen-Z-f / SCA3300 follow
the pad envelope, not the body; MagI2C symbol side pins read bottom-up.

Konnect's library search covers only the three Rev A project libraries — the stock KiCad
libs are not in the global table it reads. Harmless in the GUI; matters for Konnect-driven
placement later.

- [x] SEN-XY symbol + footprint (PN 13104)
- [x] **SEN-Z symbol + footprint (PN 13101) — precision-critical.** Pad, paste and mask
      geometry straight from the PNI manual: 5 mil stencil, paste area much smaller than
      pad. Coil tilt → axis misalignment → azimuth error.
- [x] MagI2C symbol + footprint (PN 13156, 28-pin MLF 4×4)
- [x] SCA3300-D01 symbol + footprint (12-pin pre-molded, 8.6 × 7.6 × 3.3 mm)
- [x] Pt1000 symbol — footprint per the chosen **non-magnetic package** (leaded or verified
      Cu/Ag terminations), not a generic 0603
- [x] Verify the chosen STM32U585 variant in `MCU_ST_STM32U5` (see the stock note in
      Phase 4); check ICM-42688-P, the ESD array and TVS; TMP117 only if the single
      board-reference unit is kept
- [ ] **Verify every footprint against the physical parts in hand** before releasing
      gerbers — 1:1 paper printouts, parts laid on top, calipers on the MLF. This is the
      step a prototype would have caught for you.

---

## Phase 4 — Schematic

- [ ] **Go hierarchical.** Rev A is one flat sheet with 113 symbols.
- [ ] Sheets: power, MCU, magnetometer (**instantiated 3×**), accel, interface
- [ ] Read the PNI manual for REXT value and cycle-count configuration
- [ ] Pin budget check before committing to the package. **Stock note (LCSC, Nov 2026):**
      the non-Q LQFP-64 **RIT6 is out of stock**; non-Q LQFP-144 ZIT6 has 3; Q variants
      LQFP-100 VIT6Q (60) and QFN-48 CIU6Q (10) are available. Options: consign RIT6 from
      the ST eStore (in stock), or take **VIT6Q (LQFP-100, 14 × 14 mm) wired LDO-only** —
      the board has room. QFN-48 is pin-marginal for 3× CS/DRDY + 2× CS + UART + USB + SWD
- [ ] **Design for bring-up** — needed now that there is no prototype:
  - [ ] **USB-C on the U585's USB OTG** — streaming raw counts during calibration
        development is far faster than BLE through a carrier that does not exist
  - [ ] **Standalone bench power** — jumper or header so the board runs without a carrier
  - [ ] **Test points**: each supply rail, SPI bus, DRDY per MagI2C
  - [ ] **Raw-count path in the protocol from day one** — calibration needs it
- [ ] **Power path:** 5 V in → TVS → low-noise LDO; **the LDO's EN pin is `/EN`**. Drop
      Rev A's PMOS gate (Q1) and its exposed-gate ESD problem. Split **AVDD/DVDD with a
      ferrite bead from the same 3.3 V rail** — the MagI2C's 0.1 V tracking rule forbids
      separate regulators. Per-triad AVDD via load switches (see H1 findings).
- [ ] **No crystal, no RTC.** USB FS on HSI48 + CRS; SPI on the internal oscillator; the
      carrier sends wall-clock time at power-up and the module timestamps relatively.
- [ ] **STM32U585 in LDO mode, no SMPS inductor.** Prefer a non-Q part. If a Q variant is
      used, wire it LDO-only per **AN5373**: VDDSMPS, VLXSMPS and VSSSMPS → VSS; each VDD11
      pin → 2.2 µF + 100 nF to GND; **no 2.2 µH inductor**; leave `PWR_CR3.REGSEL = 0`
      (LDO is the reset default). The no-buck rule is about the inductor on the board,
      not the silicon.

---

## Phase 5 — Layout, in this order

1. [ ] Board outline + mounting/datum features from the enclosure
2. [ ] **Place the 3 coil clusters first, maximally separated.** Everything works around them
3. [ ] Per cluster: **the three coils of a triad as close together as possible** (PNI:
       minimises gradient error across the triad), MagI2C within 10 cm, the two
       ASIC→coil traces routed as a tight pair, and **decoupling at the ASIC — capacitors
       kept away from the coils**. **Orient each triad with its MagI2C on the chassis (+x)
       side** — keeps every coil ≥10 mm from the aluminium and points all three SPI runs at
       the MCU
4. [ ] Sen-Z-f: **polarity indicator orientation is critical** (Sen-XY-f is insensitive).
       Follow the NED layout in manual Figure 4-4. Sen-Z-f **may be bottom-mounted** with
       ZDRVN/ZDRVP swapped — an option for managing its 6.35 mm height
5. [ ] Accelerometers near the mechanical datum, close together (same temperature, same
       stress). **Orient each SCA3300 so the sight axis lies along its X or Y axis, never Z.**
       Offset temperature dependency is ±10 mg on X/Y but **±15 mg on Z**, and inclination
       error is dominated by the axis along the sight line — a free 1.5× on the budget's
       largest term. **Rotate the pair 90° about the sight axis** relative to each other:
       on steep shots the perpendicular axes dominate and one must be Z, so this gives every
       geometry one sensor with good axes in the critical direction and turns common-mode
       bias into a visible disagreement. Use the **SCA3300 internal die sensor** for its own
       temperature compensation
6. [ ] MCU, LDO, interface last
7. [ ] Planes: **no pour keepout this time.** Keep GND under the coils, but route so no
       unrelated return current passes beneath them
8. [ ] Keep Rev A's tight In2(+3.3V)/In3(GND) 0.1 mm plane pair — it was a good decision

---

## Phase 6 — Verification

- [x] Set real design rules — fix Rev A's `min_clearance: 0.0` and `min_track_width: 0.0`
      to JLCPCB's 6-layer capability (table in
      [Manufacturing at JLCPCB](#manufacturing-at-jlcpcb)). Suggested: clearance 0.15,
      track 0.15, via 0.3/0.5 as Rev A, NPTH ±0.08, hole position ±0.075
- [x] **Set `copper_finish` to OSP** in the board stackup. Rev A has `"None"`, which lets
      the fab choose ENIG and its 3–6 µm of ferromagnetic nickel. Select
      **JLC06161H-3313** as the stackup
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
- [ ] Absolute verification method: surveyed baseline or solar sighting. A smartphone
      supplies what both need with no app development — position → IGRF/WMM expected
      \|B\|, dip and declination, plus precise time and position for solar azimuth

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
gradiometric *measurement* of nearby iron rather than mere detection (paired with IGRF
magnitude and dip checks for distant anomalies, which gradiometry cannot see); open hardware resilience
in an ecosystem that is one person deep twice over (DistoX2 discontinued, Cavway X1 a
single developer); and temperature-compensated calibration, which appears unexploited
commercially. Raw accuracy is the *weakest* justification — the instrument stops being
the limiting factor somewhere around 0.2–0.3°.
