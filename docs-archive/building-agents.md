# Building Agents with Agent Runtime

This guide covers how to build custom agents that can send and receive payments using Agent Runtime.

---

## Architecture Overview

Agent Runtime handles all the protocol complexity. You only implement the **Business Logic** — your custom payment handler.

```
┌─────────────┐      ┌─────────────────────┐      ┌───────────────────┐
│  Connector  │      │    Agent Runtime    │      │  Business Logic   │
│             │ HTTP │    (provided)       │ HTTP │  (YOUR CODE)      │
│  Routes to  │─────►│  - SPSP endpoint    │─────►│  - Accept/reject  │
│  local addr │      │  - Session mgmt     │      │  - Custom rules   │
│             │◄─────│  - STREAM fulfill   │◄─────│  - Your database  │
└─────────────┘      └─────────────────────┘      └───────────────────┘
```

**You build the Business Logic. Everything else is provided.**

---

## Quick Start

### 1. Create Your Business Logic Server

Your server needs to implement these endpoints:

| Endpoint          | Method | Purpose                                 |
| ----------------- | ------ | --------------------------------------- |
| `/handle-payment` | POST   | **Required.** Process incoming payments |
| `/payment-setup`  | POST   | _Optional._ Customize SPSP setup        |
| `/health`         | GET    | _Recommended._ Health check             |

### 2. Use the TypeScript Boilerplate

```bash
# Copy the boilerplate
cp -r examples/business-logic-typescript my-payment-handler
cd my-payment-handler

# Install dependencies
npm install

# Run in development
npm run dev
```

### 3. Implement Your Logic

Edit `src/server.ts` and implement the `handlePayment` function:

```typescript
async function handlePayment(request: PaymentRequest): Promise<PaymentResponse> {
  const { paymentId, amount, destination, data, expiresAt, metadata } = request;

  // Your business logic here:
  // - Check inventory
  // - Validate user
  // - Record payment
  // - Decode and process STREAM data

  // Accept the payment
  return { accept: true };

  // Or reject with reason
  return {
    accept: false,
    rejectReason: {
      code: 'insufficient_funds',
      message: 'Account balance too low',
    },
  };
}
```

---

## API Reference

### POST /handle-payment

Called for each incoming payment message.

**Request:**

```json
{
  "paymentId": "abc123",
  "destination": "g.connector.agent.abc123",
  "amount": "1000000",
  "expiresAt": "2024-01-15T12:00:00.000Z",
  "data": "base64-encoded-stream-data",
  "metadata": {
    "productId": "prod-456",
    "userId": "user-789"
  }
}
```

**Response (Accept):**

```json
{
  "accept": true,
  "data": "base64-encoded-response-data"
}
```

**Response (Reject):**

```json
{
  "accept": false,
  "rejectReason": {
    "code": "invalid_amount",
    "message": "Amount exceeds maximum allowed"
  }
}
```

### Reject Codes

| Code                 | Error Code | Description                |
| -------------------- | ---------- | -------------------------- |
| `insufficient_funds` | T04        | Account balance too low    |
| `expired`            | R00        | Payment expired            |
| `invalid_request`    | F00        | Bad request format         |
| `invalid_amount`     | F03        | Amount out of range        |
| `unexpected_payment` | F06        | Not expecting this payment |
| `application_error`  | F99        | Generic application error  |
| `internal_error`     | T00        | Temporary internal error   |

### POST /payment-setup (Optional)

Called when SPSP endpoint is queried (before payment begins).

**Request:**

```json
{
  "paymentId": "custom-id",
  "queryParams": {
    "product": "premium-plan",
    "user": "user-123"
  }
}
```

**Response:**

```json
{
  "allow": true,
  "metadata": {
    "productId": "premium-plan",
    "userId": "user-123"
  },
  "paymentId": "custom-payment-id"
}
```

---

## Example Use Cases

### E-Commerce

```typescript
async function handlePayment(req: PaymentRequest) {
  const { productId } = req.metadata || {};

  // Check inventory
  const product = await db.products.findById(productId);
  if (!product || product.stock <= 0) {
    return { accept: false, rejectReason: { code: 'invalid_request', message: 'Out of stock' } };
  }

  // Reserve item and accept payment
  await db.products.decrementStock(productId);
  await db.orders.create({ paymentId: req.paymentId, productId, amount: req.amount });

  return { accept: true };
}
```

### API Monetization

```typescript
async function handlePayment(req: PaymentRequest) {
  const { apiKey } = req.metadata || {};

  // Validate API key
  const user = await db.users.findByApiKey(apiKey);
  if (!user) {
    return { accept: false, rejectReason: { code: 'invalid_request', message: 'Invalid API key' } };
  }

  // Credit the user's balance
  await db.users.creditBalance(user.id, BigInt(req.amount));

  return { accept: true };
}
```

### Streaming Payments

```typescript
// Accept streaming micropayments
async function handlePayment(req: PaymentRequest) {
  const amount = BigInt(req.amount);

  // Track cumulative payment
  const session = sessions.get(req.paymentId) || { total: 0n };
  session.total += amount;
  sessions.set(req.paymentId, session);

  console.log(`Payment ${req.paymentId}: received ${amount}, total ${session.total}`);

  // Always accept streaming chunks
  return { accept: true };
}
```

---

## Deployment

### Docker Compose

```yaml
# docker-compose.yml
services:
  connector:
    image: agent-runtime
    environment:
      LOCAL_DELIVERY_ENABLED: 'true'
      LOCAL_DELIVERY_URL: http://agent-runtime:3100
    depends_on:
      - agent-runtime

  agent-runtime:
    image: agent-runtime
    environment:
      BASE_ADDRESS: g.connector.agent
      BUSINESS_LOGIC_URL: http://business-logic:8080
    depends_on:
      - business-logic

  business-logic:
    build: ./my-payment-handler
    ports:
      - '8080:8080'
```

### Kubernetes

```bash
# Deploy agent runtime
kubectl apply -k k8s/agent-runtime

# Deploy your business logic
kubectl apply -f my-business-logic-deployment.yaml

# Configure connector
kubectl -n agent-runtime set env deployment/connector \
  LOCAL_DELIVERY_ENABLED=true \
  LOCAL_DELIVERY_URL=http://agent-runtime.m2m-agent-runtime.svc.cluster.local:3100
```

---

## Environment Variables

### Agent Runtime

| Variable                 | Description                                               | Default    |
| ------------------------ | --------------------------------------------------------- | ---------- |
| `PORT`                   | HTTP server port                                          | `3100`     |
| `BASE_ADDRESS`           | Your agent's address on the network (e.g., `g.hub.agent`) | _Required_ |
| `BUSINESS_LOGIC_URL`     | URL to your business logic server                         | _Required_ |
| `BUSINESS_LOGIC_TIMEOUT` | Request timeout (ms)                                      | `5000`     |
| `SPSP_ENABLED`           | Enable SPSP endpoint                                      | `true`     |
| `SESSION_TTL_MS`         | Payment session TTL (ms)                                  | `3600000`  |
| `LOG_LEVEL`              | Logging level                                             | `info`     |

### Connector (Local Delivery)

| Variable                 | Description                        | Default |
| ------------------------ | ---------------------------------- | ------- |
| `LOCAL_DELIVERY_ENABLED` | Enable forwarding to agent runtime | `false` |
| `LOCAL_DELIVERY_URL`     | Agent runtime URL                  | —       |
| `LOCAL_DELIVERY_TIMEOUT` | Request timeout (ms)               | `30000` |
