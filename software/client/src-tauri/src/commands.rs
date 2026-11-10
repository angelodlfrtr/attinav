//! Tauri commands: the whole surface the frontend can call.
//!
//! Protocol traffic goes through one generic [`request`] command carrying a [`HostMessage`]; the
//! frontend owns the protocol semantics (what to send, how to interpret replies) with the help of
//! the TypeScript mirror of the message types. Multi-step procedures that must not be interrupted
//! by the UI (the firmware update) live here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use attinav_core::protocol::{DeviceMessage, HostMessage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::error::{ClientError, Result};
use crate::link::Link;
use crate::ota;
use crate::sim::{self, SimControl};
use crate::transport::{self, PortInfo};

/// Shared application state.
pub struct AppState {
    link: tokio::sync::Mutex<Option<Link>>,
    sim: Arc<Mutex<SimControl>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            link: tokio::sync::Mutex::new(None),
            sim: Arc::new(Mutex::new(SimControl::default())),
        }
    }
}

impl AppState {
    async fn link(&self) -> Result<Link> {
        let guard = self.link.lock().await;
        match guard.as_ref() {
            Some(l) if l.is_open() => Ok(l.clone()),
            _ => Err(ClientError::NotConnected),
        }
    }
}

/// What to connect to.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectTarget {
    /// A serial port (USB CDC or UART adapter).
    Serial {
        /// Device path.
        path: String,
        /// Baud rate (ignored by USB CDC).
        #[serde(default = "default_baud")]
        baud: u32,
    },
    /// The built-in simulator.
    Simulator,
}

const fn default_baud() -> u32 {
    115_200
}

/// Current link state.
#[derive(Debug, Serialize)]
pub struct LinkStatus {
    /// `true` while a transport is alive.
    pub connected: bool,
    /// Transport description.
    pub description: Option<String>,
    /// `true` for the simulator.
    pub simulator: bool,
}

/// Static facts about this build.
#[derive(Debug, Serialize)]
pub struct AppInfo {
    /// Client version.
    pub version: &'static str,
    /// Protocol version this client speaks.
    pub protocol_version: u8,
    /// Firmware version the bundled `attinav-core` was built with.
    pub core_firmware_version: [u8; 3],
    /// Calibration procedure size.
    pub cal_groups: usize,
    /// Roll angles per group.
    pub cal_rolls: usize,
    /// Maximum temperature sets in a calibration.
    pub max_sets: usize,
    /// Largest firmware chunk.
    pub fw_chunk_max: usize,
}

/// Build facts.
#[tauri::command]
#[must_use]
pub const fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        protocol_version: attinav_core::PROTOCOL_VERSION,
        core_firmware_version: attinav_core::protocol::FIRMWARE_VERSION,
        cal_groups: attinav_core::solver::GROUPS,
        cal_rolls: attinav_core::solver::ROLLS,
        max_sets: attinav_core::calibration::MAX_SETS,
        fw_chunk_max: attinav_core::protocol::FW_CHUNK_MAX,
    }
}

/// Serial ports on this machine, Attinav devices first.
#[tauri::command]
pub fn list_ports() -> Result<Vec<PortInfo>> {
    transport::list_ports()
}

/// Opens a link. Fails when one is already open.
#[tauri::command]
pub async fn connect(app: AppHandle, state: State<'_, AppState>, target: ConnectTarget) -> Result<LinkStatus> {
    let mut guard = state.link.lock().await;
    if guard.as_ref().is_some_and(Link::is_open) {
        return Err(ClientError::AlreadyConnected);
    }
    let (transport, description, simulator) = match target {
        ConnectTarget::Serial { path, baud } => {
            (transport::open_serial(&path, baud)?, format!("{path} @ {baud}"), false)
        }
        ConnectTarget::Simulator => (
            sim::open_simulator(Arc::clone(&state.sim)),
            "simulator".to_string(),
            true,
        ),
    };
    let link = Link::start(Arc::new(app), transport, description.clone(), simulator);
    *guard = Some(link);
    drop(guard);
    Ok(LinkStatus {
        connected: true,
        description: Some(description),
        simulator,
    })
}

