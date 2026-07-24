//! Python bindings for the HTTP/1.1 library.

use h11r as core;
use h11r::{Method, StatusCode};
use pyo3::PyTypeInfo;
use pyo3::exceptions::{PyAttributeError, PyException, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyMemoryView, PyModule, PyString, PyTuple};

pyo3::create_exception!(
    h11r,
    ProtocolError,
    PyException,
    "Base HTTP/1 protocol error."
);
pyo3::create_exception!(
    h11r,
    LocalProtocolError,
    ProtocolError,
    "The caller attempted an invalid local HTTP operation."
);
pyo3::create_exception!(
    h11r,
    RemoteProtocolError,
    ProtocolError,
    "The peer sent invalid HTTP or exceeded an inbound limit.\n\n\
     Attributes:\n    \
         suggested_status_code (int | None): A suitable HTTP response status, \
         when one is known."
);

/// An HTTP actor role.
///
/// Attributes:
///     CLIENT (Role): The actor that sends requests and receives responses.
///     SERVER (Role): The actor that receives requests and sends responses.
#[pyclass(
    name = "Role",
    module = "h11r",
    eq,
    eq_int,
    rename_all = "SCREAMING_SNAKE_CASE",
    from_py_object
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PyRole {
    Client,
    Server,
}

impl From<PyRole> for core::Role {
    fn from(value: PyRole) -> Self {
        match value {
            PyRole::Client => Self::Client,
            PyRole::Server => Self::Server,
        }
    }
}

/// An observable HTTP/1 actor lifecycle state.
///
/// Attributes:
///     IDLE (State): No message has started.
///     SEND_RESPONSE (State): The server can send informational or final responses.
///     SEND_BODY (State): The actor can send or receive body data and message end.
///     DONE (State): The actor completed its message for the current cycle.
///     MIGHT_SWITCH_PROTOCOL (State): The request ended while a switch decision is pending.
///     SWITCHED_PROTOCOL (State): HTTP processing ended after a successful switch.
///     MUST_CLOSE (State): The actor completed its message but reuse is forbidden.
///     CLOSED (State): The actor closed its transport side.
///     ERROR (State): A protocol error permanently poisoned the actor.
#[pyclass(
    name = "State",
    module = "h11r",
    eq,
    eq_int,
    rename_all = "SCREAMING_SNAKE_CASE",
    skip_from_py_object
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PyState {
    Idle,
    SendResponse,
    SendBody,
    Done,
    MightSwitchProtocol,
    SwitchedProtocol,
    MustClose,
    Closed,
    Error,
}

impl From<core::State> for PyState {
    fn from(value: core::State) -> Self {
        match value {
            core::State::Idle => Self::Idle,
            core::State::SendResponse => Self::SendResponse,
            core::State::SendBody => Self::SendBody,
            core::State::Done => Self::Done,
            core::State::MightSwitchProtocol => Self::MightSwitchProtocol,
            core::State::SwitchedProtocol => Self::SwitchedProtocol,
            core::State::MustClose => Self::MustClose,
            core::State::Closed => Self::Closed,
            core::State::Error => Self::Error,
        }
    }
}

/// A non-event result returned by `Connection.next_event()`.
///
/// Attributes:
///     NEED_DATA (ReceiveStatus): Supply more peer bytes before polling again.
///     PAUSED (ReceiveStatus): Complete the active cycle or hand off a switched protocol before
///         polling HTTP again.
#[pyclass(
    name = "ReceiveStatus",
    module = "h11r",
    eq,
    eq_int,
    rename_all = "SCREAMING_SNAKE_CASE",
    skip_from_py_object
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PyReceiveStatus {
    NeedData,
    Paused,
}

