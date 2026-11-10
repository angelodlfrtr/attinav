//! Error type shared by every command. Serialised for the frontend as `{ kind, message }`.

use attinav_core::protocol::Status;
use serde::Serialize;

/// Anything a command can fail with.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No device link is open.
    #[error("not connected")]
    NotConnected,
    /// A link is already open; disconnect first.
    #[error("already connected")]
    AlreadyConnected,
    /// The serial port could not be opened or failed.
    #[error("serial port: {0}")]
    Serial(String),
    /// The device did not answer in time.
    #[error("timeout waiting for the device")]
    Timeout,
    /// The link closed while a request was pending.
    #[error("link closed")]
    Closed,
    /// A message could not be encoded or decoded.
    #[error("wire: {0}")]
    Wire(String),
    /// The device refused a request.
    #[error("device answered {status:?} to request {request:#04x} (detail {detail})")]
    Nack {
        /// Kind of the refused request.
        request: u8,
        /// Reply status.
        status: Status,
        /// Status-specific detail.
        detail: u16,
    },
    /// The device answered with an unexpected message.
    #[error("unexpected reply: {0}")]
    Unexpected(String),
    /// File system.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// JSON (de)serialisation.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// A command argument was invalid.
    #[error("{0}")]
    Invalid(String),
}

impl From<attinav_core::wire::WireError> for ClientError {
    fn from(e: attinav_core::wire::WireError) -> Self {
        Self::Wire(e.to_string())
    }
}

impl ClientError {
    const fn kind(&self) -> &'static str {
        match self {
            Self::NotConnected => "not_connected",
            Self::AlreadyConnected => "already_connected",
            Self::Serial(_) => "serial",
            Self::Timeout => "timeout",
            Self::Closed => "closed",
            Self::Wire(_) => "wire",
            Self::Nack { .. } => "nack",
            Self::Unexpected(_) => "unexpected",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
            Self::Invalid(_) => "invalid",
        }
    }
}

impl Serialize for ClientError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("ClientError", 4)?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("message", &self.to_string())?;
        if let Self::Nack {
            request,
            status,
            detail,
        } = self
        {
            st.serialize_field("request", request)?;
            st.serialize_field("status", status)?;
            st.serialize_field("detail", detail)?;
        }
        st.end()
    }
}

/// Shorthand.
pub type Result<T, E = ClientError> = std::result::Result<T, E>;
