//! Sans-I/O HTTP/1.1 connection engine.
//!
//! [`Connection`] owns protocol state and buffering but performs no socket I/O.
//! Pass received bytes to [`Connection::receive_data`], poll [`Event`] values
//! with [`Connection::next_event`], and write bytes returned by its send methods.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

mod connection;
mod method;
mod state;
mod status;
mod wire;

pub use connection::Connection;
pub use method::{InvalidMethod, Method};
pub use state::{Role, State};
pub use status::{InvalidStatusCode, StatusCode};

use std::{error::Error as StdError, fmt};

const DEFAULT_MAX_HEAD_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_HEADER_COUNT: usize = 100;

/// One header field, preserving input order and name casing.
///
/// The value is the parsed field value; leading and trailing line whitespace
/// is excluded as required by RFC 9110 Section 5.5.
pub type Header = (Vec<u8>, Vec<u8>);

/// HTTP protocol version carried by a request or response head.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Version {
    /// HTTP/1.0.
    Http10,
    /// HTTP/1.1.
    Http11,
}

impl Version {
    pub(crate) const fn wire(self) -> &'static [u8] {
        match self {
            Self::Http10 => b"1.0",
            Self::Http11 => b"1.1",
        }
    }
}

/// A request head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// Case-sensitive request method.
    pub method: Method,
    /// Request-target bytes from the HTTP request line.
    ///
    /// See [RFC 9112 Section 3.2](https://www.rfc-editor.org/rfc/rfc9112.html#section-3.2).
    pub target: Vec<u8>,
    /// Ordered header fields.
    pub headers: Vec<Header>,
    /// HTTP version.
    pub http_version: Version,
}

/// A final response head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Response {
    /// Final status code.
    pub status: StatusCode,
    /// Raw reason-phrase bytes, including any legal SP, HTAB, or obs-text.
    pub reason: Vec<u8>,
    /// Ordered header fields.
    pub headers: Vec<Header>,
    /// HTTP version.
    pub http_version: Version,
}

/// A non-final `1xx` response head.
pub type InformationalResponse = Response;

/// A piece of message body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Data {
    /// Decoded body bytes.
    pub data: Vec<u8>,
    /// Whether these bytes begin an HTTP chunk.
    pub chunk_start: bool,
    /// Whether these bytes end an HTTP chunk.
    pub chunk_end: bool,
}

/// Framing plus an unchanged caller-owned body object.
///
/// Write the prefix, body, and suffix in order without changing the body until
/// all three writes complete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataParts<T> {
    /// Bytes to write before the body object.
    pub prefix: Vec<u8>,
    /// The original body object.
    pub data: T,
    /// Bytes to write after the body object.
    pub suffix: Vec<u8>,
}

/// The end of one HTTP message, optionally carrying trailer fields.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EndOfMessage {
    /// Ordered trailer fields.
    pub trailers: Vec<Header>,
}

/// A semantic event decoded from peer bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// A request head.
    Request(Request),
    /// A non-final response head.
    InformationalResponse(InformationalResponse),
    /// A final response head.
    Response(Response),
    /// Decoded body data.
    Data(Data),
    /// Message completion and optional trailers.
    EndOfMessage(EndOfMessage),
    /// Clean transport EOF.
    ConnectionClosed,
}

/// Result of polling [`Connection::next_event`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NextEvent {
    /// One decoded protocol event.
    Event(Event),
    /// More peer bytes are needed.
    NeedData,
    /// HTTP parsing is paused until the caller advances the cycle or protocol.
    Paused,
}

/// Independent resource limits applied to inbound field sections.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum bytes through the terminating empty line of a head or trailers.
    /// This also bounds an incomplete chunk-size line.
    max_head_bytes: usize,
    /// Maximum number of header or trailer fields.
    max_header_count: usize,
}

impl Limits {
    /// Creates non-zero field-section byte and field-count limits.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidLimits`] if any limit is zero.
    pub const fn new(head: usize, headers: usize) -> Result<Self, InvalidLimits> {
        if head == 0 || headers == 0 {
            Err(InvalidLimits)
        } else {
            Ok(Self {
                max_head_bytes: head,
                max_header_count: headers,
            })
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_head_bytes: DEFAULT_MAX_HEAD_BYTES,
            max_header_count: DEFAULT_MAX_HEADER_COUNT,
        }
    }
}

/// A zero inbound limit is invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidLimits;

impl fmt::Display for InvalidLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HTTP limits must be non-zero")
    }
}

impl StdError for InvalidLimits {}

/// The caller attempted an operation forbidden by the local state or framing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalProtocolError(pub(crate) String);

impl fmt::Display for LocalProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl StdError for LocalProtocolError {}

/// The peer sent malformed HTTP or exceeded an inbound limit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteProtocolError {
    pub(crate) message: String,
    pub(crate) suggested_status_code: Option<u16>,
}

impl RemoteProtocolError {
    /// Status a server can use when rejecting this input.
    #[must_use]
    pub const fn suggested_status_code(&self) -> Option<u16> {
        self.suggested_status_code
    }
}

impl fmt::Display for RemoteProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl StdError for RemoteProtocolError {}
