#!/usr/bin/env bash
# =============================================================================
# build_android.sh — Build the Android APK with embedded proot + rootfs.
#
# Usage:
#   ./build_android.sh [--release] [--image TAG] [--skip-dx] [--skip-embed]
#
# Environment overrides (defaults shown):
#   ANDROID_HOME     ~/Projects/AndroidSdk
#   ANDROID_NDK_HOME $ANDROID_HOME/ndk/30.0.14904198
#   DOCKER_IMAGE     yeicor/colmap-openmvs:latest
#   BUILD_MODE       debug  (or "release")
#   TARGET_ARCH      aarch64-linux-android
#   ARCH_ABI         arm64-v8a
#
# What it does:
#   1. Run `dx build --android` (compiles Rust + generates gradle project).
#   2. Download proot and libtalloc for aarch64 from Termux APT repos.
#   3. Pull the Docker image for linux/arm64, export & compress its rootfs.
#   4. Embed all assets as *.so files in the gradle project's jniLibs dir:
#        - proot binary         → libproot.so
#        - rootfs content files → librootfs-<hash>.so
#        - rootfs manifest JSON → librootfs-manifest.so
#        - libtalloc            → libtalloc.so.2 (already *.so, unchanged)
#      All *.so files are auto-included by AGP — no custom merge task needed.
#   5. Patch the generated build.gradle.kts (app module) to set
#        useLegacyPackaging = true  (so *.so files are extracted to disk).
#   6. Patch AndroidManifest.xml for extractNativeLibs=true.
#   7. Re-run `./gradlew assemble<Mode>` to produce the final APK.
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# ─── Parse flags ──────────────────────────────────────────────────────────────
BUILD_MODE="${BUILD_MODE:-debug}"
DOCKER_IMAGE="${DOCKER_IMAGE:-mirror.gcr.io/yeicor/colmap-openmvs:cpu-latest}"
TARGET_ARCH="${TARGET_ARCH:-aarch64-linux-android}"
ARCH_ABI="${ARCH_ABI:-arm64-v8a}"
SKIP_DX=0
SKIP_EMBED=0

for arg in "$@"; do
    case "$arg" in
        --release)    BUILD_MODE="release" ;;
        --image=*)    DOCKER_IMAGE="${arg#*=}" ;;
        --skip-dx)    SKIP_DX=1 ;;
        --skip-embed) SKIP_EMBED=1 ;;
        --help|-h)
            grep '^#' "$0" | head -20 | sed 's/^# \?//'
            exit 0 ;;
    esac
done

# ─── Environment ──────────────────────────────────────────────────────────────
ANDROID_HOME="${ANDROID_HOME:-$HOME/Projects/AndroidSdk}"
ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/30.0.14904198}"
export ANDROID_HOME ANDROID_NDK_HOME
export PATH="$ANDROID_HOME/platform-tools:$HOME/.cargo/bin:$PATH"

echo "==================================================================="
echo " colmap-openmvs-app Android build"
echo "   BUILD_MODE  : $BUILD_MODE"
echo "   DOCKER_IMAGE: $DOCKER_IMAGE"
echo "   TARGET_ARCH : $TARGET_ARCH"
echo "   ARCH_ABI    : $ARCH_ABI"
echo "==================================================================="

# Capitalise mode name for Gradle (e.g. debug→Debug, release→Release)
GRADLE_MODE="$(echo "${BUILD_MODE:0:1}" | tr '[:lower:]' '[:upper:]')${BUILD_MODE:1}"

GRADLE_PROJECT="$SCRIPT_DIR/target/dx/colmap-openmvs-app/$BUILD_MODE/android/app"
JNILIB_DIR="$GRADLE_PROJECT/app/src/main/jniLibs/$ARCH_ABI"
CACHE_DIR="$SCRIPT_DIR/target/android-embed-cache"

mkdir -p "$CACHE_DIR"

# =============================================================================
# STEP 1 — dx build (compile Rust → gradle project)
# =============================================================================
if [ "$SKIP_DX" -eq 0 ]; then
    echo ""
    echo "─── Step 1: dx build --android ──────────────────────────────────────"
    DX_FLAGS="--android --features server --target $TARGET_ARCH"
    if [ "$BUILD_MODE" = "release" ]; then
        DX_FLAGS="$DX_FLAGS --release"
    fi
    # shellcheck disable=SC2086
    dx build $DX_FLAGS
    echo "dx build complete."
else
    echo "─── Step 1: skipped (--skip-dx) ─────────────────────────────────────"
fi