/// A parsed request head.
///
/// Attributes:
///     method (bytes): Case-sensitive method bytes.
///     target (bytes): Request-target bytes from the HTTP request line. See RFC 9112
///         Section 3.2.
///     headers (tuple[tuple[bytes, bytes], ...]): Ordered `(name, value)` byte pairs.
///     http_version (bytes): Peer HTTP version bytes.
#[pyclass(
    name = "Request",
    module = "h11r",
    frozen,
    eq,
    get_all,
    skip_from_py_object
)]
#[derive(Debug)]
struct PyRequest {
    /// The case-sensitive request method as bytes.
    method: Py<PyBytes>,
    /// The request target as bytes.
    target: Py<PyBytes>,
    /// Ordered `(name, value)` byte pairs.
    headers: Py<PyTuple>,
    /// The peer HTTP version as bytes.
    http_version: Py<PyBytes>,
}

impl PyRequest {
    fn from_core(py: Python<'_>, value: core::Request) -> PyResult<Self> {
        Ok(Self {
            method: PyBytes::new(py, value.method.as_bytes()).unbind(),
            target: PyBytes::new(py, &value.target).unbind(),
            headers: py_headers(py, &value.headers)?.unbind(),
            http_version: py_version(py, value.http_version).unbind(),
        })
    }
}

impl PartialEq for PyRequest {
    fn eq(&self, other: &Self) -> bool {
        bytes_eq(&self.method, &other.method)
            && bytes_eq(&self.target, &other.target)
            && tuple_eq(&self.headers, &other.headers)
            && bytes_eq(&self.http_version, &other.http_version)
    }
}

impl Eq for PyRequest {}

#[derive(Debug)]
struct PyResponseValue {
    status_code: u16,
    reason: Py<PyBytes>,
    headers: Py<PyTuple>,
    http_version: Py<PyBytes>,
}

impl PyResponseValue {
    fn from_core(py: Python<'_>, value: core::Response) -> PyResult<Self> {
        Ok(Self {
            status_code: value.status.as_u16(),
            reason: PyBytes::new(py, &value.reason).unbind(),
            headers: py_headers(py, &value.headers)?.unbind(),
            http_version: py_version(py, value.http_version).unbind(),
        })
    }
}

impl PartialEq for PyResponseValue {
    fn eq(&self, other: &Self) -> bool {
        self.status_code == other.status_code
            && bytes_eq(&self.reason, &other.reason)
            && tuple_eq(&self.headers, &other.headers)
            && bytes_eq(&self.http_version, &other.http_version)
    }
}

impl Eq for PyResponseValue {}

/// A parsed non-final `1xx` response head.
///
/// Attributes:
///     status_code (int): Informational status code.
///     reason (bytes): Reason-phrase bytes.
///     headers (tuple[tuple[bytes, bytes], ...]): Ordered `(name, value)` byte pairs.
///     http_version (bytes): Peer HTTP version bytes.
#[pyclass(
    name = "InformationalResponse",
    module = "h11r",
    frozen,
    eq,
    skip_from_py_object
)]
#[derive(Debug)]
struct PyInformationalResponse(PyResponseValue);

