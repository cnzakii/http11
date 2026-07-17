"""Handle pipelined HTTP/1.1 requests without reordering responses.

HTTP pipelining lets a client send another request before receiving the first
response. This example focuses on the server side, so the client is represented
by two complete requests written together to a socket.

HTTP requires pipelined responses to stay in request order. One h11r connection
enforces that handoff by returning ``PAUSED`` until response one is complete and
the caller starts the next cycle, at which point request two becomes visible.
"""

from __future__ import annotations

import socket

import h11r


def next_event(connection: h11r.Connection, transport: socket.socket) -> object:
    while True:
        event = connection.next_event()
        if event is h11r.ReceiveStatus.NEED_DATA:
            connection.receive_data(transport.recv(64 * 1024))
            continue
        return event


def receive_request(
    connection: h11r.Connection, transport: socket.socket
) -> h11r.Request:
    """Read one complete request while handling every possible body fragment."""
    request: h11r.Request | None = None

    while True:
        event = next_event(connection, transport)

        if isinstance(event, h11r.Request):
            request = event
        elif isinstance(event, h11r.Data):
            # These GET requests have no body. A general server would stream or
            # collect every Data event here instead of ignoring its contents.
            print(f"server discarded {len(event.data)} unexpected body bytes")
        elif isinstance(event, h11r.EndOfMessage):
            if request is None:
                raise RuntimeError("request ended before its Request event")
            return request
        elif isinstance(event, h11r.ConnectionClosed):
            raise ConnectionError("client closed before finishing the request")
        elif event is h11r.ReceiveStatus.PAUSED:
            raise RuntimeError("connection paused before the request completed")
        else:
            raise RuntimeError(f"unexpected request event: {event!r}")


def send_text_response(
    connection: h11r.Connection, transport: socket.socket, text: str
) -> None:
    body = text.encode("utf-8")
    transport.sendall(
        connection.send_response(
            200,
            [("Content-Type", "text/plain"), ("Content-Length", str(len(body)))],
            reason="OK",
        )
    )
    transport.sendall(connection.send_data(body))
    transport.sendall(connection.end_of_message())


def main() -> None:
    client_socket, server_socket = socket.socketpair()
    client_socket.settimeout(2)
    server_socket.settimeout(2)
    server = h11r.Connection(h11r.Role.SERVER)

    try:
        # One transport write can contain multiple HTTP messages. A server must
        # therefore treat socket reads and protocol events as separate concepts.
        client_socket.sendall(
            b"GET /one HTTP/1.1\r\nHost: example.test\r\n\r\n"
            b"GET /two HTTP/1.1\r\nHost: example.test\r\n\r\n"
        )

        first = receive_request(server, server_socket)
        print(f"server is handling {first.target.decode()}")

        # Request two is already inside h11r's receive buffer. h11r returns
        # PAUSED instead of exposing it early, protecting response ordering.
        boundary = server.next_event()
        if boundary is not h11r.ReceiveStatus.PAUSED:
            raise RuntimeError(f"expected the pipeline boundary, got {boundary!r}")

        send_text_response(server, server_socket, "response one")

        # start_next_cycle() is the deliberate point where the application says
        # response one is complete and request two may become active.
        server.start_next_cycle()
        second = receive_request(server, server_socket)
        print(f"server is now handling {second.target.decode()}")
        send_text_response(server, server_socket, "response two")

        server.start_next_cycle()
        print("both pipelined responses were sent in request order")
    finally:
        client_socket.close()
        server_socket.close()


if __name__ == "__main__":
    main()
