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

dashboard-smoke: ## Run Grafana dashboard smoke test against grafana/demo
	cd grafana/demo && docker compose up -d --wait
	# Prometheus scrape_interval is 5s — 90s buys ~18 points so rate()
	# over a 1m window is already meaningful for every counter.
	@echo "Warmup 90s for Prometheus to accumulate scrape points..."
	@sleep 90
	python3 scripts/dashboard-smoke.py
	@echo "Tip: 'cd grafana/demo && docker compose down -v' to tear down."

dashboard-ground-truth: ## Correlate dashboard values with logs/pg_stat/toml/proc on grafana/demo
	cd grafana/demo && docker compose up -d --wait
	# pg_doorman writes print_all_stats every 60s, so 60s warmup
	# guarantees the parser has at least one data line per pool.
	@echo "Warmup 60s for Prometheus and pg_doorman log to accumulate..."
	@sleep 60
	python3 scripts/dashboard-ground-truth.py
	@echo "Tip: 'cd grafana/demo && docker compose down -v' to tear down."
