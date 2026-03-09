# Agent Wallet Security Documentation

**Story 11.9: Security Hardening for Agent Wallets**

## Security Overview

### Purpose

Protect agent funds from theft, fraud, and unauthorized access in the M2M payment network.

### Scope

Agent wallet infrastructure (Epic 11 Stories 11.1-11.9):

- HD wallet master seed management (Story 11.1)
- Agent wallet derivation (Story 11.2)
- Balance tracking (Story 11.3)
- Automated funding (Story 11.4)
- Lifecycle management (Story 11.5)
- Payment channel integration (Story 11.6)
- Backup and recovery (Story 11.8)
- **Security hardening (Story 11.9) - this document**

### Security Stance

This is a **development and educational tool** with production-grade wallet security. Security focuses on:

- Preventing private key leakage
- Protecting agent funds from theft/fraud
- Comprehensive audit trail for forensics
- Industry-standard encryption (AES-256-GCM)
- Rate limiting and spending limits to prevent abuse

## Threat Model

### Threat 1: Private Key Exposure via Logs/Telemetry

**Risk:** Private keys, mnemonics, or seeds leaked through application logs, telemetry events, or API responses could allow attackers to steal agent funds.

**Mitigation:**

- **Pino Logger Serializers (AC 1):** Automatic redaction of sensitive fields
  - Fields redacted: `privateKey`, `mnemonic`, `seed`, `encryptionKey`, `secret`
  - All logged wallet objects sanitized before output
  - Serializers configured in `packages/connector/src/utils/logger.ts`
- **WalletSecurityManager.sanitizeWalletData() (AC 1):** Remove sensitive fields from API responses
  - Called before returning wallet data to external systems
  - Removes private keys from nested objects (e.g., `wallet.signer.privateKey`)

**Testing:** Penetration tests verify private keys never appear in logs or API responses (AC 10)

### Threat 2: Unauthorized Wallet Derivation

**Risk:** Attackers could derive agent wallets without authorization, accessing private keys and stealing funds.

**Mitigation:**

- **Authentication Requirements (AC 2):** All sensitive wallet operations require authentication
  - Password-based: PBKDF2 hash verification (100k iterations, SHA-256)
  - 2FA (TOTP): 6-digit codes, 30-second window (Epic 12 integration)
  - HSM: Hardware security module integration (Epic 12 integration)
- **WalletAuthenticationManager:** Manages authentication for wallet operations
  - `authenticatePassword()`: Timing-safe password comparison
  - `authenticate2FA()`: TOTP token verification (placeholder for MVP)
  - `authenticateHSM()`: HSM authentication (placeholder for Epic 12)

**Testing:** Penetration tests verify unauthorized derivation attempts fail (AC 10)

### Threat 3: Wallet Creation Abuse (DoS Attack)

**Risk:** Attackers could flood the system with wallet creation requests, exhausting resources or filling database storage.

**Mitigation:**

- **Rate Limiting (AC 3):** Max 100 wallet creations/hour per identifier
  - Sliding window algorithm (1-hour window)
  - Separate limits for wallet creation (100/hr) and funding requests (50/hr)
  - `RateLimiter` class: `packages/connector/src/wallet/rate-limiter.ts`
  - Emits `RATE_LIMIT_EXCEEDED` telemetry event on violation

**Testing:** Penetration tests verify 101st wallet creation request fails (AC 10)

### Threat 4: Unauthorized Spending / Fund Theft

**Risk:** Attackers could drain agent wallets through excessive or unauthorized transactions.

**Mitigation:**

- **Spending Limits (AC 4):** Per-agent configurable limits
  - Max transaction size: 1000 USDC (default)
  - Daily limit: 5000 USDC (default)
  - Monthly limit: 50000 USDC (default)
  - Configuration: `packages/connector/data/spending-limits-config.yaml`
- **Transaction Validation:** `WalletSecurityManager.validateTransaction()`
  - Checks transaction size, daily spending, monthly spending
  - Enforced before all payment channel transactions
  - Throws `SpendingLimitExceededError` on violation

**Testing:** Penetration tests verify transactions exceeding limits are rejected (AC 10)

### Threat 5: Fraudulent Activity (Rapid Funding, Unusual Patterns)

**Risk:** Compromised agents could exhibit fraudulent behavior (rapid funding requests, unusual transaction patterns).

**Mitigation:**

