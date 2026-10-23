#!/usr/bin/env bash
#
# Build a statically linked PRoot 5.4.0 for the current architecture.
#
# The binary is used as a fallback when the app runs nested inside another
# PRoot sandbox (DroidDeck on Android).  Distribution packages of 5.4.0 are
# linked against glibc 2.38, so they cannot be installed on older guests even
# though 5.4.0 is the only release that nests reliably.  A static build does
# not depend on the guest's libc at all.
#
# 5.4.0 is intentional: 5.1.x and 5.4.1+ crash when nested under the Termux
# PRoot fork that DroidDeck uses.
#
# Usage: scripts/build-static-proot.sh [output-path]

set -euo pipefail

PROOT_VERSION="5.4.0"
PROOT_SHA256="29248aac2a7ce10c3bd5ee5602742ec33b2532310ff9cf73b79f3c133e5a5f68"
TALLOC_VERSION="2.4.3"
TALLOC_SHA256="dc46c40b9f46bb34dd97fe41f548b0e8b247b77a918576733c528e83abd854dd"

OUTPUT="${1:-$PWD/proot-static}"
WORK_DIR="$(mktemp -d /tmp/build-static-proot_XXXXXX)"

cleanup() {
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

fetch() {
    local url="$1"
    local destination="$2"
    local checksum="$3"

    curl -fsSL --retry 3 -o "$destination" "$url"
    echo "$checksum  $destination" | sha256sum -c -
}

mkdir -p "$WORK_DIR/talloc" "$WORK_DIR/proot"

echo "=== Fetching sources ==="
fetch \
    "https://github.com/proot-me/proot/archive/v${PROOT_VERSION}.tar.gz" \
    "$WORK_DIR/proot.tar.gz" \
    "$PROOT_SHA256"
fetch \
    "https://www.samba.org/ftp/talloc/talloc-${TALLOC_VERSION}.tar.gz" \
    "$WORK_DIR/talloc.tar.gz" \
    "$TALLOC_SHA256"

tar -xzf "$WORK_DIR/proot.tar.gz" -C "$WORK_DIR/proot" --strip-components=1
tar -xzf "$WORK_DIR/talloc.tar.gz" -C "$WORK_DIR/talloc" --strip-components=1

echo "=== Building static libtalloc ==="
(
    cd "$WORK_DIR/talloc"
    ./configure --disable-python --without-gettext >/dev/null
    make -j"$(getconf _NPROCESSORS_ONLN)" >/dev/null

    # The waf build only produces the shared library.  Compile the same
    # source again as an object so that PRoot can be linked statically.
    gcc -c -O2 -fPIC -DHAVE_CONFIG_H -D__STDC_WANT_LIB_EXT1__=1 \
        -I bin/default -I . -I lib/replace \
        -o "$WORK_DIR/talloc.o" talloc.c
    ar rcs "$WORK_DIR/libtalloc.a" "$WORK_DIR/talloc.o"
)

echo "=== Building static PRoot ${PROOT_VERSION} ==="
(
    cd "$WORK_DIR/proot/src"
    make clean >/dev/null 2>&1 || true
    make -j"$(getconf _NPROCESSORS_ONLN)" \
        PYTHON=/bin/false \
        LDFLAGS="$WORK_DIR/libtalloc.a -static"
)

if ! file "$WORK_DIR/proot/src/proot" | grep -q "statically linked"; then
    echo "error: PRoot was not linked statically" >&2
    file "$WORK_DIR/proot/src/proot" >&2
    exit 1
fi

strip "$WORK_DIR/proot/src/proot"
install -m 755 "$WORK_DIR/proot/src/proot" "$OUTPUT"

echo "=== Built $OUTPUT ==="
file "$OUTPUT"
