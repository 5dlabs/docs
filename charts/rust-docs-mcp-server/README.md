# Rust Docs MCP Server Helm Chart

This Helm chart deploys the Rust Docs MCP Server with PostgreSQL and pgvector support on Kubernetes.

## Prerequisites

- Kubernetes 1.19+
- Helm 3.2.0+
- PostgreSQL 15+ with pgvector extension (provided by Bitnami chart)

## Installation

### Add the Bitnami repository (required for PostgreSQL dependency)

```bash
helm repo add bitnami https://charts.bitnami.com/bitnami
helm repo update
```

### Install the chart

```bash
# Install with default values
helm install rust-docs-mcp ./charts/rust-docs-mcp-server

# Install with custom values
helm install rust-docs-mcp ./charts/rust-docs-mcp-server -f my-values.yaml

# Install with inline values
helm install rust-docs-mcp ./charts/rust-docs-mcp-server \
  --set app.secrets.openaiApiKey="sk-your-api-key" \
  --set postgresql.auth.password="your-password"
```

## Configuration

### Required Configuration

Before deploying, you must configure at least one API key:

```yaml
app:
  secrets:
    # Required: OpenAI API key (if using OpenAI embedding provider)
    openaiApiKey: "sk-your-openai-api-key"

    # OR: Voyage AI API key (if using Voyage embedding provider)
    voyageApiKey: "your-voyage-api-key"
```

### Basic Configuration

```yaml
# values.yaml
app:
  env:
    EMBEDDING_PROVIDER: "openai"  # or "voyage"
    EMBEDDING_MODEL: "text-embedding-3-large"
    LLM_MODEL: "gpt-4o-mini-2024-07-18"

  secrets:
    openaiApiKey: "sk-your-api-key"

postgresql:
  enabled: true
  auth:
    postgresPassword: "your-postgres-password"
    username: "rustdocs"
    password: "your-user-password"
    database: "rust_docs_vectors"
```

### Advanced Configuration

#### Using External PostgreSQL

```yaml
postgresql:
  enabled: false

externalDatabase:
  host: "your-postgres-host"
  port: 5432
  username: "rustdocs"
  database: "rust_docs_vectors"
  existingSecret: "postgres-secret"
  existingSecretPasswordKey: "password"
```

#### Enabling Ingress

```yaml
ingress:
  enabled: true
  className: "nginx"
  annotations:
    kubernetes.io/tls-acme: "true"
    nginx.ingress.kubernetes.io/cors-allow-methods: "GET, POST, OPTIONS"
    nginx.ingress.kubernetes.io/cors-allow-headers: "DNT,User-Agent,X-Requested-With,If-Modified-Since,Cache-Control,Content-Type,Range,Authorization"
  hosts:
    - host: rust-docs-mcp.example.com
      paths:
        - path: /
          pathType: Prefix
  tls:
    - secretName: rust-docs-mcp-tls
      hosts:
        - rust-docs-mcp.example.com
```

#### Horizontal Pod Autoscaling

```yaml
autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilizationPercentage: 80
  targetMemoryUtilizationPercentage: 80
```

#### Resource Limits

```yaml
resources:
  limits:
    cpu: 1000m
    memory: 2Gi
  requests:
    cpu: 200m
    memory: 512Mi

postgresql:
  primary:
    resources:
      requests:
        memory: 512Mi
        cpu: 200m
      limits:
        memory: 2Gi
        cpu: 1000m
```

## Values

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `replicaCount` | int | `1` | Number of replicas |
| `image.repository` | string | `"ghcr.io/5dlabs/rust-docs-mcp-server"` | Image repository |
| `image.pullPolicy` | string | `"IfNotPresent"` | Image pull policy |
| `image.tag` | string | `""` | Image tag (defaults to chart appVersion) |
| `service.type` | string | `"ClusterIP"` | Service type |
| `service.port` | int | `3000` | Service port |
| `ingress.enabled` | bool | `false` | Enable ingress |
| `app.env.EMBEDDING_PROVIDER` | string | `"openai"` | Embedding provider (openai or voyage) |
| `app.env.EMBEDDING_MODEL` | string | `"text-embedding-3-large"` | Embedding model |
| `app.secrets.openaiApiKey` | string | `""` | OpenAI API key |
| `app.secrets.voyageApiKey` | string | `""` | Voyage AI API key |
| `postgresql.enabled` | bool | `true` | Enable PostgreSQL subchart |
| `postgresql.auth.database` | string | `"rust_docs_vectors"` | PostgreSQL database name |
| `postgresql.auth.username` | string | `"rustdocs"` | PostgreSQL username |
| `postgresql.auth.password` | string | `"rustdocs123"` | PostgreSQL password |

## Usage

### Accessing the Server

After installation, the server provides these endpoints:

- `/sse` - Server-Sent Events endpoint for MCP protocol
- `/message` - HTTP POST endpoint for MCP messages
- `/health` - Health check endpoint

### Configuring Claude Desktop

Add this to your Claude Desktop configuration:

```json
{
  "mcpServers": {
    "rust-docs": {
      "url": "http://your-server-url:3000",
      "transport": "sse"
    }
  }
}
```

### Configuring Claude Code

```bash
claude mcp add rust-docs http://your-server-url:3000 --transport sse
```

### Populating the Database

After deployment, you need to populate the database with Rust crate documentation:

```bash
# Get a shell in the running pod
kubectl exec -it deployment/rust-docs-mcp-server -- /bin/bash

# Populate a single crate
cargo run --bin populate_db -- --crate-name tokio

# Populate all configured crates
cargo run --bin populate_all
```

## Monitoring

### Health Checks

The application provides a `/health` endpoint for health checks:

```bash
kubectl get pods -l app.kubernetes.io/name=rust-docs-mcp-server
```

### Logs

View application logs:

```bash
kubectl logs -f deployment/rust-docs-mcp-server
```

### Database Status

Check database connectivity:

```bash
kubectl exec -it deployment/rust-docs-mcp-server -- env | grep DATABASE
```

## Troubleshooting

### Common Issues

1. **Database Connection Issues**
   - Check PostgreSQL pod status: `kubectl get pods -l app.kubernetes.io/name=postgresql`
   - Verify database credentials in secrets
   - Check network policies

2. **API Key Issues**
   - Verify API keys are set in secrets: `kubectl get secret rust-docs-mcp-server-secrets -o yaml`
   - Check environment variables: `kubectl exec deployment/rust-docs-mcp-server -- env | grep API`

3. **Ingress Issues**
   - Check ingress controller logs
   - Verify DNS resolution
   - Check TLS certificate status

### Getting Support

- Check the [GitHub repository](https://github.com/5dlabs/rust-docs-mcp-server)
- Review application logs for error messages
- Verify all prerequisites are met

## Uninstallation

```bash
helm uninstall rust-docs-mcp
```

This will remove all resources created by the chart, including the PostgreSQL database and all data.

## Contributing

Contributions are welcome! Please see the main repository for contribution guidelines.

## License

This chart is licensed under the MIT License.