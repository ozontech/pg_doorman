.PHONY: test-bdd-patroni-proxy

# Directory where this Makefile is located (works when included from parent)
PATRONI_PROXY_DIR := $(dir $(lastword $(MAKEFILE_LIST)))

# Project root directory (works both when run from patroni_proxy/ or from project root via include)
PROJECT_ROOT := $(abspath $(PATRONI_PROXY_DIR)../../..)

test-bdd-patroni-proxy: ## Run patroni_proxy BDD tests
	@cd $(PROJECT_ROOT) && cargo test --test patroni_proxy_bdd