# Verify the gradle project was generated
if [ ! -f "$GRADLE_PROJECT/gradlew" ]; then
    echo "ERROR: gradle project not found at $GRADLE_PROJECT"
    echo "       Run without --skip-dx first."
    exit 1
fi

# =============================================================================
# STEP 2 — Download proot (Termux APT) if not cached
# =============================================================================
if [ "$SKIP_EMBED" -eq 0 ]; then

echo ""
echo "─── Step 2: proot / libtalloc from Termux ───────────────────────────"

PROOT_BASE_URL="https://packages.termux.dev/apt/termux-main/pool/main/p/proot/"
TALLOC_BASE_URL="https://packages.termux.dev/apt/termux-main/pool/main/libt/libtalloc/"

# ── proot binary ──
if [ ! -f "$CACHE_DIR/proot" ]; then
    echo "Fetching latest proot package index..."
    PROOT_DEB_NAME="$(curl -fsSL "$PROOT_BASE_URL" \
        | grep -oP 'href="\K[^"]*proot_[^"]*_aarch64\.deb' \
        | sort -V | tail -1)"
    if [ -z "$PROOT_DEB_NAME" ]; then
        echo "ERROR: could not find proot deb on $PROOT_BASE_URL" >&2
        exit 1
    fi
    echo "Downloading proot: $PROOT_DEB_NAME"
    curl -fsSL -o "$CACHE_DIR/proot.deb" "${PROOT_BASE_URL}${PROOT_DEB_NAME}"
    echo "Extracting proot binary..."
    WORK_DIR="$(mktemp -d)"
    cp "$CACHE_DIR/proot.deb" "$WORK_DIR/pkg.deb"
    (
        cd "$WORK_DIR"
        ar x pkg.deb
        DATA_TAR="$(ls data.tar.* 2>/dev/null | head -1)"
        tar --wildcards -xf "$DATA_TAR" "*/bin/proot"
        PROOT_BIN="$(find . -name proot -path "*/bin/proot" | head -1)"
        cp -L "$PROOT_BIN" "$CACHE_DIR/proot"
    )
    rm -rf "$WORK_DIR"
    chmod +x "$CACHE_DIR/proot"
    rm -f "$CACHE_DIR/proot.deb"
    echo "proot extracted → $CACHE_DIR/proot"
else
    echo "proot already cached."
fi

# ── libtalloc ──
if [ ! -f "$CACHE_DIR/libtalloc.so.2" ]; then
    echo "Fetching latest libtalloc package index..."
    TALLOC_DEB_NAME="$(curl -fsSL "$TALLOC_BASE_URL" \
        | grep -oP 'href="\K[^"]*libtalloc_[^"]*_aarch64\.deb' \
        | sort -V | tail -1)"
    if [ -z "$TALLOC_DEB_NAME" ]; then
        echo "ERROR: could not find libtalloc deb on $TALLOC_BASE_URL" >&2
        exit 1
    fi
    echo "Downloading libtalloc: $TALLOC_DEB_NAME"
    curl -fsSL -o "$CACHE_DIR/libtalloc.deb" "${TALLOC_BASE_URL}${TALLOC_DEB_NAME}"
    echo "Extracting libtalloc..."
    WORK_DIR="$(mktemp -d)"
    cp "$CACHE_DIR/libtalloc.deb" "$WORK_DIR/pkg.deb"
    (
        cd "$WORK_DIR"
        ar x pkg.deb
        DATA_TAR="$(ls data.tar.* 2>/dev/null | head -1)"
        tar --wildcards -xf "$DATA_TAR" "*/usr/lib/libtalloc.so*"
        for f in $(find . -name "libtalloc.so*" -path "*/usr/lib/*"); do
            cp -L "$f" "$CACHE_DIR/$(basename "$f")"
        done
    )
    rm -rf "$WORK_DIR"
    rm -f "$CACHE_DIR/libtalloc.deb"
    echo "libtalloc extracted → $CACHE_DIR/libtalloc.so*"
else
    echo "libtalloc already cached."
fi

# =============================================================================
# STEP 3 — Export Docker image rootfs (linux/arm64) if not cached
# =============================================================================
echo ""
echo "─── Step 3: Docker rootfs export ───────────────────────────────────"

ROOTFS_STAGE="$CACHE_DIR/rootfs_stage"
MANIFEST_CACHE="$CACHE_DIR/embedded_rootfs_manifest.json"

