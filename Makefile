.DEFAULT_GOAL := build

include tests/Makefile

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
	cargo test

clippy:
	cargo clippy -- --deny "warnings"
