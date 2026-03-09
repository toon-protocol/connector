# Kubernetes Agent Deployment with TigerBeetle

Complete guide for deploying an ILP agent on Kubernetes with the full stack:

- TigerBeetle (3 replicas for HA)
- ILP Connector (with settlement)
- Agent Runtime (SPSP/STREAM handler)
- Your Business Logic

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│ Namespace: tigerbeetle                                  │
│                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐ │
│  │ TigerBeetle  │  │ TigerBeetle  │  │ TigerBeetle  │ │
│  │  Replica 0   │  │  Replica 1   │  │  Replica 2   │ │
│  └──────────────┘  └──────────────┘  └──────────────┘ │
│         ▲                  ▲                  ▲         │
└─────────┼──────────────────┼──────────────────┼─────────┘
          │                  │                  │
          └──────────────────┴──────────────────┘
                             │
┌────────────────────────────┼─────────────────────────────┐
│ Namespace: agent-runtime   │                             │
│                            ▼                             │
│  ┌─────────────────────────────────────────┐            │
│  │ Deployment: connector                   │            │
│  │ - Settlement: TigerBeetle cluster       │            │
│  │ - Local delivery: agent-runtime         │            │
│  │ - Routes: g.connector.agent.* → local   │            │
│  └─────────────────────────────────────────┘            │
└──────────────────────────┬───────────────────────────────┘
                           │
┌──────────────────────────┼───────────────────────────────┐
│ Namespace: m2m-agent-runtime                             │
│                          ▼                               │
│  ┌─────────────────────────────────────────┐            │
│  │ Deployment: agent-runtime               │            │
│  │ - SPSP endpoints                        │            │
│  │ - STREAM fulfillment                    │            │
│  │ - Calls business logic                  │            │
│  └─────────────────────────────────────────┘            │
└──────────────────────────┬───────────────────────────────┘
                           │
┌──────────────────────────┼───────────────────────────────┐
│ Namespace: my-agent      │                               │
│                          ▼                               │
│  ┌─────────────────────────────────────────┐            │
│  │ Deployment: business-logic              │            │
│  │ - Your payment handler                  │            │
│  │ - Database: PostgreSQL                  │            │
│  └─────────────────────────────────────────┘            │
└─────────────────────────────────────────────────────────┘
```

## Prerequisites

- Kubernetes cluster (1.25+)
- kubectl configured
- Container registry access
- TigerBeetle image access

## Step-by-Step Deployment

### 1. Deploy TigerBeetle Cluster (3 Replicas)

```bash
# Deploy TigerBeetle with 3 replicas for high availability
kubectl apply -k k8s/tigerbeetle

# Verify TigerBeetle pods are running
kubectl -n tigerbeetle get pods
# Expected output:
# NAME             READY   STATUS    RESTARTS   AGE
# tigerbeetle-0    1/1     Running   0          2m
# tigerbeetle-1    1/1     Running   0          2m
# tigerbeetle-2    1/1     Running   0          2m

# Check TigerBeetle service
kubectl -n tigerbeetle get svc
# NAME                    TYPE        CLUSTER-IP     PORT(S)
# tigerbeetle             ClusterIP   10.96.x.x      3000/TCP
# tigerbeetle-headless    ClusterIP   None           3000/TCP
```

The TigerBeetle StatefulSet creates 3 replicas with persistent volumes for data redundancy.

### 2. Build and Push Docker Images

```bash
# Navigate to m2m repo
cd /path/to/m2m

# Build connector image
docker build -t your-registry/agent-runtime:v1.0.0 .
docker push your-registry/agent-runtime:v1.0.0

# Build agent-runtime image
docker build -t your-registry/m2m-agent-runtime:v1.0.0 \
  -f packages/agent-runtime/Dockerfile .
docker push your-registry/m2m-agent-runtime:v1.0.0

# Build your business logic image
cd /path/to/my-agent
docker build -t your-registry/my-business-logic:v1.0.0 .
docker push your-registry/my-business-logic:v1.0.0
```

### 3. Deploy Connector with TigerBeetle Integration

The connector needs to connect to the TigerBeetle cluster for settlement.

#### Update Connector ConfigMap

```yaml
# connector-configmap-patch.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: connector-config
  namespace: agent-runtime
