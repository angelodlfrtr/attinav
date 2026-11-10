# Attinav desktop client

Tauri v2 desktop application to test, calibrate, visualise, configure and update an Attinav
survey module over USB or a UART adapter. React 19 + Redux Toolkit + Tailwind v4 in the
window, Rust behind it. The Rust side links the firmware's own `attinav-core` crate, so the
protocol codec, the calibration model and the solver are the same code the module runs.

It ships with a **simulated device** behind the real wire protocol: every page works without
hardware, and a full 56-shot calibration against the simulator recovers the simulated sensor
models with the production solver.

## Layout

```
software/client
├── flake.nix              dev shell: Rust toolchain, cargo-tauri, Node 22, WebKitGTK and friends
├── package.json           frontend (Vite, React, Redux Toolkit, Tailwind, TypeScript, ESLint)
├── src/                   frontend
│   ├── protocol.ts        TypeScript mirror of the wire messages (+ flags, names, helpers)
│   ├── api.ts             typed Tauri commands and events; `expect()` / `ack()` request helpers
│   ├── listeners.ts       Tauri events → Redux store / stream ring buffers
│   ├── streamBuffers.ts   ring buffers for raw streaming (outside Redux, read by the canvas chart)
│   ├── store/             one slice per concern (link, device, shots, stream, calibration, …)
│   └── components/        App shell, connection bar, shared widgets, charts, pages
└── src-tauri/             Rust
    ├── src/transport.rs   serial port threads, port enumeration
    ├── src/link.rs        framing, sequence numbers, reply collection, `device:message` events
    ├── src/sim.rs         the simulated device
    ├── src/ota.rs         firmware update procedure (host side of OTA.md)
    ├── src/commands.rs    the Tauri command surface
    └── tests/simulator.rs end-to-end tests through the protocol
```

## Building and running

`make help` lists the common operations: `make dev` (hot reload), `make build`, `make check`
(formatting, tsc, eslint, clippy, tests, build), `make bundle`. Every target enters the Nix dev
shell by itself when run from outside it. The underlying commands:

```sh
cd software/client
nix develop path:$PWD        # or `direnv allow`
npm install                  # once

cargo tauri dev              # run with hot reload (Vite on :1420)
cargo tauri build            # release bundle under src-tauri/target/release/bundle
cargo tauri build --no-bundle

npm run check                # tsc (strict) + eslint (typescript-eslint strict-type-checked)
(cd src-tauri && cargo lint && cargo test)   # clippy pedantic + unit and end-to-end tests
```

The flake pins the same Rust channel as the firmware (`rust-toolchain.toml`). Serial access on
Linux needs the user in the `dialout` (Debian) or `uucp` (Arch/NixOS) group, or a udev rule for
VID `1209` PID `0001`.

## Pages

| page          | what it does                                                                                 |
|---------------|----------------------------------------------------------------------------------------------|
| Dashboard     | `Info`: firmware, boot state, sensors answering, clock; set the clock; reset                 |
| Shots         | take shots (samples, cycle count, raw data, QUIET); compass / inclinometer; per-sensor raw statistics; history with CSV export |
| Stream        | raw samples from any sensor set at 1–500 Hz on canvas strip charts; temperatures and VDDA   |
| Calibration   | 14 × 4 grid driving `CalShot`, retake / remove, fetch buffered shots, solve on device, store, stored-set table and matrices, JSON export / import, clear, reload |
| Configuration | every `DeviceConfig` field with the firmware's validation rules; apply persists to flash     |
| Firmware      | pick a `.bin`, see size / CRC, run the OTA update with progress, boot-state guidance         |
| Console       | every frame with time, seq and summary; counters; send any `HostMessage` as JSON             |
| Simulator     | pose, site field, noise, motion, nearby iron, sensor faults (only when the simulator is connected) |

## How the pieces talk

* The frontend never parses bytes. It sends a `HostMessage` (JSON) through the `request`
  command and gets back the list of `DeviceMessage`s that answered it, collected by the Rust link
  layer according to the reply shapes in `PROTOCOL.md` (a shot with raw data is six frames, a
  shot-buffer dump is *n* frames plus an `Ack`, everything else is one frame).
* Every decoded frame, solicited or not, is also emitted as a `device:message` event. The
  listeners module routes it into the store (`Info`, `Config`, `CalInfo`, events, calibration
  shots) or into the stream ring buffers (`Sample`, `Temps`). Pages therefore stay in sync even
  for frames consumed by a request elsewhere.
* `device:link` announces connection and loss; `fw:progress` streams update progress.
* Timeouts are chosen by the caller: shots from the configured sample count and cycle count, a
  solve generously (minutes on the Cortex-M33 for 56 shots), everything else 3 s.

## Simulator

`src-tauri/src/sim.rs` implements the device side of the protocol: `Info`, configuration,
shots evaluated with `attinav_core::quality::evaluate`, streaming, the calibration session with
`attinav_core::solver::solve`, flash-less `CalStore`/`CalLoad`/`CalSet`, and the firmware update
state machine with a simulated reset. Each sensor has a distinct affine truth model (offsets,
gains, misalignments, the second accelerometer mounted at 90°), Gaussian noise, and the knobs on
the Simulator page: pose, field, dip, noise, motion (trips `MOTION`), nearby iron on triad 1
(trips `MAG_DISAGREE`), temperature, missing sensors. With `auto_pose_for_calibration` the
calibration shots take a well-spread pose per `(group, roll)`, so the whole procedure can be run
from the Calibration page in a minute.

## Status

Builds, lints and tests clean (2026-11-10): 3 unit tests and 6 end-to-end tests on the Rust side
(ping/info/config, shot shapes, busy/abort/stream, full calibration incl. `CalSet` round trip,
OTA incl. rejected image, timeouts/closure). Not yet exercised against hardware: Rev B boards are
not back from fabrication. The first things to try with a board: port appears as Attinav in the
connection bar, Dashboard shows all six sensor bits, Stream at 20 Hz shows plausible counts,
then a shot.
