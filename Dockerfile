ARG RUNTIME_IMAGE=debian:bookworm-slim
ARG BUILD_TYPE=debug

FROM rust:1 AS chef
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG BUILD_TYPE

RUN apt-get update && apt-get install -y \
    curl nodejs npm pkg-config libwayland-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -L --proto '=https' --tlsv1.2 -sSf \
    https://raw.githubusercontent.com/DioxusLabs/dioxus/refs/heads/main/.github/install.sh | bash

COPY --from=planner /app/recipe.json recipe.json

RUN if [ "$BUILD_TYPE" = "debug" ]; then \
    cargo chef cook --recipe-path recipe.json; \
    else \
    cargo chef cook --release --recipe-path recipe.json; \
    fi

COPY . .

RUN if [ "$BUILD_TYPE" = "debug" ]; then \
    /usr/local/cargo/bin/dx bundle --web; \
    else \
    /usr/local/cargo/bin/dx bundle --web --release; \
    fi


# ---------------------------
# Runtime selection layer
# ---------------------------
FROM ${RUNTIME_IMAGE} AS runtime

ARG BUILD_TYPE
ARG ENABLE_DOCKER=true  # proot runtime is a valid alternative to avoid messing with docker socket and too many permissions issues, but it is not as performant as docker runtime
RUN if [ "$ENABLE_DOCKER" = "true" ]; then \
    apt-get update && apt-get install -y docker.io && rm -rf /var/lib/apt/lists/*; \
    fi

WORKDIR /usr/local/app

# minimal deps for running binary (adjust if needed)
RUN apt-get update && apt-get install -y \
    ca-certificates libwayland-dev \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder \
    /app/target/dx/colmap-openmvs-app/${BUILD_TYPE}/web \
    /usr/local/app

# ------------------------------------------------------------------
# Server configuration — all settings are overridable at runtime
# via environment variables (or CLI flags when using docker run).
#
# Persistent data volume (mount here for remote development):
#   docker run -v /host/data:/data ...
#
# Override individual settings with -e:
#   docker run -e COLMAP_PROJECTS_FOLDER=/data/projects ...
# ------------------------------------------------------------------

# Server address (Dioxus reads IP / PORT env vars internally).
ENV IP=0.0.0.0
ENV PORT=8080
EXPOSE 8080

# ── Core data directories ──────────────────────────────────────────────
#
# Mount a persistent volume at /data to keep projects and images across
# container restarts:
#
#   docker run -v /my/storage:/data ...
#
# Or override individual paths via env vars / CLI flags:
#
#   --projects-folder  /data/projects    (COLMAP_PROJECTS_FOLDER)
#   --proot-images-dir /data/proot-images (COLMAP_PROOT_IMAGES_DIR)
# -----------------------------------------------------------------------
ENV COLMAP_PROJECTS_FOLDER=/data/projects
ENV COLMAP_PROOT_IMAGES_DIR=/data/proot-images

# ── PRoot binary directory ─────────────────────────────────────────────
# Directory containing the PRoot binary and its supporting libraries.
# Default inside the container — override if you distribute PRoot separately.
#   --proot-binary-dir /tmp/  (COLMAP_PROOT_BINARY_DIR)
# -----------------------------------------------------------------------
ENV COLMAP_PROOT_BINARY_DIR=/tmp/

# ── Config file ────────────────────────────────────────────────────────
# Point to an alternative settings.json anywhere on the filesystem:
#   --config /data/settings.json   (COLMAP_CONFIG)
# -----------------------------------------------------------------------

# ── Default image tag ──────────────────────────────────────────────────
# Pre-configured container image for running COLMAP/OpenMVS pipelines.
# Examples:
#   "proot:mirror.gcr.io/yeicor/colmap-openmvs:cpu-latest"
#   "docker:mirror.gcr.io/yeicor/colmap-openmvs:cpu-latest"
# -----------------------------------------------------------------------

# ── Custom mounts ──────────────────────────────────────────────────────
# Additional filesystem mounts for PRoot/Docker runtime.
# Can be specified multiple times (repeat --custom-mount or set
# COLMAP_CUSTOM_MOUNT to a space-separated list).
# Format: "/host/path:/container/path" or just "/host/path"
#
# Example (CUDA):
#   --custom-mount /usr/lib/x86_64-linux-gnu/libcuda.so
# -----------------------------------------------------------------------

ENTRYPOINT ["/usr/local/app/server"]
