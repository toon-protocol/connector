# Performance Tuning Guide

**Epic 12 Story 12.5 - Performance Optimization for 10K+ TPS**

This guide provides recommendations for tuning the M2M ILP Connector to achieve sustained throughput of 10,000+ transactions per second with low latency (<10ms p99).

## Table of Contents

1. [Introduction](#introduction)
2. [Hardware Requirements](#hardware-requirements)
3. [Configuration Overview](#configuration-overview)
4. [Worker Thread Configuration](#worker-thread-configuration)
5. [Batch Size Tuning](#batch-size-tuning)
6. [Connection Pool Sizing](#connection-pool-sizing)
7. [Memory Optimization](#memory-optimization)
8. [CPU Optimization](#cpu-optimization)
9. [Monitoring and Metrics](#monitoring-and-metrics)
10. [Troubleshooting](#troubleshooting)

---

## Introduction

The M2M ILP Connector implements several performance optimizations to achieve high throughput:

- **Packet Processing Parallelization**: Distributes packet processing across multiple worker threads
- **TigerBeetle Transfer Batching**: Batches balance updates to reduce database round-trips
- **Telemetry Event Buffering**: Batches telemetry events to minimize logging overhead
- **Connection Pooling**: Maintains pools of connections to external services (EVM RPC)

These optimizations can be configured via the `performance` section of the connector configuration file.

## Hardware Requirements

### Minimum Requirements for 10K TPS

| Resource              | Minimum     | Recommended       |
| --------------------- | ----------- | ----------------- |
| CPU Cores             | 8 cores     | 16+ cores         |
| RAM                   | 8 GB        | 16+ GB            |
| Network               | 1 Gbps      | 10 Gbps           |
| Storage (TigerBeetle) | SSD, 100 GB | NVMe SSD, 500+ GB |

**Notes:**

- More CPU cores enable greater parallelism for packet processing
- Sufficient RAM prevents memory pressure under high load
- Fast network connectivity is critical for blockchain RPC communication
- TigerBeetle benefits significantly from fast storage (NVMe SSDs recommended)

---

## Configuration Overview

Add the following `performance` section to your connector configuration file:

```yaml
# connector-config.yaml
nodeId: high-throughput-connector
btpServerPort: 3000
environment: production

# Performance optimization configuration
performance:
  packetProcessing:
    workerThreads: 8 # Match CPU core count
    batchSize: 100 # Packets per worker batch

  tigerbeetle:
    batchSize: 100 # Transfers per batch
    flushIntervalMs: 10 # Flush every 10ms
    maxPendingTransfers: 1000 # Queue size

  telemetry:
    bufferSize: 1000 # Events per batch
    flushIntervalMs: 100 # Flush every 100ms

  connectionPools:
    evm:
      poolSize: 10 # RPC connections
      rpcUrls:
        - https://mainnet.base.org
        - https://base.llamarpc.com
        - https://base-rpc.publicnode.com

# ... rest of configuration (peers, routes, etc.)
```

---

## Worker Thread Configuration

### Overview

The `packetProcessing.workerThreads` setting controls how many worker threads are used for parallel packet processing. Each worker thread processes packet batches independently.

### Recommendations

**General Rule**: Set `workerThreads` to match the number of CPU cores.

```yaml
performance:
  packetProcessing:
    workerThreads: 8 # For 8-core system
```

**Example Configurations:**

| System             | CPU Cores | Recommended `workerThreads` |
| ------------------ | --------- | --------------------------- |
| Development Laptop | 4 cores   | 4                           |
| Small VM           | 8 cores   | 8                           |
| Production Server  | 16 cores  | 16                          |
| High-End Server    | 32+ cores | 32                          |

**Advanced Tuning:**

- **Under-subscribe** (workerThreads < CPU cores): If the connector shares the server with other services
  - Example: 12 cores total, reserve 4 for OS/other services → set `workerThreads: 8`

- **Over-subscribe** (workerThreads > CPU cores): Generally not recommended
  - Can cause context switching overhead and reduced throughput

### Batch Size

The `packetProcessing.batchSize` controls how many packets are processed in each worker batch.

**Default**: 100 packets per batch

**Tuning Guidelines:**

- **Higher batch size** (200-500): Better throughput, higher latency
- **Lower batch size** (50-100): Lower latency, slightly reduced throughput
- **Starting point**: Keep default (100) unless specific latency requirements

```yaml
performance:
  packetProcessing:
    batchSize: 100 # Default balances latency and throughput
```

---

## Batch Size Tuning

### TigerBeetle Transfer Batching

Controls how transfers are batched before sending to TigerBeetle for balance updates.

**Configuration:**

```yaml
performance:
  tigerbeetle:
    batchSize: 100 # Transfers per batch
    flushIntervalMs: 10 # Periodic flush interval
    maxPendingTransfers: 1000 # Maximum queued transfers
```

**Tuning Guidelines:**

| Scenario               | `batchSize` | `flushIntervalMs` | Notes                              |
| ---------------------- | ----------- | ----------------- | ---------------------------------- |
| **Low Latency**        | 50          | 5ms               | Faster settlement, more DB calls   |
| **Balanced** (Default) | 100         | 10ms              | Good latency/throughput balance    |
| **High Throughput**    | 200         | 20ms              | Maximum throughput, higher latency |

**Trade-offs:**

- **Larger batches**: Fewer TigerBeetle calls, higher throughput, but increased latency
- **Smaller batches**: Lower latency, more TigerBeetle calls, reduced throughput
- **Flush interval**: Safety valve - ensures transfers flush even if batch not full

**Memory Considerations:**

The `maxPendingTransfers` setting prevents unbounded memory growth under extreme load:

```yaml
performance:
  tigerbeetle:
    maxPendingTransfers: 1000 # Adjust based on available RAM
```

- Higher values allow more buffering during traffic spikes
- Lower values prevent memory exhaustion
- Monitor queue depth metrics to tune appropriately

### Telemetry Event Buffering

Controls how telemetry events (logs, metrics) are batched before writing.

**Configuration:**

```yaml
performance:
  telemetry:
    bufferSize: 1000 # Events per batch
    flushIntervalMs: 100 # Flush every 100ms
```

**Tuning Guidelines:**

| Scenario                   | `bufferSize` | `flushIntervalMs` |
| -------------------------- | ------------ | ----------------- |
| **Low Overhead** (Default) | 1000         | 100ms             |
| **Real-time Logging**      | 500          | 50ms              |
| **Maximum Throughput**     | 2000         | 200ms             |

**Trade-offs:**

- Larger buffers reduce logging overhead but delay event visibility
- Smaller buffers provide near-real-time telemetry at cost of overhead
- Recommended: Start with defaults (1000 events, 100ms flush)

---

## Connection Pool Sizing

### Overview

Connection pools maintain multiple connections to external services to prevent connection exhaustion and reduce setup overhead.

### EVM RPC Connection Pool

For Base L2, Ethereum, and other EVM-compatible chains.

**Configuration:**

```yaml
performance:
  connectionPools:
    evm:
      poolSize: 10 # Number of concurrent RPC connections
      rpcUrls:
        - https://mainnet.base.org
        - https://base.llamarpc.com
        - https://base-rpc.publicnode.com
```

**Sizing Guidelines:**

| Expected TPS | Recommended `poolSize` | RPC URLs |
| ------------ | ---------------------- | -------- |
| 1,000 TPS    | 5                      | 2-3 URLs |
| 5,000 TPS    | 10                     | 3-5 URLs |
| 10,000+ TPS  | 15-20                  | 5+ URLs  |

**Best Practices:**

1. **Multiple RPC Endpoints**: Use 3+ different RPC providers for redundancy
2. **Health Checks**: Pool automatically removes unhealthy connections
3. **Round-Robin**: Requests distributed evenly across all connections
4. **Auto-Reconnect**: Failed connections automatically retry

**Example - High Availability Setup:**

```yaml
performance:
  connectionPools:
    evm:
      poolSize: 15
      rpcUrls:
        # Primary providers
        - https://mainnet.base.org
        - https://base.llamarpc.com

        # Backup providers
        - https://base-rpc.publicnode.com
        - https://base.gateway.tenderly.co
        - https://1rpc.io/base
```

---

## Memory Optimization

### Heap Size Tuning

Set Node.js heap size based on expected load and available RAM.

**Configuration via Environment Variable:**

```bash
# For 10K TPS workload, allocate 4-8 GB heap
export NODE_OPTIONS="--max-old-space-size=8192"  # 8 GB heap
```

**Sizing Guidelines:**

| Expected TPS | Recommended Heap Size | Notes                       |
| ------------ | --------------------- | --------------------------- |
| 1,000 TPS    | 2 GB                  | `--max-old-space-size=2048` |
| 5,000 TPS    | 4 GB                  | `--max-old-space-size=4096` |
| 10,000+ TPS  | 8 GB                  | `--max-old-space-size=8192` |

### Memory Leak Prevention

**Monitor these metrics:**

- Heap usage over time (should stabilize, not grow indefinitely)
- Garbage collection frequency (excessive GC indicates memory pressure)
- Pending transfer queue depth (should not grow unbounded)

**Diagnostic Commands:**

```bash
# Check heap usage
node --expose-gc connector.js &
kill -USR2 <pid>  # Trigger heap dump

# Analyze heap dump
npm install -g node-heapdump
node --heapsnapshot-signal=SIGUSR2 connector.js
```

---

## CPU Optimization

### Process Priority

Run the connector with higher CPU priority for consistent performance.

**Linux:**

```bash
# Nice value -10 (higher priority)
nice -n -10 node connector.js

# Or use systemd service
[Service]
ExecStart=/usr/bin/node /path/to/connector.js
Nice=-10
```

**Docker:**

```yaml
# docker-compose.yml
services:
  connector:
    image: agent-runtime:latest
    cpus: '8.0' # Limit to 8 CPUs
    cpu_shares: 2048 # Higher priority
```

### CPU Affinity

Pin connector process to specific CPU cores for cache locality.

**Linux:**

```bash
# Pin to cores 0-7
taskset -c 0-7 node connector.js
```

### Monitoring CPU Usage

Target CPU usage under load: **<80%** at 10K TPS

**Diagnostic Commands:**

```bash
# Monitor CPU usage
top -p <pid>

# Detailed CPU profiling
node --prof connector.js
node --prof-process isolate-*.log > profile.txt
```

---

## Monitoring and Metrics

### Key Performance Metrics

Monitor these metrics to ensure optimal performance:

| Metric                     | Target     | Alert Threshold |
| -------------------------- | ---------- | --------------- |
| **Throughput**             | 10,000 TPS | <8,000 TPS      |
| **p50 Latency**            | <5ms       | >10ms           |
| **p99 Latency**            | <10ms      | >20ms           |
| **p999 Latency**           | <50ms      | >100ms          |
| **Heap Usage**             | <500 MB    | >750 MB         |
| **CPU Usage**              | <80%       | >90%            |
| **Pending Transfers**      | <100       | >500            |
| **Connection Pool Health** | 100%       | <80% healthy    |

### Prometheus Metrics

The connector exposes the following Prometheus metrics:

```
# Throughput
ilp_packets_processed_total{status="fulfilled|rejected"}

# Latency
ilp_packet_processing_duration_seconds{quantile="0.5|0.99|0.999"}

# Memory
process_resident_memory_bytes
process_heap_bytes

# CPU
process_cpu_seconds_total

# Batching
tigerbeetle_batch_size{quantile="0.5|0.99"}
tigerbeetle_pending_transfers

# Connection Pools
connection_pool_healthy_connections{pool="evm"}
connection_pool_total_connections{pool="evm"}
```

### Grafana Dashboard

Example queries for Grafana dashboards:

```promql
# Throughput (TPS)
rate(ilp_packets_processed_total[1m])

# p99 Latency
histogram_quantile(0.99, rate(ilp_packet_processing_duration_seconds_bucket[1m]))

# Heap Usage
process_heap_bytes / (1024 * 1024)  # Convert to MB

# CPU Usage
rate(process_cpu_seconds_total[1m]) * 100

# Connection Pool Health Percentage
(connection_pool_healthy_connections / connection_pool_total_connections) * 100
```

---

## Troubleshooting

### Performance Issues

#### Symptom: Low Throughput (<5K TPS)

**Possible Causes:**

1. **Insufficient Worker Threads**
   - Check: `workerThreads` < CPU cores
   - Fix: Increase `workerThreads` to match CPU count

2. **Blockchain RPC Bottleneck**
   - Check: EVM connection pool health metrics
   - Fix: Increase `connectionPools.evm.poolSize` or add more RPC URLs

3. **CPU Saturation**
   - Check: CPU usage >90%
   - Fix: Reduce load or scale horizontally

#### Symptom: High Latency (p99 >50ms)

**Possible Causes:**

1. **Large Batch Sizes**
   - Check: `tigerbeetle.batchSize` >200
   - Fix: Reduce batch size to 100 or lower

2. **Slow TigerBeetle Storage**
   - Check: TigerBeetle disk I/O metrics
   - Fix: Upgrade to NVMe SSD

3. **Network Latency**
   - Check: Network RTT to blockchain RPC endpoints
   - Fix: Use geographically closer RPC endpoints

#### Symptom: Memory Leaks (Heap Growing Indefinitely)

**Possible Causes:**

1. **Unbounded Transfer Queue**
   - Check: `tigerbeetle_pending_transfers` metric
   - Fix: Reduce `maxPendingTransfers` or increase TigerBeetle flush rate

2. **Telemetry Buffer Leak**
   - Check: Telemetry buffer size metrics
   - Fix: Reduce `telemetry.bufferSize`

3. **Connection Pool Leak**
   - Check: Connection pool disconnection errors
   - Fix: Review connection lifecycle handling

#### Symptom: Connection Pool Failures

**Possible Causes:**

1. **RPC Rate Limiting**
   - Check: HTTP 429 errors in logs
   - Fix: Add more RPC endpoints or reduce request rate

2. **WebSocket Disconnections**
   - Check: Connection pool health metrics
   - Fix: Add more RPC URLs for redundancy

3. **Network Connectivity Issues**
   - Check: Network errors in logs
   - Fix: Verify firewall rules and network connectivity

---

## Performance Testing

### Load Testing Tools

Use these tools to validate 10K TPS performance:

**Artillery.io Configuration:**

```yaml
# load-test.yml
config:
  target: 'http://connector:8080'
  phases:
    - duration: 60
      arrivalRate: 10000 # 10K TPS
      name: 'Sustained load'

scenarios:
  - name: 'Send ILP packet'
    flow:
      - post:
          url: '/packets'
          json:
            destination: 'g.alice'
            amount: '1000'
```

**Run Load Test:**

```bash
artillery run load-test.yml
```

### Performance Benchmarks

Run the included performance benchmark suite:

```bash
# From connector package
npm run test:performance

# Individual benchmarks
npm test -- test/performance/throughput-benchmark.test.ts
npm test -- test/performance/latency-benchmark.test.ts
npm test -- test/performance/memory-profile.test.ts
npm test -- test/performance/cpu-profile.test.ts
```

---

## Conclusion

Achieving 10K+ TPS requires careful tuning of all performance parameters. Start with the recommended defaults, monitor key metrics, and adjust based on observed performance.

**Quick Start Checklist:**

- ✅ Configure `workerThreads` to match CPU cores
- ✅ Use default batch sizes (100 for TigerBeetle, 1000 for telemetry)
- ✅ Configure connection pools with 3+ RPC/WebSocket endpoints
- ✅ Allocate sufficient heap size (8 GB for 10K TPS)
- ✅ Monitor throughput, latency, memory, and CPU metrics
- ✅ Run load tests to validate performance under sustained load

For additional support or questions, consult the [M2M Documentation](../README.md) or file an issue on GitHub.

---

**Document Version**: 1.0
**Last Updated**: 2026-01-22
**Author**: Dev Agent James (Story 12.5)
