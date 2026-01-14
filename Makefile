.PHONY: lint typecheck check format install dev clean

# Run ruff linter
lint:
	uv run ruff check forestui/

# Run mypy type checker
typecheck:
	uv run mypy forestui/

# Run all checks (lint + typecheck)
check: lint typecheck

# Format code with ruff
format:
	uv run ruff format forestui/
	uv run ruff check --fix forestui/

# Install the package locally
install:
	uv pip install -e .

# Install with dev dependencies
dev:
	uv sync --group dev

# Clean build artifacts
clean:
	rm -rf build/ dist/ *.egg-info/ .mypy_cache/ .ruff_cache/
	find . -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true

# Run the app
run:
	uv run forestui

# Show help
help:
	@echo "Available targets:"
	@echo "  make lint      - Run ruff linter"
	@echo "  make typecheck - Run mypy type checker"
	@echo "  make check     - Run all checks (lint + typecheck)"
	@echo "  make format    - Format code with ruff"
	@echo "  make install   - Install package locally"
	@echo "  make dev       - Install with dev dependencies"
	@echo "  make clean     - Clean build artifacts"
	@echo "  make run       - Run the app"
