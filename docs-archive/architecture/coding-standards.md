# Coding Standards

**CRITICAL: These standards are MANDATORY for AI code generation**

## Core Standards

- **Languages & Runtimes:** TypeScript 5.3.3 (strict mode), Node.js 22.11.0 LTS
- **Style & Linting:** ESLint (@typescript-eslint/recommended), Prettier (line length 100, single quotes)
- **Test Organization:** Co-located tests (`*.test.ts` next to `*.ts`), `__mocks__` for shared mocks

## Naming Conventions

| Element            | Convention                                | Example                          |
| ------------------ | ----------------------------------------- | -------------------------------- |
| Files (TypeScript) | kebab-case                                | `packet-handler.ts`              |
| Classes            | PascalCase                                | `PacketHandler`                  |
| Interfaces/Types   | PascalCase with `I` prefix for interfaces | `ILPPacket`, `RoutingTableEntry` |
| Functions/Methods  | camelCase                                 | `validatePacket()`               |
| Constants          | UPPER_SNAKE_CASE                          | `DEFAULT_BTP_PORT`               |
| Private members    | camelCase with `_` prefix                 | `_internalState`                 |

## Critical Rules

- **NEVER use console.log:** Use Pino logger exclusively (`logger.info()`, `logger.error()`, etc.)
- **All ILP packet responses use typed returns:** Functions return `ILPFulfillPacket | ILPRejectPacket`, never plain objects
- **BTP connections must use BTPClient/BTPServer classes:** No raw WebSocket usage outside BTP module
- **Telemetry emission is non-blocking:** Always use `try-catch` around `telemetryEmitter.emit()` to prevent packet processing failures
- **Configuration loaded at startup only:** No runtime config changes for MVP
- **NEVER hardcode ports/URLs:** Use environment variables with defaults
- **All async functions must handle errors:** Use try-catch or .catch() - no unhandled promise rejections
- **OER encoding must validate packet structure:** Throw `InvalidPacketError` for malformed data
- **Routing table lookups return null for no match:** Caller handles null by generating F02 error

## Language-Specific Guidelines

### TypeScript Specifics

- **Strict mode enabled:** `strict: true` in tsconfig.json - no `any` types except in test mocks
- **Prefer interfaces over type aliases** for object shapes (better error messages)
- **Use `Buffer` for binary data:** Not `Uint8Array` or `ArrayBuffer` (Node.js convention)
- **Async/await over callbacks:** All asynchronous code uses `async/await` pattern
- **Optional chaining for safety:** Use `peer?.connected` instead of `peer && peer.connected`
