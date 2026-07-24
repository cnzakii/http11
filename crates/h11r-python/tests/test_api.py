from __future__ import annotations

import h11r
import pytest


@pytest.mark.parametrize(
    ("method", "target"),
    [
        pytest.param("GET", "/items?q=1", id="origin-form"),
        pytest.param("GET", "https://example.test/items", id="absolute-form"),
        pytest.param("CONNECT", "example.test:443", id="authority-form"),
        pytest.param("OPTIONS", "*", id="asterisk-form"),
    ],
)
def test_request_target_forms_cross_the_python_boundary(
    method: str, target: str
) -> None:
    # RFC 9112 Section 3.2 defines the four request-target forms. This verifies
    # that the Python/Rust boundary preserves each form as bytes.
    # https://www.rfc-editor.org/rfc/rfc9112.html#section-3.2
    connection = h11r.Connection(h11r.Role.CLIENT)
    wire = connection.send_request(method, target, [("Host", "example.test")])
    assert wire.startswith(f"{method} {target} HTTP/1.1\r\n".encode())

    server = h11r.Connection(h11r.Role.SERVER)
    server.receive_data(wire)
    request = server.next_event()
    assert isinstance(request, h11r.Request)
    assert request.target == target.encode()


@pytest.mark.parametrize("target", ["", "/bad target", b"/bad\x7f", b"/bad\xff"])
def test_request_target_rejects_bytes_outside_its_field_boundary(
    target: bytes | str,
) -> None:
    connection = h11r.Connection(h11r.Role.CLIENT)
    with pytest.raises(h11r.LocalProtocolError):
        connection.send_request("GET", target, [("Host", "example.test")])


def test_response_octets_and_folded_fields_cross_the_python_boundary() -> None:
    # RFC 9112 Sections 4 and 5.2 permit obs-text in reason-phrase and require
    # a user agent to replace response obs-fold with SP.
    # https://www.rfc-editor.org/rfc/rfc9112.html#section-4
    # https://www.rfc-editor.org/rfc/rfc9112.html#section-5.2
    connection = h11r.Connection(h11r.Role.CLIENT)
    connection.send_request("GET", "/", [("Host", "example.test")])
    connection.end_of_message()
    connection.receive_data(b"HTTP/1.1 200 OK\xff\r\nX-Test: one\r\n two\r\n\r\n")

    response = connection.next_event()
    assert isinstance(response, h11r.Response)
    assert response.reason == b"OK\xff"
    assert response.headers == ((b"X-Test", b"one two"),)


def test_request_framing_errors_keep_the_rfc_status_across_python() -> None:
    # RFC 9112 Section 6.1 recommends 501 for an unsupported request transfer
    # coding. The Python exception must retain that Rust protocol diagnosis.
    # https://www.rfc-editor.org/rfc/rfc9112.html#section-6.1
    connection = h11r.Connection(h11r.Role.SERVER)
    connection.receive_data(
        b"POST / HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: gzip\r\n\r\n"
    )
    with pytest.raises(h11r.RemoteProtocolError) as raised:
        connection.next_event()
    assert raised.value.suggested_status_code == 501


def test_python_text_and_buffer_inputs_preserve_http_octets() -> None:
    # RFC 9110 Section 5.5 permits obs-text in a field value. Python str is an
    # ASCII convenience input; bytes remain the lossless protocol form.
    # https://www.rfc-editor.org/rfc/rfc9110.html#section-5.5
    connection = h11r.Connection(h11r.Role.CLIENT)
    wire = connection.send_request(
        b"GET",
        memoryview(b"/"),
        (("Host", "example.test"), (b"X-Octet", b"\xff")),
    )
    assert b"X-Octet: \xff\r\n" in wire

    invalid_text = h11r.Connection(h11r.Role.CLIENT)
    with pytest.raises(ValueError, match="ASCII"):
        invalid_text.send_request("GET", "/", (("Host", "é.example"),))

    invalid_pair = h11r.Connection(h11r.Role.CLIENT)
    with pytest.raises(TypeError, match="2-tuple"):
        invalid_pair.send_request("GET", "/", [["Host", "example.test"]])


def test_receive_data_accepts_reused_receive_buffer() -> None:
    # The socket.recv_into() pattern reads into one reused bytearray and hands
    # the filled prefix over as a memoryview, without a bytes object per read.
    request = b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n"
    buffer = bytearray(16)
    connection = h11r.Connection(h11r.Role.SERVER)
    for start in range(0, len(request), len(buffer)):
        chunk = request[start : start + len(buffer)]
        buffer[: len(chunk)] = chunk
        connection.receive_data(memoryview(buffer)[: len(chunk)])

    request_event = connection.next_event()
    assert isinstance(request_event, h11r.Request)
    assert request_event.headers == ((b"Host", b"example.test"),)


