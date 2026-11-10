//! Firmware update driver (the host side of `OTA.md`).

use std::time::Duration;

use attinav_core::protocol::{DeviceMessage, FW_CHUNK_MAX, FwBegin, FwChunk, HostMessage};
use serde::Serialize;
use tracing::{info, warn};

use crate::error::{ClientError, Result};
use crate::link::Link;

/// Progress report emitted as the `fw:progress` event.
#[derive(Clone, Debug, Serialize)]
pub struct FwProgress {
    /// `preflight`, `begin`, `transfer`, `verify`, `done` or `failed`.
    pub stage: &'static str,
    /// Bytes acknowledged by the device.
    pub sent: usize,
    /// Image size.
    pub total: usize,
    /// Error text in the `failed` stage.
    pub message: Option<String>,
}

const T_SHORT: Duration = Duration::from_secs(3);
const T_CHUNK: Duration = Duration::from_secs(5);
/// Read-back of up to 456 KiB plus the bootloader state write.
const T_FINISH: Duration = Duration::from_secs(30);

/// Streams `image` to the device and asks it to swap. On success the device resets ~200 ms after
/// the final acknowledgement; the caller should expect the link to drop.
pub async fn update(link: &Link, image: &[u8], version: [u8; 3], mut progress: impl FnMut(FwProgress)) -> Result<()> {
    let total = image.len();
    if total == 0 {
        return Err(ClientError::Invalid("empty firmware image".into()));
    }
    if u32::try_from(total).is_err() {
        return Err(ClientError::Invalid("image too large".into()));
    }
    progress(FwProgress {
        stage: "preflight",
        sent: 0,
        total,
        message: None,
    });
    let info = match link.request_one(&HostMessage::GetInfo, T_SHORT).await? {
        DeviceMessage::Info(i) => i,
        other => return Err(ClientError::Unexpected(format!("{other:?}"))),
    };
    if info.boot_state == 1 {
        return Err(ClientError::Invalid(
            "the device is still on an unconfirmed trial image; reset it and check `boot_state` first".into(),
        ));
    }

    let crc32 = crc32fast::hash(image);
    info!("fw: begin {total} bytes, crc {crc32:#010x}, version {version:?}");
    progress(FwProgress {
        stage: "begin",
        sent: 0,
        total,
        message: None,
    });
    link.ack(
        &HostMessage::FwBegin(FwBegin {
            size: total as u32,
            crc32,
            version,
        }),
        T_SHORT,
    )
    .await?;

    let result = transfer(link, image, &mut progress).await;
    if let Err(e) = &result {
        warn!("fw: transfer failed: {e}");
        // Best effort: leave the device without an open session.
        let _ = link.ack(&HostMessage::FwAbort, T_SHORT).await;
        progress(FwProgress {
            stage: "failed",
            sent: 0,
            total,
            message: Some(e.to_string()),
        });
        return result;
    }

    progress(FwProgress {
        stage: "verify",
        sent: total,
        total,
        message: None,
    });
    if let Err(e) = link.ack(&HostMessage::FwFinish, T_FINISH).await {
        progress(FwProgress {
            stage: "failed",
            sent: total,
            total,
            message: Some(e.to_string()),
        });
        return Err(e);
    }
    info!("fw: image accepted, device resetting");
    progress(FwProgress {
        stage: "done",
        sent: total,
        total,
        message: None,
    });
    Ok(())
}

async fn transfer(link: &Link, image: &[u8], progress: &mut impl FnMut(FwProgress)) -> Result<()> {
    let total = image.len();
    for (i, chunk) in image.chunks(FW_CHUNK_MAX).enumerate() {
        let offset = (i * FW_CHUNK_MAX) as u32;
        let c = FwChunk::new(offset, chunk).ok_or_else(|| ClientError::Invalid("bad chunk".into()))?;
        link.ack(&HostMessage::FwChunk(c), T_CHUNK).await?;
        progress(FwProgress {
            stage: "transfer",
            sent: (i * FW_CHUNK_MAX + chunk.len()).min(total),
            total,
            message: None,
        });
    }
    Ok(())
}