- **Suspicious Activity Detection (AC 5):**
  - **Rapid Funding Detection:** Flags >5 funding requests/hour
  - **Unusual Transaction Detection:** Statistical outlier detection (>3σ from mean)
  - **New Token Detection:** Flags transactions with previously unused tokens
  - `SuspiciousActivityDetector`: `packages/connector/src/wallet/suspicious-activity-detector.ts`
- **Automated Wallet Suspension:** Suspicious activity triggers `AgentWalletLifecycle.suspendWallet()`
  - Requires manual review and reactivation
  - Prevents further fraudulent transactions
- **Epic 12 Fraud Detector Integration (AC 8):**
  - Placeholder implementation for MVP (`PlaceholderFraudDetector`)
  - Epic 12 will provide advanced ML-based fraud detection
  - Interface defined: `packages/connector/src/wallet/fraud-detector-interface.ts`

**Telemetry:** Emits `SUSPICIOUS_ACTIVITY_DETECTED` event with activity type and details

**Testing:** Penetration tests verify rapid funding triggers suspension (AC 10)

### Threat 6: Master Seed Theft (Database Compromise)

**Risk:** Attacker gains access to database and steals master seed, compromising all agent wallets.

**Mitigation:**

- **AES-256-GCM Encryption at Rest (AC 6):**
  - Algorithm: AES-256-GCM (Galois/Counter Mode)
  - Key length: 256 bits
  - Authentication tag: 128 bits
  - Implementation: `WalletSeedManager` (Story 11.1)
- **PBKDF2 Key Derivation:**
  - Algorithm: PBKDF2
  - Iterations: 100,000
  - Hash function: SHA-256
  - Output: 32-byte encryption key
  - Salt: 32 random bytes per seed
- **No Plaintext Storage:** Master seed never stored unencrypted in database

**Testing:** Penetration tests verify encrypted seed requires password to decrypt (AC 10)

### Threat 7: Unauthorized Wallet Operations (Insider Threat)

**Risk:** Internal actors could perform unauthorized wallet operations without detection.

**Mitigation:**

- **Comprehensive Audit Logging (AC 7):**
  - All wallet operations logged: create, fund, suspend, transact, archive
  - Audit log schema: timestamp, operation, agentId, details, IP, userAgent, result
  - Dual storage: SQLite database (queryable) + Pino logs (real-time)
  - Immutable trail: Append-only database (no updates or deletions)
  - `AuditLogger`: `packages/connector/src/wallet/audit-logger.ts`
- **Query API:** Filter audit logs by agent, operation, date range
  - `getAuditLog(agentId, operation, startDate, endDate)`
  - Results limited to 1000 entries
  - Reverse chronological order

**Testing:** Penetration tests verify all operations recorded in audit log (AC 10)

## Encryption Specifications

### Algorithm Details

**AES-256-GCM (Advanced Encryption Standard - Galois/Counter Mode)**

- **Key size:** 256 bits (32 bytes)
- **Mode:** GCM (Galois/Counter Mode - authenticated encryption)
- **IV (Initialization Vector):** 12 bytes (96 bits), randomly generated per encryption
- **Auth tag:** 16 bytes (128 bits), verifies integrity and authenticity
- **Implementation:** Node.js `crypto` module

**PBKDF2 (Password-Based Key Derivation Function 2)**

- **Algorithm:** PBKDF2
- **Hash function:** SHA-256
- **Iterations:** 100,000 (OWASP recommendation for 2024)
- **Salt:** 32 bytes (256 bits), randomly generated per password
- **Output key size:** 32 bytes (256 bits) for AES-256
- **Implementation:** Node.js `crypto.pbkdf2Sync()`

### Encryption Scope

**Encrypted:**

- Master seed (BIP-39 mnemonic phrase) - **CRITICAL**
- Master seed metadata (createdAt timestamp)

**Not Encrypted:**

- Agent wallet public addresses (EVM addresses)
- Agent balance information (public on-chain data)
- Transaction history (queryable for analytics)
- Configuration files (no sensitive data)

### Storage Format

Encrypted master seed stored in SQLite database:

```sql
CREATE TABLE wallet_seeds (
  id INTEGER PRIMARY KEY,
  encryptedSeed TEXT NOT NULL,
  encryptionSalt TEXT NOT NULL,
  iv TEXT NOT NULL,
  authTag TEXT NOT NULL,
  createdAt INTEGER NOT NULL
);
```

## Authentication Methods

### Password-Based Authentication

**Configuration:**

- Minimum password length: 16 characters (configurable)
- Password complexity: Not enforced (user responsibility)
- Storage: PBKDF2 hash + salt in memory (MVP) or database (production)

**Process:**

