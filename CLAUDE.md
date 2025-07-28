# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Quick Start for Development

Run these commands to get started with local development:

```bash
# Setup database with pgvector
createdb rust_docs_vectors
psql rust_docs_vectors -c "CREATE EXTENSION IF NOT EXISTS vector;"
psql rust_docs_vectors < sql/schema.sql

# Set required environment variables
export MCPDOCS_DATABASE_URL="postgresql://username@localhost/rust_docs_vectors"
export OPENAI_API_KEY="sk-..."

# Run HTTP server for development
cargo run --bin rustdocs_mcp_server_http -- --all
```

## Project Overview

This is a Rust-based MCP (Model Context Protocol) server that provides AI
assistants with up-to-date Rust crate documentation. It uses PostgreSQL with
pgvector for semantic search capabilities and supports multiple embedding
providers (OpenAI and Voyage AI).

**Repository**: <https://github.com/5dlabs/rust-docs> (renamed from
rust-docs-mcp-server)

## Essential Commands

### Linting and Code Quality

```bash
# Run Clippy with all targets and features
cargo clippy --all-targets --all-features -- -D warnings

# Run rustfmt
cargo fmt --all -- --check

# Fix format issues
cargo fmt --all
```

### Building and Testing

```bash
# Build the project
cargo build --release

# Build specific binary targets
cargo build --release --bin rustdocs_mcp_server_http
cargo build --release --bin populate_db

# Run tests
cargo test

# Run specific test
cargo test test_search

# Check for compilation errors
cargo check --all-targets --all-features
```

### Running Binaries

```bash
# HTTP MCP server (main server for production)
cargo run --bin rustdocs_mcp_server_http -- --all

# Stdio MCP server (for single-user scenarios)
cargo run --bin rustdocs_mcp_server -- --all

# Population tools
cargo run --bin populate_db -- --crate-name tokio --features full
cargo run --bin populate_all
cargo run --bin backfill_versions

# Migration from old config
cargo run --bin migrate_config
```

### Database Operations

```bash
# Create database with pgvector
createdb rust_docs_vectors
psql rust_docs_vectors -c "CREATE EXTENSION IF NOT EXISTS vector;"
psql rust_docs_vectors < sql/schema.sql
psql rust_docs_vectors < sql/migrations/add_crate_configs.sql

# Required environment variables
export MCPDOCS_DATABASE_URL="postgresql://username@localhost/rust_docs_vectors"
export OPENAI_API_KEY="sk-..." # Or VOYAGE_API_KEY for Voyage embeddings
```

### Kubernetes/Helm Deployment

```bash
# Deploy to mcp namespace
helm upgrade --install rustdocs-mcp ./charts/rust-docs-mcp-server \
  --namespace mcp \
  --create-namespace \
  --set image.tag=latest \
  --set postgresql.enabled=true

# Port forward for local access
kubectl port-forward -n mcp service/rustdocs-mcp-rust-docs-mcp-server 3000:3000

# Or use the service URL directly (if you have cluster access via TwinGate, etc)
# Service URL: http://rustdocs-mcp-rust-docs-mcp-server.mcp.svc.cluster.local:3000
```

## Architecture

### Core Components

1. **Database Layer** (`src/database.rs`)
   - PostgreSQL with pgvector extension
   - Tables: `crates`, `doc_embeddings`, `crate_configs`, `population_jobs`
   - Vector similarity search using 3072-dimensional embeddings
   - Database-driven crate configuration (replaced proxy-config.json)

2. **MCP Servers**
   - **HTTP Server** (`src/bin/http_server.rs`): Production server with SSE transport
   - **Stdio Server** (`src/main.rs`): Single-user server with stdio transport
   - Both expose the same MCP tools

3. **MCP Tools**
   - `query_rust_docs`: Semantic search across documentation
   - `add_crate`: Add/update crate configuration
   - `list_crates`: List configured crates
   - `remove_crate`: Remove crate configuration

