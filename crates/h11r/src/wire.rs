//! HTTP/1 wire parsing, message framing, buffering, and serialization.

use crate::{
    DEFAULT_MAX_HEADER_COUNT, Header, LocalProtocolError, RemoteProtocolError, Request, Response,
    Version,
};
use crate::{Method, StatusCode, method::is_token_byte};
use std::{borrow::Cow, mem::MaybeUninit, net::Ipv6Addr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Framing {
    Fixed(usize),
    Chunked,
    CloseDelimited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Chunk {
    Size { scanned: usize },
    Data { remaining: usize, first: bool },
    DataEnd,
    Trailers,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Reader {
    Head,
    Fixed(usize),
    Chunked(Chunk),
    CloseDelimited,
}

#[derive(Debug, Default)]
pub(crate) struct Buffer {
    bytes: Vec<u8>,
    start: usize,
}

impl Buffer {
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes[self.start..]
    }
    pub(crate) fn extend(&mut self, data: &[u8]) {
        // Moving no more live bytes than were discarded keeps compaction
        // amortized linear across adversarial receive boundaries.
        let live = self.bytes.len() - self.start;
        if self.start > 0 && self.start >= live {
            self.bytes.copy_within(self.start.., 0);
            self.bytes.truncate(live);
            self.start = 0;
        }
        self.bytes.extend_from_slice(data);
    }
    pub(crate) fn consume(&mut self, count: usize) {
        self.start += count;
    }
    pub(crate) fn take(&mut self, count: usize) -> Vec<u8> {
        let value = self.as_slice()[..count].to_vec();
        self.consume(count);
        value
    }
}

/// Remembers how far an incomplete field section has been searched.
///
/// This is only a retry hint. `httparse` remains the sole syntax authority.
#[derive(Debug, Default)]
pub(crate) struct SectionProgress {
    at: usize,
    line_start: usize,
    started: bool,
}

impl SectionProgress {
    pub(crate) fn poll(&mut self, data: &[u8], skip_leading_empty: bool) -> Option<usize> {
        while self.at < data.len() {
            self.at += 1;
            if data[self.at - 1] == b'\n' {
                let line = &data[self.line_start..self.at - 1];
                let empty = line.is_empty() || line == b"\r";
                self.line_start = self.at;
                if empty {
                    if self.started || !skip_leading_empty {
                        return Some(self.at);
                    }
                } else {
                    self.started = true;
                }
            }
        }
        None
    }
}

pub(crate) fn reader(framing: Framing) -> Reader {
    match framing {
        Framing::Fixed(size) => Reader::Fixed(size),
        Framing::Chunked => Reader::Chunked(Chunk::Size { scanned: 0 }),
        Framing::CloseDelimited => Reader::CloseDelimited,
    }
}

pub(crate) fn parse_request_head(
    head: &[u8],
    max_bytes: usize,
    max_headers: usize,
) -> Result<Option<(usize, Request)>, RemoteProtocolError> {
    let mut inline = [MaybeUninit::uninit(); DEFAULT_MAX_HEADER_COUNT];
    let mut overflow =
        (max_headers > inline.len()).then(|| vec![MaybeUninit::uninit(); max_headers]);
    let slots = overflow
        .as_deref_mut()
        .unwrap_or_else(|| &mut inline[..max_headers]);
    let mut parsed = httparse::Request::new(&mut []);
    let status = httparse::ParserConfig::default()
        .parse_request_with_uninit_headers(&mut parsed, head, slots)
        .map_err(|error| {
            remote(
                format!("invalid request head: {error}"),
                Some(if error == httparse::Error::TooManyHeaders {
                    431
                } else {
                    400
                }),
            )
        })?;
    let httparse::Status::Complete(end) = status else {
        return Ok(None);
    };
    if end > max_bytes {
        return Err(remote("HTTP head exceeds max_head_bytes", Some(431)));
    }
    let http_version = version(parsed.version)?;
    let method = Method::from_bytes(
        parsed
            .method
            .ok_or_else(|| remote("missing request method", Some(400)))?
            .as_bytes(),
    )
    .map_err(|_| remote("invalid request method", Some(400)))?;
    let target = parsed
        .path
        .ok_or_else(|| remote("missing request target", Some(400)))?
        .as_bytes()
        .to_vec();
    Ok(Some((
        end,
        Request {
            method,
            target,
            headers: copy_headers(parsed.headers, false),
            http_version,
        },
    )))
}

fn normalize_obs_fold(data: &[u8]) -> Cow<'_, [u8]> {
    if !data
        .windows(2)
        .any(|bytes| bytes[0] == b'\n' && matches!(bytes[1], b' ' | b'\t'))
    {
        return Cow::Borrowed(data);
    }

    let mut normalized = Vec::with_capacity(data.len());
    let mut at = 0;
    while at < data.len() {
        if data[at] == b'\n'
            && data
                .get(at + 1)
                .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
        {
            if normalized.last() == Some(&b'\r') {
                normalized.pop();
            }
            normalized.push(b' ');
            at += 1;
            while data
                .get(at)
                .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
            {
                at += 1;
            }
        } else {
            normalized.push(data[at]);
            at += 1;
        }
    }
    Cow::Owned(normalized)
}

pub(crate) fn parse_response_head(
    head: &[u8],
    max_bytes: usize,
    max_headers: usize,
) -> Result<Option<(usize, Response)>, RemoteProtocolError> {
    let mut inline = [MaybeUninit::uninit(); DEFAULT_MAX_HEADER_COUNT];
    let mut overflow =
        (max_headers > inline.len()).then(|| vec![MaybeUninit::uninit(); max_headers]);
    let slots = overflow
        .as_deref_mut()
        .unwrap_or_else(|| &mut inline[..max_headers]);
    let mut parsed = httparse::Response::new(&mut []);
    let mut config = httparse::ParserConfig::default();
    config.allow_obsolete_multiline_headers_in_responses(true);
    let status = config
        .parse_response_with_uninit_headers(&mut parsed, head, slots)
        .map_err(|error| remote(format!("invalid response head: {error}"), None))?;
    let httparse::Status::Complete(end) = status else {
        return Ok(None);
    };
    if end > max_bytes {
        return Err(remote("HTTP head exceeds max_head_bytes", None));
    }
    let http_version = version(parsed.version)?;
    let status = StatusCode::from_u16(
        parsed
            .code
            .ok_or_else(|| remote("missing response status", None))?,
    )
    .map_err(|_| remote("invalid response status", None))?;
    Ok(Some((
        end,
        Response {
            status,
            // httparse exposes the reason phrase as UTF-8 and returns an empty
            // value for obs-text, so preserve the bytes from the validated line.
            // https://github.com/seanmonstar/httparse/blob/v1.10.1/src/lib.rs#L1732-L1739
            reason: first_nonempty_line(&head[..end])
                .get(13..)
                .unwrap_or_default()
                .to_vec(),
            headers: copy_headers(parsed.headers, true),
            http_version,
        },
    )))
}

fn first_nonempty_line(data: &[u8]) -> &[u8] {
    let mut start = 0;
    while let Some(offset) = data[start..].iter().position(|byte| *byte == b'\n') {
        let end = start + offset;
        let content_end = end - usize::from(end > start && data[end - 1] == b'\r');
        let line = &data[start..content_end];
        if !line.is_empty() {
            return line;
        }
        start = end + 1;
    }
    &[]
}

pub(crate) fn version(value: Option<u8>) -> Result<Version, RemoteProtocolError> {
    match value {
        Some(0) => Ok(Version::Http10),
        Some(1) => Ok(Version::Http11),
        _ => Err(remote("unsupported HTTP version", Some(400))),
    }
}

pub(crate) fn copy_headers(headers: &[httparse::Header<'_>], unfold: bool) -> Vec<Header> {
    headers
        .iter()
        .map(|header| {
            (
                header.name.as_bytes().to_vec(),
                if unfold {
                    normalize_obs_fold(header.value).into_owned()
                } else {
                    header.value.to_vec()
                },
            )
        })
        .collect()
}

pub(crate) fn header_values<'a>(
    headers: &'a [Header],
    name: &'a [u8],
) -> impl Iterator<Item = &'a [u8]> {
    headers
        .iter()
        .filter(move |(field, _)| field.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_slice())
}

