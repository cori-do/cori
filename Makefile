.PHONY: build install build-install clean help

# Build the cori CLI in release mode
build:
	cargo build --release --package cori-cli

# Install the cori CLI to the system
install:
	cargo install --path crates/cori-cli --force

# Build and install in one command
build-install: build install

# Clean build artifacts
clean:
	cargo clean

# Show available commands
help:
	@echo "Available commands:"
	@echo "  make build         - Build the cori CLI in release mode"
	@echo "  make install       - Install the cori CLI to ~/.cargo/bin/"
	@echo "  make build-install - Build and install the cori CLI"
	@echo "  make clean         - Clean build artifacts"
	@echo "  make help          - Show this help message"

