# Contributing to h11r

Thank you for your interest in improving h11r.

Participation in this project is governed by the
[Code of Conduct ↗](https://github.com/cnzakii/h11r/blob/main/CODE_OF_CONDUCT.md).

## Before You Start

Bug fixes, tests, documentation, API feedback, performance improvements, and
changes that simplify the implementation are welcome.

h11r is a Sans-I/O HTTP/1.1 protocol library. Work outside that scope is not
accepted. Please open an issue before starting a new public API, changing
protocol behavior, or making a substantial design change. Small bug fixes and
documentation improvements can go directly to a pull request.

Security vulnerabilities must be reported according to
[the security policy ↗](https://github.com/cnzakii/h11r/blob/main/SECURITY.md),
not through a public issue.

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
GIL-enabled Python 3.11 through 3.14 and free-threaded Python 3.14t on Linux.
One interpreter also runs on each wheel target, while prerelease Python 3.15
and 3.15t are tested for forward compatibility. Contributors do not need to
reproduce the full matrix locally.

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
- keep the README, runnable examples, and public site aligned when they explain
  the same user path;
- include reproducible measurements for performance claims;
- justify new dependencies and new public surface;
- keep documentation-only changes free of unrelated code changes.

Documentation-only changes do not require a new behavioral test, but the
strict documentation build must pass.

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
make docs
```

Preview the documentation site at `http://localhost:8000` while editing:

```console
make docs-serve
```

Select a free-threaded interpreter explicitly when checking that support
locally:

```console
uv sync --locked --python 3.14t
UV_PYTHON=3.14t make test-python
```

Use `3.15t` in the same commands to exercise the prerelease interpreter.

Code changes run the supported Python and platform matrix in GitHub Actions.
Relevant documentation changes run the strict documentation build separately.
Mixed changes run both. Branch protection requires only the final
`Required checks` job; individual matrix jobs may be skipped when they are not
affected by a pull request.

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
project's
[MIT License ↗](https://github.com/cnzakii/h11r/blob/main/LICENSE).

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
Rust crate to crates.io, creates the GitHub Release, and publishes the
documentation.

Documentation has three kinds of address:

- `/dev/` follows documentation-relevant changes on `main`;
- `/X.Y/` is built from the corresponding release tag, and patch releases
  update the same version line;
- `latest` points to the greatest published `X.Y` line, and the site root
  redirects there. When no release documentation exists, it falls back to
  `/dev/`.

Pull requests only build affected documentation. A release moves `latest` only
when its stable version is the greatest stable tag, so a maintenance release
for an older line cannot move the default documentation backwards.

Configure the repository's GitHub Pages source as **GitHub Actions**.

To republish the latest patch in a version line from a release tag that
contains the site configuration, run the reusable documentation workflow
manually:

```console
gh workflow run publish-docs.yml \
  --ref vX.Y.Z
```

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
