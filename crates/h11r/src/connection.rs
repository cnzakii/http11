//! HTTP/1.1 connection lifecycle and semantic event flow.
//!
use crate::state::{ConnectionState, StateEvent, SwitchKind, SwitchProposals};
use crate::wire::*;
use crate::{
    Data, DataParts, EndOfMessage, Event, Header, InformationalResponse, Limits,
    LocalProtocolError, NextEvent, RemoteProtocolError, Request, Response, Role, State, Version,
};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestKind {
    Other,
    Head,
    Connect,
}

impl RequestKind {
    fn from_method(method: &[u8]) -> Self {
        match method {
            b"HEAD" => Self::Head,
            b"CONNECT" => Self::Connect,
            _ => Self::Other,
        }
    }
}

/// One Sans-I/O HTTP/1 connection.
///
/// The connection owns the client and server protocol states, input buffering,
/// message framing, and switch negotiation. It never reads from or writes to a
/// transport: callers provide received bytes and write the bytes returned by
/// the send methods.
#[derive(Debug)]
pub struct Connection {
    role: Role,
    state: ConnectionState,
    limits: Limits,
    input: Buffer,
    progress: Option<SectionProgress>,
    reader: Reader,
    writer: Option<Framing>,
    eof: bool,
    peer_version: Option<Version>,
    request_kind: Option<RequestKind>,
    upgrade_proposals: Vec<Vec<u8>>,
    waiting_for_continue: bool,
    continue_before_upgrade: bool,
}

impl Connection {
    /// Creates an idle connection with independent inbound limits.
    #[must_use]
    pub fn new(role: Role, limits: Limits) -> Self {
        Self {
            role,
            state: ConnectionState::new(),
            limits,
            input: Buffer::default(),
            progress: None,
            reader: Reader::Head,
            writer: None,
            eof: false,
            peer_version: None,
            request_kind: None,
            upgrade_proposals: Vec::new(),
            waiting_for_continue: false,
            continue_before_upgrade: false,
        }
    }

    /// Returns the local actor's lifecycle state.
    #[must_use]
    pub fn local_state(&self) -> State {
        self.state.state(self.role)
    }

    /// Returns the peer actor's lifecycle state.
    #[must_use]
    pub fn peer_state(&self) -> State {
        self.state.state(self.role.peer())
    }

    /// Returns the most recently parsed peer HTTP version.
    #[must_use]
    pub const fn peer_http_version(&self) -> Option<Version> {
        self.peer_version
    }

    /// Whether the client has sent `Expect: 100-continue` but has not begun its body.
    #[must_use]
    pub const fn client_is_waiting_for_100_continue(&self) -> bool {
        self.waiting_for_continue
    }

    /// Returns bytes that belong to a pipelined message or switched protocol,
    /// plus whether transport EOF has already been received.
    #[must_use]
    pub fn trailing_data(&self) -> (&[u8], bool) {
        (self.input.as_slice(), self.eof)
    }

    /// Appends peer bytes. Empty bytes mark transport EOF.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] if non-empty bytes are supplied after
    /// EOF has already been recorded.
    pub fn receive_data(&mut self, data: &[u8]) -> Result<(), LocalProtocolError> {
        if self.eof && !data.is_empty() {
            return Err(local("receive_data called after EOF"));
        }
        if data.is_empty() {
            self.eof = true;
        } else {
            self.input.extend(data);
        }
        Ok(())
    }

    /// Polls one peer event, `NeedData`, or `Paused`.
    ///
    /// # Errors
    ///
    /// Returns [`RemoteProtocolError`] if the peer input violates HTTP syntax,
    /// framing, configured limits, or the current protocol state.
    pub fn next_event(&mut self) -> Result<NextEvent, RemoteProtocolError> {
        let result = self.next_event_inner().map_err(|mut error| {
            if self.role == Role::Client {
                error.suggested_status_code = None;
            }
            error
        });
        if result.is_err() {
            self.state.process_error(self.role.peer());
        }
        result
    }

