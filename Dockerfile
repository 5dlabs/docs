# Multi-stage build
FROM rustlang/rust:nightly-bookworm AS builder

WORKDIR /app
COPY . .

# Build the release binary
RUN cargo build --release --bin rustdocs_mcp_server_http

# Runtime image
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy built binary from builder stage
COPY --from=builder /app/target/release/rustdocs_mcp_server_http /usr/local/bin/http_server
RUN chmod +x /usr/local/bin/http_server

# Copy entrypoint script
COPY docker-entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Create non-root user
RUN useradd -m -u 1000 rustdocs && chown -R rustdocs:rustdocs /app
USER rustdocs

# Expose port
EXPOSE 3000

# Set environment variables
ENV RUST_LOG=rustdocs_mcp_server_http=info,rmcp=info
ENV HOST=0.0.0.0
ENV PORT=3000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -f http://localhost:8080/health/live || exit 1

# Set entrypoint and default command
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["http_server", "--all"]