impl PartialEq for PyInformationalResponse {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for PyInformationalResponse {}

#[pymethods]
impl PyInformationalResponse {
    /// The informational status code.
    #[getter]
    fn status_code(&self) -> u16 {
        self.0.status_code
    }
    /// The reason phrase as bytes.
    #[getter]
    fn reason(&self, py: Python<'_>) -> Py<PyBytes> {
        self.0.reason.clone_ref(py)
    }
    /// Ordered `(name, value)` byte pairs.
    #[getter]
    fn headers(&self, py: Python<'_>) -> Py<PyTuple> {
        self.0.headers.clone_ref(py)
    }
    /// The peer HTTP version as bytes.
    #[getter]
    fn http_version(&self, py: Python<'_>) -> Py<PyBytes> {
        self.0.http_version.clone_ref(py)
    }
}

/// A parsed final response head.
///
/// Attributes:
///     status_code (int): Final status code.
///     reason (bytes): Reason-phrase bytes.
///     headers (tuple[tuple[bytes, bytes], ...]): Ordered `(name, value)` byte pairs.
///     http_version (bytes): Peer HTTP version bytes.
#[pyclass(name = "Response", module = "h11r", frozen, eq, skip_from_py_object)]
#[derive(Debug)]
struct PyResponse(PyResponseValue);

impl PartialEq for PyResponse {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for PyResponse {}

#[pymethods]
impl PyResponse {
    /// The final status code.
    #[getter]
    fn status_code(&self) -> u16 {
        self.0.status_code
    }
    /// The reason phrase as bytes.
    #[getter]
    fn reason(&self, py: Python<'_>) -> Py<PyBytes> {
        self.0.reason.clone_ref(py)
    }
    /// Ordered `(name, value)` byte pairs.
    #[getter]
    fn headers(&self, py: Python<'_>) -> Py<PyTuple> {
        self.0.headers.clone_ref(py)
    }
    /// The peer HTTP version as bytes.
    #[getter]
    fn http_version(&self, py: Python<'_>) -> Py<PyBytes> {
        self.0.http_version.clone_ref(py)
    }
}

/// A decoded piece of message body.
///
/// `chunk_start` and `chunk_end` preserve HTTP chunk boundaries when the peer
/// used chunked transfer coding.
///
/// Attributes:
///     data (bytes): Decoded body bytes.
///     chunk_start (bool): Whether this data begins an HTTP chunk.
///     chunk_end (bool): Whether this data ends an HTTP chunk.
#[pyclass(
    name = "Data",
    module = "h11r",
    frozen,
    eq,
    get_all,
    skip_from_py_object
)]
#[derive(Debug)]
struct PyData {
    /// The decoded body bytes.
    data: Py<PyBytes>,
    /// Whether this data begins an HTTP chunk.
    chunk_start: bool,
    /// Whether this data ends an HTTP chunk.
    chunk_end: bool,
}

impl PartialEq for PyData {
    fn eq(&self, other: &Self) -> bool {
        bytes_eq(&self.data, &other.data)
            && self.chunk_start == other.chunk_start
            && self.chunk_end == other.chunk_end
    }
}

impl Eq for PyData {}

/// The end of one HTTP message, optionally carrying trailer fields.
///
/// `EndOfMessage()` creates an event without trailers. Pass an iterable of
/// `(name, value)` pairs to construct one with trailers.
///
/// Attributes:
///     trailers (tuple[tuple[bytes, bytes], ...]): Ordered trailer `(name, value)`
///         byte pairs.
#[pyclass(
    name = "EndOfMessage",
    module = "h11r",
    frozen,
    eq,
    get_all,
    skip_from_py_object
)]
#[derive(Debug)]
struct PyEndOfMessage {
    /// Ordered trailer `(name, value)` byte pairs.
    trailers: Py<PyTuple>,
}

impl PartialEq for PyEndOfMessage {
    fn eq(&self, other: &Self) -> bool {
        tuple_eq(&self.trailers, &other.trailers)
    }
}

impl Eq for PyEndOfMessage {}

#[pymethods]
impl PyEndOfMessage {
    #[new]
    #[pyo3(signature = (trailers = None))]
    fn new(py: Python<'_>, trailers: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let trailers = extract_headers(py, trailers)?;
        Ok(Self {
            trailers: py_headers(py, &trailers)?.unbind(),
        })
    }
}

/// Clean transport EOF from the peer.
#[pyclass(
    name = "ConnectionClosed",
    module = "h11r",
    frozen,
    eq,
    skip_from_py_object
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PyConnectionClosed;

#[pymethods]
impl PyConnectionClosed {
    #[new]
    fn new() -> Self {
        Self
    }
}

