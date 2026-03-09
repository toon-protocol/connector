# Error Handling Strategy

## General Approach

- **Error Model:** Exception-based error handling with typed error classes
- **Exception Hierarchy:**
  - `ILPError` (base class for ILP protocol errors)
    - `ILPFinalError` (F-prefix errors - permanent failures)
    - `ILPTemporaryError` (T-prefix errors - retryable)
    - `ILPRelativeError` (R-prefix errors - protocol violations)
  - `BTPError` (BTP protocol errors)
  - `ConfigurationError` (startup configuration issues)
  - `TelemetryError` (telemetry emission failures - non-critical)
- **Error Propagation:**
  - ILP errors converted to ILPRejectPacket and returned to sender
  - BTP errors logged and trigger connection retry
  - Configuration errors cause startup failure with clear messages
  - Telemetry errors logged but do not block packet processing

## Logging Standards

- **Library:** Pino 8.17.x
- **Format:** Structured JSON with consistent schema
- **Levels:** DEBUG, INFO, WARN, ERROR
  - **DEBUG:** Detailed packet contents, routing table lookups
  - **INFO:** Packet forwarding events, connection state changes
  - **WARN:** Retry attempts, degraded performance
  - **ERROR:** Unrecoverable errors, configuration issues
- **Required Context:**
  - **Correlation ID:** Generated for each ILP Prepare packet, tracked through entire flow
  - **Service Context:** `nodeId` included in every log entry
  - **User Context:** Not applicable (no user authentication in MVP)

**Example Structured Log Entry:**

```json
{
  "level": "info",
  "time": 1703620800000,
  "nodeId": "connector-a",
  "correlationId": "pkt_abc123",
  "msg": "Packet forwarded",
  "packetType": "PREPARE",
  "destination": "g.connectorC.dest",
  "nextHop": "connectorB",
  "amount": "1000"
}
```

## Error Handling Patterns

### External API Errors (BTP Connections)

- **Retry Policy:** Exponential backoff (1s, 2s, 4s, 8s, 16s) up to 5 attempts
- **Circuit Breaker:** After 5 consecutive failures, mark peer as DISCONNECTED for 60s before retry
- **Timeout Configuration:** BTP connection timeout 5s, packet send timeout 10s
- **Error Translation:**
  - BTP connection failure → ILP T01 (Ledger Unreachable) error
  - BTP timeout → ILP T00 (Transfer Timed Out) error
  - BTP authentication failure → Startup failure (configuration error)

### Business Logic Errors

- **Custom Exceptions:**
  - `PacketExpiredError` → ILP T00 (Transfer Timed Out)
  - `RouteNotFoundError` → ILP F02 (Unreachable)
  - `InvalidPacketError` → ILP R00 (Transfer Cancelled)
- **Error Logging:** Errors logged with structured JSON for consumption by external monitoring tools
- **Error Codes:** ILP standard error codes (RFC-0027) used consistently

**Note:** Dashboard visualization deferred - see DASHBOARD-DEFERRED.md in root

### Data Consistency

- **Transaction Strategy:** No database transactions (in-memory only for MVP)
- **Compensation Logic:** Not applicable for MVP (no distributed transactions)
- **Idempotency:** Packet IDs used to detect duplicates (best-effort, not guaranteed in MVP)
