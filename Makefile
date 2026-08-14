.PHONY: lint typecheck format-check check test format install dev run clean help

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
