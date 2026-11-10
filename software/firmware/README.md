# Attinav Rev B firmware

Rust / [Embassy](https://embassy.dev) firmware for the Attinav cave-survey module
(STM32U575RGT6, three PNI RM3100 magnetometer triads, two Murata SCA3300 accelerometers, one TDK
ICM-42688-P motion gate, Pt1000 coil thermometers). It takes "point and shoot" station
measurements — azimuth, inclination, roll with quality flags — serves them to a host over USB or
the carrier UART, calibrates itself on the device, and updates itself over the same links.

Hardware design notes: `hardware/kicad/README.md` and `ROADMAP.md`. Wire protocol: `PROTOCOL.md`.
Over-the-air update: `OTA.md`.

## Layout

```
software/firmware
├── attinav-core      no_std, host-tested: maths, survey geometry, calibration model, LM solver,
│                     quality flags, protocol codec, flash record framing
├── attinav-sensors   async embedded-hal drivers: rm3100, sca3300, icm42688 (host-tested with mocks)
├── attinav-fw        the application (Embassy, STM32U575)
├── attinav-boot      embassy-boot bootloader (swap + rollback)
├── flake.nix         development shell (toolchain, probe-rs, flip-link, binutils, dfu-util, picocom)
├── rust-toolchain.toml
└── .cargo/config.toml  default target, probe-rs runner, cargo aliases
```

`attinav-core` and `attinav-sensors` have no dependency on the MCU and are reused as-is by the
desktop client (`software/client`, feature `std` + `serde`).

## Building

`make help` lists the common operations (`make check` runs formatting, lints, tests and both
builds; `make flash`, `make bin`, `make size`, …). Every target enters the Nix dev shell by
itself when run from outside it. The underlying commands:

The flake pins the toolchain and every tool. The sources are untracked in git until the first
commit, hence `path:`:

```sh
cd software/firmware
nix develop path:$PWD           # or `direnv allow` (.envrc)

cargo build --release           # firmware for thumbv8m.main-none-eabihf
cargo run --release             # flash through the Tag-Connect SWD pads and stream defmt logs
cargo boot-flash                # build + flash the bootloader (once per board)
cargo fw-bin                    # raw image for an OTA update → target/attinav-fw.bin
cargo fw-size                   # section sizes

cargo test-host                 # unit tests of core + sensors on the host (51 tests)
cargo lint                      # clippy, pedantic + nursery, -D warnings, firmware target
cargo lint-host                 # same for the libraries incl. their tests
```

Release profile: `opt-level = "s"`, fat LTO, one codegen unit, debug info kept for defmt (never
reaches the flash image). Current size: ~138 KiB text, ~100 KiB static RAM (frame queues,
decoders, record buffers) out of 256 KiB used.

## Runtime architecture

Single-core thread-mode executor, three tasks:

| task        | module      | role                                                                 |
|-------------|-------------|----------------------------------------------------------------------|
| `main`      | `engine`    | owns the sensors, flash state and protocol semantics; one cooperative loop |
| `usb_task`  | `link`      | CDC-ACM; COBS decoder → `INBOUND` channel, `OUT_USB` channel → bulk IN |
| `uart_task` | `link`      | LPUART1 with DMA ring buffer; same framing                           |

Boot: bootloader (watchdog 32 s, swap/revert) → clocks (HSI → PLL 160 MHz, HSI48 + CRS for USB) →
peripherals, watchdog re-armed at 16 s, USB →
load config + calibration from flash (defaults when absent) → start the UART at the stored baud
rate → power the triads, probe every sensor → confirm the image to the bootloader → serve requests.

Request handling is strictly sequential. Long operations keep the device responsive:

* a **shot** interleaves polled RM3100 measurements (data-ready on EXTI) with SCA3300 / IMU reads
  on a ticker, checks the inbound queue between magnetometer samples (`Abort` and `Ping` are
  honoured, anything else gets `Busy`) and pets the watchdog every sample;
* a **solve** runs one Levenberg–Marquardt iteration per step and yields to the link tasks between
  steps (the Cortex-M33 has no double-precision FPU; a full 56-shot solve takes seconds);
* **streaming** is driven by a ticker inside the main `select`, so requests are still served.

Unsolicited frames are dropped on a link whose outbound queue is full; replies wait for room.

## Shot pipeline

1. Optional cycle-count change on the triads, Pt1000 + VDDA reading.
2. Assert `QUIET` (open drain to the carrier: no BLE, backlight fixed, converters steady), wait
   `quiet_lead_ms`.
3. Acquire `mag_samples` single measurements from all three triads in lock-step while the two
   accelerometers and the IMU are read at `acc_rate_hz`. Means and sample standard deviations
   accumulate in Welford form; the IMU gyro peak is kept.
4. Release `QUIET`, evaluate (`attinav_core::quality::evaluate`): temperature-interpolated affine
   models per sensor, triad agreement (the closest pair is fused when one disagrees), norm and dip
   checks against the stored or external reference, noise, motion → `ShotResult` + flags.

## Calibration

Heeb / DistoX procedure: 14 directions × 4 roll angles, no reference direction needed. The solver
(`attinav_core::solver`) fits accelerometer 1 and triad 1 jointly (offsets, full matrices, dip;
the roll axis of each group defines the laser axis, so sensor-to-laser tilts are calibrated),
rejects outlier shots, then fits each remaining sensor against that pair. Synthetic instruments
calibrate to the sensor noise floor (≈ 0.03° azimuth, 0.006° inclination RMS; see
`attinav-core/examples/montecarlo.rs`). Up to four temperature sets are stored; shots interpolate between the two sets
bracketing the sensor's current temperature. The whole calibration plus the configuration lives in
two alternating flash slots with a CRC-32 and a sequence number, so a reset during a write never
loses the previous record.

## Non-volatile state and updates

See `OTA.md` for the flash map, the update procedure and the rollback guarantees. In short: the
application stages the new image into bank 2, verifies its CRC from flash, marks it and resets;
the bootloader swaps and reverts if the new image never confirms.

Flash ECC: a quad-word whose programming was interrupted raises an NMI when read. Both the
application and the bootloader clear the flag; the application treats the affected slot or staged
image as invalid instead of resetting in a loop.

## Logging

defmt over RTT (`cargo run --release`, `DEFMT_LOG=debug` in the shell). `info` for state
transitions, `warn` for every sensor or flash problem, `debug` for dropped frames. Nothing in the
shot path allocates or panics on bad input; the only panics left are `StaticCell` double-inits
(programming errors) and `unwrap!` on task spawning.

## Conventions

* Pedantic clippy is enforced; opt-outs are workspace-wide in `Cargo.toml` with the reason, or
  per site with `#[allow(..., reason = "...")]`.
* Every public item is documented (`missing_docs = warn`).
* Hardware facts (pins, clocks, timings) live in one place each: `board.rs` for pins and clocks,
  the drivers for register maps and timings, `memory.x` for the flash map.
* The protocol document is normative; add a round-trip test for any new message.

## Status

Builds, lints and tests clean (2026-11-10). Not yet run on hardware: Rev B boards are not back
from fabrication. Things to verify on the first board, in order: USB enumeration and defmt output,
sensor identity bits in `Info.sensors_ok`, Pt1000 readings against a thermometer, raw streaming
noise at cycle count 400, a shot with `NO_QUIET`, then a full calibration.
