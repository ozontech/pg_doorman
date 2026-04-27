# Installing PgDoorman

PgDoorman runs on Linux and macOS. The recommended path for production is to build from source against the Rust toolchain you control. Pre-built distribution packages and binaries are also available; Docker is intended for testing.

## System requirements

- Linux (recommended) or macOS
- PostgreSQL 10 or newer (any supported version)
- Memory budget proportional to pool size (a few MB per pool plus prepared statement cache)
- Rust 1.87 or newer if building from source

## Build from source (recommended)

Build against your own toolchain so you control compiler version, target platform, and dependencies:

```bash
git clone https://github.com/ozontech/pg_doorman.git
cd pg_doorman
cargo build --release
sudo install -m 0755 target/release/pg_doorman /usr/local/bin/pg_doorman
```

`cargo build --release` produces an optimized binary at `target/release/pg_doorman`. Build prerequisites and the development workflow are in [Contributing](./contributing.md).

### Cargo features

| Feature | Default | Effect |
| --- | --- | --- |
| `tls-migration` | off | Vendored OpenSSL 3.5.5 with a patch that lets TLS-encrypted clients survive a binary upgrade. **Required for zero-downtime restart of TLS clients.** |
| `pam` | off | PAM authentication support (Linux only). |

### Building with TLS client migration

By default, TLS clients cannot migrate to the new process during binary upgrade — they disconnect with `58006` and reconnect. Enable seamless migration with the `tls-migration` feature:

```bash
cargo build --release --features tls-migration
```

This compiles a vendored OpenSSL 3.5.5 with a custom patch that exports and re-imports TLS cipher state (keys, IVs, sequence numbers, TLS 1.3 traffic secrets) across the binary handover. Encrypted clients keep the same TCP connection without re-handshaking.

**Requirements:**

- Linux only (macOS and Windows use platform-native TLS, not OpenSSL).
- `perl` and `patch` utilities in `PATH`.
- Roughly 5 minutes of additional build time for OpenSSL compilation.

**Offline / air-gapped builds:**

```bash
curl -fLO https://github.com/openssl/openssl/releases/download/openssl-3.5.5/openssl-3.5.5.tar.gz
OPENSSL_SOURCE_TARBALL=$(pwd)/openssl-3.5.5.tar.gz \
  cargo build --release --features tls-migration
```

Both the old and the new process must use identical `tls_certificate` and `tls_private_key` files. For the full upgrade flow, monitoring, and troubleshooting, see [Binary Upgrade → TLS migration](./binary-upgrade.md#tls-migration).

For deb/rpm packaging see `debian/` and `pkg/` in the repository. The supplied `Dockerfile.ubuntu22-tls` builds a TLS-migration-capable image on Ubuntu 22.04.

## Distribution packages

Pre-built deb and rpm packages are published from the same release tags. Use these when you cannot or do not want to build from source.

```admonish warning title="No TLS support in distro packages"
Packages from the Ubuntu PPA and Fedora COPR are built **without TLS support**. If you need TLS — for client connections, for server connections to PostgreSQL, or for graceful TLS migration during binary upgrade — build from source with the TLS feature enabled. See [Build from source](#build-from-source-recommended) above.
```

### Ubuntu / Debian (PPA)

```bash
sudo add-apt-repository ppa:vadv/pg-doorman
sudo apt update
sudo apt install pg-doorman
```

Supported releases: `jammy` (22.04 LTS), `noble` (24.04 LTS), `questing` (25.10), `resolute` (26.04 LTS).

### Fedora / RHEL / CentOS / Rocky / AlmaLinux (COPR)

```bash
sudo dnf copr enable @pg-doorman/pg-doorman
sudo dnf install pg_doorman
```

Supported targets: Fedora 39, 40, 41; EPEL 8 and 9 for RHEL-family distributions.

The systemd unit, default config layout, and `pg_doorman` user are set up by the package.

## Pre-built binaries from GitHub Releases

If neither building from source nor distribution packages fit, download a static binary from the [releases page](https://github.com/ozontech/pg_doorman/releases):

```bash
# Replace VERSION and TARGET with the desired values from the releases page.
curl -L -o pg_doorman \
  "https://github.com/ozontech/pg_doorman/releases/download/VERSION/pg_doorman-TARGET"
curl -L -o pg_doorman.sha256 \
  "https://github.com/ozontech/pg_doorman/releases/download/VERSION/pg_doorman-TARGET.sha256"
sha256sum -c pg_doorman.sha256                    # must print "OK"
chmod +x pg_doorman
sudo mv pg_doorman /usr/local/bin/
```

Skipping the checksum step means trusting the network path between you and `objects.githubusercontent.com`. Don't.

## Docker (testing only)

Docker is supported for development, CI, and quick demos. We do not recommend it for production — packaging and lifecycle management are simpler with the system packages above.

```bash
docker run -p 6432:6432 \
  -v $(pwd)/pg_doorman.yaml:/etc/pg_doorman/pg_doorman.yaml \
  ghcr.io/ozontech/pg_doorman
```

A `docker-compose.yaml` with a sidecar PostgreSQL is in [`example/`](https://github.com/ozontech/pg_doorman/tree/master/example) for end-to-end smoke tests.

## Verifying the installation

```bash
pg_doorman --version
pg_doorman -t /etc/pg_doorman/pg_doorman.yaml   # validates config
psql -h 127.0.0.1 -p 6432 -U admin pgdoorman -c "SHOW VERSION;"
```

`pg_doorman -t` validates the config file before deploy — PgBouncer and Odyssey lack this.

## Where to next

- [Basic Usage](./basic-usage.md) — first config, admin console, monitoring.
- [Authentication](../authentication/overview.md) — pick the right auth method.
- [Operations](../operations/signals.md) — signals, reload, systemd integration.
- [Binary Upgrade](./binary-upgrade.md) — replacing the binary without dropping clients.
