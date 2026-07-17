"""Compare steady-state HTTP/1 scenarios through the public Python APIs."""

from __future__ import annotations

import argparse
import platform
import subprocess
import sys

import h11 as reference
import h11r as candidate
import pyperf

SMALL_GET = b"GET /items?q=1 HTTP/1.1\r\nHost: example.test\r\n\r\n"
FIXED_BODY = b"x" * 1024
FIXED_POST = (
    b"POST /items HTTP/1.1\r\n"
    b"Host: example.test\r\n"
    b"Content-Length: 1024\r\n\r\n" + FIXED_BODY
)
FRAGMENTED_GET = tuple(
    SMALL_GET[index : index + 16] for index in range(0, len(SMALL_GET), 16)
)
CHUNK = b"x" * 4096
CHUNKED_STREAM = (
    b"POST /upload HTTP/1.1\r\n"
    b"Host: example.test\r\n"
    b"Transfer-Encoding: chunked\r\n\r\n",
    *(b"1000\r\n" + CHUNK + b"\r\n" for _ in range(16)),
    b"0\r\nDigest: ok\r\n\r\n",
)
NO_CONTENT = b"HTTP/1.1 204 No Content\r\n\r\n"
PING_REQUEST = b"GET /ping HTTP/1.1\r\nHost: example.test\r\n\r\n"
PING_RESPONSE = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK"

SCENARIOS = (
    ("small_get", (SMALL_GET,)),
    ("fixed_body_1k", (FIXED_POST,)),
    ("fragmented_get_16b", FRAGMENTED_GET),
    ("chunked_stream_64k", CHUNKED_STREAM),
)


class CandidateServerCycle:
    __slots__ = ("connection", "parts")

    def __init__(self, parts: tuple[bytes, ...]) -> None:
        self.connection = candidate.Connection(candidate.Role.SERVER)
        self.parts = parts

    def __call__(self) -> tuple[bytes, int, int, object, bytes, bytes]:
        method = b""
        event_count = body_bytes = 0
        last_event: object = candidate.ReceiveStatus.NEED_DATA

        for part in self.parts:
            self.connection.receive_data(part)
            while True:
                event = self.connection.next_event()
                if (
                    event is candidate.ReceiveStatus.NEED_DATA
                    or event is candidate.ReceiveStatus.PAUSED
                ):
                    break
                event_count += 1
                last_event = event
                if isinstance(event, candidate.Request):
                    method = event.method
                elif isinstance(event, candidate.Data):
                    body_bytes += len(event.data)

        head = self.connection.send_response(204, reason=b"No Content")
        tail = self.connection.end_of_message()
        self.connection.start_next_cycle()
        return method, event_count, body_bytes, last_event, head, tail


class H11ServerCycle:
    __slots__ = ("connection", "parts")

    def __init__(self, parts: tuple[bytes, ...]) -> None:
        self.connection = reference.Connection(reference.SERVER)
        self.parts = parts

    def __call__(self) -> tuple[bytes, int, int, object, bytes, bytes]:
        method = b""
        event_count = body_bytes = 0
        last_event: object = reference.NEED_DATA

        for part in self.parts:
            self.connection.receive_data(part)
            while True:
                event = self.connection.next_event()
                if event is reference.NEED_DATA or event is reference.PAUSED:
                    break
                event_count += 1
                last_event = event
                if isinstance(event, reference.Request):
                    method = event.method
                elif isinstance(event, reference.Data):
                    body_bytes += len(event.data)

        head = self.connection.send(
            reference.Response(status_code=204, headers=[], reason=b"No Content")
        )
        tail = self.connection.send(reference.EndOfMessage())
        self.connection.start_next_cycle()
        return method, event_count, body_bytes, last_event, head, tail


class CandidateRoundTrip:
    __slots__ = ("client", "server")

    def __init__(self) -> None:
        self.client = candidate.Connection(candidate.Role.CLIENT)
        self.server = candidate.Connection(candidate.Role.SERVER)

    def __call__(self) -> tuple[bytes, bytes, object, object]:
        request_wire = (
            self.client.send_request(b"GET", b"/ping", [(b"Host", b"example.test")])
            + self.client.end_of_message()
        )
        self.server.receive_data(request_wire)
        self.server.next_event()
        request_end = self.server.next_event()

        response_wire = (
            self.server.send_response(200, [(b"Content-Length", b"2")], reason=b"OK")
            + self.server.send_data(b"OK")
            + self.server.end_of_message()
        )
        self.client.receive_data(response_wire)
        self.client.next_event()
        self.client.next_event()
        response_end = self.client.next_event()

        self.client.start_next_cycle()
        self.server.start_next_cycle()
        return request_wire, response_wire, request_end, response_end


