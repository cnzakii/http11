"""Upgrade an HTTP/1.1 connection, then hand it to wsproto.

h11r owns the HTTP/1.1 request, the 101 response, and the exact byte where HTTP
ends. wsproto owns WebSocket frames after that boundary. The byte strings in
this example stand in for transport writes and reads, keeping the handoff easy
to see without also building a network server.

The fixed ``Sec-WebSocket-Key`` makes the output reproducible. A real client
must generate a fresh random 16-byte value for every WebSocket handshake.
"""

from __future__ import annotations

import base64
import binascii
import hashlib
from collections.abc import Iterable

import h11r
from wsproto.connection import Connection as WebSocketConnection
from wsproto.connection import ConnectionType
from wsproto.events import CloseConnection, Ping, TextMessage

WEBSOCKET_GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def header_value(headers: Iterable[tuple[bytes, bytes]], name: bytes) -> bytes:
    """Return one required header value, rejecting missing or repeated fields."""
    values = [value for field, value in headers if field.lower() == name]
    if len(values) != 1:
        raise ValueError(f"expected exactly one {name.decode()} header")
    return values[0]


def contains_token(value: bytes, token: bytes) -> bool:
    """Match a token inside a comma-separated HTTP field value."""
    return token in {part.strip().lower() for part in value.split(b",")}


def contains_header_token(
    headers: Iterable[tuple[bytes, bytes]],
    name: bytes,
    token: bytes,
) -> bool:
    """Match a token across every field line with the given name."""
    return any(
        field.lower() == name and contains_token(value, token)
        for field, value in headers
    )


def websocket_accept(request: h11r.Request) -> bytes:
    """Validate the essential WebSocket request fields and create the accept."""
    if request.method != b"GET" or request.http_version != b"1.1":
        raise ValueError("WebSocket over HTTP/1.1 requires a GET request")

    version = header_value(request.headers, b"sec-websocket-version")
    key = header_value(request.headers, b"sec-websocket-key")

    if not contains_header_token(
        request.headers, b"connection", b"upgrade"
    ) or not contains_header_token(request.headers, b"upgrade", b"websocket"):
        raise ValueError("request did not offer a WebSocket upgrade")
    if version != b"13":
        raise ValueError("only WebSocket version 13 is supported")

    try:
        decoded_key = base64.b64decode(key, validate=True)
    except binascii.Error as error:
        raise ValueError("Sec-WebSocket-Key is not valid base64") from error
    if len(decoded_key) != 16:
        raise ValueError("Sec-WebSocket-Key must decode to 16 bytes")

    return base64.b64encode(hashlib.sha1(key + WEBSOCKET_GUID).digest())


def receive_upgrade_request(connection: h11r.Connection) -> h11r.Request:
    """Consume the complete HTTP request already supplied to h11r."""
    request: h11r.Request | None = None

    while True:
        event = connection.next_event()

        if isinstance(event, h11r.Request):
            request = event
            print(f"server received an Upgrade request for {event.target.decode()}")
        elif isinstance(event, h11r.Data):
            raise ValueError("a WebSocket handshake must not contain a body")
        elif isinstance(event, h11r.EndOfMessage):
            if request is None:
                raise RuntimeError("upgrade ended before its Request event")
            return request
        elif isinstance(event, h11r.ConnectionClosed):
            raise ConnectionError("client closed during the HTTP handshake")
        elif event is h11r.ReceiveStatus.NEED_DATA:
            raise RuntimeError("the example did not supply the complete request")
        elif event is h11r.ReceiveStatus.PAUSED:
            raise RuntimeError("connection paused before the request completed")
        else:
            raise RuntimeError(f"unexpected handshake event: {event!r}")


def receive_switch_response(connection: h11r.Connection) -> h11r.InformationalResponse:
    """Consume a successful 101 response and stop at the protocol boundary."""
    response: h11r.InformationalResponse | None = None

    while True:
        event = connection.next_event()

        if isinstance(event, h11r.InformationalResponse):
            if event.status_code != 101:
                print(f"client received informational response {event.status_code}")
                continue
            response = event
            print("client accepted HTTP 101 Switching Protocols")
        elif event is h11r.ReceiveStatus.PAUSED:
            if response is None:
                raise RuntimeError("HTTP paused without a 101 response")
            return response
        elif event is h11r.ReceiveStatus.NEED_DATA:
            raise RuntimeError("the example did not supply the complete response")
        elif isinstance(event, h11r.ConnectionClosed):
            raise ConnectionError("server closed during the HTTP handshake")
        else:
            raise RuntimeError(f"unexpected handshake event: {event!r}")


