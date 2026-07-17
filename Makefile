.DEFAULT_GOAL := check

.PHONY: check lint test-rust test-python coverage

check: lint test-rust test-python

lint:
	cargo fmt --all -- --check
	cargo fmt --manifest-path fuzz/Cargo.toml -- --check
	cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
	cargo clippy --manifest-path fuzz/Cargo.toml --bins --locked -- -D warnings
	uv run --locked ruff check .
	uv run --locked ruff format --check .
	uv run --locked ty check

test-rust:
	cargo test --workspace --all-features --locked

test-python:
	uv --directory crates/h11r-python run --locked maturin develop --release
	uv run --locked pytest

# Use one LLVM environment for Cargo tests and the PyO3 extension build so the
# Rust report includes both execution paths.
coverage:
	set -e; \
	eval "$$(cargo llvm-cov show-env --sh)"; \
	cargo llvm-cov clean --workspace; \
	cargo test --workspace --all-features --locked; \
	uv --directory crates/h11r-python run --locked maturin develop; \
	uv run --locked coverage run -m pytest; \
	uv run --locked coverage xml; \
	cargo llvm-cov report --lcov --output-path rust.lcov