pub(crate) fn count_header(headers: &[Header], name: &[u8]) -> usize {
    header_values(headers, name).count()
}

pub(crate) fn has_token(headers: &[Header], name: &[u8], token: &[u8]) -> bool {
    header_values(headers, name)
        .flat_map(|value| value.split(|byte| *byte == b','))
        .any(|value| trim_ows(value).eq_ignore_ascii_case(token))
}

pub(crate) fn upgrade_protocols(headers: &[Header]) -> Option<Vec<Vec<u8>>> {
    let present = count_header(headers, b"upgrade") > 0;
    let protocols = header_values(headers, b"upgrade")
        .flat_map(|value| value.split(|byte| *byte == b','))
        .map(trim_ows)
        .filter(|value| !value.is_empty())
        .map(|value| valid_protocol(value).then(|| value.to_vec()))
        .collect::<Option<Vec<_>>>()?;
    (!present || !protocols.is_empty()).then_some(protocols)
}

pub(crate) fn protocol_matches(left: &[u8], right: &[u8]) -> bool {
    let mut left = left.splitn(2, |byte| *byte == b'/');
    let mut right = right.splitn(2, |byte| *byte == b'/');
    left.next()
        .zip(right.next())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        && left.next() == right.next()
}

fn valid_protocol(value: &[u8]) -> bool {
    let mut parts = value.split(|byte| *byte == b'/');
    parts.next().is_some_and(valid_name)
        && parts.next().is_none_or(valid_name)
        && parts.next().is_none()
}