    fn next_event_inner(&mut self) -> Result<NextEvent, RemoteProtocolError> {
        match self.peer_state() {
            State::Closed => return Ok(NextEvent::Event(Event::ConnectionClosed)),
            State::Idle if self.eof && self.input.as_slice().is_empty() => {
                return self.peer_closed();
            }
            State::MightSwitchProtocol | State::SwitchedProtocol => {
                return Ok(NextEvent::Paused);
            }
            State::Done | State::MustClose if !self.input.as_slice().is_empty() => {
                return Ok(NextEvent::Paused);
            }
            State::Done | State::MustClose => {
                if self.eof {
                    return self.peer_closed();
                }
                return Ok(NextEvent::NeedData);
            }
            State::Error => return Err(remote("peer state is ERROR", None)),
            _ => {}
        }

        match self.reader {
            Reader::Head => self.read_head(),
            Reader::Fixed(0) => self.finish_peer(Vec::new()),
            Reader::Fixed(remaining) => {
                let count = remaining.min(self.input.as_slice().len());
                if count == 0 {
                    return self.need_data_or_eof("unexpected EOF in Content-Length body");
                }
                self.reader = Reader::Fixed(remaining - count);
                let data = self.input.take(count);
                self.peer_data(data, false, false)
            }
            Reader::CloseDelimited => {
                let count = self.input.as_slice().len();
                if count > 0 {
                    let data = self.input.take(count);
                    return self.peer_data(data, false, false);
                }
                if self.eof {
                    return self.finish_peer(Vec::new());
                }
                Ok(NextEvent::NeedData)
            }
            Reader::Chunked(chunk) => self.read_chunked(chunk),
        }
    }

    fn advance_peer(&mut self, event: StateEvent) -> Result<(), RemoteProtocolError> {
        self.state
            .process_event(self.role.peer(), event)
            .map_err(|_| remote("event is invalid in the peer state", Some(400)))
    }

    fn advance_local(&mut self, event: StateEvent) -> Result<(), LocalProtocolError> {
        self.state
            .process_event(self.role, event)
            .map_err(|_| local("event is invalid in the local state"))
    }

    fn request_is(&self, kind: RequestKind) -> bool {
        self.request_kind == Some(kind)
    }

    fn read_head(&mut self) -> Result<NextEvent, RemoteProtocolError> {
        let request = self.role == Role::Server;
        if !self.section_ready(true) {
            return self.wait_for_section(true);
        }
        if request {
            let Some((end, request)) = parse_request_head(
                self.input.as_slice(),
                self.limits.max_head_bytes,
                self.limits.max_header_count,
            )?
            else {
                return self.wait_for_section(true);
            };
            self.input.consume(end);
            self.progress = None;
            self.accept_request(request)
        } else {
            let Some((end, response)) = parse_response_head(
                self.input.as_slice(),
                self.limits.max_head_bytes,
                self.limits.max_header_count,
            )?
            else {
                return self.wait_for_section(true);
            };
            self.input.consume(end);
            self.progress = None;
            self.accept_response(response)
        }
    }

    fn section_ready(&mut self, skip_leading_empty: bool) -> bool {
        self.eof
            || self.progress.as_mut().is_none_or(|progress| {
                progress
                    .poll(self.input.as_slice(), skip_leading_empty)
                    .is_some()
            })
    }

    fn wait_for_section(
        &mut self,
        skip_leading_empty: bool,
    ) -> Result<NextEvent, RemoteProtocolError> {
        let (limit_message, eof_message, status) = if skip_leading_empty {
            (
                "HTTP head exceeds max_head_bytes",
                "unexpected EOF in HTTP head",
                (self.role == Role::Server).then_some(431),
            )
        } else {
            (
                "HTTP trailers exceed max_head_bytes",
                "unexpected EOF in trailers",
                Some(400),
            )
        };
        if self.input.as_slice().len() > self.limits.max_head_bytes {
            return Err(remote(limit_message, status));
        }
        self.progress
            .get_or_insert_default()
            .poll(self.input.as_slice(), skip_leading_empty);
        self.need_data_or_eof(eof_message)
    }

    fn accept_request(&mut self, request: Request) -> Result<NextEvent, RemoteProtocolError> {
        let Request {
            method,
            target,
            headers,
            http_version: version,
        } = request;
        if !valid_request_target(&target) {
            return Err(remote("invalid request target", Some(400)));
        }
        if !valid_host(version, &headers) {
            return Err(remote(
                "request has a missing, duplicate, or invalid Host field",
                Some(400),
            ));
        }
        let framing = framing(&headers, version)?;
        let upgrades = self.proposed_upgrades(version, &headers);
        let proposals =
            SwitchProposals::from_flags(method.as_bytes() == b"CONNECT", !upgrades.is_empty());
        self.advance_peer(StateEvent::Request(proposals))?;
        self.apply_persistence(version, &headers);
        self.request_kind = Some(RequestKind::from_method(method.as_bytes()));
        self.waiting_for_continue = expects_continue(version, &headers, framing);
        self.continue_before_upgrade = self.waiting_for_continue && !upgrades.is_empty();
        self.upgrade_proposals = upgrades;
        self.peer_version = Some(version);
        self.reader = reader(framing);
        Ok(NextEvent::Event(Event::Request(Request {
            method,
            target,
            headers,
            http_version: version,
        })))
    }

