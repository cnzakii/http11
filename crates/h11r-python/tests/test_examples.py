from __future__ import annotations

import asyncio
import runpy
from pathlib import Path

import h11r
import pytest

EXAMPLES = [
    ("round_trip.py", "connection is ready for another request"),
    ("streaming_body.py", "streamed 36 bytes without collecting the body"),
    ("pipelining.py", "both pipelined responses were sent in request order"),
    ("zero_copy_body.py", "h11r added chunk framing"),
    ("websocket_upgrade.py", "WebSocket client received text: 'echo: hello'"),
]


@pytest.mark.parametrize(
    ("filename", "expected_output"),
    EXAMPLES,
    ids=[
        "round-trip",
        "streaming-body",
        "pipelining",
        "zero-copy",
        "websocket-upgrade",
    ],
)
def test_python_example_runs(
    filename: str,
    expected_output: str,
    capsys: pytest.CaptureFixture[str],
) -> None:
    example = Path(__file__).parents[3] / "examples" / "python" / filename

    runpy.run_path(str(example), run_name="__main__")

    assert expected_output in capsys.readouterr().out


async def next_async_event(
    connection: h11r.Connection,
    reader: asyncio.StreamReader,
) -> object:
    while True:
        event = connection.next_event()
        if event is h11r.ReceiveStatus.NEED_DATA:
            connection.receive_data(await reader.read(64 * 1024))
            continue
        return event


async def receive_final_response(
    connection: h11r.Connection,
    reader: asyncio.StreamReader,
) -> tuple[h11r.Response, bytes]:
    response: h11r.Response | None = None
    body = bytearray()

    while True:
        event = await next_async_event(connection, reader)
        if isinstance(event, h11r.InformationalResponse):
            continue
        if isinstance(event, h11r.Response):
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
            raise RuntimeError("client paused before the response completed")


def test_asyncio_server_runs_complete_connection_flow() -> None:
    example = Path(__file__).parents[3] / "examples" / "python" / "asyncio_server.py"
    namespace = runpy.run_path(str(example))
    handle_connection = namespace["handle_connection"]

    async def exercise_server() -> None:
        server = await asyncio.start_server(handle_connection, "127.0.0.1", 0)
        if not server.sockets:
            raise RuntimeError("asyncio did not create a listening socket")
        port = server.sockets[0].getsockname()[1]

        try:
            reader, writer = await asyncio.open_connection("127.0.0.1", port)
            client = h11r.Connection(h11r.Role.CLIENT)

            request = client.send_request(
                "POST",
                "/echo",
                [
                    ("Host", "example.test"),
                    ("Content-Length", "5"),
                    ("Expect", "100-continue"),
                ],
            )
            writer.write(request)
            await writer.drain()

            informational = await next_async_event(client, reader)
            assert isinstance(informational, h11r.InformationalResponse)
            assert informational.status_code == 100

            writer.write(client.send_data(b"hello") + client.end_of_message())
            await writer.drain()
            response, body = await receive_final_response(client, reader)
            assert response.status_code == 200
            assert body == b"hello"

            client.start_next_cycle()
            writer.write(
                client.send_request("GET", "/missing", [("Host", "example.test")])
                + client.end_of_message()
            )
            await writer.drain()
            response, body = await receive_final_response(client, reader)
            assert response.status_code == 404
            assert body == b"not found\n"

            client.start_next_cycle()
            writer.write(
                client.send_request("HEAD", "/", [("Host", "example.test")])
                + client.end_of_message()
            )
            await writer.drain()
            response, body = await receive_final_response(client, reader)
            assert response.status_code == 200
            assert body == b""
            assert (b"Content-Length", b"42") in response.headers

            client.start_next_cycle()
            writer.write(
                client.send_request("PUT", "/", [("Host", "example.test")])
                + client.end_of_message()
            )
            await writer.drain()
            response, body = await receive_final_response(client, reader)
            assert response.status_code == 405
            assert body == b"method not allowed\n"
            assert (b"Allow", b"GET, HEAD") in response.headers

            writer.close()
            await writer.wait_closed()

            bad_reader, bad_writer = await asyncio.open_connection("127.0.0.1", port)
            bad_writer.write(b"NOT HTTP\r\n\r\n")
            await bad_writer.drain()
            error_response = await bad_reader.read()
            assert error_response.startswith(b"HTTP/1.1 400 Bad Request\r\n")
            assert error_response.endswith(b"invalid HTTP request\n")
            bad_writer.close()
            await bad_writer.wait_closed()
        finally:
            server.close()
            await server.wait_closed()

    asyncio.run(asyncio.wait_for(exercise_server(), timeout=2))


def test_websocket_upgrade_accepts_recombined_list_fields() -> None:
    example = Path(__file__).parents[3] / "examples" / "python" / "websocket_upgrade.py"
    websocket_accept = runpy.run_path(str(example))["websocket_accept"]
    server = h11r.Connection(h11r.Role.SERVER)
    server.receive_data(
        b"GET /chat HTTP/1.1\r\n"
        b"Host: example.test\r\n"
        b"Connection: keep-alive\r\n"
        b"Connection: Upgrade\r\n"
        b"Upgrade: example-protocol, websocket\r\n"
        b"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
        b"Sec-WebSocket-Version: 13\r\n"
        b"\r\n"
    )

    request = server.next_event()
    assert isinstance(request, h11r.Request)
    assert websocket_accept(request) == b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
