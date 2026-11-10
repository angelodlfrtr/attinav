//! Little-endian byte cursors.
//!
//! Every serialised structure in this crate (link frames, calibration records) goes through these
//! two types, so the wire layout is defined in exactly one place per structure and bounds are
//! always checked.

/// Serialisation / deserialisation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum WireError {
    /// Destination buffer too small.
    BufferFull,
    /// Source ran out of bytes.
    Truncated,
    /// A field held a value that has no meaning (unknown enum tag, bad magic, ...).
    Invalid,
}

#[cfg(feature = "std")]
impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BufferFull => f.write_str("buffer full"),
            Self::Truncated => f.write_str("truncated"),
            Self::Invalid => f.write_str("invalid value"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for WireError {}

/// Appends little-endian values to a byte slice.
pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    /// Writer over an empty buffer.
    pub const fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes written so far.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.pos
    }

    /// `true` when nothing has been written.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.pos == 0
    }

    /// The written prefix.
    #[must_use]
    pub fn written(&self) -> &[u8] {
        &self.buf[..self.pos]
    }

    /// Appends raw bytes.
    pub fn bytes(&mut self, b: &[u8]) -> Result<(), WireError> {
        let end = self.pos.checked_add(b.len()).ok_or(WireError::BufferFull)?;
        if end > self.buf.len() {
            return Err(WireError::BufferFull);
        }
        self.buf[self.pos..end].copy_from_slice(b);
        self.pos = end;
        Ok(())
    }

    /// Appends zero padding up to a multiple of `align` bytes.
    pub fn pad_to(&mut self, align: usize) -> Result<(), WireError> {
        while !self.pos.is_multiple_of(align) {
            self.u8(0)?;
        }
        Ok(())
    }

    /// Appends a `u8`.
    pub fn u8(&mut self, v: u8) -> Result<(), WireError> {
        self.bytes(&[v])
    }
    /// Appends an `i8`.
    pub fn i8(&mut self, v: i8) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }
    /// Appends a `u16`.
    pub fn u16(&mut self, v: u16) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }
    /// Appends an `i16`.
    pub fn i16(&mut self, v: i16) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }
    /// Appends a `u32`.
    pub fn u32(&mut self, v: u32) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }
    /// Appends an `i32`.
    pub fn i32(&mut self, v: i32) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }
    /// Appends an `f32`.
    pub fn f32(&mut self, v: f32) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }
    /// Appends an `f64`.
    pub fn f64(&mut self, v: f64) -> Result<(), WireError> {
        self.bytes(&v.to_le_bytes())
    }
    /// Appends three `f32`.
    pub fn f32x3(&mut self, v: [f32; 3]) -> Result<(), WireError> {
        for x in v {
            self.f32(x)?;
        }
        Ok(())
    }
}

/// Reads little-endian values from a byte slice.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Reader over `buf`.
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// The unread tail.
    #[must_use]
    pub fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    /// Takes `n` raw bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self.pos.checked_add(n).ok_or(WireError::Truncated)?;
        if end > self.buf.len() {
            return Err(WireError::Truncated);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Takes a fixed-size array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    /// Skips padding up to a multiple of `align` bytes.
    pub fn skip_to(&mut self, align: usize) -> Result<(), WireError> {
        while !self.pos.is_multiple_of(align) {
            self.u8()?;
        }
        Ok(())
    }

    /// Reads a `u8`.
    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.array::<1>()?[0])
    }
    /// Reads an `i8`.
    pub fn i8(&mut self) -> Result<i8, WireError> {
        Ok(i8::from_le_bytes(self.array()?))
    }
    /// Reads a `u16`.
    pub fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    /// Reads an `i16`.
    pub fn i16(&mut self) -> Result<i16, WireError> {
        Ok(i16::from_le_bytes(self.array()?))
    }
    /// Reads a `u32`.
    pub fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    /// Reads an `i32`.
    pub fn i32(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    /// Reads an `f32`.
    pub fn f32(&mut self) -> Result<f32, WireError> {
        Ok(f32::from_le_bytes(self.array()?))
    }
    /// Reads an `f64`.
    pub fn f64(&mut self) -> Result<f64, WireError> {
        Ok(f64::from_le_bytes(self.array()?))
    }
    /// Reads three `f32`.
    pub fn f32x3(&mut self) -> Result<[f32; 3], WireError> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_bounds() {
        let mut buf = [0u8; 16];
        let mut w = Writer::new(&mut buf);
        w.u8(1).unwrap();
        w.u16(0x0203).unwrap();
        w.i32(-5).unwrap();
        w.f32(1.5).unwrap();
        w.pad_to(4).unwrap();
        assert_eq!(w.len(), 12);
        assert_eq!(w.bytes(&[0; 5]), Err(WireError::BufferFull));
        assert_eq!(w.len(), 12);

        let mut r = Reader::new(w.written());
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16().unwrap(), 0x0203);
        assert_eq!(r.i32().unwrap(), -5);
        assert_eq!(r.f32().unwrap(), 1.5);
        r.skip_to(4).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(r.u8(), Err(WireError::Truncated));
    }
}
