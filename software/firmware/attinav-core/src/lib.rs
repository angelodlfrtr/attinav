//! Hardware-independent core of the Attinav cave-survey module firmware.
//!
//! Everything in this crate is `no_std`, allocation-free and deterministic, so that it can be unit
//! tested on the host (`cargo test-host`) and reused unchanged by the desktop tooling. The firmware
//! crate wires it to the sensors; nothing here knows about SPI, flash or USB.
//!
//! Module map:
//!
//! * [`math`]      — small fixed-size `f64` vector / matrix types used everywhere else.
//! * [`crc`]       — CRC-16/CCITT-FALSE (link frames) and CRC-32/ISO-HDLC (flash records).
//! * [`pt1000`]    — divider + Callendar–Van Dusen conversion for the coil thermometers.
//! * [`stats`]     — running mean / variance of 3-vectors (shot averaging).
//! * [`survey`]    — azimuth / inclination / roll from a gravity and a field vector.
//! * [`calibration`] — the Heeb/DistoX affine sensor model, multi-temperature sets, (de)serialisation.
//! * [`solver`]    — Levenberg–Marquardt calibration solver over the 56-shot procedure.
//! * [`quality`]   — redundancy checks that turn a shot into quality flags.
//! * [`protocol`]  — COBS-framed binary host protocol shared by the carrier UART and USB CDC.
//! * [`store`]     — power-fail-safe record framing for the calibration store in flash.
//! * [`wire`]      — little-endian cursor reader/writer shared by `protocol`, `calibration`, `store`.
//!
//! Frame conventions (used by `survey`, `calibration`, `solver`) are documented in [`survey`].
#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![warn(missing_docs)]

pub mod calibration;
pub mod crc;
pub mod math;
pub mod protocol;
pub mod pt1000;
pub mod quality;
pub mod solver;
pub mod stats;
pub mod store;
pub mod survey;
pub mod wire;

/// Protocol version carried in every `Info` reply. Bump on any wire-incompatible change.
pub const PROTOCOL_VERSION: u8 = 1;
