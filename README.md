# Rust Docs

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A high-performance Model Context Protocol (MCP) server that provides AI
assistants with access to Rust crate documentation through semantic search and
automatic population.

## Overview

This server enables AI assistants to query Rust documentation using natural
language, powered by PostgreSQL with pgvector for efficient similarity search.
It features automatic crate population, supports multiple embedding providers,
and can scale to handle documentation for numerous crates simultaneously.

## ✨ Key Features

- 🚀 **Automatic Population**: Crates are automatically populated with
  documentation on first access
- 🔍 **Semantic Search**: Vector-based similarity search across Rust crate documentation
- 🌐 **MCP-Compliant**: HTTP/SSE transport for broad compatibility with AI assistants
- 📊 **PostgreSQL + pgvector**: Scalable vector database with optimized indexing
- 🤖 **Multiple Embedding Providers**: OpenAI and Voyage AI support
- 🐳 **Production Ready**: Kubernetes deployment with Helm charts
- 🛠️ **Zero Configuration**: Add crates via MCP tools - population happens automatically

## Architecture

```text
┌─────────────────┐     ┌─────────────────┐
│   Cursor IDE    │     │   Claude Code   │
│                 │     │                 │
└────────┬────────┘     └────────┬────────┘
         │ MCP Protocol          │
         │ (HTTP/SSE)            │
         ▼                       ▼
┌─────────────────────────────────────────┐
│        Rust Docs Server (Port 3000)    │
│  ┌─────────────────────────────────┐   │
│  │     HTTP/SSE Transport Layer     │   │
│  └─────────────────────────────────┘   │
│  ┌─────────────────────────────────┐   │
│  │  Tools: add_crate, query_docs   │   │
│  │  🚀 Auto-population on startup  │   │
│  └─────────────────────────────────┘   │
└────────────────┬────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────┐
│         PostgreSQL + pgvector           │
│  ┌─────────────────────────────────┐   │
│  │   Vector Embeddings (3072-dim)   │   │
│  │   Crate Configs & Metadata      │   │
│  └─────────────────────────────────┘   │
└─────────────────────────────────────────┘
```

## 🚀 Quick Start

### Prerequisites

- Docker & Kubernetes (for deployment) OR
- PostgreSQL 15+ with pgvector extension (for local development)
- OpenAI API key or Voyage AI API key

### Option 1: Kubernetes Deployment (For Production)

```bash
# Clone the repository
git clone https://github.com/5dlabs/rust-docs
cd rust-docs

# Update values.yaml with your OpenAI API key
# Edit charts/rust-docs-mcp-server/values.yaml:
#   app.secrets.openaiApiKey: "sk-your-key-here"

# Deploy to Kubernetes
helm install rustdocs charts/rust-docs-mcp-server/ --namespace mcp --create-namespace

# Get service URL
kubectl get svc -n mcp rustdocs-rust-docs-mcp-server
```

### Option 2: Local Development

```bash
# 1. Setup PostgreSQL with pgvector
createdb rust_docs_vectors
psql rust_docs_vectors -c "CREATE EXTENSION IF NOT EXISTS vector;"

# 2. Set environment variables
export MCPDOCS_DATABASE_URL="postgresql://username@localhost/rust_docs_vectors"
export OPENAI_API_KEY="sk-your-key-here"

# 3. Run the HTTP server
cargo run --bin rustdocs_mcp_server_http --all
```

## 🔧 MCP Client Configuration

### Cursor IDE

Cursor connects to the MCP server via HTTP/SSE from your local machine,
regardless of where the server is hosted.

1. **Add to Cursor's MCP configuration** (File → Preferences → Features → MCP):

```json
{
  "mcpServers": {
    "rust-docs": {
      "url": "http://localhost:3000",
      "description": "Rust crate documentation with semantic search"
    }
  }
}
```

1. **If the server is deployed on Kubernetes**, use the external service URL:

```json
{
  "mcpServers": {
    "rust-docs": {
      "url": "http://your-k8s-cluster-ip:3000"
    }
  }
}
```

1. **Usage in Cursor**:
   - Open any Rust project
   - Ask questions like: "How do I use tokio's select! macro?"
   - The assistant will automatically use the server to search Rust documentation

### Claude Code

1. **Add the server**:

```bash
# For local server
claude mcp add rust-docs http://localhost:3000

# If server is deployed on Kubernetes
claude mcp add rust-docs http://your-k8s-cluster-ip:3000
```

1. **Verify the connection**:

```bash
claude mcp list
claude mcp test rust-docs
```

1. **Usage with Claude Code**:

```bash
# Claude will automatically use the server for Rust questions
claude ask "How do I use async/await with tokio?"
claude ask "What's the difference between Vec and VecDeque?"
```

## 🎯 Using the MCP Tools

The server provides several MCP tools for managing crate documentation:

### `add_crate` - Add and Auto-Populate Crates

```javascript
// In Cursor or Claude Code, you can ask:
"Add the axum crate with the 'macros' feature"

// This will automatically:
// 1. Configure the crate in the database
// 2. Load documentation from docs.rs
// 3. Generate embeddings with OpenAI
// 4. Store in PostgreSQL for search
```

### `query_rust_docs` - Search Documentation