if [ ! -f "$MANIFEST_CACHE" ]; then
    # Verify docker is available
    if ! command -v docker &>/dev/null; then
        echo "ERROR: docker is required to export the rootfs." >&2
        echo "       Install Docker and try again, or use --skip-embed to build" >&2
        echo "       without the embedded rootfs (will require network on device)." >&2
        exit 1
    fi

    echo "Pulling Docker image for linux/arm64: $DOCKER_IMAGE"
    docker pull --platform linux/arm64 "$DOCKER_IMAGE"

    echo "Creating container for export..."
    CONTAINER_ID="$(docker create --platform linux/arm64 "$DOCKER_IMAGE")"

    echo "Exporting rootfs (this may take several minutes)..."
    mkdir -p "$ROOTFS_STAGE"
    docker export "$CONTAINER_ID" | tar -C "$ROOTFS_STAGE" --no-same-owner --no-same-permissions -xf -

    echo "Building rootfs manifest and copying individual files..."
    python3 - "$ROOTFS_STAGE" "$CACHE_DIR" "$DOCKER_IMAGE" "$CONTAINER_ID" <<'PYEOF'
import os, json, hashlib, shutil, stat, subprocess, datetime, sys

source, cache_dir, docker_image, container_id = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]

dest_files = os.path.join(cache_dir, "rootfs_files")
os.makedirs(dest_files, exist_ok=True)

config_raw = subprocess.check_output(
    ["docker", "inspect", "--format", "{{json .Config}}", container_id],
    text=True
).strip()
config = json.loads(config_raw)

now_iso = datetime.datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")
manifest = {
    "version": 1,
    "tag": docker_image,
    "build_date": now_iso,
    "env": config.get("Env") or [],
    "entrypoint": config.get("Entrypoint"),
    "cmd": config.get("Cmd"),
    "working_dir": "/",
    "dirs": [],
    "files": {},
    "symlinks": {}
}

for dirpath, dirnames, filenames in os.walk(source, followlinks=False):
    # Handle directories (and symlinks-to-dirs)
    for dname in list(dirnames):
        dir_full = os.path.join(dirpath, dname)
        rel = os.path.relpath(dir_full, source)
        container_path = "/" + rel
        if os.path.islink(dir_full):
            manifest["symlinks"][container_path] = os.readlink(dir_full)
            dirnames.remove(dname)
        else:
            manifest["dirs"].append(container_path)

    # Handle files (and symlinks-to-files)
    for filename in filenames:
        full_path = os.path.join(dirpath, filename)
        rel = os.path.relpath(full_path, source)
        container_path = "/" + rel
        if os.path.islink(full_path):
            manifest["symlinks"][container_path] = os.readlink(full_path)
        else:
            path_hash = hashlib.sha256(container_path.encode()).hexdigest()[:16]
            dest_path = os.path.join(dest_files, path_hash)
            shutil.copy2(full_path, dest_path)
            file_stat = os.stat(full_path)
            is_exec = bool(file_stat.st_mode & 0o111)
            os.chmod(dest_path, 0o755 if is_exec else 0o644)
            manifest["files"][path_hash] = {
                "path": container_path,
                "executable": is_exec,
                "size": file_stat.st_size
            }

manifest_path = os.path.join(cache_dir, "embedded_rootfs_manifest.json")
with open(manifest_path, "w") as f:
    json.dump(manifest, f, separators=(",", ":"))

print(f"Manifest written: {len(manifest['files'])} files, "
      f"{len(manifest['dirs'])} dirs, {len(manifest['symlinks'])} symlinks")
PYEOF

    docker rm "$CONTAINER_ID"
else
    echo "Rootfs manifest already cached."
fi

# =============================================================================
# STEP 4 — Copy assets into jniLibs + ensure all native dependencies are present
# =============================================================================
echo ""
echo "─── Step 4: Copying assets into jniLibs ────────────────────────────"
mkdir -p "$JNILIB_DIR"

# proot executable → libproot.so (auto-included by AGP as a native lib)
cp "$CACHE_DIR/proot" "$JNILIB_DIR/libproot.so"
chmod +x "$JNILIB_DIR/libproot.so"
# libtalloc - copy all libtalloc.so* files with original names (already *.so)
for f in "$CACHE_DIR"/libtalloc.so*; do
    [ -f "$f" ] && cp "$f" "$JNILIB_DIR/$(basename "$f")"
