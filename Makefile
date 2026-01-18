.PHONY: build install clean test-e2e help demo-db-up demo-db-down

# Build the cori CLI in release mode
build:
	cargo build --release --package cori

# Install the cori CLI to the system
install:
	cargo install --path crates/cori-cli --force --locked

# Clean build artifacts
clean:
	cargo clean

test-e2e: build
	@echo "Running end-to-end tests..."
	cargo test -p cori-mcp --test e2e -- --nocapture --test-threads=1
# Run demo database (PostgreSQL)
demo-db-up:
	docker compose -f examples/demo/docker-compose.yml up -d

# Stop demo database
demo-db-down:
	docker compose -f examples/demo/docker-compose.yml down

# Show available commands
help:
	@echo "Available commands:"
	@echo "  make build         	- Build the cori CLI in release mode"
	@echo "  make install       	- Install the cori CLI to ~/.cargo/bin/"
	@echo "  make clean         	- Clean build artifacts"
	@echo "  make test-e2e      	- Run comprehensive e2e tests (using demo database)"
	@echo "  make demo-db-up    	- Start demo database (PostgreSQL)"
	@echo "  make demo-db-down  	- Stop demo database"
	@echo "  make help          	- Show this help message"

