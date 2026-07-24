# Python examples

These examples are runnable lessons for h11r's Python API. They teach how a
Sans-I/O HTTP/1.1 engine fits between application code and a byte transport;
they are not miniature framework implementations.

## Set up

From the repository root, install the locked development environment:

```console
uv sync --locked
```

Run the first lesson:

```console
uv run python examples/python/first_client.py
```

All examples use only h11r and the Python standard library except
`websocket_upgrade.py`, which uses the development dependency `wsproto` after
the HTTP connection switches protocols.

## The h11r mental model

One `h11r.Connection` represents one endpoint's HTTP view of one transport
connection. It does not own a socket, event loop, timeout, task, or application
handler.

Sending follows one rule:

1. Call a `send_*` method or `end_of_message()`.
2. Write every returned byte to the transport in the same order.

For pull-style transports such as the sockets and asyncio streams used here,
receiving follows another rule:

1. Pass bytes from the transport to `receive_data()`.
2. Call `next_event()` repeatedly to drain buffered events.
3. Read the transport again only after `NEED_DATA`.

Push-style adapters instead feed bytes from their receive callback, drain
events, and pause transport reading on `PAUSED` until HTTP can advance again.

`PAUSED` is different from `NEED_DATA`: reading more bytes cannot advance the
HTTP state. Finish the current response and call `start_next_cycle()`, or hand
the transport to the selected protocol after a successful Upgrade.

When a transport read returns `b""`, pass that empty value to `receive_data()`.
This lets h11r distinguish a clean close from a truncated HTTP message.

## Recommended learning path

| Lesson | What it teaches |
| --- | --- |
| [`first_client.py`](python/first_client.py) | Serialize one client request and turn a complete response into events |
| [`round_trip.py`](python/round_trip.py) | Request, `Data`, `EndOfMessage`, response, and keep-alive reuse over a synchronous byte stream |
| [`streaming_body.py`](python/streaming_body.py) | Incremental body consumption, chunked framing, and trailers without collecting the full body |
| [`pipelining.py`](python/pipelining.py) | Why buffered pipelined requests pause until the preceding response finishes |
| [`zero_copy_body.py`](python/zero_copy_body.py) | Passing a file-region proxy through `send_data_parts()` to `socket.sendfile()` |
| [`websocket_upgrade.py`](python/websocket_upgrade.py) | WebSocket handshake validation, HTTP 101, `trailing_data`, and wsproto ownership after handoff |
| [`asyncio_server.py`](python/asyncio_server.py) | A real asynchronous server loop with back-pressure, timeouts, limits, errors, keep-alive, and shutdown |

Read the files in this order if h11r is new to you. If you already have a
transport adapter, jump directly to the protocol behavior you need.

## Why streaming and zero-copy are separate lessons

Streaming controls how much application data must exist at once. The receiver
in `streaming_body.py` updates a checksum for each `Data` event and waits until
`EndOfMessage` to inspect trailers; it never builds one combined body.

`send_data_parts()` solves a different problem. It determines framing from the
body's byte length and returns the original object instead of copying it.
Buffers use their full `nbytes`; other objects declare their exact byte length
through an `nbytes` property. This lets a file-region proxy pass through unchanged so a
transport can use `sendfile()`.

Write the prefix, exactly the declared number of file bytes, and the suffix in
order. If a partial send cannot be resumed, discard the connection because
h11r has already accounted for those bytes. h11r does not inspect the file
contents or take ownership of the file or its transmission. Actual kernel
zero-copy depends on the transport and operating system.

The byte-size contract differs deliberately from h11 0.16's passthrough API:

```python
# h11 0.16 interprets len(region) as a byte count.
parts = connection.send_with_data_passthrough(h11.Data(data=region))

# h11r requires region.nbytes to make the unit explicit.
parts = connection.send_data_parts(region)
```

In either case, the transport processes the returned parts in order. h11r's
wider send API remains intentionally distinct from h11's event-based API.

Transport reads, HTTP chunks, and `Data` events do not have a one-to-one
relationship. Applications must handle any number of `Data` events and use
`EndOfMessage`—not a short read—to recognize completion.

## The asyncio server

Start the server and leave it running:

```console
uv run python examples/python/asyncio_server.py
```

In another terminal, exercise its routes and HEAD handling:

```console
curl -v http://127.0.0.1:8080/
curl -v --data-binary 'hello' http://127.0.0.1:8080/echo
curl -I http://127.0.0.1:8080/
```

The example is organized by ownership:

1. `AsyncHTTPConnection` owns the asyncio streams and one h11r connection. Its
   `next_event()` drains protocol events before awaiting another socket read.
2. `write()` pairs transport writes with `drain()`, allowing a slow peer to
   apply back-pressure to that connection task.
3. `read_request()` handles request events, body fragments, EOF, the body
   limit, and `Expect: 100-continue`.
4. `handle_connection()` owns lifecycle policy: route a request, send its
   response, decide whether reuse is legal, and only then call
   `start_next_cycle()` to release a pipelined request.
5. `asyncio.start_server()` creates a separate task and h11r state machine for
   each accepted connection.

The one-megabyte request limit and 30-second idle-read timeout are example
policy, not h11r defaults or protocol requirements. Production services should
choose limits appropriate to their workloads and stream large bodies instead
of accumulating them as this small router does.

## Correctness

The public examples use normal event dispatch and explicit error handling;
they do not contain test assertions. Automated tests execute every focused
example and exercise the asyncio server over a real loopback TCP connection,
including `100 Continue`, keep-alive, HEAD, a 404 response, and malformed input.

The examples intentionally remain readable rather than production-complete.
Before deploying similar code, add the security, observability, cancellation,
resource, and shutdown policy required by the surrounding application.
