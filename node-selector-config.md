# Node Selection Configuration

This guide explains how to configure the Helm chart for different node scenarios.

## Current Cluster Status
- **Control Plane**: `talos-evr-4zu` (always available)
- **Worker Node**: `talos-a43-ee1` (currently offline/removed)

## Configuration Options

### Option 1: Single-Node (Control Plane Only) - CURRENT DEFAULT
The chart is pre-configured to run on control plane nodes when workers are unavailable.

**Deploy with current settings:**
```bash
helm upgrade --install rustdocs charts/rust-docs-mcp-server/ --namespace mcp --create-namespace
```

### Option 2: Force Worker Node (when talos-a43-ee1 is online)
Edit `charts/rust-docs-mcp-server/values.yaml` and uncomment:

```yaml
nodeSelector:
  kubernetes.io/hostname: talos-a43-ee1

# And for PostgreSQL:
postgresql:
  primary:
    nodeSelector:
      kubernetes.io/hostname: talos-a43-ee1
```

**Deploy to worker node:**
```bash
helm upgrade --install rustdocs charts/rust-docs-mcp-server/ \
  --namespace mcp --create-namespace \
  --set nodeSelector."kubernetes\.io/hostname"="talos-a43-ee1" \
  --set postgresql.primary.nodeSelector."kubernetes\.io/hostname"="talos-a43-ee1"
```

### Option 3: Prefer Worker, Fallback to Control Plane
Use affinity instead of nodeSelector for flexible scheduling:

```bash
helm upgrade --install rustdocs charts/rust-docs-mcp-server/ \
  --namespace mcp --create-namespace \
  --set affinity.nodeAffinity.preferredDuringSchedulingIgnoredDuringExecution[0].weight=100 \
  --set affinity.nodeAffinity.preferredDuringSchedulingIgnoredDuringExecution[0].preference.matchExpressions[0].key="kubernetes.io/hostname" \
  --set affinity.nodeAffinity.preferredDuringSchedulingIgnoredDuringExecution[0].preference.matchExpressions[0].operator="In" \
  --set affinity.nodeAffinity.preferredDuringSchedulingIgnoredDuringExecution[0].preference.matchExpressions[0].values[0]="talos-a43-ee1"
```

## Quick Commands

**Check current node status:**
```bash
kubectl get nodes --show-labels
kubectl get pods -o wide -A | grep -E "(rustdocs|postgres)"
```

**Force reschedule pods to different nodes:**
```bash
kubectl rollout restart deployment/rustdocs-rust-docs-mcp-server -n mcp
kubectl rollout restart statefulset/rustdocs-postgresql -n mcp
```

**Check why pods aren't scheduling:**
```bash
kubectl describe pod <pod-name> -n mcp
kubectl get events --sort-by=.metadata.creationTimestamp -n mcp
```