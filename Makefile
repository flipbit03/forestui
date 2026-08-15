.PHONY: lint typecheck format-check check check-shipped test format install dev run clean help

# Run the linter
lint:
	cargo clippy --all-targets -- -D warnings

# Type-check without producing a binary
typecheck:
	cargo check --all-targets

# Check formatting without modifying files
format-check:
	cargo fmt --check

# Run the test suite
test:
	cargo test

# Run all checks (lint + typecheck + format + tests)
check: format-check lint typecheck test

# The configuration the release actually ships.
#
# `binary-release` gates the in-place updater, so the default build compiles a
# strictly smaller amount of code: the download path is absent, and anything it
# alone uses reads as dead. The two configurations therefore fail differently,
# and the one that ships is the one `check` was not building.
check-shipped:
	cargo clippy --all-targets --features binary-release -- -D warnings
	cargo test --features binary-release

# Format code
format:
	cargo fmt

# Install the binary locally
install:
	cargo install --path . --locked

# Set up the development toolchain
dev:
	rustup component add clippy rustfmt

# Clean build artifacts
clean:
	cargo clean

# Run the app
run:
	cargo run

# Show help
help:
	@echo "Available targets:"
	@echo "  make lint         - Run clippy"
	@echo "  make typecheck    - Run cargo check"
	@echo "  make format-check - Verify formatting"
	@echo "  make test         - Run the test suite"
	@echo "  make check        - Run all checks (format + lint + typecheck + tests)"
	@echo "  make format       - Format code"
	@echo "  make install      - Install the binary locally"
	@echo "  make dev          - Install clippy and rustfmt"
	@echo "  make clean        - Clean build artifacts"
	@echo "  make run          - Run the app"
