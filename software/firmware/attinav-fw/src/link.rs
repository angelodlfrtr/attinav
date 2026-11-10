//! Host links: USB CDC-ACM and the carrier LPUART, both speaking the same framed protocol.
//!
//! Each link task decodes incoming bytes into [`Inbound`] frames pushed to the shared
//! [`INBOUND`] channel (tagged with the link they came from) and drains its own outbound channel
//! of pre-encoded [`OutFrame`]s. The engine replies on the link a request came from and broadcasts
//! unsolicited frames (events, stream samples) to every connected link.

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{debug, info, warn};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{Either3, select3};
use embassy_stm32::mode::Async;
use embassy_stm32::usart::Uart;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, with_timeout};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::driver::EndpointError;
use embassy_usb::{Builder, UsbDevice};
use static_cell::StaticCell;

use attinav_core::protocol::{Decoder, DeviceMessage, FrameHeader, MAX_ENCODED, MAX_PAYLOAD};

use crate::board::UsbDrv;

/// Which physical link a frame travelled on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub enum LinkId {
    /// USB CDC-ACM.
    Usb = 0,
    /// Carrier LPUART1.
    Uart = 1,
}

/// A decoded request.
pub struct Inbound {
    /// Origin.
    pub link: LinkId,
    /// Kind and sequence number.
    pub header: FrameHeader,
    len: u16,
    payload: [u8; MAX_PAYLOAD],
}

impl Inbound {
    /// The payload bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.len)]
    }
}

/// An encoded frame ready to transmit.
pub struct OutFrame {
    len: u16,
    buf: [u8; MAX_ENCODED],
}

impl OutFrame {
    /// Encodes a message; `None` if it does not fit (cannot happen for well-formed messages).
    #[must_use]
    pub fn encode(msg: &DeviceMessage, seq: u8) -> Option<Self> {
        let mut f = Self {
            len: 0,
            buf: [0; MAX_ENCODED],
        };
        let n = msg.encode(seq, &mut f.buf).ok()?;
        f.len = n as u16;
        Some(f)
    }

    fn bytes(&self) -> &[u8] {
        &self.buf[..usize::from(self.len)]
    }
}

/// Requests from both links to the engine.
pub static INBOUND: Channel<CriticalSectionRawMutex, Inbound, 2> = Channel::new();
static OUT_USB: Channel<CriticalSectionRawMutex, OutFrame, 6> = Channel::new();
static OUT_UART: Channel<CriticalSectionRawMutex, OutFrame, 6> = Channel::new();
/// `true` while a USB host has the CDC port open (DTR asserted).
pub static USB_CONNECTED: AtomicBool = AtomicBool::new(false);

const fn outbound(link: LinkId) -> &'static Channel<CriticalSectionRawMutex, OutFrame, 6> {
    match link {
        LinkId::Usb => &OUT_USB,
        LinkId::Uart => &OUT_UART,
    }
}

/// Sends a reply on one link, waiting (bounded) for room. A reply to a USB request whose port has
/// been closed in the meantime is dropped; a link whose queue stays full for two seconds (a host
/// that stopped reading) loses the reply rather than stalling the engine.
pub async fn send(link: LinkId, msg: &DeviceMessage, seq: u8) {
    let Some(f) = OutFrame::encode(msg, seq) else { return };
    if link == LinkId::Usb && !USB_CONNECTED.load(Ordering::Relaxed) {
        debug!("usb port closed, reply dropped");
        return;
    }
    if with_timeout(Duration::from_secs(2), outbound(link).send(f))
        .await
        .is_err()
    {
        warn!("{}: outbound stalled, reply dropped", link);
    }
}

/// Sends an unsolicited frame to every connected link, dropping it on a link whose queue is full
/// (streams must never stall the engine).
pub fn broadcast(msg: &DeviceMessage) {
    if let Some(f) = OutFrame::encode(msg, 0)
        && USB_CONNECTED.load(Ordering::Relaxed)
        && OUT_USB.try_send(f).is_err()
    {
        debug!("usb outbound full, frame dropped");
    }
    if let Some(f) = OutFrame::encode(msg, 0)
        && OUT_UART.try_send(f).is_err()
    {
        debug!("uart outbound full, frame dropped");
    }
}

/// Pushes decoder output into the inbound channel.
async fn deliver(link: LinkId, dec: &Decoder, header: FrameHeader) {
    let payload = dec.payload();
    let mut inbound = Inbound {
        link,
        header,
        len: payload.len() as u16,
        payload: [0; MAX_PAYLOAD],
    };
    inbound.payload[..payload.len()].copy_from_slice(payload);
    INBOUND.send(inbound).await;
}

/// Feeds received bytes through the decoder and delivers complete frames.
async fn feed(link: LinkId, dec: &mut Decoder, bytes: &[u8]) {
    for &b in bytes {
        match dec.push(b) {
            None => {}
            Some(Ok(header)) => deliver(link, dec, header).await,
            Some(Err(e)) => warn!("{}: dropped frame: {:?}", link, e),
        }
    }
}

