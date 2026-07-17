"""Send one HTTP/1.1 request and response over a synchronous byte stream.

This example keeps a client and a server in one process so the complete flow is
visible in one file. ``socket.socketpair()`` supplies the transport; h11r only
translates between HTTP events and bytes.

A real client and server would own separate sockets and would also need timeout,
cancellation, logging, and application-specific error handling.
"""

from __future__ import annotations

import socket

import h11r


def next_event(connection: h11r.Connection, transport: socket.socket) -> object:
    """Return the next buffered event, reading only when h11r needs data."""
    while True:
        # Always ask h11r first. A previous socket read may have contained
        # several HTTP events, so reading again here could block unnecessarily.
        event = connection.next_event()

        if event is h11r.ReceiveStatus.NEED_DATA:
            # recv() returning b"" means EOF. Passing that empty value to h11r
            # lets the protocol engine produce ConnectionClosed or report a
            # truncated message instead of silently losing the close event.
            connection.receive_data(transport.recv(64 * 1024))
            continue

        return event


def receive_request(
    connection: h11r.Connection, transport: socket.socket
) -> tuple[h11r.Request, bytes]:
    """Receive one complete request and collect its body."""
    request: h11r.Request | None = None
    body = bytearray()

    while True:
        event = next_event(connection, transport)

        if isinstance(event, h11r.Request):
            request = event
            print(f"server received {event.method.decode()} {event.target.decode()}")
        elif isinstance(event, h11r.Data):
            # A body may arrive as any number of Data events. Applications must
            # not assume one network read maps to one Data event.
            body.extend(event.data)
        elif isinstance(event, h11r.EndOfMessage):
            if request is None:
                raise RuntimeError("request ended before its Request event")
            return request, bytes(body)
        elif isinstance(event, h11r.ConnectionClosed):
            raise ConnectionError("client closed before finishing the request")
        elif event is h11r.ReceiveStatus.PAUSED:
            raise RuntimeError("connection paused before the request completed")
        else:
            raise RuntimeError(f"unexpected request event: {event!r}")


def receive_response(
    connection: h11r.Connection, transport: socket.socket
) -> tuple[h11r.Response, bytes]:
    """Receive one complete final response and collect its body."""
    response: h11r.Response | None = None
    body = bytearray()

    while True:
        event = next_event(connection, transport)

        if isinstance(event, h11r.InformationalResponse):
            # 1xx responses can precede the final response. This demo does not
            # need to act on them, but a real client should not mistake one for
            # the final response.
            print(f"client received informational response {event.status_code}")
        elif isinstance(event, h11r.Response):
            response = event
        elif isinstance(event, h11r.Data):
            body.extend(event.data)
        elif isinstance(event, h11r.EndOfMessage):
            if response is None:
                raise RuntimeError("response ended before its Response event")
            return response, bytes(body)
        elif isinstance(event, h11r.ConnectionClosed):
            raise ConnectionError("server closed before finishing the response")
        elif event is h11r.ReceiveStatus.PAUSED:
            raise RuntimeError("connection paused before the response completed")
        else:
            raise RuntimeError(f"unexpected response event: {event!r}")


def main() -> None:
    client_socket, server_socket = socket.socketpair()
    client_socket.settimeout(2)
    server_socket.settimeout(2)

    # Each object tracks one endpoint's view of the same HTTP connection.
    client = h11r.Connection(h11r.Role.CLIENT)
    server = h11r.Connection(h11r.Role.SERVER)

    try:
        # Sending methods update h11r's state and return wire bytes. The caller
        # must write every returned byte to its transport in the same order.
        client_socket.sendall(
            client.send_request(
                "POST",
                "/echo",
                [("Host", "example.test"), ("Content-Length", "4")],
            )
        )
        client_socket.sendall(client.send_data(b"ping"))
        client_socket.sendall(client.end_of_message())

        _request, request_body = receive_request(server, server_socket)

        # Echo the request body. Content-Length tells the peer exactly where
        # this response ends, which makes the connection reusable afterward.
        server_socket.sendall(
            server.send_response(
                200,
                [("Content-Length", str(len(request_body)))],
                reason="OK",
            )
        )
        server_socket.sendall(server.send_data(request_body))
        server_socket.sendall(server.end_of_message())

        response, response_body = receive_response(client, client_socket)
        print(f"client received {response.status_code} with {response_body!r}")

        # Keep-alive reuse is explicit. Both the outgoing and incoming message
        # must be complete before either endpoint starts its next cycle.
        client.start_next_cycle()
        server.start_next_cycle()
        print("connection is ready for another request")
    finally:
        client_socket.close()
        server_socket.close()


if __name__ == "__main__":
    main()
