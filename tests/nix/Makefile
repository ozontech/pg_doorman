.PHONY: help local-build pull shell build test-all test-bdd test-go test-python test-ruby test-nodejs test-dotnet clean

REGISTRY ?= ghcr.io
IMAGE_TAG ?= latest

help: ## Show this help message
	@echo "pg_doorman Test Environment - Make targets"
	@echo ""
	@echo "Prerequisites: Docker (Nix not required!)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'

local-build: ## Build Docker image locally using Docker (no Nix installation required)
	@echo "[INFO] Building Docker image using nixos/nix container..."
	@echo "[INFO] This may take 10-15 minutes on first run..."
	@docker run --rm \
		-v $(shell pwd)/../..:/workspace \
		-v nix-build-cache:/nix \
		-w /workspace/tests/nix \
		nixos/nix:latest \
		sh -c "nix --extra-experimental-features 'nix-command flakes' build .#dockerImage --show-trace && cp result /workspace/tests/nix/docker-image.tar.gz"
	@echo "[INFO] Loading image into Docker..."
	@docker load < $(shell pwd)/docker-image.tar.gz
	@rm -f $(shell pwd)/docker-image.tar.gz
	@echo "[INFO] Tagging image as $(REGISTRY)/ozontech/pg_doorman/test-runner:$(IMAGE_TAG)"
	@docker tag pg_doorman-test-env:latest $(REGISTRY)/ozontech/pg_doorman/test-runner:$(IMAGE_TAG)
	@echo "[SUCCESS] Image built and tagged successfully!"
	@echo "[INFO] You can now run: make build"

pull: ## Pull the latest test image from registry
	@./run-tests.sh pull

shell: ## Open interactive shell in test container
	@./run-tests.sh shell

build: ## Build pg_doorman inside container
	@./run-tests.sh build

test-bdd: ## Run BDD/Cucumber tests (use TAGS=@go to filter)
ifdef TAGS
	@./run-tests.sh bdd $(TAGS)
else
	@./run-tests.sh bdd
endif

test-go: ## Run Go client tests
	@./run-tests.sh test-go

test-python: ## Run Python client tests
	@./run-tests.sh test-python

test-ruby: ## Run Ruby client tests
	@./run-tests.sh test-ruby

test-nodejs: ## Run Node.js client tests
	@./run-tests.sh test-nodejs

test-dotnet: ## Run .NET client tests
	@./run-tests.sh test-dotnet

test-all: ## Run all language tests
	@./run-tests.sh test-all

clean: ## Remove cached Docker volumes
	@echo "Removing cached Docker volumes..."
	@docker volume rm pg_doorman_cargo_cache pg_doorman_cargo_git \
		pg_doorman_go_cache pg_doorman_go_build \
		pg_doorman_ruby_gems pg_doorman_npm_cache \
		pg_doorman_dotnet pg_doorman_nuget 2>/dev/null || true
	@echo "Cache cleaned!"

# Quick test targets with BDD tags
test-bdd-go: ## Run BDD tests for Go clients
	@./run-tests.sh bdd @go

test-bdd-go-basic: ## Run BDD basic Go tests
	@./run-tests.sh bdd @go-basic

test-bdd-go-prepared: ## Run BDD Go prepared statements tests
	@./run-tests.sh bdd @go-prepared

test-bdd-go-copy: ## Run BDD Go COPY operations tests
	@./run-tests.sh bdd @go-copy

test-bdd-go-extended: ## Run BDD Go extended protocol tests
	@./run-tests.sh bdd @go-extended

test-bdd-go-special: ## Run BDD Go special functionality tests
	@./run-tests.sh bdd @go-special

test-bdd-python: ## Run BDD tests for Python clients
	@./run-tests.sh bdd @python

test-bdd-ruby: ## Run BDD tests for Ruby clients
	@./run-tests.sh bdd @ruby

test-bdd-nodejs: ## Run BDD tests for Node.js clients
	@./run-tests.sh bdd @nodejs

test-bdd-dotnet: ## Run BDD tests for .NET clients
	@./run-tests.sh bdd @dotnet
