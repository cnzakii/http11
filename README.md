# http11

[![CI](https://github.com/cnzakii/http11/actions/workflows/ci.yml/badge.svg)](https://github.com/cnzakii/http11/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/cnzakii/http11/graph/badge.svg)](https://codecov.io/gh/cnzakii/http11)
[![PyPI](https://img.shields.io/pypi/v/http11.svg)](https://pypi.org/project/http11/)
[![Crates.io](https://img.shields.io/crates/v/http11.svg)](https://crates.io/crates/http11)
[![docs.rs](https://docs.rs/http11/badge.svg)](https://docs.rs/http11)
[![Python 3.10–3.14](https://img.shields.io/badge/Python-3.10%20to%203.14-3776AB?logo=python&logoColor=white)][python-package]
[![Rust 1.88+](https://img.shields.io/badge/Rust-1.88%2B-000000?logo=rust&logoColor=white)][rust-manifest]
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)][license-file]

**A [Sans-I/O][sans-io] HTTP/1.1 protocol engine for Python, powered by Rust.**

`http11` translates between bytes and HTTP events. It handles message framing,
connection state, and protocol errors without reading from or writing to the
network. Connect it to synchronous sockets, asyncio, Trio, or any other
transport that can move bytes.

The protocol core is an independent Rust crate exposed as a typed Python
package through PyO3. Its connection model is inspired by
[`h11`](https://github.com/python-hyper/h11), while request, response, and
trailer head parsing is built on
[`httparse`](https://github.com/seanmonstar/httparse).

> `http11` is currently alpha software. Its public API may change before the
> first stable release.

## Performance

![Python benchmark comparing http11 and h11][benchmark-chart]

The chart compares protocol-layer throughput for `http11` and `h11 0.16.0`
across five equivalent Python HTTP/1.1 workloads that reuse their connections.
Each workload uses public APIs and includes protocol state transitions. It does
not include socket, TLS, or asynchronous runtime overhead. Higher is faster.

The results were produced by the [`pyperf` benchmark][benchmark-script]. The
[raw pyperf result][benchmark-results] used to render the chart is included for
reproduction and inspection. The measurement environment is recorded in the
chart. Results will vary with hardware and Python version.

## Quick Start

Add `http11` to a uv-managed project:

```console
uv add http11
```

Or install it with pip:

```console
pip install http11
```

This server-side example parses one request and produces a complete response.
The caller remains responsible for network I/O.

```python
import http11

connection = http11.Connection(http11.Role.SERVER)

# Bytes received from any synchronous or asynchronous transport.
connection.receive_data(
    b"GET / HTTP/1.1\r\n"
    b"Host: example.com\r\n"
    b"\r\n"
)

request = connection.next_event()
end = connection.next_event()

assert isinstance(request, http11.Request)
assert request.method == b"GET"
assert isinstance(end, http11.EndOfMessage)

# Write the returned bytes to the same transport.
outbound = connection.send_response(
    200,
    [("Content-Length", "2")],
    reason="OK",
)
outbound += connection.send_data(b"OK")
outbound += connection.end_of_message()
```

## A Protocol Component, Not an HTTP Client

`http11` does not open sockets, choose a concurrency model, or handle TLS,
connection pooling, redirects, cookies, or routing. It maintains the protocol
state of one HTTP/1.1 connection while higher-level clients, servers, proxies,
and test tools decide how to schedule I/O.

It supports client and server roles, HTTP/1.0 peers, `Content-Length` and
chunked framing, keep-alive cycles, informational responses, trailers, and
protocol handoff after Upgrade. Input size and header count have independent
limits, and local API misuse is reported separately from remote protocol
errors.

## Acknowledgements

`http11` follows the [Sans-I/O][sans-io] approach of keeping protocol state
separate from network I/O. Its connection lifecycle and event model are
inspired by [`h11`](https://github.com/python-hyper/h11). It is not a drop-in
replacement for `h11`; it is a Rust-powered Python implementation built in the
same tradition.

Low-level HTTP head parsing is provided by
[`httparse`](https://github.com/seanmonstar/httparse). `http11` owns the full
message framing, connection state machine, resource boundaries, and Python
API.

## Contributing

See [CONTRIBUTING.md][contributing-guide] for development and release guidance.

## License

MIT

[benchmark-chart]: https://raw.githubusercontent.com/cnzakii/http11/main/docs/assets/python-benchmark.svg
[benchmark-results]: https://github.com/cnzakii/http11/blob/main/docs/assets/python-benchmark.json
[benchmark-script]: https://github.com/cnzakii/http11/blob/main/crates/http11-python/benchmarks/compare_h11.py
[contributing-guide]: https://github.com/cnzakii/http11/blob/main/CONTRIBUTING.md
[license-file]: https://github.com/cnzakii/http11/blob/main/LICENSE
[python-package]: https://github.com/cnzakii/http11/blob/main/crates/http11-python/pyproject.toml
[rust-manifest]: https://github.com/cnzakii/http11/blob/main/Cargo.toml
[sans-io]: https://sans-io.readthedocs.io/
