%global debug_package %{nil}
%global rust_version 1.87.0

Name:           pg-doorman
Version:        3.0.0
Release:        1%{?dist}
Summary:        PostgreSQL connection pooler and proxy

License:        MIT
URL:            https://github.com/ozontech/pg_doorman
Source0:        %{name}-%{version}.tar.gz
Source1:        vendor.tar.gz
Source2:        rust-%{rust_version}-x86_64-unknown-linux-gnu.tar.gz

BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  make
BuildRequires:  openssl-devel
BuildRequires:  cmake
BuildRequires:  clang
BuildRequires:  tar

Requires:       openssl

%description
pg_doorman is a high-performance PostgreSQL connection pooler and proxy
written in Rust. It provides efficient connection pooling, load balancing,
and query routing capabilities for PostgreSQL databases.

This package includes:
 - pg_doorman: main PostgreSQL connection pooler and proxy
 - patroni_proxy: Patroni integration proxy for high availability setups

%prep
%setup -q -n %{name}-%{version}
# Extract vendored dependencies
tar xzf %{SOURCE1}

%build
# Install Rust toolchain from local tarball (COPR has no network access)
RUST_INSTALL_DIR=%{_builddir}/rust-install
mkdir -p "$RUST_INSTALL_DIR"
tar xzf %{SOURCE2} -C %{_builddir}
%{_builddir}/rust-%{rust_version}-x86_64-unknown-linux-gnu/install.sh --prefix="$RUST_INSTALL_DIR" --without=rust-docs

export PATH="$RUST_INSTALL_DIR/bin:$PATH"

# Configure cargo to use vendored dependencies
mkdir -p .cargo
cat > .cargo/config.toml << EOF
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

# Set jemalloc configuration
export JEMALLOC_SYS_WITH_MALLOC_CONF="dirty_decay_ms:30000,muzzy_decay_ms:30000,background_thread:true,metadata_thp:auto"

# Build release binaries
cargo build --release

%install
install -D -m 755 target/release/pg_doorman %{buildroot}%{_bindir}/pg_doorman
install -D -m 755 target/release/patroni_proxy %{buildroot}%{_bindir}/patroni_proxy

%files
%license LICENSE
%{_bindir}/pg_doorman
%{_bindir}/patroni_proxy

%changelog
* Sun Jan 12 2025 pg-doorman maintainers <pg-doorman@launchpad.net> - 3.0.0-1
- Initial RPM package
- PostgreSQL connection pooler and proxy
- Includes pg_doorman and patroni_proxy binaries
