.DEFAULT_GOAL := build

include tests/Makefile
include src/bin/patroni_proxy/Makefile

vendor.tar.gz:
	cargo vendor ./vendor
	tar czf vendor.tar.gz ./vendor
	@rm -rf vendor

vendor-licenses.txt:
	cd /tmp && cargo install cargo-license
	cargo license --json > ./vendor-licenses.json
	python ./pkg/make_vendor_license.py ./vendor-licenses.json ./vendor-licenses.txt

build:
	cargo build --release

install: build
	mkdir -p $(DESTDIR)/usr/bin/
	install -c -m 755 ./target/release/pg_doorman $(DESTDIR)/usr/bin/

test:
	cargo test --lib

clippy:
	cargo clippy -- --deny "warnings"

generate:
	cargo run --bin pg_doorman -- generate --reference -o pg_doorman.toml
	cargo run --bin pg_doorman -- generate --reference -o pg_doorman.yaml
	cargo run --bin pg_doorman -- generate-docs -o documentation/en/src/reference

flamegraph: ## Generate CPU flamegraph (perf + pgbench load)
	./scripts/flamegraph.sh

# Default to the local demo image so `make docker-smoke` runs after a
# `dashboard-up` build. Override with `make docker-smoke IMAGE=...`
# when validating a freshly built or pulled image.
IMAGE ?= pg_doorman:demo

docker-smoke: ## End-to-end smoke (postgres sidecar + generate + SELECT 1) against $(IMAGE)
	./scripts/docker-smoke.sh $(IMAGE)

dashboard-up: ## Bring up grafana/demo and wait until pg_doorman emits enough scrape points
	cd grafana/demo && docker compose up -d --wait
	# Wait until Prometheus has at least 12 scrape points for a counter
	# we know pg_doorman+pgbench will produce — at scrape_interval 5 s
	# that is ~60 s of steady traffic, matching the rate(...[1m]) window
	# the dashboard and the ground-truth checks use. Polling beats a
	# flat sleep: a warm laptop unblocks early, a cold runner does not
	# flake on a fixed assumption.
	scripts/dashboard-wait-ready.sh

dashboard-smoke: dashboard-up ## Run Grafana dashboard smoke test against grafana/demo
	python3 scripts/dashboard-smoke.py

dashboard-ground-truth: dashboard-up ## Correlate dashboard values with logs/pg_stat/toml/proc on grafana/demo
	python3 scripts/dashboard-ground-truth.py

dashboard-validate: dashboard-up ## Run smoke + ground-truth against grafana/demo (single warmup)
	python3 scripts/dashboard-smoke.py
	python3 scripts/dashboard-ground-truth.py
	@echo "Both layers passed. Run 'make dashboard-down' to tear down."

dashboard-down: ## Tear down grafana/demo and remove its volumes
	cd grafana/demo && docker compose down -v

dashboard-validate-ci: ## Up + validate + down with pinned image tags. Cleanup runs on SIGINT/SIGTERM.
	@set -eu; \
	compose='docker compose -f grafana/demo/docker-compose.yml -f grafana/demo/docker-compose.ci.yml'; \
	trap "$$compose down -v" EXIT; \
	$$compose up -d --wait; \
	scripts/dashboard-wait-ready.sh; \
	python3 scripts/dashboard-smoke.py; \
	python3 scripts/dashboard-ground-truth.py
