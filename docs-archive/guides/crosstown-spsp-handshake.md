# Crosstown Node SPSP Handshake with Connector

## Overview

This document explains how a Crosstown node performs the Simple Payment Setup Protocol (SPSP) handshake with a Connector to enable payment reception. This flow is essential for Crosstown nodes to receive ILP payments through the connector infrastructure.

## What is SPSP?

**Simple Payment Setup Protocol (SPSP)** is a lightweight protocol for setting up ILP payments between a sender and receiver. It uses HTTPS to exchange payment information without requiring pre-existing trust relationships.

**Key Points:**

- Based on RFC 0009: Simple Payment Setup Protocol
- Uses HTTPS for discovery and setup
- Returns ILP address and shared secret for payment routing
- Enables receivers to accept payments without complex setup

## SPSP Handshake Flow

```
┌──────────────┐                              ┌──────────────┐
│   Crosstown  │                              │  Connector   │
│     Node     │                              │  (Receiver)  │
└──────┬───────┘                              └──────┬───────┘
       │                                             │
       │  1. HTTP GET /.well-known/pay              │
       │  Accept: application/spsp4+json             │
       │─────────────────────────────────────────────>│
       │                                             │
       │                                             │ 2. Generate
       │                                             │    payment info
       │                                             │
       │  3. SPSP Response (200 OK)                  │
       │  Content-Type: application/spsp4+json       │
       │<─────────────────────────────────────────────│
       │  {                                          │
       │    "destination_account": "g.connector...", │
       │    "shared_secret": "base64...",            │
       │    "receipts_enabled": false                │
       │  }                                          │
       │                                             │
       │  4. Establish BTP WebSocket Connection      │
       │─────────────────────────────────────────────>│
       │                                             │
       │  5. Send ILP PREPARE packets                │
       │─────────────────────────────────────────────>│
       │                                             │
       │  6. Receive ILP FULFILL/REJECT              │
       │<─────────────────────────────────────────────│
       │                                             │
```

## Step-by-Step Breakdown

### 1. Discovery Request (Crosstown → Connector)

The Crosstown node initiates the handshake by making an HTTPS GET request to the connector's SPSP endpoint.

**HTTP Request:**

```http
GET /.well-known/pay HTTP/1.1
Host: connector.example.com
Accept: application/spsp4+json
```

**Key Details:**

- **Endpoint:** `/.well-known/pay` (standard SPSP discovery endpoint)
- **Method:** GET
- **Accept Header:** `application/spsp4+json` (SPSP version 4)
- **Host:** The connector's hostname or IP address

### 2. Payment Information Generation (Connector)

The connector receives the SPSP request and generates payment information:

1. **Creates a unique ILP address** for this payment session
2. **Generates a shared secret** for STREAM protocol encryption
3. **Prepares response** with payment details

**Connector Implementation Reference:**

- File: `packages/connector/src/api/spsp-server.ts` (if implemented)
- Or: Uses the public API's SPSP endpoint handler

### 3. SPSP Response (Connector → Crosstown)

The connector responds with JSON containing payment details.

**HTTP Response:**

```http
HTTP/1.1 200 OK
Content-Type: application/spsp4+json
Cache-Control: no-cache

{
  "destination_account": "g.connector-receiver.local.~payment-id-abc123",
  "shared_secret": "dGVzdC1zaGFyZWQtc2VjcmV0LWJhc2U2NA==",
  "receipts_enabled": false
}
```

**Response Fields:**
| Field | Type | Description |
|-------|------|-------------|
| `destination_account` | string | ILP address where payments should be sent |
| `shared_secret` | string | Base64-encoded shared secret for STREAM encryption |
| `receipts_enabled` | boolean | Whether STREAM receipts are supported (RFC 0039) |

**ILP Address Format:**

- Follows RFC 0015 hierarchical addressing
- Example: `g.connector-receiver.local.~payment-id-abc123`
- Segments:
  - `g` - Global allocation scheme
  - `connector-receiver` - Connector node ID
  - `local` - Local routing segment
  - `~payment-id-abc123` - Dynamic payment session ID

### 4. BTP Connection Establishment

After receiving the SPSP response, the Crosstown node establishes a WebSocket connection using BTP (Bilateral Transfer Protocol).

**WebSocket Connection:**

```javascript
const ws = new WebSocket('ws://connector.example.com:3000');

// BTP authentication message
const authMessage = {
  type: 'auth',
  peerId: 'crosstown-node-1',
  token: 'optional-auth-token',
};
```

**Connection Details:**