pub(crate) fn set_connection_close(headers: &mut Vec<Header>) {
    let mut options = header_values(headers, b"connection")
        .flat_map(|value| value.split(|byte| *byte == b','))
        .map(trim_ows)
        .filter(|value| {
            !value.is_empty()
                && !value.eq_ignore_ascii_case(b"close")
                && !value.eq_ignore_ascii_case(b"keep-alive")
        })
        .map(Vec::from)
        .collect::<Vec<_>>();
    options.push(b"close".to_vec());
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case(b"connection"));
    headers.push((b"Connection".to_vec(), options.join(&b", "[..])));
}

pub(crate) fn expects_continue(version: Version, headers: &[Header], framing: Framing) -> bool {
    version == Version::Http11
        && framing != Framing::Fixed(0)
        && has_token(headers, b"expect", b"100-continue")
}

pub(crate) fn framing(
    headers: &[Header],
    version: Version,
) -> Result<Framing, RemoteProtocolError> {
    let transfer = header_values(headers, b"transfer-encoding")
        .flat_map(|value| value.split(|byte| *byte == b','))
        .map(trim_ows)
        .collect::<Vec<_>>();
    let lengths = header_values(headers, b"content-length")
        .flat_map(|value| value.split(|byte| *byte == b','))
        .map(trim_ows)
        .collect::<Vec<_>>();
    if !transfer.is_empty() && !lengths.is_empty() {
        return Err(remote(
            "Transfer-Encoding and Content-Length cannot be combined",
            Some(400),
        ));
    }
    if !transfer.is_empty() {
        if version == Version::Http10 {
            return Err(remote(
                "Transfer-Encoding is not valid HTTP/1.0 framing",
                Some(400),
            ));
        }
        if transfer.iter().any(|value| value.is_empty()) {
            return Err(remote("invalid Transfer-Encoding field", Some(400)));
        }
        if transfer.len() == 1 && !transfer[0].eq_ignore_ascii_case(b"chunked") {
            return Err(remote(
                "only Transfer-Encoding: chunked is supported",
                Some(501),
            ));
        }
        if !transfer
            .last()
            .is_some_and(|value| value.eq_ignore_ascii_case(b"chunked"))
            || transfer[..transfer.len() - 1]
                .iter()
                .any(|value| value.eq_ignore_ascii_case(b"chunked"))
        {
            return Err(remote(
                "chunked must be the final transfer coding",
                Some(400),
            ));
        }
        if transfer[..transfer.len() - 1]
            .iter()
            .any(|value| !value.eq_ignore_ascii_case(b"chunked"))
        {
            return Err(remote(
                "only Transfer-Encoding: chunked is supported",
                Some(501),
            ));
        }
        return Ok(Framing::Chunked);
    }
    if lengths.is_empty() {
        return Ok(Framing::Fixed(0));
    }
    let first =
        parse_number(lengths[0], 10).ok_or_else(|| remote("invalid Content-Length", Some(400)))?;
    if lengths
        .iter()
        .any(|value| parse_number(value, 10) != Some(first))
    {
        return Err(remote("conflicting Content-Length fields", Some(400)));
    }
    Ok(Framing::Fixed(first))
}