class H11RoundTrip:
    __slots__ = ("client", "server")

    def __init__(self) -> None:
        self.client = reference.Connection(reference.CLIENT)
        self.server = reference.Connection(reference.SERVER)

    def __call__(self) -> tuple[bytes, bytes, object, object]:
        request_wire = self.client.send(
            reference.Request(
                method=b"GET",
                target=b"/ping",
                headers=[(b"Host", b"example.test")],
            )
        ) + self.client.send(reference.EndOfMessage())
        self.server.receive_data(request_wire)
        self.server.next_event()
        request_end = self.server.next_event()

        response_wire = (
            self.server.send(
                reference.Response(
                    status_code=200,
                    headers=[(b"Content-Length", b"2")],
                    reason=b"OK",
                )
            )
            + self.server.send(reference.Data(data=b"OK"))
            + self.server.send(reference.EndOfMessage())
        )
        self.client.receive_data(response_wire)
        self.client.next_event()
        self.client.next_event()
        response_end = self.client.next_event()

        self.client.start_next_cycle()
        self.server.start_next_cycle()
        return request_wire, response_wire, request_end, response_end


def trailers(event: object) -> tuple[tuple[bytes, bytes], ...]:
    if isinstance(event, candidate.EndOfMessage):
        fields = event.trailers
    else:
        assert isinstance(event, reference.EndOfMessage)
        fields = event.headers
    return tuple((name.lower(), value) for name, value in fields)


def check_workloads() -> None:
    expected = {
        "small_get": (b"GET", 2, 0, ()),
        "fixed_body_1k": (b"POST", 3, 1024, ()),
        "fragmented_get_16b": (b"GET", 2, 0, ()),
        "chunked_stream_64k": (b"POST", 18, 64 * 1024, ((b"digest", b"ok"),)),
    }
    for name, parts in SCENARIOS:
        for workload in (CandidateServerCycle(parts), H11ServerCycle(parts)):
            for _ in range(2):
                method, count, size, end, head, tail = workload()
                assert (method, count, size, trailers(end)) == expected[name]
                assert head + tail == NO_CONTENT

    for workload in (CandidateRoundTrip(), H11RoundTrip()):
        for _ in range(2):
            request, response, request_end, response_end = workload()
            assert request == PING_REQUEST
            assert response == PING_RESPONSE
            assert trailers(request_end) == trailers(response_end) == ()


def machine_name() -> str:
    if sys.platform == "darwin":
        result = subprocess.run(
            ("sysctl", "-n", "machdep.cpu.brand_string"),
            capture_output=True,
            check=False,
            text=True,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    return platform.processor() or platform.machine()


def main() -> None:
    check_workloads()

    def add_cmdline_args(command: list[str], args: argparse.Namespace) -> None:
        scenario = args.scenario
        if scenario:
            command.extend(("--scenario", scenario))
        implementation = args.implementation
        if implementation:
            command.extend(("--implementation", implementation))

    runner = pyperf.Runner(
        metadata={
            "h11r_version": candidate.__version__,
            "h11_version": reference.__version__,
            "machine_name": machine_name(),
        },
        add_cmdline_args=add_cmdline_args,
    )
    choices = [name for name, _ in SCENARIOS] + ["client_server_round_trip"]
    runner.argparser.add_argument("--scenario", choices=choices)
    runner.argparser.add_argument("--implementation", choices=("h11r", "h11"))
    args = runner.parse_args()
    for name, parts in SCENARIOS:
        if args.scenario and args.scenario != name:
            continue
        if args.implementation in (None, "h11r"):
            runner.bench_func(f"scenario/{name}/h11r", CandidateServerCycle(parts))
        if args.implementation in (None, "h11"):
            runner.bench_func(f"scenario/{name}/h11-0.16.0", H11ServerCycle(parts))
    if args.scenario in (None, "client_server_round_trip"):
        if args.implementation in (None, "h11r"):
            runner.bench_func(
                "scenario/client_server_round_trip/h11r", CandidateRoundTrip()
            )
        if args.implementation in (None, "h11"):
            runner.bench_func(
                "scenario/client_server_round_trip/h11-0.16.0", H11RoundTrip()
            )


if __name__ == "__main__":
    main()