data:
  NODE_ID: 'agent-connector'
  LOG_LEVEL: 'info'
  HEALTH_CHECK_PORT: '8080'

  # TigerBeetle cluster connection
  TIGERBEETLE_CLUSTER_ID: '0'
  TIGERBEETLE_REPLICAS: 'tigerbeetle-0.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000,tigerbeetle-1.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000,tigerbeetle-2.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000'

  # Settlement configuration
  SETTLEMENT_ENABLED: 'true'
  SETTLEMENT_FEE_PERCENTAGE: '0.1'

  # Local delivery to agent runtime
  LOCAL_DELIVERY_ENABLED: 'true'
  LOCAL_DELIVERY_URL: 'http://agent-runtime.m2m-agent-runtime.svc.cluster.local:3100'
  LOCAL_DELIVERY_TIMEOUT: '30000'
```

Apply:

```bash
kubectl apply -f connector-configmap-patch.yaml
```

#### Deploy Connector

```bash
# Update the image in k8s/connector/base/kustomization.yaml
cd k8s/connector/base
# Edit kustomization.yaml to set your image

# Or patch it
kubectl apply -k k8s/connector/base

# Patch the image if needed
kubectl -n agent-runtime set image deployment/connector \
  connector=your-registry/agent-runtime:v1.0.0
```

### 4. Deploy Agent Runtime

#### Update Agent Runtime ConfigMap

```yaml
# agent-runtime-configmap-patch.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: agent-runtime-config
  namespace: m2m-agent-runtime
data:
  PORT: '3100'
  BASE_ADDRESS: 'g.connector.agent'
  BUSINESS_LOGIC_URL: 'http://business-logic.my-agent.svc.cluster.local:8080'
  BUSINESS_LOGIC_TIMEOUT: '5000'
  SPSP_ENABLED: 'true'
  SESSION_TTL_MS: '3600000'
  LOG_LEVEL: 'info'
  NODE_ID: 'agent-runtime'
```

Apply:

```bash
kubectl apply -f agent-runtime-configmap-patch.yaml
```

#### Deploy Agent Runtime

```bash
# Deploy from m2m repo
kubectl apply -k k8s/agent-runtime

# Update image
kubectl -n m2m-agent-runtime set image deployment/agent-runtime \
  agent-runtime=your-registry/m2m-agent-runtime:v1.0.0
```

### 5. Deploy Your Business Logic

Create manifests for your business logic:

```yaml
# my-agent-manifests.yaml
---
apiVersion: v1
kind: Namespace
metadata:
  name: my-agent
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: business-logic-config
  namespace: my-agent
data:
  PORT: '8080'
  LOG_LEVEL: 'info'
  # Add your config here
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: business-logic
  namespace: my-agent
spec:
  replicas: 2
  selector:
    matchLabels:
      app: business-logic
  template:
    metadata:
      labels:
        app: business-logic
    spec:
      containers:
        - name: business-logic
          image: your-registry/my-business-logic:v1.0.0
          ports:
            - containerPort: 8080
          envFrom:
            - configMapRef:
                name: business-logic-config
          resources:
            requests:
              memory: '128Mi'
              cpu: '100m'
            limits:
              memory: '512Mi'
              cpu: '500m'
          livenessProbe:
            httpGet:
              path: /health
              port: 8080
            initialDelaySeconds: 10
            periodSeconds: 15
          readinessProbe:
            httpGet:
              path: /health
              port: 8080
            initialDelaySeconds: 5
            periodSeconds: 10
---
apiVersion: v1
kind: Service
metadata:
  name: business-logic
  namespace: my-agent
spec:
  type: ClusterIP
  ports:
    - port: 8080
      targetPort: 8080
  selector:
    app: business-logic
```

Deploy:

```bash
kubectl apply -f my-agent-manifests.yaml
```

### 6. Create Connector Configuration

The connector needs routes to send packets to your agent.

#### Option A: ConfigMap Volume

Create `connector-app-config.yaml`:

```yaml
# Connector application configuration
nodeId: agent-connector
btpServerPort: 4000
healthCheckPort: 8080
logLevel: info
environment: production

# No external peers in this simple example
peers: []

# Route local packets to agent runtime
routes:
  - prefix: g.connector.agent
    nextHop: local
    priority: 100

# Local delivery configuration
localDelivery:
  enabled: true
  handlerUrl: http://agent-runtime.m2m-agent-runtime.svc.cluster.local:3100
  timeout: 30000

# Settlement with TigerBeetle
settlement:
  enableSettlement: true
  connectorFeePercentage: 0.1
  tigerBeetleClusterId: 0
  tigerBeetleReplicas:
    - tigerbeetle-0.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000
    - tigerbeetle-1.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000
    - tigerbeetle-2.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000

# Explorer UI
explorer:
  enabled: true
  port: 5173
  retentionDays: 7
  maxEvents: 1000000
