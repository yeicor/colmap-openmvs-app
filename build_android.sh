#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BUILD_MODE="${BUILD_MODE:-debug}"
TARGET_PLATFORM="${TARGET_PLATFORM:-arm64}"
DOCKER_IMAGE="${DOCKER_IMAGE:-mirror.gcr.io/yeicor/colmap-openmvs:cpu-latest}"

SKIP_DX=0
SKIP_EMBED=0

TEMP_DIRS=()
CONTAINERS=()

die() { echo "ERROR: $*" >&2; exit 1; }

cleanup() {
    for c in "${CONTAINERS[@]:-}"; do
        docker rm -f "$c" >/dev/null 2>&1 || true
    done
    for d in "${TEMP_DIRS[@]:-}"; do
        rm -rf "$d" || true
    done
}
trap cleanup EXIT INT TERM

parse_args() {
    for arg in "$@"; do
        case "$arg" in
            --release) BUILD_MODE=release ;;
            --debug) BUILD_MODE=debug ;;
            --skip-dx) SKIP_DX=1 ;;
            --skip-embed) SKIP_EMBED=1 ;;
            --target=*) TARGET_PLATFORM="${arg#*=}" ;;
            --image=*) DOCKER_IMAGE="${arg#*=}" ;;
            -h|--help)
                cat <<EOF
Usage:
  ./build_android.sh [options]

Options:
  --release
  --debug
  --target=arm64|x86_64|x86|armv7
  --image=<docker-image>
  --skip-dx
  --skip-embed
EOF
                exit 0
                ;;
        esac
    done
}

resolve_platform() {
    case "$TARGET_PLATFORM" in
        arm64|aarch64)
            DOCKER_PLATFORM="linux/arm64"
            TARGET_ARCH="aarch64-linux-android"
            ARCH_ABI="arm64-v8a"
            TERMUX_ARCH="aarch64"
            ;;
        x86_64|amd64)
            DOCKER_PLATFORM="linux/amd64"
            TARGET_ARCH="x86_64-linux-android"
            ARCH_ABI="x86_64"
            TERMUX_ARCH="x86_64"
            ;;
        *)
            die "Unsupported target platform: $TARGET_PLATFORM"
            ;;
    esac
}

require_tools() {
    local tools=(curl tar ar python3)
    [[ $SKIP_EMBED -eq 0 ]] && tools+=(docker)
    for t in "${tools[@]}"; do
        command -v "$t" >/dev/null || die "Missing dependency: $t"
    done
}

setup_android_env() {
    ANDROID_HOME="${ANDROID_HOME:-$HOME/Projects/AndroidSdk}"
    ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/30.0.14904198}"
    export ANDROID_HOME ANDROID_NDK_HOME
    export PATH="$ANDROID_HOME/platform-tools:$HOME/.cargo/bin:$PATH"
}

run_dx_build() {
    [[ $SKIP_DX -eq 1 ]] && return

    local flags=(
        --android
        --codesign
        --package-types aab
        --features server
        --target "$TARGET_ARCH"
    )

    [[ "$BUILD_MODE" == release ]] && flags+=(--release)

    dx bundle "${flags[@]}"
}

compute_cache_dir() {
    docker pull --platform "$DOCKER_PLATFORM" "$DOCKER_IMAGE" >/dev/null

    IMAGE_ID="$(docker image inspect \
        --format='{{.Id}}' \
        "$DOCKER_IMAGE")"

    IMAGE_TAG="$(docker image inspect \
        --format='{{join .RepoTags ","}}' \
        "$DOCKER_IMAGE" 2>/dev/null || true)"

    CACHE_KEY="$(printf '%s|%s|%s' \
        "$DOCKER_IMAGE" \
        "$IMAGE_ID" \
        "$DOCKER_PLATFORM" | sha256sum | cut -c1-16)"

    CACHE_DIR="$SCRIPT_DIR/target/android-cache/$CACHE_KEY"
    mkdir -p "$CACHE_DIR"
}

download_termux_package() {
    local package="$1"
    local arch="$2"
    local base="$3"

    curl -fsSL "$base" |
        grep -oP "href=\"\K[^\"]*${package}_[^\"]*_${arch}\.deb" |
        sort -V |
        tail -1
}