/// A Sans-I/O HTTP/1 connection.
///
/// Supply transport bytes with `receive_data()`, poll semantic events with
/// `next_event()`, and write bytes returned by the send methods. Client and
/// server state are tracked together regardless of the local role.
///
/// Construct a connection as:
///
/// ```text
/// Connection(
///     role,
///     *,
///     max_head_bytes=65536,
///     max_header_count=100,
/// )
/// ```
///
/// `role` selects the local client or server endpoint. `max_head_bytes`
/// defaults to 65,536 and limits each inbound head or trailer section,
/// including an incomplete chunk-size line. `max_header_count` defaults to 100
/// and limits each inbound head or trailer section's field count.
///
/// Attributes:
///     local_state (State): The local protocol actor's lifecycle state.
///     peer_state (State): The peer protocol actor's lifecycle state.
///     peer_http_version (bytes | None): The most recently parsed peer HTTP
///         version, if any.
///     client_is_waiting_for_100_continue (bool): Whether the client is waiting
///         for `100 Continue`.
///     trailing_data (tuple[bytes, bool]): Bytes retained beyond the HTTP
///         boundary and whether transport EOF was received.
///
/// Raises:
///     ValueError: If any inbound limit is zero.
#[pyclass(name = "Connection", module = "h11r")]
#[derive(Debug)]
struct PyConnection(core::Connection);

