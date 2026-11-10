//! CRCs used on the wire and in flash. Bitwise implementations: the frames are tens of bytes and
//! the flash records a few kilobytes, so a lookup table buys nothing worth its RAM.

/// CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF, no reflection, no final XOR).
///
/// Check value for `"123456789"` is `0x29B1`.
#[must_use]
pub fn crc16_ccitt_false(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Incremental CRC-32/ISO-HDLC for data that arrives in pieces.
///
/// Same CRC as [`crc32_iso_hdlc`] (the zlib / Ethernet CRC: poly 0xEDB88320 reflected, init and
/// final XOR `0xFFFF_FFFF`), used when the data is not in one buffer, e.g. a firmware image read
/// back from flash one block at a time.
///
/// ```
/// use attinav_core::crc::Crc32;
/// let mut c = Crc32::new();
/// c.update(b"1234");
/// c.update(b"56789");
/// assert_eq!(c.finish(), 0xCBF4_3926);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    /// Fresh register (init value `0xFFFF_FFFF`).
    #[must_use]
    pub const fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    /// Feeds `data` into the register.
    pub fn update(&mut self, data: &[u8]) {
        let mut crc = self.0;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        self.0 = crc;
    }

    /// The CRC of everything fed so far (final XOR applied).
    #[must_use]
    pub const fn finish(self) -> u32 {
        !self.0
    }
}

/// CRC-32/ISO-HDLC over the concatenation of `parts`.
///
/// Check value for `"123456789"` is `0xCBF4_3926`.
#[must_use]
pub fn crc32_iso_hdlc_parts(parts: &[&[u8]]) -> u32 {
    let mut crc = Crc32::new();
    for p in parts {
        crc.update(p);
    }
    crc.finish()
}

/// [`crc32_iso_hdlc_parts`] over a single slice.
#[must_use]
pub fn crc32_iso_hdlc(data: &[u8]) -> u32 {
    crc32_iso_hdlc_parts(&[data])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_values() {
        assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
        assert_eq!(crc32_iso_hdlc(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_iso_hdlc_parts(&[b"1234", b"56789"]), 0xCBF4_3926);
        assert_eq!(crc16_ccitt_false(b""), 0xFFFF);
        assert_eq!(crc32_iso_hdlc(b""), 0);
    }

    #[test]
    fn incremental_matches_one_shot() {
        let data: [u8; 300] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let mut c = Crc32::new();
        for chunk in data.chunks(17) {
            c.update(chunk);
        }
        assert_eq!(c.finish(), crc32_iso_hdlc(&data));
        assert_eq!(Crc32::default().finish(), 0);
    }
}
