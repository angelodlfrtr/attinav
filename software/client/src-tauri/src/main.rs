//! Binary entry point; everything lives in the library so the Tauri mobile targets could reuse it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    attinav_client::run();
}
