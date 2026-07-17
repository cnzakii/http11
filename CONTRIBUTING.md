# Contributing to h11r

Thank you for your interest in improving h11r.

## Before You Start

Bug fixes, tests, documentation, API feedback, performance improvements, and
changes that simplify the implementation are welcome.

h11r is a Sans-I/O HTTP/1.1 protocol library. Work outside that scope is not
accepted. Please open an issue before starting a new public API, changing
protocol behavior, or making a substantial design change. Small bug fixes and
documentation improvements can go directly to a pull request.

Security vulnerabilities must be reported according to
[SECURITY.md](SECURITY.md), not through a public issue.

## Development Setup

You need:

- Git
- Rust 1.88 or newer, installed with rustup
- the `rustfmt` and `clippy` Rust components
- uv
- Make
- a C compiler and linker supported by Rust

Fork the repository, then clone your fork:

```console
git clone https://github.com/<your-name>/h11r.git
cd h11r
git remote add upstream https://github.com/cnzakii/h11r.git
```

Install the Rust components and locked development dependencies:

```console
rustup component add rustfmt clippy
uv sync --locked
```

The default development version is Python 3.12. Continuous integration tests
Python 3.10 through 3.14; contributors do not need to reproduce the full matrix
locally.

## Making Changes

Create a branch from the latest `main`:

```console
git fetch upstream
git switch -c fix/short-description upstream/main
```

Keep each pull request focused on one problem. In particular:

- add or update tests for behavior changes;
- update Python runtime exports, type stubs, and tests together when changing
  the Python API;
- update Rust documentation and tests when changing the Rust API;
- include reproducible measurements for performance claims;
- justify new dependencies and new public surface;
- keep documentation-only changes free of unrelated code changes.

Documentation-only changes do not require a new test.

## Running Checks

Run the complete local gate before submitting a pull request:

```console
make check
```

For focused iteration, use:

```console
make lint
make test-rust
make test-python
```

The complete GitHub Actions matrix runs after the pull request is opened.

## Performance Changes

Build the Python extension in release mode before running the Python benchmark:

```console
uv --directory crates/h11r-python run --locked maturin develop --release
uv run --locked --group benchmark python \
  crates/h11r-python/benchmarks/compare_h11.py
```

Report the workload, environment, versions, and before-and-after results. A
faster result must preserve equivalent protocol behavior.

## Commits

Keep commits focused and use a concise, imperative subject. Explain why in the
commit body when the reason is not obvious. Do not mix unrelated changes or
commit generated build artifacts.

Pull requests are squash-merged, so use a clear PR title; it becomes the commit
subject on `main`. Conventional Commit prefixes such as `feat:` and `fix:` are
not required.

## Pull Requests

A pull request should:

- explain the problem and the chosen solution;
- link the relevant issue when one exists;
- describe user-visible or compatibility effects;
- include the required tests and documentation;
- pass `make check` and the GitHub Actions checks.

By contributing, you agree that your contribution is licensed under the
project's [MIT License](LICENSE).

## Releases

The Rust crate and Python package share the version in
`Cargo.toml` under `[workspace.package]`. During `0.x`, increment the patch
version for compatible changes and the minor version for breaking public API
changes.

Prepare a release in a pull request:

1. Update the workspace version in `Cargo.toml`.
2. Move the relevant entries from `Unreleased` in `CHANGELOG.md` under
   `## [X.Y.Z] - YYYY-MM-DD`.
3. Run `make check` and commit `Cargo.lock` if it changed.
4. Merge the pull request into `main`.

Publish the merged commit:

```console
git switch main
git pull --ff-only
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

The tag starts the release workflow. After approval of the `release`
environment, GitHub Actions publishes the Python distributions to PyPI and the
Rust crate to crates.io, then creates the GitHub Release.

Before the first automated release, configure the PyPI trusted publisher and
the GitHub `release` environment. crates.io requires the first crate version to
be published manually with `cargo publish -p h11r --locked`; configure its
trusted publisher after that first publication.

### Recovering a Partial Release

Package registry files are immutable. If a release fails after either PyPI or
crates.io accepts the version:

1. Keep the original tag and artifacts. Do not rebuild or replace files under
   the same version.
2. Use **Re-run failed jobs** on the original GitHub Actions run, not **Re-run
   all jobs**, so a successful publish job is not repeated. The CLI equivalent
   is `gh run rerun RUN_ID --failed`.
3. If both registries contain the version but the GitHub Release is missing,
   create it from the existing tag with
   `gh release create vX.Y.Z --verify-tag --generate-notes`.
4. If a registry contains an incomplete or incorrect release, yank it where
   supported and publish a new version. Never reuse an uploaded filename.
