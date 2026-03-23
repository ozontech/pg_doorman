#!/usr/bin/env bash
#
# Build fully static pg_doorman + patroni_proxy binaries (glibc, not musl).
#
# Must run inside the BDD test Docker image which has:
#   - glibc.static (libc.a, libpthread.a, libm.a, ...)
#   - openssl-static (libssl.a, libcrypto.a)
#   - zlib.static (libz.a)
#
# The build.rs in the project root emits `cargo:rustc-link-arg=-static`
# when PG_DOORMAN_STATIC=1, which tells the linker to produce a static
# binary. This flag only applies to the final binary link step, not to
# proc-macros (they remain dynamic cdylib as required by Cargo).
#
# Usage:
#   docker run --rm -v $(pwd):/workspace -w /workspace IMAGE scripts/build-static.sh
#   # Output: target/release/pg_doorman, target/release/patroni_proxy
#
set -euo pipefail

# Point OpenSSL build to static libs
export OPENSSL_STATIC=1
export OPENSSL_LIB_DIR="${OPENSSL_STATIC_LIB_DIR:-$OPENSSL_LIB_DIR}"
export OPENSSL_INCLUDE_DIR="${OPENSSL_STATIC_INCLUDE_DIR:-$OPENSSL_INCLUDE_DIR}"

# Enable static linking in build.rs
export PG_DOORMAN_STATIC=1

# jemalloc tuning
export JEMALLOC_SYS_WITH_MALLOC_CONF="dirty_decay_ms:30000,muzzy_decay_ms:30000,background_thread:true,metadata_thp:auto"

echo "Building static binaries..."
echo "  OPENSSL_LIB_DIR=$OPENSSL_LIB_DIR"
echo "  OPENSSL_INCLUDE_DIR=$OPENSSL_INCLUDE_DIR"
echo "  OPENSSL_STATIC=$OPENSSL_STATIC"
echo "  PG_DOORMAN_STATIC=$PG_DOORMAN_STATIC"

cargo build --release

echo ""
echo "Verifying static linking..."
for bin in target/release/pg_doorman target/release/patroni_proxy; do
    if [ -f "$bin" ]; then
        echo ""
        echo "=== $bin ==="
        file "$bin"
        # ldd should report "not a dynamic executable" for static binaries
        ldd "$bin" 2>&1 || true
        "$bin" --version 2>/dev/null || true
    fi
done