pub(crate) fn response_framing(
    headers: &[Header],
    version: Version,
) -> Result<Framing, RemoteProtocolError> {
    if count_header(headers, b"transfer-encoding") + count_header(headers, b"content-length") == 0 {
        Ok(Framing::CloseDelimited)
    } else {
        framing(headers, version)
    }
}

pub(crate) fn parse_number(value: &[u8], radix: u32) -> Option<usize> {
    if value.is_empty() {
        return None;
    }
    value.iter().try_fold(0usize, |number, byte| {
        number
            .checked_mul(radix as usize)?
            .checked_add((*byte as char).to_digit(radix)? as usize)
    })
}

pub(crate) fn parse_chunk_header(line: &[u8]) -> Option<usize> {
    let digits = line
        .iter()
        .take_while(|byte| byte.is_ascii_hexdigit())
        .count();
    let size = parse_number(&line[..digits], 16)?;
    let mut at = digits;
    while at < line.len() {
        at = skip_ows(line, at);
        if line.get(at) != Some(&b';') {
            return None;
        }
        at = skip_ows(line, at + 1);
        let name = at;
        while line.get(at).is_some_and(|byte| is_token_byte(*byte)) {
            at += 1;
        }
        if at == name {
            return None;
        }
        let after_name = at;
        at = skip_ows(line, at);
        if line.get(at) == Some(&b'=') {
            at = skip_ows(line, at + 1);
            if line.get(at) == Some(&b'"') {
                at = parse_quoted_string(line, at)?;
            } else {
                let value = at;
                while line.get(at).is_some_and(|byte| is_token_byte(*byte)) {
                    at += 1;
                }
                if at == value {
                    return None;
                }
            }
        } else if at != after_name {
            return None;
        }
    }
    Some(size)
}

pub(crate) fn parse_quoted_string(value: &[u8], mut at: usize) -> Option<usize> {
    at += 1;
    while let Some(&byte) = value.get(at) {
        match byte {
            b'"' => return Some(at + 1),
            b'\\' => {
                at += 1;
                if !value.get(at).is_some_and(|byte| is_line_value_byte(*byte)) {
                    return None;
                }
            }
            b'\t' | b' ' | b'!' | b'#'..=b'[' | b']'..=b'~' | 0x80..=0xff => {}
            _ => return None,
        }
        at += 1;
    }
    None
}

pub(crate) fn skip_ows(value: &[u8], mut at: usize) -> usize {
    while value
        .get(at)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        at += 1;
    }
    at
}

pub(crate) fn find_crlf(data: &[u8], start: usize) -> Option<usize> {
    data.get(start..)?
        .windows(2)
        .position(|pair| pair == b"\r\n")
        .map(|at| start + at + 2)
}

pub(crate) fn incomplete_line_len(data: &[u8], start: usize) -> usize {
    data.len()
        .saturating_sub(start + usize::from(data.last() == Some(&b'\r')))
}

pub(crate) fn parse_trailers(
    data: &[u8],
    max_bytes: usize,
    max_headers: usize,
    normalize_folding: bool,
) -> Result<Option<(usize, Vec<Header>)>, RemoteProtocolError> {
    let original_end = SectionProgress::default().poll(data, false);
    if original_end.unwrap_or(data.len()) > max_bytes {
        return Err(remote("HTTP trailers exceed max_head_bytes", Some(400)));
    }
    let normalized = if normalize_folding {
        normalize_obs_fold(data)
    } else {
        Cow::Borrowed(data)
    };
    let mut inline = [httparse::EMPTY_HEADER; DEFAULT_MAX_HEADER_COUNT];
    let mut overflow =
        (max_headers > inline.len()).then(|| vec![httparse::EMPTY_HEADER; max_headers]);
    let slots = overflow
        .as_deref_mut()
        .unwrap_or_else(|| &mut inline[..max_headers]);
    match httparse::parse_headers(&normalized, slots)
        .map_err(|error| remote(format!("invalid trailer field: {error}"), Some(400)))?
    {
        httparse::Status::Complete((parsed_end, headers)) => Ok(Some((
            original_end.unwrap_or(parsed_end),
            copy_headers(headers, false),
        ))),
        httparse::Status::Partial => Ok(None),
    }
}