- **Protocol:** WebSocket (ws:// or wss://)
- **Default Port:** 3000 (configurable via `BTP_SERVER_PORT`)
- **Authentication:** Optional token-based auth
- **Peer ID:** Unique identifier for the Crosstown node

### 5. Send ILP PREPARE Packets

Once connected, the Crosstown node sends ILP PREPARE packets to transfer value.

**ILP PREPARE Packet Structure:**

```typescript
{
  type: PacketType.PREPARE,
  destination: "g.connector-receiver.local.~payment-id-abc123", // from SPSP
  amount: 1000n, // Amount in base units
  executionCondition: Buffer.from('...'), // 32-byte hash condition
  expiresAt: new Date(Date.now() + 30000), // 30 second timeout
  data: Buffer.from('...') // STREAM protocol data (encrypted with shared_secret)
}
```

**Key Fields:**

- `destination`: ILP address from SPSP response
- `amount`: Payment amount in smallest unit (e.g., satoshis, wei)
- `executionCondition`: SHA-256 hash of the fulfillment (HTLC mechanism)
- `expiresAt`: Expiration timestamp for the conditional payment
- `data`: Encrypted STREAM data containing payment details

### 6. Receive ILP FULFILL or REJECT

The connector processes the packet and responds with either FULFILL or REJECT.

**ILP FULFILL (Success):**

```typescript
{
  type: PacketType.FULFILL,
  fulfillment: Buffer.from('...'), // 32-byte preimage
  data: Buffer.from('...') // Optional response data
}
```

**ILP REJECT (Failure):**

```typescript
{
  type: PacketType.REJECT,
  code: 'F99', // Error code (e.g., F99 = Application Error)
  triggeredBy: 'g.connector-receiver',
  message: 'Insufficient balance',
  data: Buffer.from('...')
}
```

## Crosstown Node Implementation Guide

### Required Dependencies

```bash
npm install ws @crosstown/shared ethers
```

### Example SPSP Client Code

```typescript
import fetch from 'node-fetch';
import WebSocket from 'ws';
import { PacketType, ILPPreparePacket } from '@crosstown/shared';

/**
 * Perform SPSP handshake and get payment details
 */
async function performSPSPHandshake(connectorUrl: string): Promise<SPSPResponse> {
  const response = await fetch(`${connectorUrl}/.well-known/pay`, {
    method: 'GET',
    headers: {
      Accept: 'application/spsp4+json',
    },
  });

  if (!response.ok) {
    throw new Error(`SPSP request failed: ${response.statusText}`);
  }

  const spspData = await response.json();
  return {
    destinationAccount: spspData.destination_account,
    sharedSecret: Buffer.from(spspData.shared_secret, 'base64'),
    receiptsEnabled: spspData.receipts_enabled || false,
  };
}

/**
 * Establish BTP WebSocket connection
 */
function connectBTP(btpUrl: string, peerId: string): Promise<WebSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(btpUrl);

    ws.on('open', () => {
      // Send BTP auth message
      ws.send(
        JSON.stringify({
          type: 'auth',
          peerId,
          token: process.env.BTP_AUTH_TOKEN || '',
        })
      );
      resolve(ws);
    });

    ws.on('error', reject);
  });
}

/**
 * Send ILP payment
 */
async function sendPayment(
  ws: WebSocket,
  destination: string,
  amount: bigint,
  sharedSecret: Buffer
): Promise<void> {
  const prepare: ILPPreparePacket = {
    type: PacketType.PREPARE,
    destination,
    amount,
    executionCondition: Buffer.alloc(32, 1), // Simplified for example
    expiresAt: new Date(Date.now() + 30000),
    data: encryptStreamData(sharedSecret, amount), // STREAM encryption
  };

  ws.send(JSON.stringify(prepare));

  // Wait for FULFILL or REJECT response
  return new Promise((resolve, reject) => {
    ws.once('message', (data) => {
      const response = JSON.parse(data.toString());
      if (response.type === PacketType.FULFILL) {
        resolve();
      } else {
        reject(new Error(`Payment rejected: ${response.message}`));
      }
    });
  });
}

/**
 * Complete SPSP payment flow
 */
async function performSPSPPayment(
  connectorHttpUrl: string,
  connectorBtpUrl: string,
  amount: bigint,
  peerId: string
): Promise<void> {
  // 1. SPSP handshake
  console.log('📡 Performing SPSP handshake...');
  const spspInfo = await performSPSPHandshake(connectorHttpUrl);
  console.log(`✅ Received destination: ${spspInfo.destinationAccount}`);

  // 2. Connect via BTP
  console.log('🔌 Establishing BTP connection...');
  const ws = await connectBTP(connectorBtpUrl, peerId);
  console.log('✅ BTP connection established');

  try {
    // 3. Send payment
    console.log(`💸 Sending payment of ${amount}...`);
    await sendPayment(ws, spspInfo.destinationAccount, amount, spspInfo.sharedSecret);
    console.log('✅ Payment successful!');
  } finally {
    ws.close();
  }
}

// Usage Example
performSPSPPayment(
  'http://localhost:8080', // Connector HTTP API
  'ws://localhost:3000', // Connector BTP WebSocket
  1000n, // Amount
  'crosstown-node-1' // Peer ID
);
```

## Testing the SPSP Handshake

### Test Scenario: Crosstown Node as Sender

The E2E test validates the connector acting as the **receiver**. For testing Crosstown as the sender:

**Test Steps:**

1. Start connector with SPSP endpoint enabled
2. Crosstown node performs SPSP handshake (GET /.well-known/pay)
3. Verify SPSP response contains valid ILP address and shared secret
4. Establish BTP WebSocket connection
5. Send ILP PREPARE packet with payment
6. Verify FULFILL response received
7. Check accounting balances updated

**Test File Location:**

```
packages/crosstown/test/integration/spsp-sender.test.ts
```

**Test Structure:**

```typescript
describe('Crosstown SPSP Sender', () => {
  it('should perform SPSP handshake with connector', async () => {
    // 1. SPSP discovery
    const spspResponse = await fetch('http://localhost:8080/.well-known/pay');
    expect(spspResponse.status).toBe(200);

    const spspData = await spspResponse.json();
    expect(spspData.destination_account).toMatch(/^g\./);
    expect(spspData.shared_secret).toBeDefined();
  });

  it('should send payment via BTP after SPSP handshake', async () => {
    // Test BTP connection and payment flow
  });
});
```

## Configuration Requirements

### Connector Configuration (Receiver)

The connector must be configured to accept incoming payments:

```yaml
# connector-config.yaml
nodeId: connector-receiver
btpServerPort: 3000
healthCheckPort: 8080

# SPSP endpoint enabled via public API
publicApi:
  enabled: true
  port: 8080
  spspEnabled: true

# Settlement infrastructure (for on-chain settlement)
settlementInfra:
  enabled: true
  rpcUrl: http://localhost:8545
  registryAddress: '0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512'
  privateKey: '0x...'
```

### Crosstown Node Configuration (Sender)

```yaml
# crosstown-config.yaml
nodeId: crosstown-node-1
connectorUrl: http://connector-receiver:8080
btpUrl: ws://connector-receiver:3000

# Authentication token for BTP (if required)
btpAuthToken: optional-secret-token
```

## Security Considerations

### 1. HTTPS in Production

- Always use HTTPS for SPSP requests in production
- Prevents man-in-the-middle attacks on shared secret exchange

### 2. Shared Secret Handling

- The shared secret is used to encrypt STREAM protocol data
- Never log or expose shared secrets
- Use secure random generation (crypto.randomBytes(32))

### 3. BTP Authentication

- Configure BTP auth tokens for peer verification
- Rotate tokens regularly
- Use WSS (WebSocket Secure) in production

### 4. Rate Limiting

- Implement rate limits on SPSP endpoint
- Prevent DoS attacks on payment setup
- Configure in connector's public API settings

## Troubleshooting

### SPSP Request Fails (404)

**Problem:** `GET /.well-known/pay` returns 404

**Solutions:**

- Verify connector's public API is enabled
- Check `publicApi.spspEnabled` is true in config
- Ensure connector is running and healthy

### BTP Connection Refused

**Problem:** WebSocket connection to BTP port fails

**Solutions:**

- Verify `btpServerPort` matches connection URL
- Check firewall rules allow WebSocket connections
- Ensure connector has started BTP server (check logs)

### ILP Packet Rejected

**Problem:** PREPARE packets return REJECT with error codes

**Common Error Codes:**

- `F99` - Application error (check connector logs)
- `T00` - Temporary failure (retry with backoff)
- `R00` - Rejected by receiver (insufficient balance, invalid destination)

**Solutions:**

- Check ILP address matches SPSP response exactly
- Verify amount doesn't exceed connector limits
- Check executionCondition/fulfillment are valid 32-byte values

## References

### Interledger RFCs

- [RFC 0009: Simple Payment Setup Protocol (SPSP)](https://interledger.org/rfcs/0009-simple-payment-setup-protocol/)
- [RFC 0027: Interledger Protocol V4 (ILPv4)](https://interledger.org/rfcs/0027-interledger-protocol-4/)
- [RFC 0029: STREAM Protocol](https://interledger.org/rfcs/0029-stream/)
- [RFC 0023: Bilateral Transfer Protocol (BTP)](https://interledger.org/rfcs/0023-bilateral-transfer-protocol/)

### Related Documentation

- [Crosstown E2E Test](../../packages/connector/test/integration/crosstown-comprehensive-e2e.test.ts)
- [Connector Configuration Guide](./connector-configuration.md)
- [BTP Protocol Guide](./btp-protocol.md)

## Next Steps for Crosstown Integration

1. **Implement SPSP Client in Crosstown**
   - Add SPSP handshake function
   - Handle shared secret encryption/decryption
   - Implement STREAM protocol data formatting

2. **Create Integration Tests**
   - Test SPSP handshake with live connector
   - Validate payment flow end-to-end
   - Test error handling and retries

3. **Add BTP Connection Management**
   - Handle connection failures and reconnection
   - Implement heartbeat/ping for connection health
   - Manage multiple concurrent connections

4. **Settlement Integration**
   - Connect SPSP payments to on-chain settlement
   - Trigger settlement when thresholds reached
   - Monitor payment channel state

5. **Production Hardening**
   - Add comprehensive error handling
   - Implement retry logic with exponential backoff
   - Add metrics and monitoring for SPSP flows
