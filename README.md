# Attinav

**Work in progress.** A precision attitude sensor module for cave survey: static "point and
shoot" azimuth and inclination to 0.1°, with the redundancy to tell a good shot from a disturbed
one. This repository covers the sensor module only. The carrier (display, battery, BLE), the laser
and the housing are separate.

## Status

| part | state |
|---|---|
| Hardware Rev B | schematic complete, PCB WIP |
| Firmware | builds, lints and tests clean, never run on hardware |
| Desktop client | builds, lints and tests clean against the built-in simulator, not yet run against hardware |

## Hardware (Rev B)

- STM32U575RGT6 (Cortex-M33, 160 MHz, 1 MiB flash, 768 KiB SRAM)
- 3 × PNI RM3100 magneto-inductive triads (gradiometer: nearby iron shows as disagreement)
- 2 × Murata SCA3300-D01 inclinometer-grade accelerometers
- TDK ICM-42688-P IMU as a motion gate during the integration window
- Pt1000 coil thermometers for temperature-compensated calibration
- USB-C to the host, 8-pin FFC (UART, power, `QUIET`) to the carrier

Design notes and error budget: `hardware/kicad/README.md`. Plan and decisions:
`hardware/kicad/ROADMAP.md`.

## Software

- `software/firmware` — Rust / Embassy firmware: shot engine, on-device Heeb/DistoX
  calibration (14 × 4 shots, up to four temperature sets), quality flags, binary protocol over
  USB CDC and the carrier UART, OTA update with rollback. `README.md`, `PROTOCOL.md`, `OTA.md`.
- `software/client` — Tauri v2 desktop client (React, Redux, Tailwind) to test, calibrate,
  visualise, configure and update the module. Includes a protocol-level simulator, so it runs
  without hardware. `README.md`.

Both projects have a Nix dev shell and a `Makefile`, the root `Makefile` delegates to them:

```sh
make help                            # targets
make check                           # formatting, lints, tests and builds of both projects
make -C software/firmware help
make -C software/client help
```

CI (`.github/workflows/ci.yml`) runs the same targets on every push and pull request, inside the
same Nix shells, plus actionlint and zizmor over the workflows (`make workflows`, root flake).

## License

- Software: AGPL-3.0-only
- Hardware design files: CERN-OHL-S-2.0
