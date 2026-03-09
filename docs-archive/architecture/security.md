# Security

**MANDATORY security requirements for AI-generated code**

## Input Validation

- **Validation Library:** Custom validators in `packages/shared/src/validation/`
- **Validation Location:** At API boundaries (BTPServer packet reception, config loading)
- **Required Rules:**
  - All ILP packets MUST be OER-decoded and validated before processing
  - ILP addresses MUST match RFC-0015 format (hierarchical, valid characters)
  - Packet expiry timestamps MUST be validated (not in past, within reasonable future bound)
  - BTP message structure MUST be validated before extracting ILP packet

## Authentication & Authorization

- **Auth Method:** Shared secrets for BTP authentication (configured per-peer in YAML)
- **Session Management:** Not applicable (no user sessions - tool runs locally)
- **Required Patterns:**
  - BTP handshake MUST validate shared secret before accepting connection
  - Invalid authentication MUST close WebSocket connection immediately
  - Health check endpoints have no authentication (localhost-only deployment)

**Note:** Dashboard visualization deferred - see DASHBOARD-DEFERRED.md in root

## Secrets Management

- **Development:** `.env` file (gitignored) for local secrets
- **Production:** Environment variables injected by Docker Compose
- **Code Requirements:**
  - NEVER hardcode BTP shared secrets - load from environment variables
  - Access secrets via `process.env` with fallback to defaults for non-sensitive config
  - No secrets in logs or error messages (redact in Pino serializers)

**Example:**

```typescript
const btpSecret =
  process.env.BTP_AUTH_SECRET ||
  (() => {
    logger.error('BTP_AUTH_SECRET not configured');
    process.exit(1);
  })();
```

## API Security

- **Rate Limiting:** Not implemented for MVP (localhost deployment, trusted environment)
- **CORS Policy:** Health check endpoints allow all origins (no CORS restrictions for localhost)
- **Security Headers:** Not required for MVP (no internet-facing deployment)
- **HTTPS Enforcement:** Not required for MVP (local Docker network uses ws://)

**Post-MVP:** Add HTTPS (wss://), CORS restrictions, rate limiting if cloud-deployed

## Data Protection

- **Encryption at Rest:** Not required (no persistent data storage)
- **Encryption in Transit:** Not required for MVP (local Docker network)
- **PII Handling:** No PII collected or processed
- **Logging Restrictions:**
  - DO NOT log BTP shared secrets
  - DO log packet amounts, addresses (not PII in test environment)
  - Redact `authToken` field in peer configuration logs

**Pino Serializer Example:**

```typescript
const logger = pino({
  serializers: {
    peer: (peer) => ({
      ...peer,
      authToken: '[REDACTED]', // Never log secrets
    }),
  },
});
```

## Dependency Security

- **Scanning Tool:** `npm audit` (built-in) + GitHub Dependabot
- **Update Policy:** Review and update dependencies monthly, critical security patches within 48 hours
- **Approval Process:** All new dependencies require rationale comment in PR

## Security Testing

- **SAST Tool:** ESLint security plugins (`eslint-plugin-security`)
- **DAST Tool:** Not applicable for MVP (no public-facing endpoints)
- **Penetration Testing:** Not required for MVP (educational tool, not production system)

**Security Stance:**
This is a **development and educational tool**, not a production payment system. Security focuses on:

- Preventing accidental secret leakage
- Basic input validation to avoid crashes
- No malicious code in dependencies

Production-grade security (encryption, formal audits, threat modeling) deferred to post-MVP if tool is adapted for real payment processing.