pub(crate) fn validate_request(request: &Request) -> Result<(), LocalProtocolError> {
    if !valid_request_target(&request.target) {
        return Err(local("request target must be non-empty visible ASCII"));
    }
    validate_headers(&request.headers)?;
    if !valid_host(request.http_version, &request.headers) {
        return Err(local(
            "request has a missing, duplicate, or invalid Host field",
        ));
    }
    Ok(())
}

pub(crate) fn valid_request_target(target: &[u8]) -> bool {
    !target.is_empty() && target.iter().all(u8::is_ascii_graphic)
}

pub(crate) fn valid_host(version: Version, headers: &[Header]) -> bool {
    let mut hosts = header_values(headers, b"host");
    let first = hosts.next();
    hosts.next().is_none()
        && match first {
            Some(value) => valid_authority(value),
            None => version == Version::Http10,
        }
}

fn valid_authority(value: &[u8]) -> bool {
    let (host, port) = if value.starts_with(b"[") {
        let Some(close) = value.iter().position(|byte| *byte == b']') else {
            return false;
        };
        let literal = &value[1..close];
        if !valid_ip_literal(literal) {
            return false;
        }
        match value.get(close + 1..) {
            Some(b"") => (&value[..=close], None),
            Some(rest) if rest.starts_with(b":") => (&value[..=close], Some(&rest[1..])),
            _ => return false,
        }
    } else {
        if value.iter().filter(|byte| **byte == b':').count() > 1 {
            return false;
        }
        split_at_byte(value, b':').map_or((value, None), |(host, port)| (host, Some(port)))
    };

    let host_valid = host.starts_with(b"[") || valid_reg_name(host);
    host_valid && port.is_none_or(|port| port.iter().all(u8::is_ascii_digit))
}

fn valid_ip_literal(value: &[u8]) -> bool {
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse::<Ipv6Addr>().ok())
        .is_some()
        || (value
            .first()
            .is_some_and(|byte| matches!(byte, b'v' | b'V'))
            && value
                .iter()
                .position(|byte| *byte == b'.')
                .is_some_and(|dot| {
                    dot > 1
                        && value[1..dot].iter().all(u8::is_ascii_hexdigit)
                        && !value[dot + 1..].is_empty()
                        && value[dot + 1..].iter().all(|byte| {
                            is_unreserved(*byte) || is_sub_delim(*byte) || *byte == b':'
                        })
                }))
}

fn valid_reg_name(value: &[u8]) -> bool {
    valid_percent_encoded(value, |byte| is_unreserved(byte) || is_sub_delim(byte))
}

fn valid_percent_encoded(value: &[u8], allowed: impl Fn(u8) -> bool) -> bool {
    let mut at = 0;
    while at < value.len() {
        if value[at] == b'%' {
            if !value
                .get(at + 1..at + 3)
                .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
            {
                return false;
            }
            at += 3;
        } else if allowed(value[at]) {
            at += 1;
        } else {
            return false;
        }
    }
    true
}

fn split_at_byte(value: &[u8], needle: u8) -> Option<(&[u8], &[u8])> {
    let at = value.iter().position(|byte| *byte == needle)?;
    Some((&value[..at], &value[at + 1..]))
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn is_sub_delim(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
    )
}

pub(crate) fn validate_headers(headers: &[Header]) -> Result<(), LocalProtocolError> {
    if headers
        .iter()
        .any(|(name, value)| !valid_name(name) || !valid_value(value))
    {
        Err(local("invalid HTTP header field"))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_reason(reason: &[u8]) -> Result<(), LocalProtocolError> {
    if reason.iter().copied().all(is_line_value_byte) {
        Ok(())
    } else {
        Err(local("invalid HTTP reason phrase"))
    }
}

pub(crate) fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && name.iter().copied().all(is_token_byte)
}

pub(crate) fn valid_value(value: &[u8]) -> bool {
    value.is_empty()
        || (value.first().is_some_and(|byte| is_field_vchar(*byte))
            && value.last().is_some_and(|byte| is_field_vchar(*byte))
            && value.iter().copied().all(is_line_value_byte))
}

pub(crate) fn is_line_value_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | b' '..=b'~' | 0x80..=0xff)
}

