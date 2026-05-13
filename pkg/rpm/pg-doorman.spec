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
# RHEL/CentOS Stream 7 and 8 ship perl-FindBin / perl-IPC-Cmd only
# inside modular Perl streams that COPR mock filters out by default,
# and COPR builders have no network access to `dnf module enable`.
# The CI workflow vendors the .pm files into this tarball; the build
# stanza extracts it and points PERL5LIB at the result so the build
# sees the modules regardless of distro.
Source3:        perl_modules.tar.gz

BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  make
BuildRequires:  openssl-devel
BuildRequires:  cmake
BuildRequires:  clang
BuildRequires:  tar
BuildRequires:  patch
BuildRequires:  perl-interpreter

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
# Extract bundled Perl modules (FindBin, IPC::Cmd, ...) — see Source3 comment.
tar xzf %{SOURCE3} -C %{_builddir}

%build
# Install Rust toolchain from local tarball (COPR has no network access)
RUST_INSTALL_DIR=%{_builddir}/rust-install
mkdir -p "$RUST_INSTALL_DIR"
tar xzf %{SOURCE2} -C %{_builddir}
%{_builddir}/rust-%{rust_version}-x86_64-unknown-linux-gnu/install.sh --prefix="$RUST_INSTALL_DIR" --without=rust-docs

export PATH="$RUST_INSTALL_DIR/bin:$PATH"
# Make the bundled pure-Perl modules visible to any build script that
# pulls them in (openssl-sys' Configure path uses FindBin and IPC::Cmd).
export PERL5LIB="%{_builddir}/perl_modules${PERL5LIB:+:$PERL5LIB}"

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