    fn accept_response(&mut self, response: Response) -> Result<NextEvent, RemoteProtocolError> {
        let Response {
            status,
            reason,
            headers,
            http_version: version,
        } = response;
        if status.is_informational() {
            if status.as_u16() == 101
                && (self.continue_before_upgrade || !self.valid_upgrade_response(version, &headers))
            {
                return Err(remote("invalid 101 Upgrade response", None));
            }
            let event = if status.as_u16() == 101 {
                StateEvent::ProtocolSwitch(SwitchKind::Upgrade)
            } else {
                StateEvent::InformationalResponse
            };
            self.advance_peer(event)?;
            if status.as_u16() == 100 {
                self.waiting_for_continue = false;
                self.continue_before_upgrade = false;
            }
            self.peer_version = Some(version);
            return Ok(NextEvent::Event(Event::InformationalResponse(
                InformationalResponse {
                    status,
                    reason,
                    headers,
                    http_version: version,
                },
            )));
        }
        let connect = self.request_is(RequestKind::Connect) && status.is_success();
        self.advance_peer(if connect {
            StateEvent::ProtocolSwitch(SwitchKind::Connect)
        } else {
            StateEvent::Response
        })?;
        self.apply_persistence(version, &headers);
        self.peer_version = Some(version);
        self.waiting_for_continue = false;
        self.continue_before_upgrade = false;
        self.upgrade_proposals.clear();
        let no_body = status.as_u16() == 204
            || status.as_u16() == 304
            || self.request_is(RequestKind::Head)
            || connect;
        let framing = if no_body {
            Framing::Fixed(0)
        } else {
            response_framing(&headers, version)?
        };
        if framing == Framing::CloseDelimited {
            self.state.disable_keep_alive();
        }
        self.reader = reader(framing);
        Ok(NextEvent::Event(Event::Response(Response {
            status,
            reason,
            headers,
            http_version: version,
        })))
    }

    fn read_chunked(&mut self, chunk: Chunk) -> Result<NextEvent, RemoteProtocolError> {
        match chunk {
            Chunk::Size { scanned } => {
                let Some(end) = find_crlf(self.input.as_slice(), scanned) else {
                    if incomplete_line_len(self.input.as_slice(), 0) > self.limits.max_head_bytes {
                        return Err(remote(
                            "chunk line exceeds max_head_bytes",
                            (self.role == Role::Server).then_some(400),
                        ));
                    }
                    self.reader = Reader::Chunked(Chunk::Size {
                        scanned: self.input.as_slice().len().saturating_sub(1),
                    });
                    return self.need_data_or_eof("unexpected EOF in chunk size");
                };
                if end - 2 > self.limits.max_head_bytes {
                    return Err(remote(
                        "chunk line exceeds max_head_bytes",
                        (self.role == Role::Server).then_some(400),
                    ));
                }
                let line = &self.input.as_slice()[..end - 2];
                let size = parse_chunk_header(line)
                    .ok_or_else(|| remote("invalid chunk size", Some(400)))?;
                self.input.consume(end);
                if size == 0 {
                    self.reader = Reader::Chunked(Chunk::Trailers);
                    self.progress = None;
                } else {
                    self.reader = Reader::Chunked(Chunk::Data {
                        remaining: size,
                        first: true,
                    });
                }
                self.next_event_inner()
            }
            Chunk::Data { remaining, first } => {
                let count = remaining.min(self.input.as_slice().len());
                if count == 0 {
                    return self.need_data_or_eof("unexpected EOF in chunk data");
                }
                let last = count == remaining;
                self.reader = if last {
                    Reader::Chunked(Chunk::DataEnd)
                } else {
                    Reader::Chunked(Chunk::Data {
                        remaining: remaining - count,
                        first: false,
                    })
                };
                let data = self.input.take(count);
                self.peer_data(data, first, last)
            }
            Chunk::DataEnd => {
                if self.input.as_slice().len() < 2 {
                    return self.need_data_or_eof("unexpected EOF after chunk data");
                }
                if &self.input.as_slice()[..2] != b"\r\n" {
                    return Err(remote("chunk data is not followed by CRLF", Some(400)));
                }
                self.input.consume(2);
                self.reader = Reader::Chunked(Chunk::Size { scanned: 0 });
                self.next_event_inner()
            }
            Chunk::Trailers => {
                if !self.section_ready(false) {
                    return self.wait_for_section(false);
                }
                let Some((end, trailers)) = parse_trailers(
                    self.input.as_slice(),
                    self.limits.max_head_bytes,
                    self.limits.max_header_count,
                    self.role == Role::Client,
                )?
                else {
                    return self.wait_for_section(false);
                };
                self.input.consume(end);
                self.progress = None;
                self.finish_peer(trailers)
            }
        }
    }