fetch_proot() {
    [[ -f "$CACHE_DIR/proot" ]] && return

    local base="https://packages.termux.dev/apt/termux-main/pool/main/p/proot/"
    local deb

    deb="$(download_termux_package proot "$TERMUX_ARCH" "$base")"
    [[ -n "$deb" ]] || die "Failed locating proot package"

    curl -fsSL -o "$CACHE_DIR/proot.deb" "${base}${deb}"

    local work
    work="$(mktemp -d)"
    TEMP_DIRS+=("$work")

    cp "$CACHE_DIR/proot.deb" "$work/pkg.deb"

    (
        cd "$work"
        ar x pkg.deb
        tar -xf data.tar.*

        cp -L "$(find . -path '*/bin/proot' | head -1)" "$CACHE_DIR/proot"

        loader="$(find . -path '*/libexec/proot/loader' | head -1 || true)"
        [[ -n "$loader" ]] && cp -L "$loader" "$CACHE_DIR/loader"
    )

    chmod +x "$CACHE_DIR/proot"
}

fetch_libtalloc() {
    [[ -f "$CACHE_DIR/libtalloc.so.2" ]] && return

    local base="https://packages.termux.dev/apt/termux-main/pool/main/libt/libtalloc/"
    local deb

    deb="$(download_termux_package libtalloc "$TERMUX_ARCH" "$base")"
    [[ -n "$deb" ]] || die "Failed locating libtalloc package"

    curl -fsSL -o "$CACHE_DIR/libtalloc.deb" "${base}${deb}"

    local work
    work="$(mktemp -d)"
    TEMP_DIRS+=("$work")

    cp "$CACHE_DIR/libtalloc.deb" "$work/pkg.deb"

    (
        cd "$work"
        ar x pkg.deb
        tar -xf data.tar.*

        find . -name 'libtalloc.so*' -exec cp -L {} "$CACHE_DIR/" \;
    )
}

export_rootfs() {
    local manifest="$CACHE_DIR/embedded_rootfs_manifest.json"

    [[ -f "$manifest" ]] && return

    local stage="$CACHE_DIR/rootfs_stage"
    mkdir -p "$stage"

    local cid
    cid="$(docker create --platform "$DOCKER_PLATFORM" "$DOCKER_IMAGE")"
    CONTAINERS+=("$cid")

    docker export "$cid" | tar -C "$stage" --no-same-owner -xf -

    python3 - "$stage" "$CACHE_DIR" "$DOCKER_IMAGE" "$cid" <<'PY'
import datetime, hashlib, json, os, shutil, stat, subprocess, sys

root, cache, image, cid = sys.argv[1:]

files_dir = os.path.join(cache, "rootfs_files")
os.makedirs(files_dir, exist_ok=True)

cfg = json.loads(subprocess.check_output(
    ["docker", "inspect", "--format", "{{json .Config}}", cid],
    text=True
))

manifest = {
    "version": 2,
    "docker_image": image,
    "created": datetime.datetime.utcnow().isoformat() + "Z",
    "env": cfg.get("Env") or [],
    "entrypoint": cfg.get("Entrypoint"),
    "cmd": cfg.get("Cmd"),
    "files": {},
    "symlinks": {}
}

for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
    for name in filenames:
        p = os.path.join(dirpath, name)
        rel = "/" + os.path.relpath(p, root)

        if os.path.islink(p):
            manifest["symlinks"][rel] = os.readlink(p)
            continue

        st = os.stat(p)
        h = hashlib.sha256(rel.encode()).hexdigest()[:16]

        dst = os.path.join(files_dir, h)
        shutil.copy2(p, dst)

        manifest["files"][h] = {
            "path": rel,
            "size": st.st_size,
            "mode": stat.S_IMODE(st.st_mode)
        }

with open(os.path.join(cache, "embedded_rootfs_manifest.json"), "w") as f:
    json.dump(manifest, f, separators=(",", ":"))
PY
}

copy_assets() {
    local jni="$JNILIB_DIR"

    mkdir -p "$jni"

    cp "$CACHE_DIR/proot" "$jni/libproot.so"
    cp "$CACHE_DIR/loader" "$jni/libloader.so"
    cp "$CACHE_DIR/embedded_rootfs_manifest.json" \
       "$jni/librootfs-manifest.so"

    find "$CACHE_DIR" -maxdepth 1 -name 'libtalloc.so*' \
        -exec cp {} "$jni/" \;

    find "$CACHE_DIR/rootfs_files" -type f | while read -r f; do
        cp "$f" "$jni/librootfs-$(basename "$f").so"
    done
}

patch_proot() {
    command -v patchelf >/dev/null || return 0

    local p="$JNILIB_DIR/libproot.so"
    [[ -f "$p" ]] || return 0

    patchelf --set-rpath '$ORIGIN' "$p" || true
    patchelf --replace-needed libtalloc.so.2 libtalloc.so "$p" || true
}

