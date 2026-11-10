//! Byte transports: a serial port (USB CDC or a UART adapter) or the built-in simulator.
//!
//! A [`Transport`] is a pair of byte channels. The link layer on top does not know which kind it
//! talks to, so the whole protocol stack (COBS, CRC, message codec) is exercised identically against
//! real hardware and the simulator.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::error::{ClientError, Result};

/// Bytes in both directions plus a stop flag.
pub struct Transport {
    /// Host → device bytes.
    pub to_device: mpsc::Sender<Vec<u8>>,
    /// Device → host bytes. Closes when the transport dies.
    pub from_device: mpsc::Receiver<Vec<u8>>,
    /// Last error reported by the transport, if it died.
    pub error: Arc<std::sync::Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}

impl Transport {
    /// Builds a transport around its channel ends.
    pub fn new(to_device: mpsc::Sender<Vec<u8>>, from_device: mpsc::Receiver<Vec<u8>>, stop: Arc<AtomicBool>) -> Self {
        Self {
            to_device,
            from_device,
            error: Arc::new(std::sync::Mutex::new(None)),
            stop,
        }
    }

    /// Asks the transport to stop; its channels close shortly after.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Stop flag shared with the worker threads / tasks.
    #[must_use]
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }
}

/// One serial port on the machine.
#[derive(Clone, Debug, Serialize)]
pub struct PortInfo {
    /// Device path (`/dev/ttyACM0`, `COM3`).
    pub path: String,
    /// `usb`, `pci`, `bluetooth` or `unknown`.
    pub kind: &'static str,
    /// USB vendor id.
    pub vid: Option<u16>,
    /// USB product id.
    pub pid: Option<u16>,
    /// USB manufacturer string.
    pub manufacturer: Option<String>,
    /// USB product string.
    pub product: Option<String>,
    /// USB serial number (the MCU unique id for an Attinav).
    pub serial: Option<String>,
    /// `true` when the VID/PID match the firmware's descriptor.
    pub is_attinav: bool,
}

/// VID/PID pair in the firmware's USB descriptor (`attinav-fw/src/link.rs`).
pub const ATTINAV_VID: u16 = 0x1209;
/// See [`ATTINAV_VID`].
pub const ATTINAV_PID: u16 = 0x0001;

/// Enumerates the serial ports, Attinav devices first.
pub fn list_ports() -> Result<Vec<PortInfo>> {
    let ports = serialport::available_ports().map_err(|e| ClientError::Serial(e.to_string()))?;
    let mut out: Vec<PortInfo> = ports
        .into_iter()
        .map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(u) => PortInfo {
                path: p.port_name,
                kind: "usb",
                vid: Some(u.vid),
                pid: Some(u.pid),
                manufacturer: u.manufacturer,
                product: u.product,
                serial: u.serial_number,
                is_attinav: u.vid == ATTINAV_VID && u.pid == ATTINAV_PID,
            },
            other => PortInfo {
                path: p.port_name,
                kind: match other {
                    serialport::SerialPortType::PciPort => "pci",
                    serialport::SerialPortType::BluetoothPort => "bluetooth",
                    _ => "unknown",
                },
                vid: None,
                pid: None,
                manufacturer: None,
                product: None,
                serial: None,
                is_attinav: false,
            },
        })
        .collect();
    out.sort_by(|a, b| b.is_attinav.cmp(&a.is_attinav).then_with(|| a.path.cmp(&b.path)));
    Ok(out)
}

/// Opens a serial port and runs it on two worker threads (blocking reader, blocking writer).
///
/// `baud` is ignored by a USB CDC port and must match `DeviceConfig::uart_baud` for the UART.
pub fn open_serial(path: &str, baud: u32) -> Result<Transport> {
    let port = serialport::new(path, baud)
        .timeout(Duration::from_millis(100))
        .open()
        .map_err(|e| ClientError::Serial(format!("{path}: {e}")))?;
    let mut writer = port.try_clone().map_err(|e| ClientError::Serial(e.to_string()))?;
    let mut reader = port;

    let (to_device, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    let (in_tx, from_device) = mpsc::channel::<Vec<u8>>(256);
    let stop = Arc::new(AtomicBool::new(false));
    let transport = Transport::new(to_device, from_device, Arc::clone(&stop));
    let error = Arc::clone(&transport.error);

    let stop_r = Arc::clone(&stop);
    let error_r = Arc::clone(&error);
    std::thread::Builder::new()
        .name(format!("serial-rx {path}"))
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while !stop_r.load(Ordering::SeqCst) {
                match reader.read(&mut buf) {
                    Ok(0) => {}
                    Ok(n) => {
                        if in_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        warn!("serial read failed: {e}");
                        *error_r.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e.to_string());
                        break;
                    }
                }
            }
            stop_r.store(true, Ordering::SeqCst);
            debug!("serial reader stopped");
        })
        .map_err(|e| ClientError::Serial(e.to_string()))?;

    let stop_w = Arc::clone(&stop);
    std::thread::Builder::new()
        .name(format!("serial-tx {path}"))
        .spawn(move || {
            while let Some(bytes) = out_rx.blocking_recv() {
                if stop_w.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(e) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
                    warn!("serial write failed: {e}");
                    *error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e.to_string());
                    stop_w.store(true, Ordering::SeqCst);
                    break;
                }
            }
            debug!("serial writer stopped");
        })
        .map_err(|e| ClientError::Serial(e.to_string()))?;

    Ok(transport)
}
