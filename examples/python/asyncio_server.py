"""A small but complete asyncio HTTP/1.1 server built around h11r.

Run it from the repository root, then send requests from another terminal::

    uv run python examples/python/asyncio_server.py
    curl -v http://127.0.0.1:8080/
    curl -v --data-binary 'hello' http://127.0.0.1:8080/echo

The example demonstrates the integration work around a Sans-I/O library:
asynchronous reads and writes, back-pressure, EOF, request bodies,
``100 Continue``, keep-alive, pipelining boundaries, timeouts, protocol errors,
and orderly transport shutdown.

It is still a teaching server. Production code would normally add TLS,
structured logging, graceful process shutdown, streaming application bodies,
and application-specific authentication and routing.
"""

from __future__ import annotations

import argparse
import asyncio
import contextlib
from http import HTTPStatus

import h11r

READ_SIZE = 64 * 1024
READ_TIMEOUT = 30.0
MAX_REQUEST_BODY = 1024 * 1024


class RequestTooLarge(Exception):
    """The application body limit was exceeded."""


class AsyncHTTPConnection:
    """Join one h11r state machine to one asyncio byte stream."""

    def __init__(
        self,
        reader: asyncio.StreamReader,
        writer: asyncio.StreamWriter,
    ) -> None:
        self.reader = reader
        self.writer = writer
        self.protocol = h11r.Connection(h11r.Role.SERVER)

    async def next_event(self) -> object:
        """Return the next event, reading only when h11r requests bytes."""
        while True:
            # A single TCP read can contain several HTTP events or pipelined
            # requests. Drain h11r's existing buffer before awaiting more I/O.
            event = self.protocol.next_event()
            if event is not h11r.ReceiveStatus.NEED_DATA:
                return event

            # wait_for supplies an idle-read timeout without coupling h11r to
            # asyncio. The h11r connection itself has no clock or scheduler.
            data = await asyncio.wait_for(
                self.reader.read(READ_SIZE),
                timeout=READ_TIMEOUT,
            )

            # StreamReader returns b"" at EOF. h11r needs to receive that empty
            # value so it can distinguish a clean close from a truncated body.
            self.protocol.receive_data(data)

    async def write(self, data: bytes) -> None:
        """Write protocol bytes and respect asyncio's flow control."""
        self.writer.write(data)
        # write() only queues bytes. drain() waits when the transport's buffer
        # crosses its high-water mark, providing back-pressure to this task.
        await self.writer.drain()

    async def read_request(self) -> tuple[h11r.Request, bytes] | None:
        """Read one request, returning None when the peer closes while idle."""
        request: h11r.Request | None = None
        body = bytearray()

        while True:
            event = await self.next_event()

            if isinstance(event, h11r.Request):
                request = event

                # A client using Expect: 100-continue may wait before sending
                # its body. Acknowledging here prevents both peers deadlocking.
                if self.protocol.client_is_waiting_for_100_continue:
                    await self.write(
                        self.protocol.send_informational_response(
                            100,
                            reason="Continue",
                        )
                    )
            elif isinstance(event, h11r.Data):
                if len(body) + len(event.data) > MAX_REQUEST_BODY:
                    raise RequestTooLarge
                body.extend(event.data)
            elif isinstance(event, h11r.EndOfMessage):
                if request is None:
                    raise RuntimeError("request ended before its Request event")
                return request, bytes(body)
            elif isinstance(event, h11r.ConnectionClosed):
                if request is None:
                    return None
                raise ConnectionError("peer closed before finishing the request")
            elif event is h11r.ReceiveStatus.PAUSED:
                # PAUSED is not a request for more network data. It means the
                # current response must finish before a buffered pipelined
                # request can be exposed.
                raise RuntimeError("HTTP parsing paused inside an active request")
            else:
                raise RuntimeError(f"unexpected server event: {event!r}")

    async def send_response(
        self,
        status_code: int,
        body: bytes,
        *,
        allow: str | None = None,
        close: bool = False,
        head_only: bool = False,
    ) -> None:
        """Send one complete, length-delimited response."""
        headers = [
            ("Content-Type", "text/plain; charset=utf-8"),
            ("Content-Length", str(len(body))),
        ]
        if allow is not None:
            headers.append(("Allow", allow))
        if close:
            headers.append(("Connection", "close"))

        # Keep the three protocol actions visible: response head, body, and
        # message boundary each advance the h11r state machine.
        await self.write(
            self.protocol.send_response(
                status_code,
                headers,
                reason=HTTPStatus(status_code).phrase,
            )
        )
        # HEAD carries the headers a GET would have produced, including its
        # Content-Length, but never sends the representation body itself.
        if body and not head_only:
            await self.write(self.protocol.send_data(body))
        await self.write(self.protocol.end_of_message())

    @property
    def must_close(self) -> bool:
        """Whether HTTP semantics forbid another request on this connection."""
        terminal = (h11r.State.MUST_CLOSE, h11r.State.CLOSED, h11r.State.ERROR)
        return (
            self.protocol.local_state in terminal
            or self.protocol.peer_state in terminal
        )

    async def close(self) -> None:
        self.writer.close()
        # The peer may reset the transport instead of closing cleanly.
        with contextlib.suppress(ConnectionError):
            await self.writer.wait_closed()


