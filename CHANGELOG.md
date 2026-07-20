# Changelog

User-visible changes to h11r are recorded here.

## [Unreleased]

### Added

- Support for free-threaded CPython 3.14t, including version-specific wheels,
  with preview CI coverage for GIL-enabled and free-threaded CPython 3.15.
- Parallel operation across independent Python `Connection` instances;
  operations on one connection remain caller-serialized in protocol order.

## [0.1.0] - 2026-07-17

### Added

- Initial Rust core and Python package for Sans-I/O HTTP/1.1.