done
# rootfs content files → librootfs-<hash>.so (auto-included by AGP)
echo "Copying rootfs files to jniLibs as librootfs-*.so (this may take a while)..."
for f in "$CACHE_DIR/rootfs_files"/*; do
    [ -f "$f" ] || continue
    hash="$(basename "$f")"
    cp "$f" "$JNILIB_DIR/librootfs-${hash}.so"
done
# manifest → librootfs-manifest.so (auto-included by AGP)
cp "$CACHE_DIR/embedded_rootfs_manifest.json" "$JNILIB_DIR/librootfs-manifest.so"

fi  # end SKIP_EMBED==0

# =============================================================================
# STEP 5 — Patch the gradle app-module build.gradle.kts
# =============================================================================
echo ""
echo "─── Step 5: Patching app build.gradle.kts ──────────────────────────"

APP_BUILD_GRADLE="$GRADLE_PROJECT/app/build.gradle.kts"
if [ ! -f "$APP_BUILD_GRADLE" ]; then
    echo "WARNING: $APP_BUILD_GRADLE not found — skipping gradle patch."
else
    if grep -q "useLegacyPackaging" "$APP_BUILD_GRADLE"; then
        echo "build.gradle.kts already patched."
    else
        python3 - <<PYEOF
import re, sys

path = "${APP_BUILD_GRADLE}"
with open(path, "r") as f:
    content = f.read()

# Add packaging block with useLegacyPackaging = true.
# All rootfs payload files are now named *.so (librootfs-*.so, libproot.so)
# so AGP picks them up automatically — no custom merge task is needed.
packaging_block = """    packaging {
        jniLibs {
            // Extract native libs to disk (extractNativeLibs = true).
            // Required so libproot.so is executable and all librootfs-*.so
            // payload files are accessible as real paths by the Rust runtime.
            useLegacyPackaging = true
        }
    }
"""

# Find the android {} block and insert packaging inside it (before the closing }).
# We use a regex to find the android { line, then locate its matching closing brace.
def find_matching_brace(s, start_pos):
    """Find the position of the closing brace that matches the opening one at start_pos."""
    count = 0
    for i in range(start_pos, len(s)):
        if s[i] == '{':
            count += 1
        elif s[i] == '}':
            count -= 1
            if count == 0:
                return i
    return -1

# Find 'android {'
android_match = re.search(r'\bandroid\s*\{', content)
if android_match:
    android_open = android_match.start() + len(android_match.group()) - 1  # Position of '{'
    android_close = find_matching_brace(content, android_open)

    if android_close != -1:
        # Insert the packaging block just before the closing brace
        content = content[:android_close] + packaging_block + content[android_close:]
        print(f"Inserted packaging block at position {android_close}", file=sys.stderr)
    else:
        print("ERROR: could not find matching closing brace for android block", file=sys.stderr)
else:
    print("ERROR: android {} block not found in build.gradle.kts", file=sys.stderr)
    sys.exit(1)

with open(path, "w") as f:
    f.write(content)

print("Patched: " + path)
PYEOF
    fi
fi

# =============================================================================
# STEP 6 — Patch AndroidManifest.xml (android:extractNativeLibs="true")
# =============================================================================
echo ""
echo "─── Step 6: Patching AndroidManifest.xml ───────────────────────────"

MANIFEST="$GRADLE_PROJECT/app/src/main/AndroidManifest.xml"
if [ ! -f "$MANIFEST" ]; then
    echo "WARNING: $MANIFEST not found — skipping manifest patch."
else
    if grep -q "extractNativeLibs" "$MANIFEST"; then
        echo "AndroidManifest.xml already has extractNativeLibs."
    else
        python3 - <<PYEOF
import re

path = "${MANIFEST}"
with open(path, "r") as f:
    content = f.read()

# Add android:extractNativeLibs="true" to the <application tag.
# Handles both self-closing and regular application tags.
content = re.sub(
    r'(<application\b)',
    r'\1\n        android:extractNativeLibs="true"',
    content,
    count=1
)

with open(path, "w") as f:
    f.write(content)

print("Patched: " + path)
PYEOF
    fi
fi

# =============================================================================
# STEP 7 — Rebuild APK with Gradle
# =============================================================================
echo ""
echo "─── Step 7: gradle assemble${GRADLE_MODE} ──────────────────────────────────"

(
    cd "$GRADLE_PROJECT"
    chmod +x gradlew
    ./gradlew "assemble${GRADLE_MODE}" --no-daemon
)

APK_DIR="$GRADLE_PROJECT/app/build/outputs/apk/${BUILD_MODE}"
echo ""
echo "==================================================================="
echo " Build complete!"
echo ""
echo " APK(s) in: $APK_DIR"
ls -lh "$APK_DIR/"*.apk 2>/dev/null || echo " (no .apk found — check Gradle output above)"
echo "==================================================================="
