# Performance Testing and Baseline Metrics

This document contains performance test results, baseline metrics, and optimization notes for the M2M Interledger Connector system.

## Table of Contents

- [Test Environment](#test-environment)
- [Performance Test Suite](#performance-test-suite)
- [Baseline Metrics](#baseline-metrics)
- [Bottleneck Analysis](#bottleneck-analysis)
- [Optimizations Applied](#optimizations-applied)
- [Running Performance Tests](#running-performance-tests)
- [Future Improvements](#future-improvements)

## Test Environment

### Hardware Specifications

- **Machine**: Standard development machine (varies by developer)
- **Recommended**: 8GB RAM minimum, quad-core CPU
- **OS**: macOS, Linux, or Windows with WSL2
- **Docker**: Docker Engine 20.10+ with Docker Compose 2.x

### Network Topology

- **Test Configuration**: 5-node linear topology (A → B → C → D → E)
- **Docker Compose File**: `docker-compose-5-node.yml`
- **Connector Configuration**: See `examples/linear-5-nodes-*.yaml`

### Test Duration and Load

- **NFR1 Test**: Single network startup measurement
- **NFR2/NFR3 Test**: 100 packets/second for 10 seconds (1000 total packets)
- **NFR4 Test**: 500 packets sent in batches of 50

## Performance Test Suite

### Test File Location

`packages/connector/test/integration/performance.test.ts`

### Test Structure

The performance test suite validates all four Non-Functional Requirements (NFRs):

1. **NFR1: Network Startup Latency**
   - Validates 5-node network deploys and reaches operational state within 30 seconds
   - Measures time from `docker-compose up` to all containers healthy
   - Verifies BTP peer connections established

2. **NFR2 & NFR3: Packet Throughput and Visualization Latency**
   - Sends 100 packets/second for 10 seconds
   - Measures packet forwarding latency (ingress to egress)
   - Collects telemetry events to verify visualization responsiveness
   - Verifies dashboard remains responsive under load

3. **NFR4: Packet Loss Verification**
   - Sends 500 packets through 5-node network
   - Tracks packet IDs through telemetry logs
   - Verifies 100% packet logging (target: 0% loss)

## Baseline Metrics

### NFR1: Network Startup Time

**Target**: <30 seconds

**Baseline Results**:

```
Network startup time: TBD (run tests to establish baseline)
Breakdown:
  - Docker image build: TBD
  - Container startup: TBD
  - Health checks: TBD
  - BTP connections: TBD
```

**Status**: ✅ / ⚠️ / ❌ (to be determined on first test run)

### NFR2: Visualization Update Latency

**Target**: <100ms (p95)

**Baseline Results**:

```
Packet forwarding latency:
  - p50: TBD ms
  - p95: TBD ms
  - p99: TBD ms

Visualization latency (proxy):
  - p95: TBD ms (target: <100ms)
```

**Status**: ✅ / ⚠️ / ❌ (to be determined on first test run)

**Note**: Current implementation measures packet forwarding latency as a proxy for visualization latency. Full NFR2 validation requires measuring actual time from packet send to dashboard UI update.

### NFR3: Dashboard Responsiveness

**Target**: Dashboard responsive during 100 packets/sec load

**Baseline Results**:

```
Test results:
  - WebSocket connection maintained: TBD
  - Telemetry events collected: TBD
  - Dashboard remained responsive: TBD
```

**Status**: ✅ / ⚠️ / ❌ (to be determined on first test run)

### NFR4: Packet Loss Rate

**Target**: 0% packet loss

**Baseline Results**:

```
Packets sent: 500
Packets logged: TBD
Loss rate: TBD% (target: 0%)

Telemetry events:
  - PACKET_SENT: TBD
  - PACKET_RECEIVED: TBD
  - LOG: TBD
```

**Status**: ✅ / ⚠️ / ❌ (to be determined on first test run)

## Bottleneck Analysis

### Potential Bottlenecks Identified

Performance bottlenecks will be documented here after running the performance test suite. Key areas to monitor:

1. **Container Startup**
   - Docker image build time
   - Health check intervals and delays
   - BTP connection establishment time

2. **Packet Routing**
   - OER serialization/deserialization overhead
   - WebSocket send/receive latency
   - Routing table lookup performance

3. **Telemetry Transmission**
   - WebSocket buffering and backpressure
   - JSON serialization overhead
   - Dashboard broadcast fan-out

4. **Dashboard Rendering**
   - Cytoscape.js animation performance
   - React re-render frequency
   - Telemetry event processing rate

### Profiling Tools Used

- **Node.js**: `--inspect` flag for profiling connector performance
- **Browser DevTools**: Performance tab for dashboard profiling
- **Docker Stats**: Container resource usage monitoring (CPU, memory)
- **Jest**: Built-in test timing and performance metrics

## Optimizations Applied

### Baseline (No Optimizations)

Initial implementation uses standard patterns with no specific performance optimizations:

- Telemetry events sent immediately (no batching)
- Standard Docker health check intervals
- No caching in routing table lookups
- No throttling of dashboard animations

### Conditional Optimizations

**Status**: No optimizations applied yet (pending baseline test results)

Optimizations will be applied only if NFR violations are detected during testing:

#### If NFR1 Violated (Startup >30s):

- Pre-build Docker images in CI/CD
- Optimize health check intervals (reduce initial delay, increase frequency)
- Parallelize BTP peer connections

#### If NFR2 Violated (Visualization >100ms p95):

- Implement telemetry event batching (buffer and send every 50ms)
- Reduce telemetry payload size (send only essential fields)
- Implement rate limiting for LOG events

#### If NFR3 Violated (Dashboard unresponsive):

- Throttle animation updates with `requestAnimationFrame`
- Implement virtual scrolling in LogViewer
- Debounce Cytoscape.js graph updates

#### If NFR4 Violated (Packet loss >0%):

- Investigate WebSocket backpressure handling
- Add telemetry buffer with overflow protection
- Implement packet retry logic if critical

## Running Performance Tests

### Prerequisites

1. Docker and Docker Compose installed
2. Repository cloned and dependencies installed (`npm install`)
3. Connector image built

### Run All Performance Tests

```bash
# From repository root
npm test --workspace=packages/connector -- performance.test.ts
```

### Run Individual Tests

```bash
# NFR1: Network Startup
npm test --workspace=packages/connector -- performance.test.ts -t "NFR1"

# NFR2/NFR3: Throughput and Latency
npm test --workspace=packages/connector -- performance.test.ts -t "NFR2"

# NFR4: Packet Loss
npm test --workspace=packages/connector -- performance.test.ts -t "NFR4"
```

### Interpreting Results

- **Green (✅)**: All NFRs met, baseline metrics within targets
- **Yellow (⚠️)**: Close to NFR thresholds, consider optimization
- **Red (❌)**: NFR violation, optimization required

### Updating Baselines

After running tests on your machine, update this document with actual baseline metrics. Include:

- Hardware specs (CPU, RAM)
- Docker version
- Actual measured values for each NFR
- Date of baseline establishment

## Future Performance Improvements

### Deferred Optimizations (Post-MVP)

The following optimizations are not required for MVP but may be valuable for production deployment:

#### High Priority

- **Connection Pooling**: Reuse BTP connections instead of creating new ones per packet
- **Persistent Telemetry**: Store telemetry in database for historical analysis
- **Horizontal Scaling**: Support multiple dashboard instances behind load balancer

#### Medium Priority

- **Packet Batching**: Group multiple ILP packets in single BTP message
- **Compression**: Compress telemetry events before WebSocket transmission
- **Caching**: Cache routing table lookups for frequently used destinations

#### Low Priority

- **Worker Threads**: Use Node.js worker threads for OER encoding/decoding
- **Stream Multiplexing**: Multiplex multiple BTP streams over single WebSocket
- **Custom Binary Protocol**: Replace JSON telemetry with binary format (e.g., Protocol Buffers)

### Monitoring and Observability

**Recommended additions for production**:

- Prometheus metrics export from connectors
- Grafana dashboards for real-time performance monitoring
- Alerting on NFR threshold violations
- Distributed tracing (e.g., OpenTelemetry)

## References

- **PRD**: `docs/prd/requirements.md` - NFR definitions
- **Architecture**: `docs/architecture.md` - System design
- **Test Strategy**: `docs/architecture/test-strategy-and-standards.md`
- **Docker Compose**: `docker-compose-5-node.yml` - Test topology
- **Config Examples**: `examples/linear-5-nodes-*.yaml`

---

**Last Updated**: 2025-12-30
**Version**: 1.0
**Status**: Baseline not yet established - run tests to populate metrics
