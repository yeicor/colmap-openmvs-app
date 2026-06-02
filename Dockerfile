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
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder \
    /app/target/dx/colmap-openmvs-app/${BUILD_TYPE}/web \
    /usr/local/app

ENV PORT=8080
ENV IP=0.0.0.0
EXPOSE 8080

ENTRYPOINT ["/usr/local/app/server"]