#[pymethods]
impl PyConnection {
    #[new]
    #[pyo3(signature = (role, *, max_head_bytes = 65536, max_header_count = 100))]
    fn new(role: PyRole, max_head_bytes: usize, max_header_count: usize) -> PyResult<Self> {
        let limits = core::Limits::new(max_head_bytes, max_header_count)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self(core::Connection::new(role.into(), limits)))
    }

    /// Append bytes received from the peer; empty bytes mark EOF.
    ///
    /// Args:
    ///     data (bytes | bytearray | memoryview): Bytes read from the transport.
    ///
    /// Raises:
    ///     TypeError: If `data` does not implement the buffer protocol.
    ///     LocalProtocolError: If non-empty data follows EOF.
    fn receive_data(&mut self, data: &Bound<'_, PyAny>) -> PyResult<()> {
        with_buffer_bytes(data, |bytes| {
            self.0.receive_data(bytes).map_err(local_error)
        })?
    }

    /// Return the next peer event or a receive status.
    ///
    /// Returns:
    ///     event_or_status (Request | InformationalResponse | Response | Data | EndOfMessage | ConnectionClosed | ReceiveStatus):
    ///         A protocol event, `ReceiveStatus.NEED_DATA` when more transport
    ///         bytes are required, or `ReceiveStatus.PAUSED` at a cycle or
    ///         switch boundary.
    ///
    /// Raises:
    ///     RemoteProtocolError: If peer bytes violate HTTP syntax, framing,
    ///         configured limits, or protocol state.
    fn next_event(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self
            .0
            .next_event()
            .map_err(|error| remote_error(py, error))?
        {
            core::NextEvent::NeedData => py_member::<PyReceiveStatus>(py, "NEED_DATA"),
            core::NextEvent::Paused => py_member::<PyReceiveStatus>(py, "PAUSED"),
            core::NextEvent::Event(event) => event_to_py(py, event),
        }
    }

    /// Serialize a request head.
    ///
    /// Args:
    ///     method (str | bytes | bytearray | memoryview): Case-sensitive ASCII
    ///         HTTP method.
    ///     target (str | bytes | bytearray | memoryview): ASCII request target.
    ///         See RFC 9112 Section 3.2.
    ///     headers (Iterable): `(name, value)` pairs. Each item accepts ASCII
    ///         `str`, `bytes`, `bytearray`, or `memoryview`.
    ///     http_version (str | bytes | bytearray | memoryview | None): `b"1.1"`
    ///         (default) or `b"1.0"`.
    ///
    /// Returns:
    ///     wire_bytes (bytes): Bytes for the caller to write to the transport.
    ///
    /// Raises:
    ///     TypeError: If a header is not a two-item tuple or an input is not
    ///         text or a buffer.
    ///     ValueError: If text is non-ASCII, the method is invalid, or the HTTP
    ///         version is unsupported.
    ///     LocalProtocolError: If the head or current state is invalid.
    #[pyo3(signature = (method, target, headers, *, http_version = None))]
    fn send_request(
        &mut self,
        py: Python<'_>,
        method: &Bound<'_, PyAny>,
        target: &Bound<'_, PyAny>,
        headers: &Bound<'_, PyAny>,
        http_version: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyBytes>> {
        let request = core::Request {
            method: extract_method(method)?,
            target: text_or_buffer(py, target)?,
            headers: extract_headers(py, Some(headers))?,
            http_version: extract_version(py, http_version)?,
        };
        self.0
            .send_request(&request)
            .map(|bytes| PyBytes::new(py, &bytes).unbind())
            .map_err(local_error)
    }

    /// Serialize an informational response head.
    ///
    /// Args:
    ///     status_code (int): A status in the range 100 through 199.
    ///     headers (Iterable | None): Optional `(name, value)` pairs. Each item
    ///         accepts ASCII `str`, `bytes`, `bytearray`, or `memoryview`.
    ///     reason (str | bytes | bytearray | memoryview | None): Optional ASCII
    ///         reason phrase.
    ///     http_version (str | bytes | bytearray | memoryview | None): `b"1.1"`
    ///         (default) or `b"1.0"`.
    ///
    /// Returns:
    ///     wire_bytes (bytes): Bytes for the caller to write to the transport.
    ///
    /// Raises:
    ///     TypeError: If a header is not a two-item tuple or an input is not
    ///         text or a buffer.
    ///     ValueError: If the status, text, or HTTP version is invalid.
    ///     LocalProtocolError: If the response or current state is invalid.
    #[pyo3(signature = (status_code, headers = None, *, reason = None, http_version = None))]
    fn send_informational_response(
        &mut self,
        py: Python<'_>,
        status_code: u16,
        headers: Option<&Bound<'_, PyAny>>,
        reason: Option<&Bound<'_, PyAny>>,
        http_version: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyBytes>> {
        let response = core::InformationalResponse {
            status: status(status_code)?,
            reason: optional_bytes(py, reason)?,
            headers: extract_headers(py, headers)?,
            http_version: extract_version(py, http_version)?,
        };
        self.0
            .send_informational_response(&response)
            .map(|bytes| PyBytes::new(py, &bytes).unbind())
            .map_err(local_error)
    }

    /// Serialize a final response head.
    ///
    /// Args:
    ///     status_code (int): A status in the range 200 through 599.
    ///     headers (Iterable | None): Optional `(name, value)` pairs. Each item
    ///         accepts ASCII `str`, `bytes`, `bytearray`, or `memoryview`.
    ///     reason (str | bytes | bytearray | memoryview | None): Optional ASCII
    ///         reason phrase.
    ///     http_version (str | bytes | bytearray | memoryview | None): `b"1.1"`
    ///         (default) or `b"1.0"`.
    ///
    /// Returns:
    ///     wire_bytes (bytes): Bytes for the caller to write to the transport.
    ///
    /// Raises:
    ///     TypeError: If a header is not a two-item tuple or an input is not
    ///         text or a buffer.
    ///     ValueError: If the status, text, or HTTP version is invalid.
    ///     LocalProtocolError: If the response or current state is invalid.
    #[pyo3(signature = (status_code, headers = None, *, reason = None, http_version = None))]
    fn send_response(
        &mut self,
        py: Python<'_>,
        status_code: u16,
        headers: Option<&Bound<'_, PyAny>>,
        reason: Option<&Bound<'_, PyAny>>,
        http_version: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyBytes>> {
        let response = core::Response {
            status: status(status_code)?,
            reason: optional_bytes(py, reason)?,
            headers: extract_headers(py, headers)?,
            http_version: extract_version(py, http_version)?,
        };
        self.0
            .send_response(&response)
            .map(|bytes| PyBytes::new(py, &bytes).unbind())
            .map_err(local_error)
    }

    /// Serialize body data into one bytes object.
    ///
    /// Args:
    ///     data (bytes | bytearray | memoryview): Body bytes.
    ///
    /// Returns:
    ///     wire_bytes (bytes): Framed bytes for the caller to write to the
    ///         transport.
    ///
    /// Raises:
    ///     TypeError: If `data` does not implement the buffer protocol.
    ///     LocalProtocolError: If body data is forbidden or violates framing.
    fn send_data(&mut self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<Py<PyBytes>> {
        with_buffer_bytes(data, |data| self.0.send_data(data))?
            .map(|bytes| PyBytes::new(py, &bytes).unbind())
            .map_err(local_error)
    }

    /// Return `(prefix, original_object, suffix)` without copying body bytes.
    ///
    /// Contiguous buffers use their byte length. Other objects must expose
    /// `nbytes` as the exact number of body bytes represented by the object.
    ///
    /// Write the prefix, exactly the determined number of body bytes, and the
    /// suffix in order. After a partial body write, resume until the body and
    /// suffix are complete or discard the connection.
    ///
    /// Args:
    ///     data (object): A contiguous buffer or an object with an integer
    ///         `nbytes` property.
    ///
    /// Returns:
    ///     parts (tuple[bytes, object, bytes]): Framing prefix, the identical
    ///         input object, and framing suffix.
    ///
    /// Raises:
    ///     TypeError: If `data` is neither a buffer nor byte-sized, or if
    ///         `nbytes` is not an integer.
    ///     ValueError: If the buffer is not contiguous.
    ///     OverflowError: If `nbytes` is negative or does not fit in the
    ///         platform's address size.
    ///     LocalProtocolError: If body data is forbidden or violates framing.
    fn send_data_parts(
        &mut self,
        py: Python<'_>,
        data: Py<PyAny>,
    ) -> PyResult<(Py<PyBytes>, Py<PyAny>, Py<PyBytes>)> {
        let length = body_nbytes(data.bind(py))?;
        let (prefix, suffix) = self.0.send_data_framing(length).map_err(local_error)?;
        Ok((
            PyBytes::new(py, &prefix).unbind(),
            data,
            PyBytes::new(py, &suffix).unbind(),
        ))
    }

    /// Finish the current message and return its terminating framing.
    ///
    /// Core fields that require header-time processing are rejected in
    /// trailers. Extension trailer fields remain the caller's responsibility.
    ///
    /// Args:
    ///     trailers (Iterable | None): Optional `(name, value)` pairs. Each item
    ///         accepts ASCII `str`, `bytes`, `bytearray`, or `memoryview`.
    ///
    /// Returns:
    ///     wire_bytes (bytes): Message terminator bytes for the caller to write
    ///         to the transport.
    ///
    /// Raises:
    ///     TypeError: If a trailer is not a two-item tuple or an input is not
    ///         text or a buffer.
    ///     ValueError: If trailer text is non-ASCII.
    ///     LocalProtocolError: If framing is incomplete or forbids trailers.
    #[pyo3(signature = (trailers = None))]
    fn end_of_message(
        &mut self,
        py: Python<'_>,
        trailers: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyBytes>> {
        let trailers = extract_headers(py, trailers)?;
        self.0
            .end_of_message(&trailers)
            .map(|bytes| PyBytes::new(py, &bytes).unbind())
            .map_err(local_error)
    }

    /// Reset a completed reusable exchange.
    ///
    /// Raises:
    ///     LocalProtocolError: If either actor is incomplete, reuse is disabled,
    ///         or a protocol switch is pending.
    fn start_next_cycle(&mut self) -> PyResult<()> {
        self.0.start_next_cycle().map_err(local_error)
    }

    /// Mark the local protocol actor as closed.
    ///
    /// Raises:
    ///     LocalProtocolError: If closing is invalid in the current local state.
    fn close(&mut self) -> PyResult<()> {
        self.0.close().map_err(local_error)
    }

    /// The local protocol actor's lifecycle state.
    #[getter]
    fn local_state(&self) -> PyState {
        self.0.local_state().into()
    }
    /// The peer protocol actor's lifecycle state.
    #[getter]
    fn peer_state(&self) -> PyState {
        self.0.peer_state().into()
    }
    /// The most recently parsed peer HTTP version, if any.
    #[getter]
    fn peer_http_version<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.0
            .peer_http_version()
            .map(|version| py_version(py, version))
    }
    /// Whether the client is waiting for `100 Continue`.
    #[getter]
    fn client_is_waiting_for_100_continue(&self) -> bool {
        self.0.client_is_waiting_for_100_continue()
    }
    /// Buffered bytes beyond HTTP plus whether EOF was received.
    #[getter]
    fn trailing_data<'py>(&self, py: Python<'py>) -> (Bound<'py, PyBytes>, bool) {
        let (data, eof) = self.0.trailing_data();
        (PyBytes::new(py, data), eof)
    }
}

fn event_to_py(py: Python<'_>, event: core::Event) -> PyResult<Py<PyAny>> {
    Ok(match event {
        core::Event::Request(value) => Py::new(py, PyRequest::from_core(py, value)?)?.into_any(),
        core::Event::InformationalResponse(value) => Py::new(
            py,
            PyInformationalResponse(PyResponseValue::from_core(py, value)?),
        )?
        .into_any(),
        core::Event::Response(value) => {
            Py::new(py, PyResponse(PyResponseValue::from_core(py, value)?))?.into_any()
        }
        core::Event::Data(value) => Py::new(
            py,
            PyData {
                data: PyBytes::new(py, &value.data).unbind(),
                chunk_start: value.chunk_start,
                chunk_end: value.chunk_end,
            },
        )?
        .into_any(),
        core::Event::EndOfMessage(value) => Py::new(
            py,
            PyEndOfMessage {
                trailers: py_headers(py, &value.trailers)?.unbind(),
            },
        )?
        .into_any(),
        core::Event::ConnectionClosed => Py::new(py, PyConnectionClosed)?.into_any(),
    })
}

fn bytes_eq(left: &Py<PyBytes>, right: &Py<PyBytes>) -> bool {
    Python::attach(|py| left.bind(py).as_bytes() == right.bind(py).as_bytes())
}

fn tuple_eq(left: &Py<PyTuple>, right: &Py<PyTuple>) -> bool {
    Python::attach(|py| left.bind(py).eq(right.bind(py)).unwrap_or(false))
}

fn py_headers<'py>(py: Python<'py>, headers: &[core::Header]) -> PyResult<Bound<'py, PyTuple>> {
    PyTuple::new(
        py,
        headers
            .iter()
            .map(|(name, value)| (PyBytes::new(py, name), PyBytes::new(py, value))),
    )
}

