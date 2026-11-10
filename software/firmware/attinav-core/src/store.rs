//! Power-fail-safe record framing for the non-volatile state (configuration + calibration).
//!
//! The firmware keeps two flash pages ("slots") in the second bank and alternates between them:
//! a new record is written into the *other* slot with a sequence number one higher than the
//! current one, so a reset in the middle of a write leaves the previous record intact. On boot the
//! valid record with the highest sequence number wins.
//!
//! Record layout (little endian):
//!
//! | offset | size | field                                      |
//! |-------:|-----:|--------------------------------------------|
//! |      0 |    4 | magic `"ATNV"`                             |
//! |      4 |    2 | record format version                      |
//! |      6 |    2 | payload length `n`                         |
//! |      8 |    4 | sequence number                            |
//! |     12 |    4 | CRC-32 of bytes `0..12` and the payload    |
//! |     16 |  `n` | payload                                    |
//!
//! The total is padded with `0xFF` to a multiple of the flash write granularity.

use crate::crc::crc32_iso_hdlc_parts;
use crate::wire::{Reader, WireError, Writer};

/// Record magic.
pub const MAGIC: [u8; 4] = *b"ATNV";
/// Record format version.
pub const RECORD_VERSION: u16 = 1;
/// Header size.
pub const HEADER_LEN: usize = 16;

/// Encodes a record into `out`, padded to a multiple of `write_align` bytes; returns the length.
pub fn encode_record(seq: u32, payload: &[u8], write_align: usize, out: &mut [u8]) -> Result<usize, WireError> {
    let len = u16::try_from(payload.len()).map_err(|_| WireError::Invalid)?;
    let mut w = Writer::new(out);
    w.bytes(&MAGIC)?;
    w.u16(RECORD_VERSION)?;
    w.u16(len)?;
    w.u32(seq)?;
    // CRC placeholder, filled in below.
    w.u32(0)?;
    w.bytes(payload)?;
    let mut total = w.len();
    while !total.is_multiple_of(write_align) {
        w.u8(0xFF)?;
        total += 1;
    }
    let crc = record_crc(&out[..HEADER_LEN + payload.len()]);
    out[12..16].copy_from_slice(&crc.to_le_bytes());
    Ok(total)
}

/// CRC over the header (with the CRC field taken as zero) and the payload.
fn record_crc(record_without_padding: &[u8]) -> u32 {
    let mut hdr = [0u8; HEADER_LEN];
    hdr.copy_from_slice(&record_without_padding[..HEADER_LEN]);
    hdr[12..16].fill(0);
    crc32_iso_hdlc_parts(&[&hdr, &record_without_padding[HEADER_LEN..]])
}

/// A decoded record header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct RecordInfo {
    /// Sequence number.
    pub seq: u32,
    /// Payload length.
    pub len: usize,
}

/// Validates a slot and returns its header and payload.
///
/// Returns `None` for an erased slot, a wrong magic / version, an impossible length or a CRC
/// mismatch — every one of these simply means "no record here".
#[must_use]
pub fn decode_record(slot: &[u8]) -> Option<(RecordInfo, &[u8])> {
    let mut r = Reader::new(slot);
    if r.array::<4>().ok()? != MAGIC {
        return None;
    }
    if r.u16().ok()? != RECORD_VERSION {
        return None;
    }
    let len = usize::from(r.u16().ok()?);
    let seq = r.u32().ok()?;
    let crc = r.u32().ok()?;
    if HEADER_LEN + len > slot.len() {
        return None;
    }
    if record_crc(&slot[..HEADER_LEN + len]) != crc {
        return None;
    }
    Some((RecordInfo { seq, len }, &slot[HEADER_LEN..HEADER_LEN + len]))
}

/// Picks the newer of two slots. Sequence numbers wrap, so "newer" is decided modulo 2³².
#[must_use]
pub const fn newer(a: Option<RecordInfo>, b: Option<RecordInfo>) -> Option<Slot> {
    match (a, b) {
        (None, None) => None,
        (Some(_), None) => Some(Slot::A),
        (None, Some(_)) => Some(Slot::B),
        (Some(a), Some(b)) => {
            if b.seq.wrapping_sub(a.seq) < 0x8000_0000 && b.seq != a.seq {
                Some(Slot::B)
            } else {
                Some(Slot::A)
            }
        }
    }
}

/// One of the two alternating slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Slot {
    /// First slot.
    A,
    /// Second slot.
    B,
}

impl Slot {
    /// The other slot.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_padding_and_rejection() {
        let payload = b"hello calibration";
        let mut buf = [0xAAu8; 64];
        let n = encode_record(7, payload, 16, &mut buf).unwrap();
        assert_eq!(n, 48);
        assert!(buf[HEADER_LEN + payload.len()..n].iter().all(|&b| b == 0xFF));
        let (info, body) = decode_record(&buf[..n]).unwrap();
        assert_eq!(
            info,
            RecordInfo {
                seq: 7,
                len: payload.len()
            }
        );
        assert_eq!(body, payload);

        // Flip a payload bit.
        let mut bad = buf;
        bad[HEADER_LEN + 3] ^= 1;
        assert!(decode_record(&bad[..n]).is_none());
        // Erased flash.
        assert!(decode_record(&[0xFF; 64]).is_none());
        // Length beyond the slot.
        let mut long = buf;
        long[6] = 0xFF;
        long[7] = 0x7F;
        assert!(decode_record(&long[..n]).is_none());
    }

    #[test]
    fn newer_handles_wraparound() {
        let a = RecordInfo { seq: 5, len: 0 };
        let b = RecordInfo { seq: 6, len: 0 };
        assert_eq!(newer(Some(a), Some(b)), Some(Slot::B));
        assert_eq!(newer(Some(b), Some(a)), Some(Slot::A));
        let a = RecordInfo { seq: u32::MAX, len: 0 };
        let b = RecordInfo { seq: 0, len: 0 };
        assert_eq!(newer(Some(a), Some(b)), Some(Slot::B));
        assert_eq!(newer(None, Some(b)), Some(Slot::B));
        assert_eq!(newer(Some(a), None), Some(Slot::A));
        assert_eq!(newer(None, None), None);
        assert_eq!(Slot::A.other(), Slot::B);
    }
}
