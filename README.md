# Attinav (WIP)

An hardware module to implement AHRS algorithm.

**Work in progress**

## Harware

- STMicroelectronics STM32F401CE: Arm Cortex-M4, 512 Kbytes of Flash memory, 84 MHz CPU, 96 Kbytes of SRAM.
- Murata SCHA63T-K03-05: low noise 6-DOF XYZ-Axis Gyroscope and xyz-Axis Accelerometer.
- PNI RM3100: low noise geomagnetic sensor.
- Texas Instruments LM94021: external temperature sensor.
- Texas Instruments TPL5010: external hardware watchdog.

## License

This project is fully free and open-source.

- Software components are licensed under AGPL-3.0
- Hardware design files are licensed under CERN-OHL-S-2.0

## Software

A `Makefile` at the root delegates to both projects: `make check` runs every lint, test and build;
`make -C software/firmware help` and `make -C software/client help` list the per-project targets.

- `software/firmware` — Rust / Embassy firmware for the Rev B module (STM32U575): shot engine,
  on-device calibration, host protocol over USB CDC and the carrier UART, OTA update with
  rollback. See its `README.md`, `PROTOCOL.md` and `OTA.md`.
- `software/client` — Tauri v2 desktop client (React, Redux, Tailwind) to test, calibrate,
  visualise, configure and update the module; includes a protocol-level simulator so it runs
  without hardware. See its `README.md`.
