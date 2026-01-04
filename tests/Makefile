.PHONY: help local-build pull shell tests-build test-all test-bdd test-rust test-go test-python test-nodejs test-dotnet clean

REGISTRY ?= ghcr.io
IMAGE_TAG ?= latest

# Directory where this Makefile is located (works when included from parent)
TESTS_DIR := $(dir $(lastword $(MAKEFILE_LIST)))

help: ## Show this help message
	@echo "pg_doorman Test Environment - Make targets"
	@echo ""
	@echo "Prerequisites: Docker (Nix not required!)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(lastword $(MAKEFILE_LIST)) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'

local-build: ## Build Docker image locally using Docker (no Nix installation required)
	@echo "[INFO] Building Docker image using nixos/nix container..."
	@echo "[INFO] This may take 10-15 minutes on first run..."
	@docker run --rm \
		-v $(shell pwd)/..:/workspace \
		-v nix-build-cache:/nix \
		-w /workspace/tests/nix \
		nixos/nix:latest \
		sh -c "nix --extra-experimental-features 'nix-command flakes' build .#dockerImage --show-trace && cp result /workspace/tests/nix/docker-image.tar.gz"
	@echo "[INFO] Loading image into Docker..."
	@docker load < $(TESTS_DIR)nix/docker-image.tar.gz
	@rm -f $(TESTS_DIR)nix/docker-image.tar.gz
	@echo "[INFO] Tagging image as $(REGISTRY)/ozontech/pg_doorman/test-runner:$(IMAGE_TAG)"
	@docker tag pg_doorman-test-env:latest $(REGISTRY)/ozontech/pg_doorman/test-runner:$(IMAGE_TAG)
	@echo "[SUCCESS] Image built and tagged successfully!"
	@echo "[INFO] You can now run: make build"

pull: ## Pull the latest test image from registry
	@$(TESTS_DIR)nix/run-tests.sh pull

shell: ## Open interactive shell in test container
	@$(TESTS_DIR)nix/run-tests.sh shell

tests-build: ## Build pg_doorman inside container
	@$(TESTS_DIR)nix/run-tests.sh build

test-bdd: ## Run BDD/Cucumber tests (use TAGS=@go to filter)
ifdef TAGS
	@$(TESTS_DIR)nix/run-tests.sh bdd $(TAGS)
else
	@$(TESTS_DIR)nix/run-tests.sh bdd
endif

test-go: ## Run Go client tests
	@$(TESTS_DIR)nix/run-tests.sh test-go

test-rust: ## Run Rust client tests
	@$(TESTS_DIR)nix/run-tests.sh test-rust

test-python: ## Run Python client tests
	@$(TESTS_DIR)nix/run-tests.sh test-python

test-nodejs: ## Run Node.js client tests
	@$(TESTS_DIR)nix/run-tests.sh test-nodejs

test-dotnet: ## Run .NET client tests
	@$(TESTS_DIR)nix/run-tests.sh test-dotnet

test-all: ## Run all language tests
	@$(TESTS_DIR)nix/run-tests.sh test-go
	@$(TESTS_DIR)nix/run-tests.sh test-rust
	@$(TESTS_DIR)nix/run-tests.sh test-python
	@$(TESTS_DIR)nix/run-tests.sh test-nodejs
	@$(TESTS_DIR)nix/run-tests.sh test-dotnet

clean: ## Remove cached Docker volumes
	@echo "Removing cached Docker volumes..."
	@docker volume rm pg_doorman_cargo_cache pg_doorman_cargo_git \
		pg_doorman_go_cache pg_doorman_go_build \
		pg_doorman_npm_cache pg_doorman_dotnet pg_doorman_nuget 2>/dev/null || true
	@echo "Cache cleaned!"