// --------------------------------------------------------------------------------------------
// USB
// --------------------------------------------------------------------------------------------

/// Builds the USB device (one CDC-ACM function) and spawns its task.
pub fn spawn_usb(spawner: Spawner, driver: UsbDrv) {
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 64]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static STATE: StaticCell<State<'static>> = StaticCell::new();

    // pid.codes test VID/PID pair for in-house hardware; replace before any distribution.
    let mut config = embassy_usb::Config::new(0x1209, 0x0001);
    config.manufacturer = Some("Neolut");
    config.product = Some("Attinav survey module");
    config.serial_number = Some(embassy_stm32::uid::uid_hex());
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 64]),
        CONTROL_BUF.init([0; 64]),
    );
    let class = CdcAcmClass::new(&mut builder, STATE.init(State::new()), 64);
    let device = builder.build();
    spawner.spawn(defmt::unwrap!(usb_task(device, class)));
}

#[embassy_executor::task]
async fn usb_task(mut device: UsbDevice<'static, UsbDrv>, class: CdcAcmClass<'static, UsbDrv>) {
    let (mut sender, mut receiver, control) = class.split_with_control();
    let io = async {
        let mut dec = Decoder::new();
        loop {
            // "Connected" here means the interface is configured; whether an application has the
            // port open is the DTR line, tracked by `ctl` below.
            receiver.wait_connection().await;
            info!("usb: configured");
            let rx = async {
                let mut buf = [0u8; 64];
                loop {
                    match receiver.read_packet(&mut buf).await {
                        Ok(n) => feed(LinkId::Usb, &mut dec, &buf[..n]).await,
                        Err(EndpointError::Disabled) => break,
                        Err(EndpointError::BufferOverflow) => warn!("usb: packet overflow"),
                    }
                }
            };
            let tx = async {
                loop {
                    let frame = OUT_USB.receive().await;
                    if !USB_CONNECTED.load(Ordering::Relaxed) {
                        // Port closed between queueing and sending.
                        continue;
                    }
                    // A host that has the port open but stopped reading would block bulk IN forever;
                    // give up on the frame after a second (the host resynchronises on the delimiter).
                    let mut complete = true;
                    for chunk in frame.bytes().chunks(64) {
                        match with_timeout(Duration::from_secs(1), sender.write_packet(chunk)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => return,
                            Err(_) => {
                                warn!("usb: host not reading, frame dropped");
                                complete = false;
                                break;
                            }
                        }
                    }
                    // A full-size final packet needs a zero-length packet to end the transfer.
                    if complete
                        && frame.bytes().len() % 64 == 0
                        && let Ok(Err(_)) = with_timeout(Duration::from_secs(1), sender.write_packet(&[])).await
                    {
                        return;
                    }
                }
            };
            let ctl = async {
                loop {
                    let dtr = control.dtr();
                    if dtr != USB_CONNECTED.load(Ordering::Relaxed) {
                        USB_CONNECTED.store(dtr, Ordering::Relaxed);
                        if dtr {
                            // Drop frames queued while nobody was listening.
                            while OUT_USB.try_receive().is_ok() {}
                            info!("usb: port opened");
                        } else {
                            info!("usb: port closed");
                        }
                    }
                    control.control_changed().await;
                }
            };
            match select3(rx, tx, ctl).await {
                Either3::First(()) | Either3::Second(()) | Either3::Third(()) => {}
            }
            USB_CONNECTED.store(false, Ordering::Relaxed);
            info!("usb: disconnected");
        }
    };
    join(device.run(), io).await;
}

// --------------------------------------------------------------------------------------------
// UART
// --------------------------------------------------------------------------------------------

/// Spawns the carrier UART task.
pub fn spawn_uart(spawner: Spawner, uart: Uart<'static, Async>) {
    spawner.spawn(defmt::unwrap!(uart_task(uart)));
}

#[embassy_executor::task]
async fn uart_task(uart: Uart<'static, Async>) {
    static DMA_BUF: StaticCell<[u8; 512]> = StaticCell::new();
    let (mut tx, rx) = uart.split();
    let mut rx = rx.into_ring_buffered(DMA_BUF.init([0; 512]));
    let rx_loop = async {
        let mut dec = Decoder::new();
        let mut buf = [0u8; 128];
        loop {
            match rx.read(&mut buf).await {
                Ok(n) => feed(LinkId::Uart, &mut dec, &buf[..n]).await,
                Err(e) => {
                    warn!("uart: rx error {:?}", e);
                    // The ring buffer restarts itself on the next read; drop partial frame state.
                    dec = Decoder::new();
                }
            }
        }
    };
    let tx_loop = async {
        loop {
            let frame = OUT_UART.receive().await;
            if let Err(e) = tx.write(frame.bytes()).await {
                warn!("uart: tx error {:?}", e);
            }
        }
    };
    join(rx_loop, tx_loop).await;
}
