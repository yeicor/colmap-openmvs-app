# Multi-stage build for the Rust application

# Stage 1: Builder
FROM rust:1 AS builder

# Install dependencies
RUN apt-get update && apt-get install -y \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
WORKDIR /build
COPY . .

# Install Dioxus CLI (prebuilt binary)
RUN curl -sSL https://dioxus.dev/install.sh | bash
ENV PATH="/root/.dx/bin:$PATH"

# Build the server binary with fullstack features using Dioxus CLI
RUN dx build --server --release

# Stage 2: Runtime
FROM debian:sid-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    libssl3 \
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
