.DEFAULT_GOAL := build

vendor.tar.gz:
	cargo vendor ./vendor
	tar czf vendor.tar.gz ./vendor
	@rm -rf vendor

build:
	cargo build --release

install: build
	mkdir -p $(DESTDIR)/usr/bin/
	install -c -m 755 ./target/release/pg_doorman $(DESTDIR)/usr/bin/

test:
	cargo test
	./tests/tests.sh