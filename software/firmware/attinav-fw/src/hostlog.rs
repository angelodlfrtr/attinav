//! Diagnostics visible to the host without a debug probe.
//!
//! `host_warn!` / `host_info!` / `host_error!` emit the usual defmt record *and* format the same
//! line into a [`LogLine`] that is broadcast as a `Log` frame on every connected link (dropped,
//! never blocking, when a queue is full). The text is truncated to [`LOG_TEXT_MAX`] bytes on a
//! character boundary.
//!
//! Only engine-side code (the main task: engine, sensors, store) may use these macros: the link
//! tasks must not log through the links they drive.

use core::fmt::{self, Write};

use attinav_core::protocol::{DeviceMessage, LOG_TEXT_MAX, LogLine};

use crate::link;

/// Fixed-capacity, truncating text sink.
struct LineBuf {
    buf: [u8; LOG_TEXT_MAX],
    len: usize,
}

impl Write for LineBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let room = LOG_TEXT_MAX - self.len;
        let mut n = s.len().min(room);
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        // Report success even when truncated: the caller must not fail over a long message.
        Ok(())
    }
}

/// Formats `args` and broadcasts it as a `Log` frame with the given level (0 error, 1 warn, 2 info).
pub fn emit(level: u8, args: fmt::Arguments<'_>) {
    let mut line = LineBuf {
        buf: [0; LOG_TEXT_MAX],
        len: 0,
    };
    let _ = line.write_fmt(args);
    // `LineBuf` only ever copies whole characters, so the prefix is valid UTF-8.
    let text = core::str::from_utf8(&line.buf[..line.len]).unwrap_or("");
    link::broadcast(&DeviceMessage::Log(LogLine::new(level, text)));
}

/// `defmt::error!` plus a level-0 `Log` frame to the host.
macro_rules! host_error {
    ($($arg:tt)*) => {{
        defmt::error!($($arg)*);
        $crate::hostlog::emit(0, format_args!($($arg)*));
    }};
}

/// `defmt::warn!` plus a level-1 `Log` frame to the host.
macro_rules! host_warn {
    ($($arg:tt)*) => {{
        defmt::warn!($($arg)*);
        $crate::hostlog::emit(1, format_args!($($arg)*));
    }};
}

/// `defmt::info!` plus a level-2 `Log` frame to the host.
macro_rules! host_info {
    ($($arg:tt)*) => {{
        defmt::info!($($arg)*);
        $crate::hostlog::emit(2, format_args!($($arg)*));
    }};
}
