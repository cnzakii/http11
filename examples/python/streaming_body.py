"""Process an HTTP body incrementally and validate a trailing field.

This example transfers bytes directly between a client and server h11r
connection so the streaming protocol flow stays visible. In a network program,
each yielded byte string would be written to a transport and each transport
read would be passed to ``receive_data()``.

The application-defined ``Example-Checksum`` trailer is known only after the
body has been sent. The server updates its checksum for every ``Data`` event
and validates the trailer at ``EndOfMessage`` without storing the whole body.
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterable, Iterator

import h11r


def upload_wire(
    connection: h11r.Connection,
    chunks: Iterable[bytes],
) -> Iterator[bytes]:
    """Yield one request head, each framed body chunk, and the final trailer."""
    yield connection.send_request(
        "POST",
        "/upload",
        [
            ("Host", "example.test"),
            ("Transfer-Encoding", "chunked"),
            ("Trailer", "Example-Checksum"),
        ],
    )

    checksum = hashlib.sha256()
    for chunk in chunks:
        checksum.update(chunk)
        # send_data() returns one framed piece immediately. The caller can write
        # it before producing the next application chunk.
        yield connection.send_data(chunk)

    yield connection.end_of_message([("Example-Checksum", checksum.hexdigest())])


def available_events(connection: h11r.Connection) -> Iterator[object]:
    """Yield buffered events without performing transport I/O."""
    while True:
        event = connection.next_event()
        if event is h11r.ReceiveStatus.NEED_DATA:
            return
        if event is h11r.ReceiveStatus.PAUSED:
            raise RuntimeError("HTTP parsing paused before the upload completed")
        yield event


def trailer_value(trailers: Iterable[tuple[bytes, bytes]], name: bytes) -> bytes:
    values = [value for field, value in trailers if field.lower() == name]
    if len(values) != 1:
        raise ValueError(f"expected exactly one {name.decode()} trailer")
    return values[0]


def main() -> None:
    client = h11r.Connection(h11r.Role.CLIENT)
    server = h11r.Connection(h11r.Role.SERVER)
    server_checksum = hashlib.sha256()
    received_bytes = 0
    upload_complete = False

    # These pieces could come from a file, an async iterator, or another
    # service. Neither h11r connection needs one combined body object.
    body_chunks = (b"first piece\n", b"second piece\n", b"last piece\n")

    for wire_bytes in upload_wire(client, body_chunks):
        # A real transport may split or combine these writes differently. Event
        # handling must therefore depend on h11r events, not write boundaries.
        server.receive_data(wire_bytes)

        for event in available_events(server):
            if isinstance(event, h11r.Request):
                print(f"server started streaming {event.target.decode()}")
            elif isinstance(event, h11r.Data):
                server_checksum.update(event.data)
                received_bytes += len(event.data)
                print(f"server processed {len(event.data)} body bytes")
            elif isinstance(event, h11r.EndOfMessage):
                expected = trailer_value(
                    event.trailers,
                    b"example-checksum",
                )
                actual = server_checksum.hexdigest().encode("ascii")
                if expected != actual:
                    raise ValueError("upload checksum did not match its trailer")
                upload_complete = True
                print("server validated Example-Checksum at EndOfMessage")
            elif isinstance(event, h11r.ConnectionClosed):
                raise ConnectionError("client closed before finishing the upload")
            else:
                raise RuntimeError(f"unexpected upload event: {event!r}")

    if not upload_complete:
        raise RuntimeError("upload ended without EndOfMessage")

    # Send a complete response so both state machines can be reused. A 204
    # response has no message body, so only the head and boundary are emitted.
    response_wire = server.send_response(204, reason="No Content")
    response_wire += server.end_of_message()
    client.receive_data(response_wire)

    for event in available_events(client):
        if isinstance(event, h11r.Response):
            print(f"client received upload response {event.status_code}")
        elif isinstance(event, h11r.EndOfMessage):
            print(f"streamed {received_bytes} bytes without collecting the body")
        elif isinstance(event, h11r.ConnectionClosed):
            raise ConnectionError("server closed before finishing the response")
        else:
            raise RuntimeError(f"unexpected response event: {event!r}")

    client.start_next_cycle()
    server.start_next_cycle()


if __name__ == "__main__":
    main()
