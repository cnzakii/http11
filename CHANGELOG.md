# Changelog

User-visible changes to h11r are recorded here.

## [Unreleased]

### Added

- A searchable Zensical documentation site with a guided learning path,
  task-oriented integration and advanced guides, generated Python API
  reference, and GitHub Pages deployment.
- Allow Python transport adapters to pass byte-sized body proxies through
  `Connection.send_data_parts(body)`. A proxy declares its exact byte length
  with `nbytes`, and h11r returns the identical object for the transport to
  write.

## [0.1.1] - 2026-07-21

### Added

- Support for free-threaded CPython 3.14t, including version-specific wheels,
  with preview CI coverage for GIL-enabled and free-threaded CPython 3.15.
- Parallel operation across independent Python `Connection` instances;
  operations on one connection remain caller-serialized in protocol order.

## [0.1.0] - 2026-07-17

### Added

- Initial Rust core and Python package for Sans-I/O HTTP/1.1.
