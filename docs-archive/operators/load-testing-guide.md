# Load Testing Guide

## Overview

This guide covers the execution of sustained load tests for the M2M economy platform. The primary test validates that the system can maintain 10,000+ transactions per second (TPS) for 24 hours without degradation.

## Prerequisites

Before running load tests, ensure:

1. **Hardware Requirements**
   - Minimum 8GB RAM available
   - Multi-core CPU (4+ cores recommended)
   - SSD storage for logs and metrics

2. **Software Requirements**
   - Docker and Docker Compose installed
   - Node.js 20.11.0 LTS
   - npm 10.x

3. **Infrastructure**
   - TigerBeetle running (for balance tracking)
   - Optional: Anvil for full EVM integration testing

## Quick Start

### Validation Test (1 hour)

For quick validation before a full 24-hour test:

```bash
./scripts/run-load-test.sh --quick
```

This runs a 1-hour test at 1,000 TPS to verify the test infrastructure works.

### Full Load Test (24 hours)

```bash
./scripts/run-load-test.sh
```

This runs the full 24-hour test at 10,000 TPS.

## Configuration

### Environment Variables

| Variable                        | Default | Description                    |
| ------------------------------- | ------- | ------------------------------ |
| `LOAD_TEST_TPS`                 | 10000   | Target transactions per second |
| `LOAD_TEST_DURATION_HOURS`      | 24      | Test duration in hours         |
| `LOAD_TEST_RAMP_UP_MINUTES`     | 5       | Gradual ramp-up period         |
| `LOAD_TEST_METRICS_INTERVAL_MS` | 1000    | Metrics collection interval    |
| `LOAD_TEST_ENABLED`             | false   | Enable load test execution     |

### Command Line Options

```bash
./scripts/run-load-test.sh [OPTIONS]

Options:
  --tps N           Target TPS (default: 10000)
  --duration N      Duration in hours (default: 24)
  --ramp-up N       Ramp-up period in minutes (default: 5)
  --quick           Quick test: 1 hour at 1000 TPS
  --help            Show help message
```

## Performance Targets

The load test validates these performance criteria:

| Metric       | Target             | Description                             |
| ------------ | ------------------ | --------------------------------------- |
| Throughput   | ≥95% of target TPS | Must maintain 9,500+ TPS for 10K target |
| Success Rate | ≥95%               | Packet processing success rate          |
| p99 Latency  | <10ms              | 99th percentile latency                 |
| Memory Usage | <500MB heap        | Maximum heap memory usage               |
| CPU Usage    | <80%               | Maximum CPU utilization                 |

## Test Phases

### 1. Ramp-Up Phase (Default: 5 minutes)

The test gradually increases TPS from 0 to target:

- Prevents sudden load spikes
- Allows system to warm up
- Validates graceful scaling

### 2. Sustained Load Phase

Maintains target TPS for the configured duration:

- Metrics collected every second
- Progress reported every minute
- Anomalies logged immediately

### 3. Cooldown and Reporting

After completion:

- Final metrics calculated
- Results written to `docs/benchmarks/`
- Summary report generated

## Output Files

Results are saved to `docs/benchmarks/`:

| File                               | Description            |
| ---------------------------------- | ---------------------- |
| `load-test-{timestamp}.json`       | Full metrics data      |
| `load-test-summary-{timestamp}.md` | Human-readable summary |
| `load-test-{timestamp}.log`        | Console output log     |

### Sample Summary Report

```markdown
# Load Test Summary

## Test Configuration

- Target TPS: 10000
- Duration: 24 hours
- Ramp-up: 5 minutes

## Results

- **Status**: ✅ PASSED
- **Actual TPS**: 10,234.56
- **Total Packets**: 885,225,984
- **Success Rate**: 99.87%
- **P99 Latency**: 3.45ms
- **Peak Memory**: 387.23MB
- **Peak CPU**: 67.82%
```

## Monitoring During Test

### Real-Time Progress

The test logs progress every minute:

```
[INFO] Load test progress: 60m elapsed, 1380m remaining
       totalPackets=36,000,000 successRate=99.9% currentTps=10000 heapMb=245.67
```

### System Monitoring

Monitor system resources separately:

```bash
# Watch memory usage
watch -n 5 'free -m'

# Watch CPU usage
htop

# Watch disk I/O
iostat -x 5
```

## Troubleshooting

### Test Fails with Memory Errors

**Symptom**: `JavaScript heap out of memory`

**Solution**:

```bash
# Increase Node.js heap size
export NODE_OPTIONS="--max-old-space-size=4096"
./scripts/run-load-test.sh
```

### Low TPS Achieved

**Symptom**: Actual TPS significantly below target

**Possible Causes**:

1. Insufficient CPU - check CPU utilization
2. Disk I/O bottleneck - use SSD storage
3. Memory pressure - increase available RAM
4. Network latency - ensure local/staging environment

### High Latency Spikes

**Symptom**: p99 latency exceeds threshold

**Possible Causes**:

1. Garbage collection pauses - monitor GC with `--expose-gc`
2. Event loop blocking - check for synchronous operations
3. External service delays - verify dependencies are healthy

## CI/CD Integration

**Important**: The 24-hour load test should NOT run in CI/CD pipelines.

For CI/CD, use the quick validation test:

```yaml
# .github/workflows/load-test.yml (manual trigger only)
name: Load Test
on:
  workflow_dispatch:
    inputs:
      duration:
        description: 'Duration in hours'
        required: true
        default: '1'

jobs:
  load-test:
    runs-on: ubuntu-latest
    timeout-minutes: 1500 # 25 hours max
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
      - run: npm ci
      - run: ./scripts/run-load-test.sh --duration ${{ inputs.duration }}
```

## Best Practices

1. **Run in Staging First**
   - Never run load tests directly in production
   - Use staging environment with production-like configuration

2. **Schedule During Low Traffic**
   - If testing staging that shares resources, schedule during off-peak

3. **Monitor External Dependencies**
   - Watch RPC endpoints, database connections
   - Set up alerts for dependency failures

4. **Document Results**
   - Save results for trend analysis
   - Compare against baseline benchmarks
   - Track performance regression over releases

5. **Clean Up After Tests**
   - Clear test data if using real databases
   - Reset any modified configurations

## Related Documentation

- [Performance Tuning Guide](./performance-tuning-guide.md)
- [Monitoring Setup Guide](./monitoring-setup-guide.md)
- [Production Deployment Guide](./production-deployment-guide.md)
