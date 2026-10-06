#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 1 ]; then
    echo "Usage: $0 <path-to-appimage>"
    exit 1
fi

APPIMAGE_PATH="$(readlink -f "$1")"
if [ ! -f "$APPIMAGE_PATH" ]; then
    echo "Error: AppImage not found at '$APPIMAGE_PATH'"
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d /tmp/patch_appimage_XXXXXX)"
EXTRACT_DIR="$WORK_DIR/appdir"

cleanup() {
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

echo "=== Extracting AppImage ==="
mkdir -p "$EXTRACT_DIR"
(
    cd "$WORK_DIR"
    chmod +x "$APPIMAGE_PATH"
    "$APPIMAGE_PATH" --appimage-extract > /dev/null
    mv squashfs-root/* "$EXTRACT_DIR/"
    rm -rf squashfs-root
)

echo "=== Locating WebKitGTK helper binaries ==="
WEBKIT_EXEC="$(find /usr/lib* /usr/libexec* -name "WebKitNetworkProcess" 2>/dev/null | head -n 1 || true)"
if [ -n "$WEBKIT_EXEC" ]; then
    WEBKIT_SRC_DIR="$(dirname "$WEBKIT_EXEC")"
    echo "Found WebKitGTK helpers at: $WEBKIT_SRC_DIR"

    # Identify architecture triple (e.g., x86_64-linux-gnu or aarch64-linux-gnu)
    ARCH_TRIPLE="$(basename "$(dirname "$WEBKIT_SRC_DIR")")"
    if [[ ! "$ARCH_TRIPLE" =~ linux ]]; then
        ARCH_TRIPLE="$(uname -m)-linux-gnu"
    fi

    # Destination directories
    DEST_TRIPLE="$EXTRACT_DIR/usr/lib/$ARCH_TRIPLE/webkit2gtk-4.1"
    DEST_STD="$EXTRACT_DIR/usr/lib/webkit2gtk-4.1"
    DEST_LIBEXEC="$EXTRACT_DIR/usr/libexec/webkit2gtk-4.1"

    mkdir -p "$DEST_TRIPLE" "$DEST_STD" "$DEST_LIBEXEC"
    cp -rL "$WEBKIT_SRC_DIR"/* "$DEST_TRIPLE/"
    cp -rL "$WEBKIT_SRC_DIR"/* "$DEST_STD/"
    cp -rL "$WEBKIT_SRC_DIR"/* "$DEST_LIBEXEC/"

    # Check and copy injected-bundle if located adjacent
    INJECTED="$(find /usr/lib* /usr/libexec* -name "libwebkit2gtkinjectedbundle.so" 2>/dev/null | head -n 1 || true)"
    if [ -n "$INJECTED" ]; then
        echo "Found injected bundle at: $INJECTED"
        mkdir -p "$DEST_TRIPLE/injected-bundle" "$DEST_STD/injected-bundle" "$DEST_LIBEXEC/injected-bundle"
        cp -L "$INJECTED" "$DEST_TRIPLE/injected-bundle/"
        cp -L "$INJECTED" "$DEST_STD/injected-bundle/"
        cp -L "$INJECTED" "$DEST_LIBEXEC/injected-bundle/"
    fi

    echo "=== Resolving ELF dependencies for WebKit helpers ==="
fi

echo "=== Resolving and bundling all non-host library dependencies ==="
mkdir -p "$EXTRACT_DIR/usr/lib"
for pass in 1 2; do
    find "$EXTRACT_DIR/usr" -type f > "$WORK_DIR/bin_list.txt"
    while read -r bin; do
        if [ -f "$bin" ] && file "$bin" | grep -q "ELF"; then
            DEPS="$(ldd "$bin" 2>/dev/null | (grep '=> /' || true) | awk '{print $3}')"
            for lib in $DEPS; do
                libname="$(basename "$lib")"
                case "$libname" in
                    ld-linux*|libc.so*|libm.so*|libdl.so*|libpthread.so*|librt.so*|libresolv.so*|\
                    libGL.so*|libEGL.so*|libOpenGL.so*|libGLdispatch.so*|libGLX.so*|libGLX_mesa.so*|libEGL_mesa.so*|libglapi.so*|\
                    libdrm.so*|libgbm.so*|libvulkan.so*|libepoxy.so*|\
                    libwayland*.so*|libxcb.so*|libxcb-dri*.so*|libX11.so*|libX11-xcb.so*|\
                    libfontconfig.so*|libfreetype.so*|libharfbuzz*.so*|libexpat.so*|libz.so*|libuuid.so*|\
                    libstdc++.so*|libgcc_s.so*)
                        # Host GPU / display / font / C++ runtime stack: must come from the host
                        # so it matches the running Mesa/drivers/fontconfig.
                        # Follows AppImageCommunity/pkg2appimage excludelist.
                        # Bundling the Ubuntu build-host copy of libstdc++ or GPU libs shadows the host one
                        # and causes eglGetDisplay to fail with EGL_BAD_PARAMETER
                        # (e.g. CXXABI/GLIBCXX symbol mismatch or undefined symbol
                        # wl_display_create_queue_with_name), crashing before the UI can render.
                        continue
                        ;;
                    *)
                        if [ ! -f "$EXTRACT_DIR/usr/lib/$libname" ] && [ ! -f "$EXTRACT_DIR/usr/lib/${ARCH_TRIPLE:-unknown}/$libname" ]; then
                            echo "Bundling missing dependency: $libname (from $lib)"
                            cp -L "$lib" "$EXTRACT_DIR/usr/lib/" || true
                        fi
                        ;;
                esac
            done
        fi
    done < "$WORK_DIR/bin_list.txt"
done

echo "=== Copying GSettings schemas ==="
if [ -d "/usr/share/glib-2.0/schemas" ]; then
    mkdir -p "$EXTRACT_DIR/usr/share/glib-2.0/schemas"
    cp -rL /usr/share/glib-2.0/schemas/* "$EXTRACT_DIR/usr/share/glib-2.0/schemas/"
    if command -v glib-compile-schemas &>/dev/null; then
        glib-compile-schemas "$EXTRACT_DIR/usr/share/glib-2.0/schemas" || true
    fi
fi

echo "=== Removing blacklisted files ==="
# libwayland-*/libepoxy/libEGL/libGL/libdrm/libgbm are blacklisted by AppImageKit:
# bundling the build-host copy breaks host Mesa (undefined symbol
# wl_display_create_queue_with_name -> "Could not create default EGL display:
# EGL_BAD_PARAMETER. Aborting..."). Their sonames are stable, so defer to host.
# NOTE: linuxdeploy (via `dx bundle`) bundles these BEFORE this script runs, and
# older linuxdeploy versions predate the upstream exclusion, so delete them here.
rm -f "$EXTRACT_DIR"/usr/lib/libwayland*.so* "$EXTRACT_DIR"/usr/lib/*/libwayland*.so*
rm -f "$EXTRACT_DIR"/usr/lib/libepoxy.so* "$EXTRACT_DIR"/usr/lib/*/libepoxy.so*
rm -f "$EXTRACT_DIR"/usr/lib/libEGL.so* "$EXTRACT_DIR"/usr/lib/*/libEGL.so* \
      "$EXTRACT_DIR"/usr/lib/libEGL_mesa.so* "$EXTRACT_DIR"/usr/lib/*/libEGL_mesa.so*
rm -f "$EXTRACT_DIR"/usr/lib/libGL.so* "$EXTRACT_DIR"/usr/lib/*/libGL.so* \
      "$EXTRACT_DIR"/usr/lib/libGLX*.so* "$EXTRACT_DIR"/usr/lib/*/libGLX*.so* \
      "$EXTRACT_DIR"/usr/lib/libOpenGL.so* "$EXTRACT_DIR"/usr/lib/*/libOpenGL.so* \
      "$EXTRACT_DIR"/usr/lib/libGLdispatch.so* "$EXTRACT_DIR"/usr/lib/*/libGLdispatch.so*
rm -f "$EXTRACT_DIR"/usr/lib/libdrm.so* "$EXTRACT_DIR"/usr/lib/*/libdrm.so* \
      "$EXTRACT_DIR"/usr/lib/libgbm.so* "$EXTRACT_DIR"/usr/lib/*/libgbm.so* \
      "$EXTRACT_DIR"/usr/lib/libvulkan.so* "$EXTRACT_DIR"/usr/lib/*/libvulkan.so* \
      "$EXTRACT_DIR"/usr/lib/libglapi.so* "$EXTRACT_DIR"/usr/lib/*/libglapi.so*
rm -f "$EXTRACT_DIR"/usr/lib/libxcb.so* "$EXTRACT_DIR"/usr/lib/*/libxcb.so* \
      "$EXTRACT_DIR"/usr/lib/libxcb-dri*.so* "$EXTRACT_DIR"/usr/lib/*/libxcb-dri*.so* \
      "$EXTRACT_DIR"/usr/lib/libX11.so* "$EXTRACT_DIR"/usr/lib/*/libX11.so* \
      "$EXTRACT_DIR"/usr/lib/libX11-xcb.so* "$EXTRACT_DIR"/usr/lib/*/libX11-xcb.so*
# Font / low-level libs from the official excludelist: the build-host copies are
# older than modern host configs (hence the fontconfig "xsi:nil" spam) and can
# break Mesa's library chain. Defer to the host; every desktop ships these.
rm -f "$EXTRACT_DIR"/usr/lib/libfontconfig.so* "$EXTRACT_DIR"/usr/lib/*/libfontconfig.so* \
      "$EXTRACT_DIR"/usr/lib/libfreetype.so* "$EXTRACT_DIR"/usr/lib/*/libfreetype.so* \
      "$EXTRACT_DIR"/usr/lib/libharfbuzz*.so* "$EXTRACT_DIR"/usr/lib/*/libharfbuzz*.so* \
      "$EXTRACT_DIR"/usr/lib/libexpat.so* "$EXTRACT_DIR"/usr/lib/*/libexpat.so* \
      "$EXTRACT_DIR"/usr/lib/libz.so* "$EXTRACT_DIR"/usr/lib/*/libz.so* \
      "$EXTRACT_DIR"/usr/lib/libuuid.so* "$EXTRACT_DIR"/usr/lib/*/libuuid.so* \
      "$EXTRACT_DIR"/usr/lib/libstdc++.so* "$EXTRACT_DIR"/usr/lib/*/libstdc++.so* \
      "$EXTRACT_DIR"/usr/lib/libgcc_s.so* "$EXTRACT_DIR"/usr/lib/*/libgcc_s.so*
# Guard: fail loudly if a future linuxdeploy re-introduces graphics/C++ driver killers
if find "$EXTRACT_DIR/usr" \( -name "libwayland-client.so*" -o -name "libepoxy.so*" -o -name "libxcb.so*" -o -name "libX11.so*" -o -name "libstdc++.so*" \) -print | grep -q .; then
    echo "ERROR: blacklisted graphics/system libs still present after stripping:"
    find "$EXTRACT_DIR/usr" \( -name "libwayland-client.so*" -o -name "libepoxy.so*" -o -name "libxcb.so*" -o -name "libX11.so*" -o -name "libstdc++.so*" \)
    exit 1
fi

echo "=== Bundling self-contained C library and dynamic loader into /opt/libc ==="
LIBC_DIR="$EXTRACT_DIR/opt/libc"
mkdir -p "$LIBC_DIR"
HOST_LOADER="$(find /lib /lib64 /usr/lib /usr/lib64 /lib/*-linux-gnu /usr/lib/*-linux-gnu -maxdepth 2 \( -name "ld-linux*.so*" -o -name "ld-2.*.so" -o -name "ld-musl-*.so.1" \) 2>/dev/null | head -n 1 || true)"
if [ -z "$HOST_LOADER" ]; then
    HOST_LOADER="$(find /lib* /usr/lib* \( -name "ld-linux*.so*" -o -name "ld-2.*.so" -o -name "ld-musl-*.so.1" \) 2>/dev/null | head -n 1 || true)"
fi
HOST_LIBC="$(find /lib /lib64 /usr/lib /usr/lib64 /lib/*-linux-gnu /usr/lib/*-linux-gnu -maxdepth 2 \( -name "libc.so.6" -o -name "libc.musl-*.so.1" \) 2>/dev/null | head -n 1 || true)"
if [ -z "$HOST_LIBC" ]; then
    HOST_LIBC="$(find /lib* /usr/lib* \( -name "libc.so.6" -o -name "libc.musl-*.so.1" \) 2>/dev/null | head -n 1 || true)"
fi
if [ -n "$HOST_LOADER" ] && [ -n "$HOST_LIBC" ]; then
    echo "Found loader at $HOST_LOADER and libc at $HOST_LIBC"
    cp -L "$HOST_LOADER" "$LIBC_DIR/"
    cp -L "$HOST_LIBC" "$LIBC_DIR/"
    HOST_LIBC_DIR="$(dirname "$HOST_LIBC")"
    for l in libm.so* libdl.so* libpthread.so* librt.so* libresolv.so*; do
        found="$(find "$HOST_LIBC_DIR" /lib* /usr/lib* -maxdepth 2 -name "$l" 2>/dev/null | head -n 1 || true)"
        if [ -n "$found" ] && [ -f "$found" ]; then
            cp -L "$found" "$LIBC_DIR/" || true
        fi
    done
fi

echo "=== Installing AppStream metadata ==="
APPDATA_SRC="$REPO_ROOT/assets/com.github.yeicor.colmap_openmvs_app.appdata.xml"
if [ -f "$APPDATA_SRC" ]; then
    mkdir -p "$EXTRACT_DIR/usr/share/metainfo"
    cp -f "$APPDATA_SRC" "$EXTRACT_DIR/usr/share/metainfo/com.github.yeicor.colmap_openmvs_app.appdata.xml"
fi

echo "=== Ensuring valid desktop files and categories ==="
for d in "$EXTRACT_DIR"/*.desktop "$EXTRACT_DIR"/usr/share/applications/*.desktop; do
    if [ -f "$d" ]; then
        if ! grep -q "^Categories=" "$d"; then
            echo "Categories=Graphics;Photography;3DGraphics;" >> "$d"
        fi
        if ! grep -q "^Terminal=" "$d"; then
            echo "Terminal=false" >> "$d"
        fi
        if ! grep -q "^StartupWMClass=" "$d"; then
            echo "StartupWMClass=colmap-openmvs-app" >> "$d"
        fi
    fi
done

copy_if_different() {
    local src="$1"
    local dst="$2"
    if [ ! -e "$dst" ] || ! [ "$src" -ef "$dst" ]; then
        rm -f "$dst"
        cp -L "$src" "$dst"
    fi
}

DESKTOP_MAIN="$(find "$EXTRACT_DIR/usr/share/applications" -name "*.desktop" 2>/dev/null | head -n 1 || true)"
if [ -n "$DESKTOP_MAIN" ]; then
    mkdir -p "$EXTRACT_DIR/usr/share/applications"
    copy_if_different "$DESKTOP_MAIN" "$EXTRACT_DIR/usr/share/applications/com.github.yeicor.colmap_openmvs_app.desktop"
    copy_if_different "$DESKTOP_MAIN" "$EXTRACT_DIR/usr/share/applications/colmap-openmvs-app.desktop"
    # Root desktop file matching AppStream component ID so appimagetool validates cleanly
    rm -f "$EXTRACT_DIR"/*.desktop
    cp -L "$DESKTOP_MAIN" "$EXTRACT_DIR/com.github.yeicor.colmap_openmvs_app.desktop"
fi

# Ensure icon files and .DirIcon exist
ICON_SRC="$(find "$EXTRACT_DIR" -name "*.png" 2>/dev/null | grep -E "colmap-openmvs-app\.png|icon\.png" | head -n 1 || true)"
if [ -n "$ICON_SRC" ]; then
    copy_if_different "$ICON_SRC" "$EXTRACT_DIR/colmap-openmvs-app.png"
    copy_if_different "$ICON_SRC" "$EXTRACT_DIR/com.github.yeicor.colmap_openmvs_app.png"
    copy_if_different "$ICON_SRC" "$EXTRACT_DIR/.DirIcon"
fi

echo "=== Compiling WebKit spawn LD_PRELOAD hook ==="
HOOK_SRC="$REPO_ROOT/scripts/webkit_spawn_hook.c"
mkdir -p "$EXTRACT_DIR/usr/lib"
gcc -shared -fPIC -O2 -Wall "$HOOK_SRC" -o "$EXTRACT_DIR/usr/lib/libwebkit_spawn_hook.so" -ldl

echo "=== Generating robust AppRun launcher ==="
rm -f "$EXTRACT_DIR/AppRun"
cat << 'EOF' > "$EXTRACT_DIR/AppRun"
#!/bin/bash
HERE="$(dirname "$(readlink -f "${0}")")"
export APPDIR="${APPDIR:-$HERE}"

export PATH="${APPDIR}/usr/bin:${APPDIR}/usr/sbin:${PATH}"
export LD_LIBRARY_PATH="${APPDIR}/usr/lib:${APPDIR}/usr/lib/x86_64-linux-gnu:${APPDIR}/usr/lib/aarch64-linux-gnu:${APPDIR}/usr/lib64:${LD_LIBRARY_PATH}"
export XDG_DATA_DIRS="${APPDIR}/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="${APPDIR}/usr/share/glib-2.0/schemas:${GSETTINGS_SCHEMA_DIR:-/usr/share/glib-2.0/schemas}"
export WEBKIT_INJECTED_BUNDLE_PATH="${APPDIR}/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/injected-bundle:${APPDIR}/usr/lib/aarch64-linux-gnu/webkit2gtk-4.1/injected-bundle:${APPDIR}/usr/lib/webkit2gtk-4.1/injected-bundle"
export WEBKIT_DISABLE_DMABUF_RENDERER=${WEBKIT_DISABLE_DMABUF_RENDERER:-1}
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export NO_AT_BRIDGE=1
export GTK_MODULES=""

if [ -f "${APPDIR}/usr/lib/libwebkit_spawn_hook.so" ]; then
    if [ -n "$LD_PRELOAD" ]; then
        export LD_PRELOAD="${APPDIR}/usr/lib/libwebkit_spawn_hook.so:${LD_PRELOAD}"
    else
        export LD_PRELOAD="${APPDIR}/usr/lib/libwebkit_spawn_hook.so"
    fi
fi

# Detect whether host glibc is older than required (build runner uses glibc 2.35).
# If host glibc is older, run via the bundled loader in /opt/libc.
# If host glibc is compatible (>= 2.35), run directly using host glibc so host GPU/Mesa drivers match.
USE_BUNDLED_LOADER=0
if [ "${APPIMAGE_USE_BUNDLED_LIBC:-0}" = "1" ]; then
    USE_BUNDLED_LOADER=1
else
    LOADER="$(find "${APPDIR}/opt/libc" \( -name "ld-linux*.so*" -o -name "ld-2.*.so" -o -name "ld-musl-*.so.1" \) 2>/dev/null | head -n 1 || true)"
    if [ -n "$LOADER" ] && [ -x "$LOADER" ]; then
        HOST_LIBC_VERSION="$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}' || true)"
        if [ -z "$HOST_LIBC_VERSION" ]; then
            HOST_LIBC_VERSION="$(ldd --version 2>&1 | head -n 1 | grep -oE '[0-9]+\.[0-9]+' | tail -n 1 || true)"
        fi
        if [ -n "$HOST_LIBC_VERSION" ]; then
            LOWEST_VERSION="$(printf '%s\n%s\n' "2.35" "$HOST_LIBC_VERSION" | sort -V | head -n 1)"
            if [ "$LOWEST_VERSION" != "2.35" ]; then
                USE_BUNDLED_LOADER=1
            fi
        fi
    fi
fi

if [ "$USE_BUNDLED_LOADER" = "1" ]; then
    exec "$LOADER" --library-path "${APPDIR}/opt/libc:${LD_LIBRARY_PATH}" "${APPDIR}/usr/bin/colmap-openmvs-app" "$@"
else
    exec "${APPDIR}/usr/bin/colmap-openmvs-app" "$@"
fi
EOF
chmod +x "$EXTRACT_DIR/AppRun"

echo "=== Repacking AppImage with static runtime ==="
UNAME_M="$(uname -m)"
if [ "$UNAME_M" = "x86_64" ]; then
    TOOL_ARCH="x86_64"
elif [ "$UNAME_M" = "aarch64" ] || [ "$UNAME_M" = "arm64" ]; then
    TOOL_ARCH="aarch64"
else
    TOOL_ARCH="$UNAME_M"
fi

TOOL_DIR="$WORK_DIR/appimagetool_dir"
mkdir -p "$TOOL_DIR"
TOOL_BIN="$WORK_DIR/appimagetool.AppImage"
echo "Fetching appimagetool for $TOOL_ARCH..."
wget -q "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${TOOL_ARCH}.AppImage" -O "$TOOL_BIN"
chmod +x "$TOOL_BIN"
TOOL_SUB="$WORK_DIR/tool_sub"
mkdir -p "$TOOL_SUB"
(
    cd "$TOOL_SUB"
    "$TOOL_BIN" --appimage-extract > /dev/null 2>&1 || true
    mv squashfs-root/* "$TOOL_DIR/"
)

OUTPUT_FILE="$WORK_DIR/patched.AppImage"
UPDATE_INFO="gh-releases-zsync|yeicor|colmap-openmvs-app|latest|*${TOOL_ARCH}.AppImage.zsync"
(
    cd "$WORK_DIR"
    ARCH="$TOOL_ARCH" "$TOOL_DIR/AppRun" -u "$UPDATE_INFO" "$EXTRACT_DIR" "$OUTPUT_FILE"
)

cp -f "$OUTPUT_FILE" "$APPIMAGE_PATH"
chmod +x "$APPIMAGE_PATH"

echo "=== Successfully patched AppImage: $APPIMAGE_PATH ==="