```

Create ConfigMap:

```bash
kubectl -n agent-runtime create configmap connector-app-config \
  --from-file=config.yaml=connector-app-config.yaml
```

Update connector deployment to mount this config:

```bash
kubectl -n agent-runtime patch deployment connector --type=json -p='[
  {
    "op": "add",
    "path": "/spec/template/spec/volumes",
    "value": [{
      "name": "config",
      "configMap": {"name": "connector-app-config"}
    }]
  },
  {
    "op": "add",
    "path": "/spec/template/spec/containers/0/volumeMounts",
    "value": [{
      "name": "config",
      "mountPath": "/app/config.yaml",
      "subPath": "config.yaml",
      "readOnly": true
    }]
  }
]'
```

#### Option B: Environment Variables

Or configure via environment variables (no YAML config file):

```bash
kubectl -n agent-runtime set env deployment/connector \
  NODE_ID=agent-connector \
  SETTLEMENT_ENABLED=true \
  TIGERBEETLE_CLUSTER_ID=0 \
  TIGERBEETLE_REPLICAS="tigerbeetle-0.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000,tigerbeetle-1.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000,tigerbeetle-2.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000" \
  LOCAL_DELIVERY_ENABLED=true \
  LOCAL_DELIVERY_URL=http://agent-runtime.m2m-agent-runtime.svc.cluster.local:3100
```

### 7. Verify Complete Stack

```bash
# Check all components
kubectl get pods -n tigerbeetle
kubectl get pods -n agent-runtime
kubectl get pods -n m2m-agent-runtime
kubectl get pods -n my-agent

# Check services
kubectl get svc -n tigerbeetle
kubectl get svc -n agent-runtime
kubectl get svc -n m2m-agent-runtime
kubectl get svc -n my-agent

# Test the stack
# 1. Port-forward SPSP endpoint
kubectl -n m2m-agent-runtime port-forward svc/agent-runtime 3100:3100

# 2. Query SPSP endpoint
curl http://localhost:3100/.well-known/pay

# 3. Check connector Explorer UI
kubectl -n agent-runtime port-forward svc/connector 5173:5173
open http://localhost:5173
```

### 8. Monitor the Stack

```bash
# Watch connector logs
kubectl -n agent-runtime logs -f deployment/connector

# Watch agent-runtime logs
kubectl -n m2m-agent-runtime logs -f deployment/agent-runtime

# Watch business logic logs
kubectl -n my-agent logs -f deployment/business-logic

# Watch TigerBeetle logs
kubectl -n tigerbeetle logs -f tigerbeetle-0
```

---

## Complete Kustomize Deployment

For a production-ready setup, use kustomize overlays:

### Directory Structure

```
my-ilp-agent/
├── k8s/
│   ├── base/
│   │   ├── namespace.yaml
│   │   ├── deployment.yaml
│   │   ├── service.yaml
│   │   ├── configmap.yaml
│   │   └── kustomization.yaml
│   └── overlays/
│       ├── dev/
│       │   └── kustomization.yaml
│       └── prod/
│           ├── kustomization.yaml
│           └── hpa.yaml
```

### base/kustomization.yaml

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization

namespace: my-agent

resources:
  - namespace.yaml
  - configmap.yaml
  - deployment.yaml
  - service.yaml

images:
  - name: business-logic
    newName: your-registry/my-business-logic
    newTag: latest
```

### overlays/prod/kustomization.yaml

```yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization

namespace: my-agent

resources:
  - ../../base
  - hpa.yaml

patches:
  # Increase replicas for production
  - patch: |-
      apiVersion: apps/v1
      kind: Deployment
      metadata:
        name: business-logic
      spec:
        replicas: 3
    target:
      kind: Deployment
      name: business-logic

  # Increase resource limits
  - patch: |-
      apiVersion: apps/v1
      kind: Deployment
      metadata:
        name: business-logic
      spec:
        template:
          spec:
            containers:
              - name: business-logic
                resources:
                  requests:
                    memory: "256Mi"
                    cpu: "200m"
                  limits:
                    memory: "1Gi"
                    cpu: "1000m"
    target:
      kind: Deployment
      name: business-logic

images:
  - name: business-logic
    newName: your-registry/my-business-logic
    newTag: v1.0.0 # Specific version for production
```

### Deploy with Overlays

```bash
# Development
kubectl apply -k k8s/overlays/dev

# Production
kubectl apply -k k8s/overlays/prod
```

---

## TigerBeetle Service Discovery