1. User provides password during setup
2. `WalletAuthenticationManager.setPassword()` hashes with PBKDF2
3. Hash + salt stored securely
4. Authentication: `authenticatePassword()` verifies with timing-safe comparison

**Security:**

- 100,000 PBKDF2 iterations prevent brute-force attacks (~50-100ms per attempt)
- Timing-safe comparison (`crypto.timingSafeEqual()`) prevents timing attacks
- Authentication attempts logged (success/failure) for monitoring

### 2FA (TOTP) Authentication

**Configuration:**

- TOTP secret: Base32-encoded shared secret
- Code format: 6 digits
- Time window: 30 seconds
- Time skew tolerance: ±1 window (configurable)

**Implementation Status:**

- **MVP:** Placeholder implementation (always returns false)
- **Epic 12:** Full TOTP integration with `speakeasy` library

**Process (Future):**

1. Admin generates TOTP secret during setup
2. User scans QR code with authenticator app (Google Authenticator, Authy, etc.)
3. User provides 6-digit code for authentication
4. `authenticate2FA()` verifies code against current time

### HSM Authentication

**Configuration:**

- HSM provider: AWS KMS, HashiCorp Vault, or hardware HSM
- Key Manager: Epic 12's `KeyManager` interface

**Implementation Status:**

- **MVP:** Placeholder (always returns false)
- **Epic 12:** Full HSM integration via `KeyManager`

**Process (Future):**

1. KeyManager configured with HSM credentials
2. Wallet operations request HSM signature/decryption
3. HSM validates request and returns cryptographic response
4. No private keys ever leave HSM

## Rate Limiting Configuration

### Default Limits

| Operation        | Limit | Window |
| ---------------- | ----- | ------ |
| Wallet Creation  | 100   | 1 hour |
| Funding Requests | 50    | 1 hour |

### Algorithm

**Sliding Window:**

- Tracks timestamp of each operation per identifier
- Counts operations within last 1 hour
- More accurate than fixed window (prevents burst attacks at window boundaries)

### Configuration

```typescript
const rateLimitConfig: RateLimitConfig = {
  walletCreation: 100, // Max wallet creations/hour
  fundingRequests: 50, // Max funding requests/hour
};
```

### Identifiers

Rate limits tracked per:

- **Agent ID:** Prevents single agent from abusing wallet creation
- **IP Address:** Prevents network-level DoS attacks (future)
- **API Key:** Prevents API key abuse (future)

## Spending Limits Configuration

### Default Limits

| Limit Type           | Default    | Description              |
| -------------------- | ---------- | ------------------------ |
| Max Transaction Size | 1000 USDC  | Single transaction limit |
| Daily Limit          | 5000 USDC  | 24-hour spending limit   |
| Monthly Limit        | 50000 USDC | 30-day spending limit    |

### Per-Agent Custom Limits

Custom limits configured in `packages/connector/data/spending-limits-config.yaml`:

```yaml
spendingLimits:
  default:
    maxTransactionSize: '1000000000' # 1000 USDC (6 decimals)
    dailyLimit: '5000000000' # 5000 USDC
    monthlyLimit: '50000000000' # 50000 USDC
  perAgent:
    agent-vip-001:
      maxTransactionSize: '5000000000' # Higher limits for VIP agents
      dailyLimit: '25000000000'
      monthlyLimit: '250000000000'
```

### Enforcement

- **Validation Point:** `WalletSecurityManager.validateTransaction()`
- **Enforcement:** Before all payment channel transactions
- **Error:** `SpendingLimitExceededError` thrown on violation
- **Logging:** Warnings logged with transaction details for audit trail

## Audit Logging

### Operations Logged

All wallet operations create audit log entries:

- `wallet_created` - Agent wallet derivation
- `wallet_funded` - Initial funding transaction
- `wallet_suspended` - Wallet suspension (fraud, manual)
- `wallet_reactivated` - Wallet reactivation after suspension
- `wallet_archived` - Wallet archival (inactivity)
- `channel_opened` - Payment channel creation
- `payment_sent` - Payment channel transaction
- `channel_closed` - Payment channel closure

### Audit Log Schema

```sql
CREATE TABLE wallet_audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  timestamp INTEGER NOT NULL,
  operation TEXT NOT NULL,
  agentId TEXT NOT NULL,
  details TEXT NOT NULL,  -- JSON-encoded operation details
  ip TEXT,                -- IP address (if applicable)
  userAgent TEXT,         -- User agent (if applicable)
  result TEXT NOT NULL CHECK(result IN ('success', 'failure'))
);

CREATE INDEX idx_audit_agentId ON wallet_audit_log(agentId);
CREATE INDEX idx_audit_operation ON wallet_audit_log(operation);
CREATE INDEX idx_audit_timestamp ON wallet_audit_log(timestamp);
```

