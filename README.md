<p align="center">
  <img src="https://github.com/cnzakii/h11r/raw/refs/heads/main/docs/assets/h11r.svg" width="144" height="144" alt="h11r logo">
</p>

<h1 align="center">h11r</h1>

<p align="center">
  <strong>A <a href="https://sans-io.readthedocs.io/">Sans-I/O</a> HTTP/1.1 protocol engine for Python, powered by Rust.</strong>
</p>

<p align="center">
  <a href="https://github.com/cnzakii/h11r/actions/workflows/ci.yml"><img src="https://github.com/cnzakii/h11r/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://codecov.io/gh/cnzakii/h11r"><img src="https://codecov.io/gh/cnzakii/h11r/graph/badge.svg" alt="codecov"></a>
  <a href="https://pypi.org/project/h11r/"><img src="https://img.shields.io/pypi/v/h11r.svg" alt="PyPI"></a>
  <a href="https://crates.io/crates/h11r"><img src="https://img.shields.io/crates/v/h11r.svg" alt="Crates.io"></a>
  <a href="https://docs.rs/h11r"><img src="https://docs.rs/h11r/badge.svg" alt="docs.rs"></a>
  <a href="https://github.com/cnzakii/h11r/blob/main/crates/h11r-python/pyproject.toml"><img src="https://img.shields.io/badge/Python-3.10%20to%203.14-3776AB?logo=python&amp;logoColor=white" alt="Python 3.10–3.14"></a>
  <a href="https://github.com/cnzakii/h11r/blob/main/Cargo.toml"><img src="https://img.shields.io/badge/Rust-1.88%2B-000000?logo=rust&amp;logoColor=white" alt="Rust 1.88+"></a>
  <a href="https://github.com/cnzakii/h11r/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

`h11r` translates between bytes and HTTP events. It handles message framing,
connection state, and protocol errors without reading from or writing to the
network. Connect it to synchronous sockets, asyncio, Trio, or any other
transport that can move bytes.

The protocol core is an independent Rust crate exposed as a typed Python
package through PyO3. Its connection model is inspired by
[`h11`](https://github.com/python-hyper/h11), while request, response, and
trailer head parsing is built on
[`httparse`](https://github.com/seanmonstar/httparse).

> `h11r` is currently alpha software. Its public API may change before the
> first stable release.

## Performance

![Python benchmark comparing h11r and h11][benchmark-chart]

The chart compares protocol-layer throughput for `h11r` and `h11 0.16.0`
across five equivalent Python HTTP/1.1 workloads that reuse their connections.
Each workload uses public APIs and includes protocol state transitions. It does
not include socket, TLS, or asynchronous runtime overhead. Higher is faster.

The results were produced by the [`pyperf` benchmark][benchmark-script]. The
[raw pyperf result][benchmark-results] used to render the chart is included for
reproduction and inspection. The measurement environment is recorded in the
chart. Results will vary with hardware and Python version.

## Quick Start

Add `h11r` to a uv-managed project:

```console
uv add h11r
```

Or install it with pip:

```console
pip install h11r
```

This server-side example parses one request and produces a complete response.
The caller remains responsible for network I/O.

```python
import h11r

connection = h11r.Connection(h11r.Role.SERVER)

# Bytes received from any synchronous or asynchronous transport.
connection.receive_data(
    b"POST /echo HTTP/1.1\r\n"
    b"Host: example.com\r\n"
    b"Content-Length: 4\r\n"
    b"\r\n"
    b"ping"
)

request = None
body = bytearray()

# Drain every event already available from the received bytes.
while True:
    event = connection.next_event()
    if isinstance(event, h11r.Request):
        request = event
    elif isinstance(event, h11r.Data):
        body.extend(event.data)
    elif isinstance(event, h11r.EndOfMessage):
        if request is None:
            raise RuntimeError("request ended before its Request event")
        break
    elif event is h11r.ReceiveStatus.NEED_DATA:
        raise RuntimeError("the request is incomplete")
    else:
        raise RuntimeError(f"unexpected request event: {event!r}")

# Echo the request body and write every returned byte to the same transport.
response_body = bytes(body)
outbound = connection.send_response(
    200,
    [("Content-Length", str(len(response_body)))],
    reason="OK",
)
outbound += connection.send_data(response_body)
outbound += connection.end_of_message()
```

See the [runnable Python examples][python-examples] for complete round-trip,
streaming body, pipelining, zero-copy body, WebSocket Upgrade, and asyncio
server lessons.

## A Protocol Component, Not an HTTP Client

`h11r` does not open sockets, choose a concurrency model, or handle TLS,
connection pooling, redirects, cookies, or routing. It maintains the protocol
state of one HTTP/1.1 connection while higher-level clients, servers, proxies,
and test tools decide how to schedule I/O.

It supports client and server roles, HTTP/1.0 peers, `Content-Length` and
chunked framing, keep-alive cycles, informational responses, trailers, and
protocol handoff after Upgrade. Input size and header count have independent
limits, and local API misuse is reported separately from remote protocol
errors.

The Python package supports GIL-enabled CPython 3.10 through 3.14 and
free-threaded CPython 3.14t. CI also exercises CPython 3.15 and 3.15t while
they are prereleases. Independent `Connection` instances may run in parallel.
Operations on one connection still have protocol order and must be serialized
by its caller.

## Acknowledgements

`h11r` follows the [Sans-I/O][sans-io] approach of keeping protocol state
separate from network I/O. Its connection lifecycle and event model are
inspired by [`h11`](https://github.com/python-hyper/h11). It is not a drop-in
replacement for `h11`; it is a Rust-powered Python implementation built in the
same tradition.

Low-level HTTP head parsing is provided by
[`httparse`](https://github.com/seanmonstar/httparse). `h11r` owns the full
message framing, connection state machine, resource boundaries, and Python
API.

## Contributing

See [CONTRIBUTING.md][contributing-guide] for development and release guidance.

## License

MIT

[benchmark-chart]: https://raw.githubusercontent.com/cnzakii/h11r/main/docs/assets/python-benchmark.svg
[benchmark-results]: https://github.com/cnzakii/h11r/blob/main/docs/assets/python-benchmark.json
[benchmark-script]: https://github.com/cnzakii/h11r/blob/main/crates/h11r-python/benchmarks/compare_h11.py
[contributing-guide]: https://github.com/cnzakii/h11r/blob/main/CONTRIBUTING.md
[python-examples]: https://github.com/cnzakii/h11r/tree/main/examples
[sans-io]: https://sans-io.readthedocs.io/