```javascript
// Examples of natural language queries:
"How do I create a web server with axum?"
"What's the syntax for serde derive macros?"
"How do I handle errors in tokio async functions?"
```

### `list_crates` - View Available Crates

```javascript
"What Rust crates are available for documentation search?"
```

### `check_crate_status` - Monitor Population

```javascript
"What's the status of the tokio crate documentation?"
```

## 🔄 Automatic Population

The server features **zero-configuration automatic population**:

### On Server Startup

- Automatically detects configured crates without documentation
- Populates missing crates in the background
- Shows detailed progress logging
- Server becomes available immediately (population happens asynchronously)

### When Adding New Crates

- Use the `add_crate` MCP tool to add any Rust crate
- Documentation is automatically downloaded from docs.rs
- Embeddings are generated and stored for instant search
- No manual intervention required

### Population Process

1. **Document Loading**: Fetches HTML documentation from docs.rs
2. **Content Extraction**: Parses and chunks documentation content
3. **Embedding Generation**: Creates vector embeddings using OpenAI/Voyage
4. **Database Storage**: Stores in PostgreSQL with pgvector for fast search
5. **Indexing**: Creates optimized indexes for similarity search

## 📊 Management and Monitoring

### Database Tables

- **`crate_configs`**: Crate configurations and metadata
- **`doc_embeddings`**: Vector embeddings with content
- **`crates`**: Crate statistics and version info
- **`population_jobs`**: Background job tracking

### Monitoring Commands

```bash
# Check server logs
kubectl logs -f deployment/rustdocs-rust-docs-mcp-server -n mcp

# Check database status
kubectl exec -n mcp rustdocs-postgresql-0 -- psql -U rustdocs \
  -d rust_docs_vectors \
  -c "SELECT name, total_docs, last_updated FROM crates ORDER BY name;"
```

## 🐳 Docker & Kubernetes

### Production Deployment

The server is designed for production Kubernetes deployment:

- **Custom PostgreSQL Image**: Includes pgvector extension
- **Helm Charts**: Complete production deployment
- **Health Checks**: Kubernetes-native health monitoring
- **Secrets Management**: Secure API key handling
- **Persistent Storage**: Durable documentation storage
- **Horizontal Scaling**: Stateless server design

### GitHub Actions

Automated CI/CD pipeline:

- Builds optimized Docker images
- Publishes to GitHub Container Registry
- Supports multi-architecture builds (AMD64/ARM64)
- Automated deployments on push

## 🔍 API Reference

### MCP Tools

#### `add_crate`

Add a new crate configuration and trigger automatic population.

**Parameters:**

- `crate_name` (string): Crate name (e.g., "tokio")
- `version_spec` (string): Version ("latest" or specific version)
- `features` (array, optional): Feature flags (e.g., ["full", "macros"])

#### `query_rust_docs`

Search documentation using natural language queries.

**Parameters:**

- `crate_name` (string): The crate to search within
- `question` (string): Natural language query

#### `list_crates`

List all configured crates and their status.

**Parameters:**

- `enabled_only` (boolean, optional): Show only enabled crates

#### `check_crate_status`

Get detailed status of a specific crate's documentation.

**Parameters:**

- `crate_name` (string): The crate to check

#### `remove_crate`

Remove a crate configuration and its documentation.

**Parameters:**

- `crate_name` (string): The crate to remove
- `version_spec` (string, optional): Specific version to remove

## 🎨 Example Usage

### In Cursor IDE

```javascript
// Natural language queries in chat:
"How do I use tokio's timeout function?"
"What's the syntax for serde's rename attribute?"
"Show me how to create a JSON API with axum"
"How do I handle database connections with sqlx?"

// Adding new crates:
"Add the reqwest crate with json features"
"Configure the clap crate for command line parsing"
```

### In Claude Code

```bash
# Ask questions about Rust APIs
claude ask "How do I use async channels in tokio?"

# Add crates for documentation
claude ask "Add the diesel crate and show me how to connect to a database"

# Check what's available
claude ask "List all available Rust crates for documentation"
```

## 🔧 Development

### Building from Source

```bash
cargo build --release --bin rustdocs_mcp_server_http
```

### Running Tests

```bash
cargo test
```

### Local Development with Hot Reload

```bash
cargo watch -x "run --bin rustdocs_mcp_server_http"
```

## 📈 Performance

- **Vector Search**: Optimized IVFFlat indexing for sub-second queries
- **Embeddings**: 3072-dimensional OpenAI text-embedding-3-large
- **Concurrent Processing**: Parallel document processing and embedding generation
- **Database**: Connection pooling and prepared statements
- **Caching**: Intelligent caching of embeddings and search results

## 🔒 Security

- **API Key Management**: Secure Kubernetes secrets for embedding providers
- **Database Security**: Encrypted connections and role-based access
- **Container Security**: Non-root containers with minimal attack surface
- **Pod Security**: Pod Security Standards compliance

## 📝 License

MIT License - see [LICENSE](LICENSE) file for details.

## 🤝 Contributing

Contributions welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## 📞 Support

- **Issues**: [GitHub Issues](https://github.com/5dlabs/rust-docs/issues)
- **Discussions**: [GitHub Discussions](https://github.com/5dlabs/rust-docs/discussions)