def route(request: h11r.Request, body: bytes) -> tuple[int, bytes, str | None]:
    """Apply the tiny example application's routing policy."""
    if request.method in {b"GET", b"HEAD"} and request.target == b"/":
        return HTTPStatus.OK, b"h11r asyncio example\nPOST a body to /echo\n", None
    if request.method == b"POST" and request.target == b"/echo":
        return HTTPStatus.OK, body, None
    if request.target == b"/":
        return HTTPStatus.METHOD_NOT_ALLOWED, b"method not allowed\n", "GET, HEAD"
    if request.target == b"/echo":
        return HTTPStatus.METHOD_NOT_ALLOWED, b"method not allowed\n", "POST"
    return HTTPStatus.NOT_FOUND, b"not found\n", None


async def handle_connection(
    reader: asyncio.StreamReader,
    writer: asyncio.StreamWriter,
) -> None:
    """Serve sequential HTTP cycles until the connection must close."""
    connection = AsyncHTTPConnection(reader, writer)
    peer = writer.get_extra_info("peername")
    print(f"connection opened: {peer}")

    try:
        while True:
            try:
                incoming = await connection.read_request()
            except RequestTooLarge:
                # Stop reading an oversized body, send a final response, and
                # close. Reusing the connection would require draining it first.
                await connection.send_response(
                    413,
                    b"request body too large\n",
                    close=True,
                )
                break
            except TimeoutError:
                # h11r deliberately has no timeout policy. This adapter chooses
                # to send 408 when possible and then closes the slow connection.
                await connection.send_response(
                    HTTPStatus.REQUEST_TIMEOUT,
                    b"request timeout\n",
                    close=True,
                )
                break
            except h11r.RemoteProtocolError as error:
                # Remote syntax/state errors carry an HTTP status suggestion.
                # A final error response is best-effort because the peer may
                # already have disconnected after sending invalid bytes.
                status = error.suggested_status_code or HTTPStatus.BAD_REQUEST
                await connection.send_response(
                    status,
                    b"invalid HTTP request\n",
                    close=True,
                )
                break

            if incoming is None:
                break

            request, body = incoming
            print(
                f"{peer}: {request.method.decode('ascii')} "
                f"{request.target.decode('ascii')} ({len(body)} body bytes)"
            )
            status, response_body, allow = route(request, body)
            await connection.send_response(
                status,
                response_body,
                allow=allow,
                head_only=request.method == b"HEAD",
            )

            if connection.must_close:
                break

            # This releases a buffered pipelined request, if one arrived in the
            # same read. h11r still exposes it only after the prior response.
            connection.protocol.start_next_cycle()
    except (ConnectionError, h11r.LocalProtocolError) as error:
        print(f"connection failed: {peer}: {error}")
    finally:
        await connection.close()
        print(f"connection closed: {peer}")


async def serve(host: str, port: int) -> None:
    server = await asyncio.start_server(handle_connection, host, port)
    addresses = ", ".join(str(sock.getsockname()) for sock in server.sockets or [])
    print(f"serving on {addresses}; press Ctrl+C to stop")

    async with server:
        await server.serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Run the h11r asyncio teaching server."
    )
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8080)
    arguments = parser.parse_args()

    try:
        asyncio.run(serve(arguments.host, arguments.port))
    except KeyboardInterrupt:
        print("server stopped")


if __name__ == "__main__":
    main()
