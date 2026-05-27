FROM rust:1 AS chef
ARG BUILD_TYPE=debug
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
RUN apt-get update && apt-get install -y curl nodejs npm && rm -rf /var/lib/apt/lists/*
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/DioxusLabs/dioxus/refs/heads/main/.github/install.sh | bash
COPY --from=planner /app/recipe.json recipe.json
# Mount target as a cache volume for build caching
RUN if [ "$BUILD_TYPE" = "debug" ]; then \
    cargo chef cook --recipe-path recipe.json; \
  else \
    cargo chef cook --$BUILD_TYPE --recipe-path recipe.json; \
  fi
COPY . .
RUN if [ "$BUILD_TYPE" = "debug" ]; then \
    /usr/local/cargo/bin/dx bundle --web; \
  else \
    /usr/local/cargo/bin/dx bundle --web --$BUILD_TYPE; \
  fi && ls

FROM chef AS runtime
COPY --from=builder /app/target/dx/colmap-openmvs-app/${BUILD_TYPE}/web /usr/local/app

ENV PORT=8080
ENV IP=0.0.0.0
EXPOSE 8080

WORKDIR /usr/local/app
ENTRYPOINT [ "/usr/local/app/server" ]
