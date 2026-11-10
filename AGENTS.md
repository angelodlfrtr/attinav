# Working in this repository

Attinav is a precision attitude sensor module for cave survey: static "point and shoot" azimuth
and inclination to 0.1°, with enough redundancy (three magnetometer triads, two accelerometers,
a motion-gating IMU, coil thermometers) to tell a good shot from a disturbed one. The repository
holds the module's hardware (KiCad), its firmware (Rust / Embassy on an STM32U575) and a desktop
client (Tauri). The carrier, laser and housing are out of scope; the firmware exposes a protocol
for them. Everything is work in progress and no board has been powered on yet.

## Commands

Each software project has a Nix dev shell pinned by its own `flake.lock`; the Makefiles enter
the right shell themselves when run from outside it. CI runs exactly these targets.

```sh
make help                              # root targets (delegates to both projects)
make check                             # everything CI runs: workflows, both projects
make -C software/firmware help         # build, boot, bin, flash, test, lint, fmt, size, …
make -C software/client help           # deps, dev, build, bundle, lint, test, fmt, …
make workflows                         # actionlint + zizmor over .github/workflows
```

Run `make check` before considering a change done. The client job rebuilds the frontend before
clippy because `tauri::generate_context!` embeds `dist/` at compile time.

The firmware shell is entered with `path:` (`nix develop path:$PWD`) when files are untracked;
the Makefiles already do this.

## Layout

- `hardware/kicad/` — Rev B schematic and PCB. `README.md` there is the design rationale and
  error budget, `ROADMAP.md` the decisions and the assumed instrument zoning; both are the
  source of truth for *why* the hardware is the way it is. `stale_vias_to_delete.txt` lists
  vias a tool could not remove; the PCB still needs that manual cleanup and J1 routed.
- `software/firmware/` — Cargo workspace of four crates:
  - `attinav-core`: `no_std`, host-tested. Maths, survey geometry, calibration model and
    solver, quality flags, protocol codec, flash record framing. Nothing hardware-specific.
  - `attinav-sensors`: async `embedded-hal` drivers (RM3100, SCA3300, ICM-42688-P), mocked
    on the host.
  - `attinav-fw`: the application. `board.rs` is the only file that names a pin.
  - `attinav-boot`: embassy-boot bootloader with swap and rollback.
  - `PROTOCOL.md` is the normative wire format, `OTA.md` the update procedure and flash map.
- `software/client/` — Tauri v2 desktop client (React 19, Redux Toolkit, Tailwind v4). The
  Rust side links `attinav-core`, so it cannot drift from the wire format. `src-tauri/src/sim.rs`
  is a simulated device behind the real protocol; every page and the end-to-end tests run
  against it. `src/protocol.ts` mirrors the message types.
- `flake.nix` at the root only carries repository tooling (actionlint, zizmor, nixpkgs-fmt).

## Conventions

- Rust: pedantic and nursery clippy at `-D warnings`, `missing_docs` on, rustfmt at 120
  columns (`rustfmt.toml`). Opt out of a lint at the workspace level with the reason in
  `Cargo.toml`, or per site with `#[allow(..., reason = "...")]`; never silently.
- TypeScript: strict `tsc` and `typescript-eslint` strict-type-checked. Everything crossing the
  IPC boundary is cast once, in `src/api.ts`.
- The firmware shot path never allocates or panics on bad input. `StaticCell` is initialised
  once per static; anything called twice must own its buffers.
- Only engine-side code logs through `host_warn!`/`host_info!`; the link tasks never log through
  the links they drive.
- The protocol is versioned (`PROTOCOL_VERSION`); until a release exists, wire changes update
  `PROTOCOL.md`, the codec round-trip tests, `src/protocol.ts` and the simulator together.
- Commits: `area: imperative summary` (`firmware:`, `client:`, `solver:`, `hardware:`, `ci:`,
  `docs:`, `build:`), a body that says why, and a trailer naming any AI assistance.

## Solver changes

`attinav-core/src/solver.rs` is judged by numbers, not by review alone. After any change run
the Monte-Carlo harness and keep its results:

```sh
cd software/firmware
MC="cargo run --release -p attinav-core --example montecarlo --target x86_64-unknown-linux-gnu --features std --"
$MC 100 --tilt 1.5                 # realistic noise and package tilt: ≈ 0.03° azimuth, 0.006° inclination RMS
$MC 100 --tilt 3 --outlier 2       # one disturbed shot: rejected, same result as clean
$MC 60 --tilt 0 --mag-noise 0 --acc-noise 0   # noiseless: exact recovery
$MC 60 --groups 8                  # reduced coverage
```

Known invariants: body `X` is the laser axis, found by the roll invariance within a group (the
accelerometer-1 matrix is full minus one roll-gauge element, never upper-triangular); every
sensor is a right-handed triad (determinants stay positive); frame conventions live in
`survey.rs` and must not be restated elsewhere.

## Hardware tooling

KiCad work goes through the Konnect MCP tools or `kicad-cli`; never edit `.kicad_*` files by
hand. Konnect's audit tools are unreliable on this project (documented in
`hardware/kicad/README.md`, "Tooling caveats"); verify with a `kicad-cli` netlist export and
your own geometry parsing. Keep footprint origins on a 0.5 mm grid.

## What is not done

No hardware has run the firmware. The client has only been exercised against the simulator.
Treat timing, SPI behaviour and the solver's run time on the Cortex-M33 as unverified; the
bring-up order is in `software/firmware/README.md`.