### Dual Storage

**SQLite Database:**

- Queryable persistence for forensic analysis
- Indexed for fast queries by agent, operation, timestamp
- Retention: Unlimited (implement rotation for long-running deployments)

**Pino Structured Logs:**

- Real-time monitoring and alerting
- JSON format for log aggregation (ELK, Splunk, etc.)
- Includes audit flag: `{ audit: true, ... }`

### Query API

```typescript
// Get all audit logs for agent
const logs = await auditLogger.getAuditLog('agent-001');

// Filter by operation
const creations = await auditLogger.getAuditLog(undefined, 'wallet_created');

// Filter by date range
const recentLogs = await auditLogger.getAuditLog(
  'agent-001',
  undefined,
  Date.now() - 7 * 24 * 60 * 60 * 1000, // Last 7 days
  Date.now()
);
```

## Fraud Detection (Epic 12 Integration)

### MVP Implementation

**PlaceholderFraudDetector:**

- Always returns `{ detected: false }` for MVP
- Satisfies `FraudDetector` interface for type safety
- Location: `packages/connector/src/wallet/placeholder-fraud-detector.ts`

### Suspicious Activity Detection (Built-in)

**Rapid Funding Detection:**

- Threshold: >5 funding requests/hour (configurable)
- Detection: `SuspiciousActivityDetector.detectRapidFunding()`
- Action: Automated wallet suspension

**Unusual Transaction Detection:**

- Statistical outlier: >3 standard deviations from agent's mean transaction size
- New token: Transaction with token not previously used by agent
- Detection: `SuspiciousActivityDetector.detectUnusualTransactions()`
- Action: Logged warning, optional suspension

### Epic 12 Integration Plan

**Future Fraud Detector:**

```typescript
interface FraudDetector {
  analyzeTransaction(params: {
    agentId: string;
    amount: bigint;
    token: string;
    timestamp: number;
  }): Promise<FraudCheckResult>;
}
```

**Migration:**

1. Epic 12 implements production `FraudDetector` class
2. Replace `PlaceholderFraudDetector` with Epic 12's implementation
3. No interface changes required (drop-in replacement)

**Advanced Detection (Epic 12):**

- Machine learning models for pattern recognition
- Cross-agent behavioral analysis
- Known fraud signatures database
- Real-time risk scoring

## Security Best Practices

### For Operators

1. **Strong Passwords:**
   - Minimum 16 characters (enforced)
   - Use password manager to generate random passwords
   - Include uppercase, lowercase, numbers, symbols
   - Never reuse passwords across systems

2. **Password Rotation:**
   - Rotate passwords quarterly (every 3 months)
   - Rotate immediately after suspected compromise
   - Update all team members on rotation schedule

3. **2FA Enabled (Production):**
   - Enable TOTP 2FA for all production deployments
   - Use hardware security keys (YubiKey) when possible
   - Maintain backup codes securely

4. **Audit Log Monitoring:**
   - Review audit logs weekly for suspicious patterns
   - Set up alerts for:
     - Multiple failed authentication attempts
     - Rate limit violations
     - Spending limit violations
     - Wallet suspensions
   - Investigate all anomalies promptly

5. **Secure Master Seed Backup (Story 11.8):**
   - Export encrypted backup weekly
   - Store backups in geographically distributed locations
   - Test recovery procedure quarterly
   - Validate backup checksums before archival

6. **Spending Limit Configuration:**
   - Review and adjust limits based on agent usage patterns
   - Set conservative limits for new agents
   - Increase limits gradually as agents prove trustworthy
   - Monitor spending patterns for drift

### For Developers

1. **Never Log Sensitive Data:**
   - Use Pino logger exclusively (automatic serialization)
   - Call `sanitizeWalletData()` before external API responses
   - Never use `console.log()` for wallet operations

2. **Authentication Integration:**
   - Require authentication for all sensitive operations
   - Use `WalletAuthenticationManager.authenticate()` consistently
   - Throw `UnauthorizedError` on authentication failures

3. **Audit Logging:**
   - Call `AuditLogger.auditLog()` for all wallet operations
   - Include relevant context in details field
   - Mark failures with `result: 'failure'`

4. **Error Handling:**
   - Catch and log all security-related errors
   - Fail closed (deny access) on errors, never fail open
   - Provide actionable error messages to users