    fn peer_data(
        &mut self,
        data: Vec<u8>,
        chunk_start: bool,
        chunk_end: bool,
    ) -> Result<NextEvent, RemoteProtocolError> {
        self.advance_peer(StateEvent::Data)?;
        self.waiting_for_continue = false;
        Ok(NextEvent::Event(Event::Data(Data {
            data,
            chunk_start,
            chunk_end,
        })))
    }

    fn finish_peer(&mut self, trailers: Vec<Header>) -> Result<NextEvent, RemoteProtocolError> {
        self.advance_peer(StateEvent::EndOfMessage)?;
        self.reader = Reader::Head;
        Ok(NextEvent::Event(Event::EndOfMessage(EndOfMessage {
            trailers,
        })))
    }

    fn peer_closed(&mut self) -> Result<NextEvent, RemoteProtocolError> {
        self.advance_peer(StateEvent::ConnectionClosed)?;
        Ok(NextEvent::Event(Event::ConnectionClosed))
    }

    fn need_data_or_eof(&mut self, message: &str) -> Result<NextEvent, RemoteProtocolError> {
        if self.eof {
            Err(remote(message, Some(400)))
        } else {
            Ok(NextEvent::NeedData)
        }
    }

    fn apply_persistence(&mut self, version: Version, headers: &[Header]) {
        let persistent = version == Version::Http11 && !has_token(headers, b"connection", b"close");
        if !persistent {
            self.state.disable_keep_alive();
        }
    }

