---
description: Send one HTTP request from a client and turn the response bytes into events.
---

# Send your first client request

You are adding HTTP/1.1 to a small client. The application chooses to request
`GET /hello`; `h11r` formats that request, tracks the protocol state, and
parses the response. A fixed response byte string takes the place of a socket
so you can see the protocol boundary first.

## Create a small project

This tutorial needs [uv ↗](https://docs.astral.sh/uv/) and CPython 3.10–3.14.
Create an empty project and install the released `h11r` package:

```console
uv init --bare h11r-tour
cd h11r-tour
uv add h11r
```

## Run one complete client-side exchange

Create `first_client.py` with the complete program below:

<!-- fmt:off -->
```python
--8<-- "first_client.py"
```
<!-- fmt:on -->

Run the file:

```console
uv run python first_client.py
```

Expected output:

```text
client would send:
GET /hello HTTP/1.1
Host: example.test

client received 200 with b'Hello from h11r!\n'
```

`Role.CLIENT` tells the connection to send requests and receive responses.
`send_request()` and `end_of_message()` return the exact bytes that a transport
would write. In the other direction, `receive_data()` accepts bytes from the
transport, and `next_event()` exposes the response head, body, and message
boundary to the application.

The application chooses the method, target, and headers for `GET /hello`.
`h11r` handles HTTP lines, body framing, and connection state; your code remains
responsible for moving the returned bytes. This separation lets the same
library work with a synchronous socket, an async stream, or an in-memory test
transport.

Keep this project for the [complete client/server round trip](round-trip.md).
You will replace the fixed response with a server-role connection, move bytes
through a local stream, and reuse the connection.
