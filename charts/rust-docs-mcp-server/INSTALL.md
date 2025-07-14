# Installation Guide

This guide provides step-by-step instructions for installing the Rust Docs MCP Server Helm chart.

## Prerequisites

### Required

- Kubernetes cluster 1.19+
- Helm 3.2.0+
- kubectl configured to access your cluster

### Optional

- Ingress controller (nginx, traefik, etc.) for external access
- Cert-manager for automatic TLS certificate management
- Prometheus for monitoring

## Step 1: Add Required Helm Repositories

```bash
# Add Bitnami repository for PostgreSQL dependency
helm repo add bitnami https://charts.bitnami.com/bitnami

# Update repositories
helm repo update
```

## Step 2: Create Namespace

```bash
# Create a dedicated namespace
kubectl create namespace rust-docs-mcp

# Set as default namespace (optional)
kubectl config set-context --current --namespace=rust-docs-mcp
```

## Step 3: Create Secrets

### Option A: Using kubectl

```bash
# Create secret with OpenAI API key
kubectl create secret generic rust-docs-mcp-secrets \
  --from-literal=openai-api-key="sk-your-openai-api-key" \
  --namespace rust-docs-mcp

# Or with Voyage AI key
kubectl create secret generic rust-docs-mcp-secrets \
  --from-literal=voyage-api-key="your-voyage-api-key" \
  --namespace rust-docs-mcp
```

### Option B: Using YAML

```yaml
# secrets.yaml
apiVersion: v1
kind: Secret
metadata:
  name: rust-docs-mcp-secrets
  namespace: rust-docs-mcp
type: Opaque
data:
  openai-api-key: <base64-encoded-api-key>
  voyage-api-key: <base64-encoded-api-key>
```

```bash
kubectl apply -f secrets.yaml
```

## Step 4: Configure Values

Create a `values.yaml` file with your configuration:

```yaml
# values.yaml
app:
  env:
    EMBEDDING_PROVIDER: "openai"
    EMBEDDING_MODEL: "text-embedding-3-large"

  # Use the existing secret
  existingSecret: "rust-docs-mcp-secrets"

# Configure PostgreSQL
postgresql:
  auth:
    postgresPassword: "your-secure-password"
    username: "rustdocs"
    password: "your-secure-password"
    database: "rust_docs_vectors"

# Enable ingress if needed
ingress:
  enabled: true
  className: "nginx"
  hosts:
    - host: rust-docs-mcp.yourdomain.com
      paths:
        - path: /
          pathType: Prefix
  tls:
    - secretName: rust-docs-mcp-tls
      hosts:
        - rust-docs-mcp.yourdomain.com
```

## Step 5: Install the Chart

### Basic Installation

```bash
# Install with default values
helm install rust-docs-mcp ./charts/rust-docs-mcp-server \
  --namespace rust-docs-mcp

# Install with custom values
helm install rust-docs-mcp ./charts/rust-docs-mcp-server \
  --namespace rust-docs-mcp \
  --values values.yaml
```

### Production Installation

```bash
# Use production values
helm install rust-docs-mcp ./charts/rust-docs-mcp-server \
  --namespace rust-docs-mcp \
  --values values-production.yaml \
  --set app.existingSecret=rust-docs-mcp-secrets
```

## Step 6: Verify Installation

```bash
# Check pod status
kubectl get pods -n rust-docs-mcp

# Check services
kubectl get svc -n rust-docs-mcp

# Check ingress (if enabled)
kubectl get ingress -n rust-docs-mcp

# View logs
kubectl logs -f deployment/rust-docs-mcp-server -n rust-docs-mcp
```

## Step 7: Populate Database

After successful installation, you need to populate the database with Rust documentation:

```bash
# Get a shell in the running pod
kubectl exec -it deployment/rust-docs-mcp-server -n rust-docs-mcp -- /bin/bash

# Inside the pod, populate documentation
cargo run --bin populate_all

# Or populate specific crates
cargo run --bin populate_db -- --crate-name tokio
cargo run --bin populate_db -- --crate-name serde
cargo run --bin populate_db -- --crate-name axum
```

## Step 8: Test the Installation

### Health Check

```bash
# Port forward to test locally
kubectl port-forward svc/rust-docs-mcp-server 8080:3000 -n rust-docs-mcp

# Test health endpoint
curl http://localhost:8080/health
```

### MCP Client Configuration

Configure your MCP client to use the server:

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

## Troubleshooting

### Common Issues

1. **Pods not starting**
   ```bash
   kubectl describe pod <pod-name> -n rust-docs-mcp
   kubectl logs <pod-name> -n rust-docs-mcp
   ```

2. **Database connection issues**
   ```bash
   # Check PostgreSQL status
   kubectl get pods -l app.kubernetes.io/name=postgresql -n rust-docs-mcp

   # Check database logs
   kubectl logs -l app.kubernetes.io/name=postgresql -n rust-docs-mcp
   ```

3. **API key issues**
   ```bash
   # Verify secret exists
   kubectl get secret rust-docs-mcp-secrets -n rust-docs-mcp

   # Check secret contents (base64 encoded)
   kubectl get secret rust-docs-mcp-secrets -o yaml -n rust-docs-mcp
   ```

4. **Ingress not working**
   ```bash
   # Check ingress status
   kubectl describe ingress rust-docs-mcp-server -n rust-docs-mcp

   # Check ingress controller logs
   kubectl logs -n ingress-nginx -l app.kubernetes.io/name=ingress-nginx
   ```

### Debugging Commands

```bash
# Get all resources
kubectl get all -n rust-docs-mcp

# Check events
kubectl get events -n rust-docs-mcp --sort-by='.lastTimestamp'

# Check resource usage
kubectl top pods -n rust-docs-mcp

# Port forward for debugging
kubectl port-forward svc/rust-docs-mcp-server 8080:3000 -n rust-docs-mcp
```

## Upgrading

```bash
# Upgrade with new values
helm upgrade rust-docs-mcp ./charts/rust-docs-mcp-server \
  --namespace rust-docs-mcp \
  --values values.yaml

# Check upgrade status
helm status rust-docs-mcp -n rust-docs-mcp
```

## Uninstalling

```bash
# Uninstall the release
helm uninstall rust-docs-mcp -n rust-docs-mcp

# Clean up namespace
kubectl delete namespace rust-docs-mcp
```

## Next Steps

1. Set up monitoring with Prometheus and Grafana
2. Configure log aggregation
3. Set up automated backups for PostgreSQL
4. Configure resource quotas and limits
5. Set up CI/CD for automated deployments

## Support

For issues and questions:
- Check the [GitHub repository](https://github.com/5dlabs/rust-docs-mcp-server)
- Review the troubleshooting section above
- Check Kubernetes and Helm documentation