4. **Document Processing**
   - `src/doc_loader.rs`: Parses HTML from `cargo doc`
   - `src/embeddings.rs`: OpenAI/Voyage embedding generation
   - `src/llm.rs`: LLM summarization of search results

### Binary Names and Docker Context

**CRITICAL**: The binary names differ between Cargo.toml and Docker:

- Cargo.toml: `rustdocs_mcp_server_http`
- Docker image: Copied as `http_server`
- Helm deployment: Uses `command: ["http_server"]`

### Key Binary Files

- `src/main.rs`: Stdio MCP server (single-user)
- `src/bin/http_server.rs`: HTTP MCP server (production)
- `src/bin/populate_db.rs`: Single crate population tool
- `src/bin/populate_all.rs`: Bulk crate population
- `src/bin/backfill_versions.rs`: Version backfill utility
- `src/bin/migrate_config.rs`: Config migration from old format

### Environment Variables

- `MCPDOCS_DATABASE_URL`: PostgreSQL connection string
- `OPENAI_API_KEY`: For OpenAI embeddings/LLM
- `VOYAGE_API_KEY`: For Voyage embeddings
- `RUST_LOG`: Logging configuration

### CI/CD Pipeline

- Uses GitHub Actions with self-hosted runners
- Runner labels: `[self-hosted, Linux, X64, k8s-runner, rust-builder, org-runner]`
- Workflow: `.github/workflows/unified-ci-cd.yml`
- Quick deploy workflow: `.github/workflows/quick-deploy.yml`

## Critical Context and Common Issues

### HTTP Server Startup

- The server can now start with zero crates configured
- It will log warnings but continue running
- Use MCP tools to add crates after startup

### Image Pull Issues

- Set `pullPolicy: Always` in values.yaml for `latest` tag
- Kubernetes may cache images with `IfNotPresent`

### Health Endpoint

- Readiness probe expects `/health` endpoint (not implemented)
- This causes deployment timeouts

### PostgreSQL Storage Class

- Configure under `postgresql.primary.persistence.storageClass`
- Also set `postgresql.global.storageClass` for global config
- Use `local-path` for local development clusters

### RBAC Configuration

- GitHub runner service account in `arc-systems` namespace
- Needs permissions in `mcp` namespace
- Apply `mcp-namespace-rbac.yaml` for proper permissions

## Development Notes

- All async code uses Tokio runtime
- Error handling with custom `ServerError` type (`src/error.rs`)
- MCP implementation uses `rmcp` crate with SSE transport for HTTP server
- Release builds optimized with LTO, strip, and panic=abort
- Database operations use SQLx with compile-time query verification
- Vector embeddings use 3072-dimensional OpenAI text-embedding-3-large model
- Database uses pgvector extension for similarity search with IVFFlat indexing

### MCP Server Architecture

The codebase implements two MCP servers that share the same core logic:

1. **HTTP Server** (`src/bin/http_server.rs`): Production server using SSE transport
   - Runs on port 3000 by default
   - Handles concurrent MCP clients
   - Used by Cursor IDE and other HTTP-based MCP clients

2. **Stdio Server** (`src/main.rs`): Single-user server using stdio transport
   - Direct stdin/stdout communication
   - Simpler deployment for single-user scenarios

Both servers expose identical MCP tools defined in `src/server.rs`:
- `query_rust_docs`: Vector search across documentation
- `add_crate`: Configure and populate new crates
- `list_crates`: List available crates
- `remove_crate`: Remove crate configurations
- `check_crate_status`: Check population status

### Core Data Flow

1. **Document Processing**: `doc_loader.rs` parses HTML from docs.rs
2. **Embedding Generation**: `embeddings.rs` creates vectors via OpenAI/Voyage APIs
3. **Database Storage**: `database.rs` handles PostgreSQL operations with pgvector
4. **Search**: Vector similarity search with LLM summarization via OpenAI