def test_buffer_inputs_must_be_contiguous() -> None:
    connection = h11r.Connection(h11r.Role.SERVER)
    with pytest.raises(ValueError, match="C-contiguous"):
        connection.receive_data(memoryview(b"GET / HTTP/1.1\r\n\r\n")[::2])


def test_direct_api_and_receive_events() -> None:
    connection = h11r.Connection(h11r.Role.SERVER)
    connection.receive_data(b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n")

    request = connection.next_event()
    assert isinstance(request, h11r.Request)
    assert request.method == b"GET"
    assert request.target == b"/"
    assert request.headers == ((b"Host", b"example.test"),)
    assert connection.next_event() == h11r.EndOfMessage()
    assert connection.next_event() is h11r.ReceiveStatus.NEED_DATA

    assert connection.send_response(204, reason=b"No Content") == (
        b"HTTP/1.1 204 No Content\r\n\r\n"
    )
    assert connection.end_of_message() == b""
    assert connection.local_state is h11r.State.DONE
    assert connection.peer_state is h11r.State.DONE


def test_continue_body_trailers_and_reuse_cross_the_python_boundary() -> None:
    # RFC 9110 Section 10.1.1 defines 100-continue, while RFC 9112 Section
    # 7.1.2 carries trailer fields at the end of a chunked message.
    # https://www.rfc-editor.org/rfc/rfc9110.html#section-10.1.1
    # https://www.rfc-editor.org/rfc/rfc9112.html#section-7.1.2
    client = h11r.Connection(h11r.Role.CLIENT)
    server = h11r.Connection(h11r.Role.SERVER)
    request_wire = client.send_request(
        "POST",
        "/upload",
        [
            ("Host", "example.test"),
            ("Transfer-Encoding", "chunked"),
            ("Expect", "100-continue"),
        ],
    )
    assert client.client_is_waiting_for_100_continue

    server.receive_data(request_wire)
    assert isinstance(server.next_event(), h11r.Request)
    assert server.peer_http_version == b"1.1"
    assert server.client_is_waiting_for_100_continue

    client.receive_data(server.send_informational_response(100, reason="Continue"))
    informational = client.next_event()
    assert isinstance(informational, h11r.InformationalResponse)
    assert informational.status_code == 100
    assert not client.client_is_waiting_for_100_continue
    assert not server.client_is_waiting_for_100_continue

    body_wire = client.send_data(bytearray(b"body"))
    body_wire += client.end_of_message([("Digest", "ok")])
    server.receive_data(body_wire)
    body = server.next_event()
    end = server.next_event()
    assert isinstance(body, h11r.Data) and body.data == b"body"
    assert isinstance(end, h11r.EndOfMessage)
    assert end.trailers == ((b"Digest", b"ok"),)

    client.receive_data(server.send_response(204) + server.end_of_message())
    assert isinstance(client.next_event(), h11r.Response)
    assert isinstance(client.next_event(), h11r.EndOfMessage)
    client.start_next_cycle()
    server.start_next_cycle()
    assert client.local_state is client.peer_state is h11r.State.IDLE
    assert server.local_state is server.peer_state is h11r.State.IDLE


def test_upgrade_pause_and_trailing_data_cross_the_python_boundary() -> None:
    # RFC 9110 Section 7.8 leaves bytes after a successful 101 to the selected
    # protocol rather than the HTTP parser.
    # https://www.rfc-editor.org/rfc/rfc9110.html#section-7.8
    client = h11r.Connection(h11r.Role.CLIENT)
    server = h11r.Connection(h11r.Role.SERVER)
    request_wire = client.send_request(
        "GET",
        "/chat",
        [
            ("Host", "example.test"),
            ("Connection", "upgrade"),
            ("Upgrade", "next-protocol"),
        ],
    )
    request_wire += client.end_of_message()
    server.receive_data(request_wire + b"client-protocol-data")
    assert isinstance(server.next_event(), h11r.Request)
    assert isinstance(server.next_event(), h11r.EndOfMessage)
    assert server.next_event() is h11r.ReceiveStatus.PAUSED
    assert server.trailing_data == (b"client-protocol-data", False)

    switch = server.send_informational_response(
        101,
        [("Connection", "upgrade"), ("Upgrade", "next-protocol")],
        reason="Switching Protocols",
    )
    client.receive_data(switch + b"server-protocol-data")
    informational = client.next_event()
    assert isinstance(informational, h11r.InformationalResponse)
    assert informational.status_code == 101
    assert client.local_state is client.peer_state is h11r.State.SWITCHED_PROTOCOL
    assert server.local_state is server.peer_state is h11r.State.SWITCHED_PROTOCOL
    assert client.trailing_data == (b"server-protocol-data", False)


def test_connection_close_event_crosses_the_python_boundary() -> None:
    # RFC 9112 Section 9.6 makes transport closure terminal.
    # https://www.rfc-editor.org/rfc/rfc9112.html#section-9.6
    connection = h11r.Connection(h11r.Role.SERVER)
    connection.receive_data(b"")
    assert isinstance(connection.next_event(), h11r.ConnectionClosed)
    assert connection.trailing_data == (b"", True)
    assert connection.local_state is h11r.State.MUST_CLOSE
    assert connection.peer_state is h11r.State.CLOSED
    connection.close()
    assert connection.local_state is connection.peer_state is h11r.State.CLOSED


def test_parser_accepted_line_endings_cross_the_python_boundary() -> None:
    # RFC 9112 Section 2.2 permits recipients to recognize LF and requires a
    # server to ignore at least one leading empty request line. The binding must
    # preserve the core parser's decision for every transport split.
    # https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2
    wire = b"\n\nGET / HTTP/1.1\nHost: example.test\n\n"
    for split in range(len(wire) + 1):
        connection = h11r.Connection(h11r.Role.SERVER)
        if split:
            connection.receive_data(wire[:split])
        if split < len(wire):
            connection.receive_data(wire[split:])
        request = connection.next_event()
        assert isinstance(request, h11r.Request)
        assert request.headers == ((b"Host", b"example.test"),)

    client = h11r.Connection(h11r.Role.CLIENT)
    client.send_request("GET", "/", [("Host", "example.test")])
    client.end_of_message()
    client.receive_data(b"HTTP/1.1 204 All Good\nX-Test: one\n two\t\n\n")
    response = client.next_event()
    assert isinstance(response, h11r.Response)
    assert response.reason == b"All Good"
    assert response.headers == ((b"X-Test", b"one two"),)


def test_data_parts_preserve_python_buffer_identity() -> None:
    connection = h11r.Connection(h11r.Role.CLIENT)
    connection.send_request(
        b"POST",
        b"/",
        [(b"Host", b"x"), (b"Transfer-Encoding", b"chunked")],
    )
    body = memoryview(b"body")
    prefix, original, suffix = connection.send_data_parts(body)
    assert prefix == b"4\r\n"
    assert original is body
    assert suffix == b"\r\n"


def test_limits_and_remote_error_status() -> None:
    connection = h11r.Connection(h11r.Role.SERVER, max_header_count=1)
    connection.receive_data(b"GET / HTTP/1.1\r\nHost: x\r\nX: y\r\n\r\n")
    try:
        connection.next_event()
    except h11r.RemoteProtocolError as error:
        assert error.suggested_status_code == 431
    else:
        raise AssertionError("expected RemoteProtocolError")


def test_events_have_value_equality_and_parts_require_contiguous_data() -> None:
    wire = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"
    first = h11r.Connection(h11r.Role.SERVER)
    second = h11r.Connection(h11r.Role.SERVER)
    first.receive_data(wire)
    second.receive_data(wire)
    assert first.next_event() == second.next_event()

    client = h11r.Connection(h11r.Role.CLIENT)
    client.send_request(
        b"POST",
        b"/",
        [(b"Host", b"x"), (b"Transfer-Encoding", b"chunked")],
    )
    try:
        client.send_data_parts(memoryview(b"abcdef")[::2])
    except ValueError:
        pass
    else:
        raise AssertionError("non-contiguous buffers cannot pass through")


def test_event_properties_reuse_their_immutable_python_values() -> None:
    connection = h11r.Connection(h11r.Role.SERVER)
    connection.receive_data(
        b"POST /items HTTP/1.1\r\nHost: example.test\r\nContent-Length: 4\r\n\r\nbody"
    )

    request = connection.next_event()
    assert isinstance(request, h11r.Request)
    assert request.method is request.method
    assert request.target is request.target
    assert request.headers is request.headers

    body = connection.next_event()
    assert isinstance(body, h11r.Data)
    assert body.data is body.data