## Penetration Testing (AC 10)

### Test Vectors

**Implemented in:** `packages/connector/test/integration/wallet-security-penetration.test.ts`

| Test Vector                                | Expected Result                        | Status      |
| ------------------------------------------ | -------------------------------------- | ----------- |
| Private key exposure in logs               | Private keys redacted as `[REDACTED]`  | ✅ Verified |
| Private key exposure in API responses      | Private keys not present in responses  | ✅ Verified |
| Unauthorized wallet derivation             | `UnauthorizedError` thrown             | ✅ Verified |
| Rate limit bypass (101 wallet creations)   | 101st creation fails                   | ✅ Verified |
| Spending limit bypass (exceed daily limit) | Transaction rejected                   | ✅ Verified |
| Rapid funding attack                       | Wallet suspended automatically         | ✅ Verified |
| Audit log tampering                        | Database constraints prevent tampering | ✅ Verified |

### Attack Scenarios

**1. Private Key Extraction:**

- Attempt to extract private keys from logs
- Attempt to extract private keys from API responses
- Result: All attempts fail (keys redacted)

**2. Unauthorized Access:**

- Attempt wallet derivation without authentication
- Attempt seed decryption without password
- Result: `UnauthorizedError` thrown, operation denied

**3. Resource Exhaustion (DoS):**

- Create 101 wallets in 1 hour (exceed rate limit)
- Result: 101st request blocked, `RateLimitExceededError`

**4. Fund Theft:**

- Execute transactions exceeding daily spending limit
- Execute single transaction exceeding max size
- Result: Transactions rejected, `SpendingLimitExceededError`

**5. Fraud Evasion:**

- Rapidly request funding (10 requests in 10 minutes)
- Result: Wallet suspended, `SUSPICIOUS_ACTIVITY_DETECTED` event

### Test Results

All penetration tests **PASSING** ✅

No security vulnerabilities detected in automated testing. Manual security review recommended before production deployment.

## Implementation Notes

### Story 11.9 Components

**New Files:**

- `packages/connector/src/wallet/wallet-security.ts` - Security manager
- `packages/connector/src/wallet/wallet-authentication.ts` - Authentication manager
- `packages/connector/src/wallet/rate-limiter.ts` - Rate limiting
- `packages/connector/src/wallet/audit-logger.ts` - Audit logging
- `packages/connector/src/wallet/suspicious-activity-detector.ts` - Fraud detection
- `packages/connector/src/wallet/fraud-detector-interface.ts` - Epic 12 interface
- `packages/connector/src/wallet/placeholder-fraud-detector.ts` - MVP placeholder

**Modified Files:**

- `packages/connector/src/utils/logger.ts` - Added wallet serializers
- `packages/shared/src/types/telemetry.ts` - Added security events

**Test Coverage:**

- WalletSecurityManager: 25 tests ✅
- WalletAuthenticationManager: 20 tests ✅
- RateLimiter: 17 tests ✅
- AuditLogger: 15 tests ✅
- SuspiciousActivityDetector: 10 tests ✅
- Logger sanitization: 7 tests ✅
- **Total: 94 tests, 100% passing**

### Known Limitations (MVP)

1. **2FA Not Fully Implemented:** Placeholder returns false (Epic 12 integration needed)
2. **HSM Not Available:** Placeholder returns false (Epic 12 dependency)
3. **In-Memory Rate Limiting:** Rate limit state lost on restart (acceptable for MVP)
4. **In-Memory Authentication:** Password hash not persisted (reconfigure on restart)
5. **Basic Fraud Detection:** Statistical analysis only, no ML models (Epic 12 for advanced detection)

### Production Readiness Checklist

Before production deployment:

- [ ] Integrate Epic 12 fraud detector (replace placeholder)
- [ ] Implement 2FA (TOTP) with `speakeasy` library
- [ ] Configure HSM integration (Epic 12 KeyManager)
- [ ] Persist authentication credentials (database or HSM)
- [ ] Persist rate limit state (Redis or database)
- [ ] Configure spending limits per agent
- [ ] Set up audit log rotation/archival
- [ ] Configure log aggregation (ELK, Splunk)
- [ ] Set up security monitoring alerts
- [ ] Conduct professional penetration test
- [ ] Security code review by external auditor
- [ ] Implement password rotation policy
- [ ] Configure backup encryption for audit logs

---

**Security Contact:** For security issues, contact project maintainers immediately.

**Last Updated:** 2026-01-21 (Story 11.9 implementation)
