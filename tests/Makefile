.PHONY: help local-build pull shell tests-build test-all test-bdd test-go test-python test-nodejs test-dotnet clean \
	test-bdd-go test-bdd-go-basic test-bdd-go-prepared test-bdd-go-copy test-bdd-go-extended \
	test-bdd-rollback test-bdd-hba test-bdd-prometheus test-bdd-alias test-bdd-check-query test-bdd-deallocate \
	test-bdd-python test-bdd-nodejs test-bdd-dotnet test-bdd-connection test-bdd-application-name test-bdd-server-auth

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

test-python: ## Run Python client tests
	@$(TESTS_DIR)nix/run-tests.sh test-python

test-nodejs: ## Run Node.js client tests
	@$(TESTS_DIR)nix/run-tests.sh test-nodejs

test-dotnet: ## Run .NET client tests
	@$(TESTS_DIR)nix/run-tests.sh test-dotnet

test-all: ## Run all language tests
	@$(TESTS_DIR)nix/run-tests.sh test-all

clean: ## Remove cached Docker volumes
	@echo "Removing cached Docker volumes..."
	@docker volume rm pg_doorman_cargo_cache pg_doorman_cargo_git \
		pg_doorman_go_cache pg_doorman_go_build \
		pg_doorman_npm_cache pg_doorman_dotnet pg_doorman_nuget 2>/dev/null || true
	@echo "Cache cleaned!"

# Quick test targets with BDD tags
test-bdd-go: ## Run BDD tests for Go clients
	@$(TESTS_DIR)nix/run-tests.sh bdd @go

test-bdd-go-basic: ## Run BDD basic Go tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @go-basic

test-bdd-go-prepared: ## Run BDD Go prepared statements tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @go-prepared

test-bdd-go-copy: ## Run BDD Go COPY operations tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @go-copy

test-bdd-go-extended: ## Run BDD Go extended protocol tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @go-extended

test-bdd-rollback: ## Run BDD rollback functionality tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @rollback

test-bdd-hba: ## Run BDD Go HBA authentication tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @hba

test-bdd-prometheus: ## Run BDD Go Prometheus metrics tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @prometheus

test-bdd-alias: ## Run BDD database alias functionality tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @alias

test-bdd-check-query: ## Run BDD check query functionality tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @check-query

test-bdd-deallocate: ## Run BDD deallocate statements tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @deallocate

test-bdd-python: ## Run BDD tests for Python clients
	@$(TESTS_DIR)nix/run-tests.sh bdd @python

test-bdd-nodejs: ## Run BDD tests for Node.js clients
	@$(TESTS_DIR)nix/run-tests.sh bdd @nodejs

test-bdd-dotnet: ## Run BDD tests for .NET clients
	@$(TESTS_DIR)nix/run-tests.sh bdd @dotnet

test-bdd-connection: ## Run BDD connection tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @connection

test-bdd-application-name: ## Run BDD application name tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @application-name

test-bdd-server-auth: ## Run BDD server authentication tests
	@$(TESTS_DIR)nix/run-tests.sh bdd @server-auth