fn extract_headers(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Vec<core::Header>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .try_iter()?
        .map(|item| {
            let item = item?;
            let pair = item
                .cast::<PyTuple>()
                .map_err(|_| PyTypeError::new_err("each header must be a 2-tuple"))?;
            if pair.len() != 2 {
                return Err(PyTypeError::new_err(
                    "each header must contain name and value",
                ));
            }
            Ok((
                text_or_buffer(py, &pair.get_item(0)?)?,
                text_or_buffer(py, &pair.get_item(1)?)?,
            ))
        })
        .collect()
}

fn with_buffer_bytes<R>(value: &Bound<'_, PyAny>, consume: impl FnOnce(&[u8]) -> R) -> PyResult<R> {
    if let Ok(bytes) = value.cast::<PyBytes>() {
        return Ok(consume(bytes.as_bytes()));
    }
    let view = PyMemoryView::from(value)?;
    let bytes = view.call_method0("tobytes")?.cast_into::<PyBytes>()?;
    Ok(consume(bytes.as_bytes()))
}

fn contiguous_buffer_len(value: &Bound<'_, PyAny>) -> PyResult<usize> {
    if let Ok(bytes) = value.cast::<PyBytes>() {
        return Ok(bytes.as_bytes().len());
    }
    let view = PyMemoryView::from(value)?;
    if !view.getattr("contiguous")?.extract::<bool>()? {
        return Err(PyValueError::new_err(
            "send_data_parts requires a contiguous buffer",
        ));
    }
    view.getattr("nbytes")?.extract()
}

