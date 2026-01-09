.PHONY: build install build-install clean test help demo-up demo-down demo-logs

# Build the cori CLI in release mode
build:
	cargo build --release --package cori

# Install the cori CLI to the system
install:
	cargo install --path crates/cori-cli --force --locked

# Build and install in one command
build-install: build install

# Clean build artifacts
clean:
	cargo clean

# Run comprehensive CLI tests against demo CRM database
test: build demo-up
	@echo "Waiting for services to start..."
	@sleep 5
	cd examples && ./test-all-commands.sh

test-e2e: build
	@echo "Running end-to-end tests..."
	cargo test -p cori-mcp --test e2e -- --nocapture --test-threads=1
# Run demo database (PostgreSQL)
demo-up:
	docker compose -f examples/docker-compose.demo.yml up -d

# Stop demo database
demo-down:
	docker compose -f examples/docker-compose.demo.yml down

# Tail demo service logs
demo-logs:
	docker compose -f examples/docker-compose.demo.yml logs -f

# Show available commands
help:
	@echo "Available commands:"
	@echo "  make build         - Build the cori CLI in release mode"
	@echo "  make install       - Install the cori CLI to ~/.cargo/bin/"
	@echo "  make build-install - Build and install the cori CLI"
	@echo "  make clean         - Clean build artifacts"
	@echo "  make test          - Run comprehensive CLI tests (starts demo services)"
	@echo "  make demo-up       - Start demo database (PostgreSQL)"
	@echo "  make demo-down     - Stop demo database"
	@echo "  make demo-logs     - Tail demo service logs"
	@echo "  make help          - Show this help message"

