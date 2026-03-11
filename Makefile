# Development workflow commands for Connector
# Run 'make help' to see all available commands

.PHONY: help build test lint clean anvil-up anvil-down anvil-logs

# Default target - show help
help:
	@echo "Connector Development Commands"
	@echo "=============================="
	@echo ""
	@echo "Build:"
	@echo "  make build                Build all packages"
	@echo ""
	@echo "Testing:"
	@echo "  make test                 Run all tests"
	@echo "  make test-unit            Run unit tests only"
	@echo "  make lint                 Run linter"
	@echo ""
	@echo "Local Blockchain:"
	@echo "  make anvil-up             Start Anvil + Faucet (docker compose)"
	@echo "  make anvil-down           Stop Anvil + Faucet"
	@echo "  make anvil-logs           Follow docker compose logs"
	@echo ""
	@echo "Maintenance:"
	@echo "  make clean                Remove build artifacts"

# Build all packages
build:
	npm run build

# Run all tests
test:
	npm test

# Run unit tests only
test-unit:
	npm run test:unit --workspace=packages/connector

# Run linter
lint:
	npm run lint

# Remove build artifacts
clean:
	rm -rf packages/connector/dist packages/shared/dist

# Local Blockchain (Anvil + Faucet)
anvil-up:
	docker compose up -d

anvil-down:
	docker compose down

anvil-logs:
	docker compose logs -f