fn body_nbytes(value: &Bound<'_, PyAny>) -> PyResult<usize> {
    match contiguous_buffer_len(value) {
        Ok(length) => Ok(length),
        Err(buffer_error) if buffer_error.is_instance_of::<PyTypeError>(value.py()) => {
            match value.getattr("nbytes") {
                Ok(length) => length.extract(),
                Err(error) if error.is_instance_of::<PyAttributeError>(value.py()) => {
                    Err(buffer_error)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn text_or_buffer(_py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(text) = value.cast::<PyString>() {
        let text = text.to_str()?;
        if !text.is_ascii() {
            return Err(PyValueError::new_err("HTTP str inputs must be ASCII"));
        }
        Ok(text.as_bytes().to_vec())
    } else {
        with_buffer_bytes(value, <[u8]>::to_vec)
    }
}

fn extract_method(value: &Bound<'_, PyAny>) -> PyResult<Method> {
    if let Ok(text) = value.cast::<PyString>() {
        let text = text.to_str()?;
        if !text.is_ascii() {
            return Err(PyValueError::new_err("HTTP str inputs must be ASCII"));
        }
        return Method::from_bytes(text.as_bytes())
            .map_err(|error| PyValueError::new_err(error.to_string()));
    }
    with_buffer_bytes(value, Method::from_bytes)?
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

fn optional_bytes(py: Python<'_>, value: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<u8>> {
    value.map_or_else(|| Ok(Vec::new()), |value| text_or_buffer(py, value))
}

fn extract_version(py: Python<'_>, value: Option<&Bound<'_, PyAny>>) -> PyResult<core::Version> {
    match value
        .map(|value| text_or_buffer(py, value))
        .transpose()?
        .as_deref()
    {
        None | Some(b"1.1") => Ok(core::Version::Http11),
        Some(b"1.0") => Ok(core::Version::Http10),
        _ => Err(PyValueError::new_err(
            "http_version must be b'1.0' or b'1.1'",
        )),
    }
}

fn py_version(py: Python<'_>, value: core::Version) -> Bound<'_, PyBytes> {
    PyBytes::new(
        py,
        match value {
            core::Version::Http10 => b"1.0",
            core::Version::Http11 => b"1.1",
        },
    )
}

fn status(value: u16) -> PyResult<StatusCode> {
    StatusCode::from_u16(value).map_err(|error| PyValueError::new_err(error.to_string()))
}
fn local_error(error: core::LocalProtocolError) -> PyErr {
    LocalProtocolError::new_err(error.to_string())
}

fn remote_error(py: Python<'_>, error: core::RemoteProtocolError) -> PyErr {
    let status = error.suggested_status_code();
    let exception = RemoteProtocolError::new_err(error.to_string());
    if let Err(set_error) = exception.value(py).setattr("suggested_status_code", status) {
        return set_error;
    }
    exception
}

fn py_member<T: PyTypeInfo>(py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
    Ok(py.get_type::<T>().getattr(name)?.unbind())
}

pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyRole>()?;
    module.add_class::<PyState>()?;
    module.add_class::<PyReceiveStatus>()?;
    module.add_class::<PyRequest>()?;
    module.add_class::<PyInformationalResponse>()?;
    module.add_class::<PyResponse>()?;
    module.add_class::<PyData>()?;
    module.add_class::<PyEndOfMessage>()?;
    module.add_class::<PyConnectionClosed>()?;
    module.add_class::<PyConnection>()?;
    module.add("ProtocolError", module.py().get_type::<ProtocolError>())?;
    module.add(
        "LocalProtocolError",
        module.py().get_type::<LocalProtocolError>(),
    )?;
    module.add(
        "RemoteProtocolError",
        module.py().get_type::<RemoteProtocolError>(),
    )?;
    Ok(())
}
