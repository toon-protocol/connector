# Introduction

This document outlines the overall project architecture for the ILP Connector with BTP and Agent Wallet for Machine-to-Machine Payments, including backend systems, shared services, and settlement infrastructure. Its primary goal is to serve as the guiding architectural blueprint for AI-driven development, ensuring consistency and adherence to chosen patterns and technologies.

**Dashboard Visualization:**
Dashboard visualization has been deferred to focus on core payment functionality. See DASHBOARD-DEFERRED.md in the root directory for details. The system uses structured logging for observability instead.

## Change Log

| Date       | Version | Description                   | Author              |
| ---------- | ------- | ----------------------------- | ------------------- |
| 2025-12-26 | 0.1     | Initial architecture creation | Winston (Architect) |

## Starter Template or Existing Project

**Decision: Greenfield Project - No Starter Template**

Based on PRD review, this is a greenfield project with no existing codebase. Given the unique architectural requirements (ILP connector + BTP + visualization), manual setup is recommended.

**Rationale:**

- Unique requirements don't align with standard starters (API frameworks, full-stack templates)
- Monorepo structure (`packages/connector`, `packages/shared`) needs custom configuration
- Machine-to-machine payment system requires specialized architecture
- Custom ILP packet implementation required for RFC compliance and educational value

**Alternatives Considered:**

- Turborepo/Nx monorepo starters - Rejected (unnecessary complexity for small monorepo)
- Express.js frameworks (Nest.js, Fastify) - Rejected (overkill for lightweight connector)
- GraphQL API layer - Rejected (no complex API requirements)

**Implementation:** Manual initialization with npm workspaces, TypeScript strict mode, and custom project structure.