The connector connects to TigerBeetle using the headless service for direct pod access:

### Service Endpoints

```bash
# Headless service (used by connector)
tigerbeetle-headless.tigerbeetle.svc.cluster.local

# Individual replica endpoints
tigerbeetle-0.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000
tigerbeetle-1.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000
tigerbeetle-2.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000
```

### Connector Configuration

The connector's `TIGERBEETLE_REPLICAS` environment variable should list all replicas:

```yaml
env:
  - name: TIGERBEETLE_REPLICAS
    value: >-
      tigerbeetle-0.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000,
      tigerbeetle-1.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000,
      tigerbeetle-2.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000
```

The TigerBeetle client library automatically handles:

- Connection to all replicas
- Quorum-based writes (2 out of 3 must acknowledge)
- Automatic failover if a replica is down
- Load balancing reads across healthy replicas

---

## Complete Example: All-in-One Deployment

Here's a complete example that deploys everything:

```bash
#!/bin/bash
# deploy-agent-to-k8s.sh

set -e

# 1. Deploy TigerBeetle
echo "Deploying TigerBeetle cluster (3 replicas)..."
kubectl apply -k k8s/tigerbeetle
kubectl -n tigerbeetle wait --for=condition=ready pod -l app=tigerbeetle --timeout=120s

# 2. Deploy Connector
echo "Deploying ILP Connector..."
kubectl apply -k k8s/connector/overlays/production

# Configure connector for TigerBeetle and local delivery
kubectl -n agent-runtime create configmap connector-app-config \
  --from-file=config.yaml=connector-config.yaml \
  --dry-run=client -o yaml | kubectl apply -f -

# Wait for connector
kubectl -n agent-runtime wait --for=condition=available deployment/connector --timeout=120s

# 3. Deploy Agent Runtime
echo "Deploying Agent Runtime..."
kubectl apply -k k8s/agent-runtime

# Configure agent runtime for your business logic
kubectl -n m2m-agent-runtime patch configmap agent-runtime-config \
  --type merge \
  -p '{"data":{"BUSINESS_LOGIC_URL":"http://business-logic.my-agent.svc.cluster.local:8080"}}'

kubectl -n m2m-agent-runtime set image deployment/agent-runtime \
  agent-runtime=your-registry/m2m-agent-runtime:v1.0.0

kubectl -n m2m-agent-runtime wait --for=condition=available deployment/agent-runtime --timeout=120s

# 4. Deploy Your Business Logic
echo "Deploying business logic..."
kubectl apply -f my-agent-manifests.yaml
kubectl -n my-agent wait --for=condition=available deployment/business-logic --timeout=120s

# 5. Verify deployment
echo "Verifying deployment..."
kubectl get pods -n tigerbeetle
kubectl get pods -n agent-runtime
kubectl get pods -n m2m-agent-runtime
kubectl get pods -n my-agent

echo "✅ Deployment complete!"
echo ""
echo "Test SPSP endpoint:"
echo "  kubectl -n m2m-agent-runtime port-forward svc/agent-runtime 3100:3100"
echo "  curl http://localhost:3100/.well-known/pay"
echo ""
echo "Access Explorer UI:"
echo "  kubectl -n agent-runtime port-forward svc/connector 5173:5173"
echo "  open http://localhost:5173"
```

Make it executable and run:

```bash
chmod +x deploy-agent-to-k8s.sh
./deploy-agent-to-k8s.sh
```

---

## TigerBeetle StatefulSet Details

The TigerBeetle deployment uses a StatefulSet for stable network identities:

```yaml
# From k8s/tigerbeetle/base/statefulset.yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: tigerbeetle
  namespace: tigerbeetle
spec:
  replicas: 3
  serviceName: tigerbeetle-headless
  selector:
    matchLabels:
      app: tigerbeetle
  template:
    spec:
      containers:
        - name: tigerbeetle
          image: ghcr.io/tigerbeetle/tigerbeetle:latest
          # Each replica gets stable hostname:
          # tigerbeetle-0, tigerbeetle-1, tigerbeetle-2
  volumeClaimTemplates:
    - metadata:
        name: data
      spec:
        accessModes: ['ReadWriteOnce']
        resources:
          requests:
            storage: 10Gi
```

Each replica gets:

- Stable hostname: `tigerbeetle-{0,1,2}`
- Persistent volume: 10Gi for data
- Fixed ordinal for cluster membership

---

## Troubleshooting

### Connector Can't Connect to TigerBeetle

Check DNS resolution:

```bash
# Exec into connector pod
kubectl -n agent-runtime exec -it deployment/connector -- sh

# Test DNS resolution
nslookup tigerbeetle-0.tigerbeetle-headless.tigerbeetle.svc.cluster.local

# Test connectivity
nc -zv tigerbeetle-0.tigerbeetle-headless.tigerbeetle.svc.cluster.local 3000
```

### Agent Runtime Can't Reach Business Logic

Check service discovery:

```bash
# Exec into agent-runtime pod
kubectl -n m2m-agent-runtime exec -it deployment/agent-runtime -- sh

# Test DNS
nslookup business-logic.my-agent.svc.cluster.local

# Test HTTP
wget -O- http://business-logic.my-agent.svc.cluster.local:8080/health
```

### Connector Not Forwarding to Agent Runtime

Check logs:

```bash
# Look for local delivery errors
kubectl -n agent-runtime logs deployment/connector | grep -i "local delivery"

# Check environment variables
kubectl -n agent-runtime get deployment connector -o yaml | grep -A5 LOCAL_DELIVERY
```

---

## Production Deployment Checklist

- [ ] TigerBeetle has 3+ replicas with persistent volumes
- [ ] Connector configured with all TigerBeetle replica addresses
- [ ] Agent Runtime deployed with proper resource limits
- [ ] Business Logic has health checks configured
- [ ] Network policies restrict traffic between components
- [ ] Secrets managed via External Secrets Operator or sealed secrets
- [ ] HPA configured for business logic based on traffic
- [ ] Monitoring and alerting set up (Prometheus, Grafana)
- [ ] Resource requests/limits tuned based on load testing
- [ ] Pod Disruption Budgets configured for HA
- [ ] Ingress/LoadBalancer for external access (if needed)

---

## Scaling

### Scale Business Logic

```bash
# Manual scaling
kubectl -n my-agent scale deployment business-logic --replicas=5

# Auto-scaling with HPA
kubectl apply -f - <<EOF
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: business-logic-hpa
  namespace: my-agent
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: business-logic
  minReplicas: 2
  maxReplicas: 20
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 70
    - type: Resource
      resource:
        name: memory
        target:
          type: Utilization
          averageUtilization: 80
EOF
```

### Scale Agent Runtime

```bash
# If you have high SPSP query load
kubectl -n m2m-agent-runtime scale deployment agent-runtime --replicas=3
```

### Scale TigerBeetle

TigerBeetle cluster size is fixed after initialization. To change:

1. Create new cluster with desired size
2. Migrate data
3. Update connector configuration

---

## Network Diagram

```
┌─────────────────────────────────────────────────────┐
│ Internet / External Clients                         │
└────────────────────┬────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────┐
│ Ingress / LoadBalancer                              │
│ - SPSP: /.well-known/pay → agent-runtime:3100      │
│ - BTP:  wss://connector:4000                        │
│ - Explorer: / → connector:5173                      │
└────────────────────┬────────────────────────────────┘
                     │
         ┌───────────┴───────────┐
         ▼                       ▼
┌────────────────┐      ┌────────────────┐
│   Connector    │      │ Agent Runtime  │
│ (agent-runtime)│◄────►│(m2m-agent-rt)  │
└────┬───────────┘      └────┬───────────┘
     │                       │
     ▼                       ▼
┌────────────┐          ┌────────────┐
│TigerBeetle │          │  Business  │
│ Cluster    │          │   Logic    │
│(tigerbeetle)          │ (my-agent) │
│ 3 replicas │          └────┬───────┘
└────────────┘               │
                             ▼
                        ┌────────────┐
                        │ PostgreSQL │
                        │ (my-agent) │
                        └────────────┘
```

---

## Summary

**To deploy your agent on Kubernetes with TigerBeetle:**

1. ✅ Deploy TigerBeetle (3 replicas) - `kubectl apply -k k8s/tigerbeetle`
2. ✅ Deploy Connector with TigerBeetle config - Point to all 3 replicas
3. ✅ Deploy Agent Runtime - `kubectl apply -k k8s/agent-runtime`
4. ✅ Deploy Your Business Logic - Your custom manifests
5. ✅ Configure connections - Environment variables or ConfigMaps
6. ✅ Verify with health checks and logs

**Key TigerBeetle Configuration:**

```yaml
TIGERBEETLE_REPLICAS: >-
  tigerbeetle-0.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000,
  tigerbeetle-1.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000,
  tigerbeetle-2.tigerbeetle-headless.tigerbeetle.svc.cluster.local:3000
```

This ensures the connector can connect to all TigerBeetle replicas for high-availability settlement recording.
