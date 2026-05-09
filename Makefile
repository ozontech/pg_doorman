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

dashboard-up: ## Bring up grafana/demo and wait for Prometheus warmup
	cd grafana/demo && docker compose up -d --wait
	# Prometheus scrape_interval is 5s — 90s gives ~18 points so rate()
	# over a 1m window is meaningful, and pg_doorman writes
	# print_all_stats every 60s so the log parser has at least one line.
	@echo "Warmup 90s for Prometheus and pg_doorman log to accumulate..."
	@sleep 90

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

dashboard-validate-ci: ## Up + validate + down. Fails fast and always tears down.
	@$(MAKE) dashboard-validate; \
	rc=$$?; \
	$(MAKE) dashboard-down; \
	exit $$rc
