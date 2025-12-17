.PHONY: build install build-install clean help cerbos-up cerbos-down cerbos-logs

# Build the cori CLI in release mode
build:
	cargo build --release --package cori

# Install the cori CLI to the system
install:
	cargo install --path crates/cori-cli --force

# Build and install in one command
build-install: build install

# Clean build artifacts
clean:
	cargo clean

# Run Cerbos PDP locally (Docker Compose) wired to the demo policies
cerbos-up:
	docker compose -f examples/docker-compose.cerbos.yml up -d

# Stop Cerbos PDP
cerbos-down:
	docker compose -f examples/docker-compose.cerbos.yml down

# Tail Cerbos logs
cerbos-logs:
	docker compose -f examples/docker-compose.cerbos.yml logs -f cerbos

# Show available commands
help:
	@echo "Available commands:"
	@echo "  make build         - Build the cori CLI in release mode"
	@echo "  make install       - Install the cori CLI to ~/.cargo/bin/"
	@echo "  make build-install - Build and install the cori CLI"
	@echo "  make clean         - Clean build artifacts"
	@echo "  make cerbos-up     - Start Cerbos PDP (Docker Compose) for examples/"
	@echo "  make cerbos-down   - Stop Cerbos PDP"
	@echo "  make cerbos-logs   - Tail Cerbos logs"
	@echo "  make help          - Show this help message"

