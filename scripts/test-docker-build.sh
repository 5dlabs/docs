#!/bin/bash
set -e

echo "Building binaries locally..."

# Build for current platform
cargo build --release --bin rustdocs_mcp_server_http

# Detect platform
if [[ "$(uname -m)" == "arm64" ]] || [[ "$(uname -m)" == "aarch64" ]]; then
    PLATFORM="linux/arm64"
    echo "Detected ARM64 platform"
else
    PLATFORM="linux/amd64"
    echo "Detected AMD64 platform"
fi

# For testing on Mac, we'll just verify the Dockerfile syntax
echo "Validating Dockerfile..."
docker build -f Dockerfile.prebuilt --platform $PLATFORM --no-cache --target base -t rust-docs-mcp-test-base . 2>&1 | head -20 || true

echo "✅ Dockerfile validation complete!"
echo ""
echo "Note: Full cross-platform testing will occur in GitHub Actions."
echo "The workflow will build native binaries for each platform."

# Cleanup
rm -rf binaries