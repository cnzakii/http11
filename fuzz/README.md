# HTTP/1 fuzzing

The targets use public Rust APIs and separate three properties:

- `h11r_receive`: arbitrary peer bytes must not panic or loop without input.
- `h11r_fragmentation`: changing transport chunk boundaries must not change the
  accepted semantic message or error classification.
- `h11r_send`: valid request and response bytes emitted by one role must be
  accepted by the opposite role with the same body.

Run a deterministic pull-request-sized budget with, for example:

```console
cargo +nightly fuzz run h11r_receive -- -runs=100000 -max_len=4096
cargo +nightly fuzz run h11r_fragmentation -- -runs=100000 -max_len=4096
cargo +nightly fuzz run h11r_send -- -runs=100000 -max_len=4096
```

One-byte inputs expand to protocol-shaped request or response seeds; longer
inputs are interpreted as raw peer bytes. Convert a confirmed minimized crash
into the smallest deterministic regression test.
