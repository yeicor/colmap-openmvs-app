# Multi-stage build for the Rust application
# TODO: https://github.com/DioxusLabs/dioxus/blob/f3a9137041877199c94db17148986ece6dc1b4c0/examples/01-app-demos/hotdog/Dockerfile#L7
#
# Stage 1: Builder
FROM rust:1 AS builder

# Install dependencies (https://dioxuslabs.com/learn/0.7/getting_started/)
RUN apt-get update && apt install -y \
    libwebkit2gtk-4.1-dev \
    build-essential \
    curl \
    wget \
    file \
    libxdo-dev \
    libssl-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    lld \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
WORKDIR /build
COPY . .

# Install Dioxus CLI (prebuilt binary)
RUN curl -sSL https://dioxus.dev/install.sh | bash
ENV PATH="/root/.dx/bin:$PATH"

# Build the server binary with fullstack features using Dioxus CLI
RUN dx build --web --features server --release

# Stage 2: Runtime
FROM debian:sid-slim

# Install runtime dependencies (https://dioxuslabs.com/learn/0.7/getting_started/)
RUN apt-get update && apt install -y \
    libwebkit2gtk-4.1-dev \
    build-essential \
    curl \
    wget \
    file \
    libxdo-dev \
    libssl-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    lld \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary from builder
COPY --from=builder /build/target/dx/colmap-openmvs-app/release/web /app/

# Expose port (adjust as needed for your app)
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD test -f /proc/1/stat || exit 1



# Run the application
CMD ["/app/server"]
