# h11r Repository Guidance

## Product Boundary

- `h11r` is a Sans-I/O HTTP/1.1 protocol engine with a Rust core and a
  Python package.
- HTTP/1.1 is the whole product, not one module in a larger protocol suite. Do
  not add other protocol implementations or an umbrella-project hierarchy.
- Upgrade and handoff behavior belongs here only where HTTP/1.1 requires the
  connection boundary; ownership of the next protocol stays with the caller.
- Keep socket, TLS, async runtime, client, server, and connection-pool ownership
  outside the protocol core.

## Repository Map

- `crates/h11r`: transport-independent Rust protocol core.
- `crates/h11r-python`: PyO3 extension, Python package, stubs, tests, and
  Python benchmarks.
- `fuzz`: Rust fuzz targets for protocol input and state exploration.
- `docs/knowledge`: source-based engineering facts, not project decisions or
  implementation plans.
- `.agents/skills`: public project workflows for implementation, audit, and
  knowledge work.

## Development

Install locked development dependencies with:

```console
uv sync --locked
```

Use focused checks while iterating:

```console
make lint
make test-rust
make test-python
```

Run the complete local gate before submitting a pull request:

```console
make check
```

## Change Rules

- Read the affected implementation, callers, tests, exports, stubs, and
  configuration before editing.
- Keep protocol state and framing in Rust. Keep Python inputs, outputs, and
  errors natural for Python.
- Update runtime exports, type stubs, and tests together when the Python API
  changes.
- Back protocol claims with applicable RFCs and current registries. Treat
  mature implementations as observed practice, not authority.
- Prefer direct code and existing dependencies over speculative abstractions,
  compatibility layers, or new tooling.
- Keep generated artifacts, build output, local environments, editor state,
  and personal instructions out of version control.

## Project Skills

- Use `h11r-implementation` for changes to code, tests, packaging, CI, or
  project documentation.
- Use `h11r-audit` for read-only review after implementation or on request.
- Use `h11r-knowledge` for source-based factual research and explicitly
  requested maintenance of `docs/knowledge`.

Work is complete only after the relevant focused checks and required aggregate
gate finish successfully, followed by a read-only audit of the final changes.