/// Closes the link, if any.
#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>) -> Result<()> {
    let link = state.link.lock().await.take();
    if let Some(link) = link {
        link.close();
    }
    Ok(())
}

/// Link state.
#[tauri::command]
pub async fn link_status(state: State<'_, AppState>) -> Result<LinkStatus> {
    let guard = state.link.lock().await;
    Ok(match guard.as_ref() {
        Some(l) if l.is_open() => LinkStatus {
            connected: true,
            description: Some(l.description().to_string()),
            simulator: l.is_simulator(),
        },
        _ => LinkStatus {
            connected: false,
            description: None,
            simulator: false,
        },
    })
}

/// Sends one request and returns every frame of its reply (see `PROTOCOL.md`, section 3).
///
/// `timeout_ms` defaults to 3 s; shots and solves need more (the frontend knows the configured
/// sample count and cycle count).
#[tauri::command]
pub async fn request(
    state: State<'_, AppState>,
    message: HostMessage,
    timeout_ms: Option<u64>,
) -> Result<Vec<DeviceMessage>> {
    let link = state.link().await?;
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(3000).clamp(100, 600_000));
    link.request(&message, timeout).await
}

/// Size and CRC of a firmware image file, for display before an update.
#[derive(Debug, Serialize)]
pub struct FirmwareFile {
    /// Bytes.
    pub size: usize,
    /// CRC-32/ISO-HDLC, as `FwBegin` carries it.
    pub crc32: u32,
}

/// Inspects an image file.
#[tauri::command]
pub async fn firmware_inspect(path: String) -> Result<FirmwareFile> {
    let data = tokio::fs::read(PathBuf::from(&path)).await?;
    Ok(FirmwareFile {
        size: data.len(),
        crc32: crc32fast::hash(&data),
    })
}

/// Runs a complete firmware update, emitting `fw:progress` events.
#[tauri::command]
pub async fn firmware_update(app: AppHandle, state: State<'_, AppState>, path: String, version: [u8; 3]) -> Result<()> {
    let link = state.link().await?;
    let image = tokio::fs::read(PathBuf::from(&path)).await?;
    ota::update(&link, &image, version, |p| {
        let _ = app.emit("fw:progress", p);
    })
    .await
}

/// Reads a JSON file (calibration import, saved sessions).
#[tauri::command]
pub async fn read_json(path: String) -> Result<serde_json::Value> {
    let text = tokio::fs::read_to_string(PathBuf::from(&path)).await?;
    Ok(serde_json::from_str(&text)?)
}

/// Writes pretty-printed JSON.
#[tauri::command]
pub async fn write_json(path: String, value: serde_json::Value) -> Result<()> {
    let text = serde_json::to_string_pretty(&value)?;
    tokio::fs::write(PathBuf::from(&path), text).await?;
    Ok(())
}

/// Writes a text file (CSV exports).
#[tauri::command]
pub async fn write_text(path: String, text: String) -> Result<()> {
    tokio::fs::write(PathBuf::from(&path), text).await?;
    Ok(())
}

/// Current simulator knobs.
#[tauri::command]
#[allow(clippy::needless_pass_by_value, reason = "Tauri hands `State` to commands by value")]
pub fn sim_get(state: State<'_, AppState>) -> SimControl {
    state
        .sim
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Replaces the simulator knobs (takes effect on the next shot / stream tick).
#[tauri::command]
#[allow(clippy::needless_pass_by_value, reason = "Tauri hands `State` to commands by value")]
pub fn sim_set(state: State<'_, AppState>, control: SimControl) -> Result<()> {
    if !(control.field_ut > 0.0 && control.field_ut.is_finite()) {
        return Err(ClientError::Invalid("field_ut must be positive".into()));
    }
    for v in [
        control.azimuth_deg,
        control.inclination_deg,
        control.roll_deg,
        control.dip_deg,
        control.mag_noise_counts,
        control.acc_noise_counts,
        control.motion_dps,
        control.temp_c,
        control.nearby_iron_ut,
    ] {
        if !v.is_finite() {
            return Err(ClientError::Invalid("non-finite value".into()));
        }
    }
    *state.sim.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = control;
    Ok(())
}