patch_gradle() {
    local file="$APP_BUILD_GRADLE"
    [[ -f "$file" ]] || return 0
    sed 's/android {/android {\n    packagingOptions {\n        jniLibs {\n            useLegacyPackaging = true\n        }\n    }\n/' -i "$file"
}

patch_manifest() {
    local file="$MANIFEST"
    [[ -f "$file" ]] || return 0

    grep -q "extractNativeLibs" "$file" && return 0

    python3 - "$file" <<'PY'
import re,sys
p=sys.argv[1]
s=open(p).read()
s=re.sub(r'(<application\b)',
            r'\1\n        android:extractNativeLibs="true"',
            s, count=1)
open(p,"w").write(s)
PY
}

build_android() {
    (
        cd "$GRADLE_PROJECT"
        chmod +x gradlew

        if [[ "$BUILD_MODE" == "release" ]]; then
            ./gradlew bundleRelease --no-daemon
        else
            ./gradlew assembleDebug --no-daemon
        fi
    )
}

get_artifact_path() {
    if [[ "$BUILD_MODE" == "release" ]]; then
        echo "$GRADLE_PROJECT/app/build/outputs/bundle/release"
    else
        echo "$GRADLE_PROJECT/app/build/outputs/apk/debug"
    fi
}

sign_aab() {
    local aab="$1"

    [[ -f "$aab" ]] || die "AAB not found: $aab"

    if [[ -n "${ANDROID_KEYSTORE_B64:-}" ]]; then
        echo "🔐 Decoding keystore from environment variable"
        echo "$ANDROID_KEYSTORE_B64" | base64 -d > "$CACHE_DIR/keystore.jks"
        export ANDROID_KEYSTORE_PATH="$CACHE_DIR/keystore.jks"
    fi

    : "${ANDROID_KEYSTORE_PATH:?Missing ANDROID_KEYSTORE_PATH}"
    : "${ANDROID_KEYSTORE_PASSWORD:?Missing ANDROID_KEYSTORE_PASSWORD}"
    : "${ANDROID_KEY_ALIAS:?Missing ANDROID_KEY_ALIAS}"
    : "${ANDROID_KEY_PASSWORD:?Missing ANDROID_KEY_PASSWORD}"

    command -v jarsigner >/dev/null || die "jarsigner not found (install JDK)"

    echo "🔐 Signing AAB: $aab"

    jarsigner \
        -sigalg SHA256withRSA \
        -digestalg SHA-256 \
        -keystore "$ANDROID_KEYSTORE_PATH" \
        -storepass "$ANDROID_KEYSTORE_PASSWORD" \
        -keypass "$ANDROID_KEY_PASSWORD" \
        "$aab" \
        "$ANDROID_KEY_ALIAS"

    echo "✔ Signature applied"

    echo "🔎 Verifying signature..."
    jarsigner -verify -verbose -certs "$aab" >/dev/null \
        || die "AAB signature verification failed"

    echo "✔ AAB verified successfully"
}

main() {
    parse_args "$@"
    resolve_platform
    require_tools
    setup_android_env

    GRADLE_MODE="$(tr '[:lower:]' '[:upper:]' <<< "${BUILD_MODE:0:1}")${BUILD_MODE:1}"

    run_dx_build

    GRADLE_PROJECT="$SCRIPT_DIR/target/dx/colmap-openmvs-app/$BUILD_MODE/android/app"
    APP_BUILD_GRADLE="$GRADLE_PROJECT/app/build.gradle.kts"
    MANIFEST="$GRADLE_PROJECT/app/src/main/AndroidManifest.xml"
    JNILIB_DIR="$GRADLE_PROJECT/app/src/main/jniLibs/$ARCH_ABI"

    [[ -f "$GRADLE_PROJECT/gradlew" ]] || die "Gradle project missing"

    if [[ $SKIP_EMBED -eq 0 ]]; then
        compute_cache_dir
        fetch_proot
        fetch_libtalloc
        export_rootfs
        copy_assets
        patch_proot
    fi

    patch_gradle
    patch_manifest
    build_android

    ARTIFACT_DIR="$(get_artifact_path)"
    if [[ "$BUILD_MODE" == "release" ]]; then
        AAB_FILE="$ARTIFACT_DIR/app-release.aab"
        [[ -n "$AAB_FILE" ]] || die "AAB not found"
        find "$ARTIFACT_DIR" -type f ! -name "app-release.aab" -delete
        sign_aab "$AAB_FILE"
    fi

    echo
    echo "Build complete"
    ls -lh "$ARTIFACT_DIR"
}

main "$@"