def echo_websocket_events(
    connection: WebSocketConnection,
    text_fragments: list[str],
) -> bytes:
    """Handle server-side WebSocket events and return frames to send."""
    outbound = bytearray()

    for event in connection.events():
        if isinstance(event, TextMessage):
            # One WebSocket message may arrive as several TextMessage events.
            # Application code owns reassembly and acts only after the final
            # fragment instead of treating every fragment as a new message.
            text_fragments.append(event.data)
            if event.message_finished:
                message = "".join(text_fragments)
                text_fragments.clear()
                print(f"WebSocket server received text: {message!r}")
                outbound.extend(connection.send(TextMessage(data=f"echo: {message}")))
        elif isinstance(event, Ping):
            # WebSocket Ping and Close events require protocol replies.
            outbound.extend(connection.send(event.response()))
        elif isinstance(event, CloseConnection):
            outbound.extend(connection.send(event.response()))
        else:
            print(f"WebSocket server ignored {event!r}")

    return bytes(outbound)


def print_websocket_events(
    connection: WebSocketConnection,
    text_fragments: list[str],
) -> None:
    for event in connection.events():
        if isinstance(event, TextMessage):
            text_fragments.append(event.data)
            if event.message_finished:
                message = "".join(text_fragments)
                text_fragments.clear()
                print(f"WebSocket client received text: {message!r}")
        else:
            print(f"WebSocket client received {event!r}")


def main() -> None:
    client_http = h11r.Connection(h11r.Role.CLIENT)
    server_http = h11r.Connection(h11r.Role.SERVER)
    client_text_fragments: list[str] = []
    server_text_fragments: list[str] = []
    key = b"dGhlIHNhbXBsZSBub25jZQ=="

    # Step 1: the client sends an ordinary HTTP/1.1 Upgrade request.
    request_wire = client_http.send_request(
        "GET",
        "/chat",
        [
            ("Host", "example.test"),
            ("Connection", "Upgrade"),
            ("Upgrade", "websocket"),
            ("Sec-WebSocket-Key", key),
            ("Sec-WebSocket-Version", "13"),
        ],
    )
    request_wire += client_http.end_of_message()
    server_http.receive_data(request_wire)

    request = receive_upgrade_request(server_http)

    # Once the request ends, h11r pauses because the server must either accept
    # the proposal with 101 or reject it with a final HTTP response.
    boundary = server_http.next_event()
    if boundary is not h11r.ReceiveStatus.PAUSED:
        raise RuntimeError(f"expected the Upgrade boundary, got {boundary!r}")

    accept = websocket_accept(request)
    switch_wire = server_http.send_informational_response(
        101,
        [
            ("Connection", "Upgrade"),
            ("Upgrade", "websocket"),
            ("Sec-WebSocket-Accept", accept),
        ],
        reason="Switching Protocols",
    )

    # Step 2: after sending 101, the server creates the next protocol engine.
    # wsproto's low-level Connection is specifically for an already-completed
    # handshake, so it does not parse or generate another HTTP exchange.
    server_websocket = WebSocketConnection(ConnectionType.SERVER)
    welcome_frame = server_websocket.send(TextMessage(data="welcome"))

    # A transport read may contain both the end of HTTP and the first bytes of
    # the new protocol. Feed the whole read to h11r; it preserves the remainder.
    client_http.receive_data(switch_wire + welcome_frame)
    switch = receive_switch_response(client_http)

    expected_accept = base64.b64encode(hashlib.sha1(key + WEBSOCKET_GUID).digest())
    if header_value(switch.headers, b"sec-websocket-accept") != expected_accept:
        raise ValueError("server returned an invalid Sec-WebSocket-Accept")

    # Step 3: trailing_data is the byte-exact handoff. Give it to wsproto before
    # performing another transport read, otherwise the welcome frame is lost.
    trailing_data, transport_closed = client_http.trailing_data
    if transport_closed:
        raise ConnectionError("transport closed at the Upgrade boundary")
    client_websocket = WebSocketConnection(
        ConnectionType.CLIENT,
        trailing_data=trailing_data,
    )
    print_websocket_events(client_websocket, client_text_fragments)

    # HTTP processing is finished. All following bytes go directly between the
    # transport and wsproto, never back through either h11r connection. Sending
    # two fragments also demonstrates why the event handler reassembles text.
    client_frames = client_websocket.send(
        TextMessage(data="hel", message_finished=False)
    )
    client_frames += client_websocket.send(TextMessage(data="lo"))
    server_websocket.receive_data(client_frames)
    server_reply = echo_websocket_events(server_websocket, server_text_fragments)
    client_websocket.receive_data(server_reply)
    print_websocket_events(client_websocket, client_text_fragments)


if __name__ == "__main__":
    main()
