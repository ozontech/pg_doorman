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