fn is_field_vchar(byte: u8) -> bool {
    matches!(byte, b'!'..=b'~' | 0x80..=0xff)
}

pub(crate) fn validate_trailers(trailers: &[Header]) -> Result<(), LocalProtocolError> {
    validate_headers(trailers)?;
    if trailers.iter().any(|(name, _)| {
        [
            b"content-length".as_slice(),
            b"transfer-encoding",
            b"host",
            b"connection",
            b"trailer",
            b"upgrade",
            b"expect",
            b"te",
        ]
        .iter()
        .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
    }) {
        return Err(local("field is not permitted in an HTTP trailer"));
    }
    Ok(())
}

pub(crate) fn trim_ows(mut value: &[u8]) -> &[u8] {
    while value
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        value = &value[1..];
    }
    while value
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        value = &value[..value.len() - 1];
    }
    value
}

pub(crate) fn serialize_request(request: &Request) -> Vec<u8> {
    let mut out = Vec::with_capacity(head_len(
        request.method.as_bytes().len() + request.target.len() + 12,
        &request.headers,
    ));
    out.extend_from_slice(request.method.as_bytes());
    out.push(b' ');
    out.extend_from_slice(&request.target);
    out.extend_from_slice(b" HTTP/");
    out.extend_from_slice(request.http_version.wire());
    out.extend_from_slice(b"\r\n");
    write_headers(&mut out, &request.headers);
    out.extend_from_slice(b"\r\n");
    out
}

pub(crate) fn serialize_response(
    version: Version,
    status: StatusCode,
    reason: &[u8],
    headers: &[Header],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(head_len(15 + reason.len(), headers));
    out.extend_from_slice(b"HTTP/");
    out.extend_from_slice(version.wire());
    out.push(b' ');
    let status = status.as_u16();
    out.extend_from_slice(&[
        b'0' + (status / 100) as u8,
        b'0' + (status / 10 % 10) as u8,
        b'0' + (status % 10) as u8,
    ]);
    out.push(b' ');
    out.extend_from_slice(reason);
    out.extend_from_slice(b"\r\n");
    write_headers(&mut out, headers);
    out.extend_from_slice(b"\r\n");
    out
}

fn head_len(start_line: usize, headers: &[Header]) -> usize {
    start_line
        + headers
            .iter()
            .map(|(name, value)| name.len() + value.len() + 4)
            .sum::<usize>()
        + 2
}

pub(crate) fn write_headers(out: &mut Vec<u8>, headers: &[Header]) {
    for (name, value) in headers {
        out.extend_from_slice(name);
        out.extend_from_slice(b": ");
        out.extend_from_slice(value);
        out.extend_from_slice(b"\r\n");
    }
}

pub(crate) fn local(message: impl Into<String>) -> LocalProtocolError {
    LocalProtocolError(message.into())
}
pub(crate) fn remote(message: impl Into<String>, status: Option<u16>) -> RemoteProtocolError {
    RemoteProtocolError {
        message: message.into(),
        suggested_status_code: status,
    }
}

#[cfg(test)]
mod tests {
    use super::{Buffer, valid_request_target};

    #[test]
    fn buffer_compaction_never_moves_more_live_bytes_than_it_discards() {
        let mut buffer = Buffer {
            bytes: vec![b'a'; 12 * 1024],
            start: 0,
        };

        buffer.consume(4 * 1024 + 1);
        buffer.extend(b"b");
        assert_eq!(buffer.start, 4 * 1024 + 1);

        buffer.consume(4 * 1024);
        let expected = buffer.as_slice().to_vec();
        buffer.extend(b"c");
        assert_eq!(buffer.start, 0);
        assert_eq!(buffer.as_slice(), [expected.as_slice(), b"c"].concat());
    }

    #[test]
    fn request_target_has_a_visible_ascii_boundary() {
        assert!(!valid_request_target(b""));
        for byte in u8::MIN..=u8::MAX {
            assert_eq!(valid_request_target(&[byte]), byte.is_ascii_graphic());
        }
    }
}
