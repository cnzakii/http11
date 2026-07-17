<p align="center">
  <img src="https://raw.githubusercontent.com/cnzakii/h11r/main/docs/assets/h11r.svg" width="144" height="144" alt="h11r logo">
</p>

# h11r

[![CI](https://github.com/cnzakii/h11r/actions/workflows/ci.yml/badge.svg)](https://github.com/cnzakii/h11r/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/cnzakii/h11r/graph/badge.svg)](https://codecov.io/gh/cnzakii/h11r)
[![PyPI](https://img.shields.io/pypi/v/h11r.svg)](https://pypi.org/project/h11r/)
[![Crates.io](https://img.shields.io/crates/v/h11r.svg)](https://crates.io/crates/h11r)
[![docs.rs](https://docs.rs/h11r/badge.svg)](https://docs.rs/h11r)
[![Python 3.10–3.14](https://img.shields.io/badge/Python-3.10%20to%203.14-3776AB?logo=python&logoColor=white)][python-package]
[![Rust 1.88+](https://img.shields.io/badge/Rust-1.88%2B-000000?logo=rust&logoColor=white)][rust-manifest]
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)][license-file]

**A [Sans-I/O][sans-io] HTTP/1.1 protocol engine for Python, powered by Rust.**

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
    b"GET / HTTP/1.1\r\n"
    b"Host: example.com\r\n"
    b"\r\n"
)

request = connection.next_event()
end = connection.next_event()

assert isinstance(request, h11r.Request)
assert request.method == b"GET"
assert isinstance(end, h11r.EndOfMessage)

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

`h11r` does not open sockets, choose a concurrency model, or handle TLS,
connection pooling, redirects, cookies, or routing. It maintains the protocol
state of one HTTP/1.1 connection while higher-level clients, servers, proxies,
and test tools decide how to schedule I/O.

It supports client and server roles, HTTP/1.0 peers, `Content-Length` and
chunked framing, keep-alive cycles, informational responses, trailers, and
protocol handoff after Upgrade. Input size and header count have independent
limits, and local API misuse is reported separately from remote protocol
errors.

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
[license-file]: https://github.com/cnzakii/h11r/blob/main/LICENSE
[python-package]: https://github.com/cnzakii/h11r/blob/main/crates/h11r-python/pyproject.toml
[rust-manifest]: https://github.com/cnzakii/h11r/blob/main/Cargo.toml
[sans-io]: https://sans-io.readthedocs.io/
