"""Write an HTTP body without asking h11r to copy the body buffer.

``send_data()`` returns one convenient ``bytes`` object containing framing and
body data. For a large body, ``send_data_parts()`` instead returns the framing
around the original Python buffer so the transport can write the three pieces
in order.

"Zero-copy" here describes the h11r boundary. Python's socket and the operating
system may still copy data while moving it to the network.
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


def receive_body(connection: h11r.Connection, transport: socket.socket) -> bytes:
    """Consume one request and return all of its body fragments."""
    body = bytearray()
    request_received = False

    while True:
        event = next_event(connection, transport)

        if isinstance(event, h11r.Request):
            request_received = True
            print(f"server received {event.method.decode()} {event.target.decode()}")
        elif isinstance(event, h11r.Data):
            body.extend(event.data)
        elif isinstance(event, h11r.EndOfMessage):
            if not request_received:
                raise RuntimeError("request ended before its Request event")
            return bytes(body)
        elif isinstance(event, h11r.ConnectionClosed):
            raise ConnectionError("client closed before finishing the upload")
        elif event is h11r.ReceiveStatus.PAUSED:
            raise RuntimeError("connection paused before the upload completed")
        else:
            raise RuntimeError(f"unexpected upload event: {event!r}")


def receive_response(connection: h11r.Connection, transport: socket.socket) -> None:
    """Consume the final response so the HTTP exchange is actually complete."""
    response_received = False

    while True:
        event = next_event(connection, transport)

        if isinstance(event, h11r.InformationalResponse):
            print(f"client received informational response {event.status_code}")
        elif isinstance(event, h11r.Response):
            response_received = True
            print(f"client received upload response {event.status_code}")
        elif isinstance(event, h11r.Data):
            print(f"client received {len(event.data)} response body bytes")
        elif isinstance(event, h11r.EndOfMessage):
            if not response_received:
                raise RuntimeError("response ended before its Response event")
            return
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
    client = h11r.Connection(h11r.Role.CLIENT)
    server = h11r.Connection(h11r.Role.SERVER)

    try:
        # Chunked encoding makes the framing bytes visible: h11r will produce a
        # hexadecimal chunk length before the body and CRLF after it.
        client_socket.sendall(
            client.send_request(
                "POST",
                "/upload",
                [("Host", "example.test"), ("Transfer-Encoding", "chunked")],
            )
        )

        body = memoryview(bytearray(b"a body kept in its original Python buffer"))
        prefix, unchanged_body, suffix = client.send_data_parts(body)

        # The returned middle object is the original buffer. Keep it alive and
        # unmodified until all writes complete, and preserve this exact order.
        client_socket.sendall(prefix)
        client_socket.sendall(unchanged_body)
        client_socket.sendall(suffix)
        client_socket.sendall(client.end_of_message())

        received_body = receive_body(server, server_socket)
        print(f"server received {len(received_body)} body bytes")
        print(f"h11r added chunk framing {prefix!r} ... {suffix!r}")

        # Finish both sides of the HTTP exchange. Closing immediately after the
        # upload would leave the client unaware of whether the server accepted it.
        server_socket.sendall(server.send_response(204, reason="No Content"))
        server_socket.sendall(server.end_of_message())
        receive_response(client, client_socket)

        client.start_next_cycle()
        server.start_next_cycle()
        print("upload exchange is complete and the connection is reusable")
    finally:
        client_socket.close()
        server_socket.close()


if __name__ == "__main__":
    main()