    fn proposed_upgrades(&self, version: Version, headers: &[Header]) -> Vec<Vec<u8>> {
        if version == Version::Http11 && has_token(headers, b"connection", b"upgrade") {
            upgrade_protocols(headers).unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    fn valid_upgrade_response(&self, version: Version, headers: &[Header]) -> bool {
        version == Version::Http11
            && has_token(headers, b"connection", b"upgrade")
            && upgrade_protocols(headers).is_some_and(|selected| {
                !selected.is_empty()
                    && selected.iter().all(|selected| {
                        self.upgrade_proposals
                            .iter()
                            .any(|offered| protocol_matches(offered, selected))
                    })
            })
    }

    /// Serializes a client request head and begins its body.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] for the wrong role or state, an invalid
    /// request target or field, or inconsistent message framing.
    pub fn send_request(&mut self, request: &Request) -> Result<Vec<u8>, LocalProtocolError> {
        if self.role != Role::Client {
            return Err(local("only a client connection can send a request"));
        }
        validate_request(request)?;
        let framing = framing(&request.headers, request.http_version)
            .map_err(|error| local(error.message))?;
        if framing == Framing::Fixed(0) && has_token(&request.headers, b"expect", b"100-continue") {
            return Err(local("100-continue requires request content"));
        }
        let upgrades =
            upgrade_protocols(&request.headers).ok_or_else(|| local("invalid Upgrade field"))?;
        if has_token(&request.headers, b"connection", b"upgrade") == upgrades.is_empty() {
            return Err(local("Upgrade requires Connection: upgrade"));
        }
        let upgrades = if request.http_version == Version::Http11 {
            upgrades
        } else {
            Vec::new()
        };
        let proposals = SwitchProposals::from_flags(
            request.method.as_bytes() == b"CONNECT",
            !upgrades.is_empty(),
        );
        self.advance_local(StateEvent::Request(proposals))?;
        self.apply_persistence(request.http_version, &request.headers);
        self.request_kind = Some(RequestKind::from_method(request.method.as_bytes()));
        self.writer = Some(framing);
        self.waiting_for_continue =
            expects_continue(request.http_version, &request.headers, framing);
        self.continue_before_upgrade = self.waiting_for_continue && !upgrades.is_empty();
        self.upgrade_proposals = upgrades;
        Ok(serialize_request(request))
    }

    /// Serializes a server informational response head.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] for the wrong role or state, an invalid
    /// response, or a protocol switch that was not validly proposed.
    pub fn send_informational_response(
        &mut self,
        response: &InformationalResponse,
    ) -> Result<Vec<u8>, LocalProtocolError> {
        if self.role != Role::Server || !response.status.is_informational() {
            return Err(local(
                "an informational response requires a server role and 1xx status",
            ));
        }
        validate_headers(&response.headers)?;
        validate_reason(&response.reason)?;
        if count_header(&response.headers, b"transfer-encoding")
            + count_header(&response.headers, b"content-length")
            > 0
        {
            return Err(local("informational responses cannot carry framing fields"));
        }
        if response.status.as_u16() == 101
            && (self.continue_before_upgrade
                || !self.valid_upgrade_response(response.http_version, &response.headers))
        {
            return Err(local("invalid 101 Upgrade response"));
        }
        let event = if response.status.as_u16() == 101 {
            StateEvent::ProtocolSwitch(SwitchKind::Upgrade)
        } else {
            StateEvent::InformationalResponse
        };
        self.advance_local(event)?;
        if response.status.as_u16() == 100 {
            self.waiting_for_continue = false;
            self.continue_before_upgrade = false;
        }
        Ok(serialize_response(
            response.http_version,
            response.status,
            &response.reason,
            &response.headers,
        ))
    }

    /// Serializes a final server response head and begins its body.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] for the wrong role or state, an invalid
    /// response, or framing forbidden by the request method or status.
    pub fn send_response(&mut self, response: &Response) -> Result<Vec<u8>, LocalProtocolError> {
        if self.role != Role::Server || response.status.is_informational() {
            return Err(local(
                "a final response requires a server role and non-1xx status",
            ));
        }
        validate_headers(&response.headers)?;
        validate_reason(&response.reason)?;
        let connect = self.request_is(RequestKind::Connect) && response.status.is_success();
        let framing_fields = count_header(&response.headers, b"transfer-encoding")
            + count_header(&response.headers, b"content-length");
        if (response.status.as_u16() == 204 || connect) && framing_fields > 0 {
            return Err(local("this response status cannot carry framing fields"));
        }
        let no_body = response.status.as_u16() == 204
            || response.status.as_u16() == 304
            || self.request_is(RequestKind::Head)
            || connect;
        // Borrow the common path; normalization clones only when fields change.
        let mut headers = Cow::Borrowed(response.headers.as_slice());
        if (self.peer_version != Some(Version::Http11) || response.http_version != Version::Http11)
            && count_header(&headers, b"transfer-encoding") > 0
        {
            headers.to_mut().retain(|(name, _)| {
                !name.eq_ignore_ascii_case(b"transfer-encoding")
                    && !name.eq_ignore_ascii_case(b"content-length")
            });
        }
        let mut force_close = false;
        let framing = if no_body {
            Framing::Fixed(0)
        } else if count_header(&headers, b"transfer-encoding")
            + count_header(&headers, b"content-length")
            == 0
        {
            if self.peer_version == Some(Version::Http11)
                && response.http_version == Version::Http11
            {
                headers
                    .to_mut()
                    .push((b"Transfer-Encoding".to_vec(), b"chunked".to_vec()));
                Framing::Chunked
            } else {
                force_close = true;
                Framing::CloseDelimited
            }
        } else {
            framing(&headers, response.http_version).map_err(|error| local(error.message))?
        };
        if !connect
            && (!self.state.keep_alive()
                || self.peer_version != Some(Version::Http11)
                || response.http_version != Version::Http11
                || force_close
                || has_token(&headers, b"connection", b"close"))
        {
            set_connection_close(headers.to_mut());
        }
        self.advance_local(if connect {
            StateEvent::ProtocolSwitch(SwitchKind::Connect)
        } else {
            StateEvent::Response
        })?;
        self.apply_persistence(response.http_version, headers.as_ref());
        self.writer = Some(framing);
        self.waiting_for_continue = false;
        self.continue_before_upgrade = false;
        self.upgrade_proposals.clear();
        Ok(serialize_response(
            response.http_version,
            response.status,
            &response.reason,
            headers.as_ref(),
        ))
    }

    /// Serializes body bytes according to the selected message framing.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] if no message body is active, the current
    /// state forbids data, or the bytes exceed `Content-Length`.
    pub fn send_data(&mut self, data: &[u8]) -> Result<Vec<u8>, LocalProtocolError> {
        let parts = self.send_data_parts(data)?;
        let mut out = parts.prefix;
        out.extend_from_slice(parts.data);
        out.extend_from_slice(&parts.suffix);
        Ok(out)
    }

    /// Returns framing around the unchanged body object, avoiding a body copy.
    ///
    /// Write the returned prefix, body, and suffix in order without changing
    /// the body until all three writes complete.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] under the same conditions as
    /// [`Connection::send_data`].
    pub fn send_data_parts<T: AsRef<[u8]>>(
        &mut self,
        data: T,
    ) -> Result<DataParts<T>, LocalProtocolError> {
        let (prefix, suffix) = self.send_data_framing(data.as_ref().len())?;
        Ok(DataParts {
            prefix,
            data,
            suffix,
        })
    }

    #[doc(hidden)]
    /// Returns framing bytes for a body object with `length` bytes.
    ///
    /// The caller must write exactly `length` body bytes between the returned
    /// prefix and suffix.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] if the current message cannot accept a
    /// body object of that length.
    pub fn send_data_framing(
        &mut self,
        length: usize,
    ) -> Result<(Vec<u8>, Vec<u8>), LocalProtocolError> {
        let (next, prefix, suffix) = match self
            .writer
            .ok_or_else(|| local("message framing is not initialized"))?
        {
            Framing::Fixed(remaining) if length <= remaining => {
                (Framing::Fixed(remaining - length), Vec::new(), Vec::new())
            }
            Framing::Fixed(_) => return Err(local("body exceeds Content-Length")),
            Framing::CloseDelimited => (Framing::CloseDelimited, Vec::new(), Vec::new()),
            Framing::Chunked if length == 0 => (Framing::Chunked, Vec::new(), Vec::new()),
            Framing::Chunked => (
                Framing::Chunked,
                format!("{length:x}\r\n").into_bytes(),
                b"\r\n".to_vec(),
            ),
        };
        self.advance_local(StateEvent::Data)?;
        self.writer = Some(next);
        self.waiting_for_continue = false;
        Ok((prefix, suffix))
    }

    /// Finishes the local message and serializes any required terminator.
    ///
    /// Core fields that require header-time processing are rejected in
    /// trailers. Callers remain responsible for ensuring that extension field
    /// definitions permit trailer use.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] if the body length is incomplete, the
    /// framing forbids trailers, a trailer field is invalid, or the state does
    /// not permit message completion.
    pub fn end_of_message(&mut self, trailers: &[Header]) -> Result<Vec<u8>, LocalProtocolError> {
        validate_trailers(trailers)?;
        let bytes = match self
            .writer
            .ok_or_else(|| local("message framing is not initialized"))?
        {
            Framing::Fixed(0) | Framing::CloseDelimited if trailers.is_empty() => Vec::new(),
            Framing::Fixed(_) => return Err(local("body is shorter than Content-Length")),
            Framing::CloseDelimited => {
                return Err(local("close-delimited messages cannot carry trailers"));
            }
            Framing::Chunked => {
                let mut out = b"0\r\n".to_vec();
                write_headers(&mut out, trailers);
                out.extend_from_slice(b"\r\n");
                out
            }
        };
        self.advance_local(StateEvent::EndOfMessage)?;
        self.writer = None;
        self.waiting_for_continue = false;
        Ok(bytes)
    }

    /// Resets a completed exchange, including to drain messages buffered before EOF.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] unless both actors completed a reusable
    /// HTTP exchange with no pending switch.
    pub fn start_next_cycle(&mut self) -> Result<(), LocalProtocolError> {
        self.state
            .start_next_cycle()
            .map_err(|_| local("connection is not reusable"))?;
        self.reader = Reader::Head;
        self.writer = None;
        self.progress = None;
        self.request_kind = None;
        self.upgrade_proposals.clear();
        self.waiting_for_continue = false;
        self.continue_before_upgrade = false;
        Ok(())
    }

    /// Marks the local protocol actor as closed.
    ///
    /// # Errors
    ///
    /// Returns [`LocalProtocolError`] if closing is invalid in the current
    /// local state.
    pub fn close(&mut self) -> Result<(), LocalProtocolError> {
        self.advance_local(StateEvent::ConnectionClosed)?;
        self.writer = None;
        Ok(())
    }
}
