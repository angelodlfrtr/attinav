//! Attinav desktop client, Rust side.
//!
//! * [`transport`] — bytes to and from a serial port, or the built-in [`sim`]ulator.
//! * [`link`]      — framing, sequence numbers, reply collection; forwards every device frame to the
//!   frontend as a `device:message` event.
//! * [`ota`]       — the firmware update procedure.
//! * [`commands`]  — the Tauri command surface.
//!
//! The protocol codec, calibration model and solver come from the firmware's own `attinav-core`
//! crate, so the client can never drift from the wire format.

pub mod commands;
pub mod error;
pub mod link;
pub mod ota;
pub mod sim;
pub mod transport;

/// Runs the Tauri application until the last window closes.
///
/// # Panics
///
/// When the webview runtime cannot start (no display, missing `WebKitGTK`); there is nothing to
/// recover from at that point.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_ports,
            commands::connect,
            commands::disconnect,
            commands::link_status,
            commands::request,
            commands::firmware_inspect,
            commands::firmware_update,
            commands::read_json,
            commands::write_json,
            commands::write_text,
            commands::sim_get,
            commands::sim_set,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Attinav client");
}
