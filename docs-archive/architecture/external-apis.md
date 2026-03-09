# External APIs

**Decision: No External APIs Required for MVP**

This project is self-contained with no external API integrations needed. All functionality is implemented using:

- Official Interledger RFCs (specifications, not API calls)
- Docker Hub for base images (node:20-alpine)
- npm registry for package dependencies

**Rationale:**

- Educational/testing tool runs entirely locally
- No real ledger integration (MVP scope limitation per PRD)
- No cloud services or third-party APIs
- BTP connections between connectors are internal (not external APIs)

**Post-MVP Considerations:**
Future versions might integrate with:

- Interledger testnet connectors (real network connectivity)
- Settlement engine APIs (RFC-0038)
- External monitoring/alerting services
