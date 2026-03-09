# ElizaOS Plugin System - Comprehensive Research Report

## Executive Summary

ElizaOS is a TypeScript framework for autonomous AI agents using a modular plugin system with 90+ plugins. Plugins are npm packages exporting a `Plugin` object containing Actions, Providers, Evaluators, Services, HTTP Routes, Event Handlers, Model Handlers, and Database Adapters. The framework uses **Bun** as package manager and **tsup** for TypeScript compilation.

### Key Design Principles

- **Everything is a Plugin**: All functionality (LLM providers, platform connectors, blockchain integrations) follows the same Plugin interface
- **Convention over Configuration**: Strict naming (`plugin-` prefix), directory structure, and registration order
- **Lifecycle-Driven**: Components register in a strict order; dependency resolution via topological sort
- **Runtime as DI Container**: `IAgentRuntime` provides access to all services, settings, memory, and models

### Critical Constraints

- Plugin names **must** start with `plugin-`
- Action names use `UPPER_SNAKE_CASE`
- Build output **must** be ESM format
- `@elizaos/core` must be marked as external in tsup config
- Registration order: adapter → actions → evaluators → providers → models → routes → events → services
- Publishing requires `images/logo.jpg` (400x400, max 500KB) and `images/banner.jpg` (1280x640, max 1MB)

---

## Section 1: Complete Type Reference

> Full verbatim type definitions are in the companion file: `elizaos-type-definitions.md`

### Plugin Interface (All Fields)

```typescript
export interface Plugin {
  name: string; // REQUIRED - unique plugin identifier
  description: string; // REQUIRED - what the plugin does
  init?: (config: Record<string, string>, runtime: IAgentRuntime) => Promise<void>;
  config?: { [key: string]: any }; // Config passed to init()
  actions?: Action[]; // User-invokable actions
  providers?: Provider[]; // Context providers for state composition
  evaluators?: Evaluator[]; // Post-message evaluators
  services?: (typeof Service)[]; // Background services (class references)
  adapter?: IDatabaseAdapter; // Custom database adapter
  models?: {
    // Model handler registrations
    [key: string]: (...args: any[]) => Promise<any>;
  };
  events?: PluginEvents; // Event subscriptions
  routes?: Route[]; // HTTP endpoint definitions
  tests?: TestSuite[]; // Built-in test suites
  componentTypes?: {
    // Custom component type schemas
    name: string;
    schema: Record<string, unknown>;
    validator?: (data: any) => boolean;
  }[];
  dependencies?: string[]; // Required plugins (loaded first)
  testDependencies?: string[]; // Test-only dependencies
  priority?: number; // Loading order (lower = earlier)
  schema?: any; // Drizzle ORM table definitions
}
```

### Action Interface

```typescript
export interface Action {
  name: string; // UPPER_SNAKE_CASE (e.g., TEXT_TO_VIDEO)
  similes?: string[]; // Alternative trigger names for matching
  description: string; // What this action does (used by LLM for selection)
  examples?: ActionExample[][]; // Conversation examples showing usage
  handler: Handler; // Execution function
  validate: Validator; // Pre-execution validation
}

type Handler = (
  runtime: IAgentRuntime,
  message: Memory,
  state?: State,
  options?: { [key: string]: unknown },
  callback?: HandlerCallback,
  responses?: Memory[]
) => Promise<unknown>;

type Validator = (runtime: IAgentRuntime, message: Memory, state?: State) => Promise<boolean>;

type HandlerCallback = (content: Content, files?: any[]) => Promise<Memory[]>;
```

### ActionResult Interface

```typescript
interface ActionResult {
  success: boolean;
  text?: string;
  error?: Error | string;
  data?: Record<string, unknown>;
}
```

### Provider Interface

```typescript
export interface Provider {
  name: string; // Provider identifier
  description?: string; // What data this provider supplies
  dynamic?: boolean; // Whether provider is context-dependent
  position?: number; // Execution order (lower = earlier)
  private?: boolean; // Hide from LLM context
  get: (runtime: IAgentRuntime, message: Memory, state: State) => Promise<ProviderResult>;
}

interface ProviderResult {
  values?: { [key: string]: any }; // Key-value pairs injected into state.values
  data?: { [key: string]: any }; // Structured data for state.data
  text?: string; // Text appended to agent context
}
```

### Evaluator Interface

```typescript
export interface Evaluator {
  name: string; // Evaluator identifier
  description: string; // What this evaluator checks
  alwaysRun?: boolean; // Run after every message (default: false)
  similes?: string[]; // Alternative names
  examples: EvaluationExample[]; // Usage examples
  handler: Handler; // Evaluation function
  validate: Validator; // Whether to run for this message
}
```

### Service Abstract Class

```typescript
export abstract class Service {
  protected runtime!: IAgentRuntime;

  constructor(runtime?: IAgentRuntime) {
    if (runtime) this.runtime = runtime;
  }

  abstract stop(): Promise<void>;
  static serviceType: string;
  abstract capabilityDescription: string;
  config?: Metadata;

  static async start(_runtime: IAgentRuntime): Promise<Service> {
    throw new Error('Not implemented');
  }
}
```

### Predefined ServiceType Values

`TRANSCRIPTION`, `VIDEO`, `BROWSER`, `PDF`, `REMOTE_FILES` (AWS S3), `WEB_SEARCH`, `EMAIL`, `TEE`, `TASK`, `WALLET`, `LP_POOL`, `TOKEN_DATA`, `DATABASE_MIGRATION`, `PLUGIN_MANAGER`, `PLUGIN_CONFIGURATION`, `PLUGIN_USER_INTERACTION`

### Route Type

```typescript
export type Route = {
  type: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE' | 'STATIC';
  path: string;
  handler?: (req: RouteRequest, res: RouteResponse, runtime: IAgentRuntime) => Promise<void>;
  filePath?: string; // For STATIC type routes
  public?: boolean; // Skip authentication
  name?: string; // Route identifier
  isMultipart?: boolean; // Accept file uploads
};
```

### Memory Type

```typescript
interface Memory {
  id?: UUID;
  type?: MemoryType;
  entityId: UUID;
  agentId?: UUID;
  roomId: UUID;
  worldId?: UUID;
  content: Content;
  embedding?: number[];
  createdAt?: number;
  updatedAt?: Date;
  unique?: boolean;
  similarity?: number;
  metadata?: MemoryMetadata;
}

interface Content {
  text?: string;
  actions?: string[];
  inReplyTo?: UUID;
  source?: string;
  metadata?: any;
  thought?: string;
  attachments?: Media[];
  [key: string]: any;
}
```

### State Type

```typescript
interface State {
  values: Record<string, any>;
  data: StateData;
  text: string;
  [key: string]: unknown;
}

interface StateData {
  room?: Room;
  world?: World;
  entity?: Entity;
  providers?: Record<string, ProviderResult>;
  actionPlan?: ActionPlan;
  actionResults?: ActionResult[];
  [key: string]: unknown;
}
```

### ModelHandler Interface

```typescript
export interface ModelHandler<TParams = Record<string, unknown>, TResult = unknown> {
  handler: (runtime: IAgentRuntime, params: TParams) => Promise<TResult>;
  provider: string; // Plugin name that registered this handler
  priority?: number; // Higher = preferred when multiple handlers exist
  registrationOrder?: number;
}
```

### ModelType Constants

```typescript
export const ModelType = {
  TEXT_SMALL: 'TEXT_SMALL',
  TEXT_LARGE: 'TEXT_LARGE',
  TEXT_EMBEDDING: 'TEXT_EMBEDDING',
  TEXT_TOKENIZER_ENCODE: 'TEXT_TOKENIZER_ENCODE',
  TEXT_TOKENIZER_DECODE: 'TEXT_TOKENIZER_DECODE',
  TEXT_REASONING_SMALL: 'REASONING_SMALL',
  TEXT_REASONING_LARGE: 'REASONING_LARGE',
  TEXT_COMPLETION: 'TEXT_COMPLETION',
  IMAGE: 'IMAGE',
  IMAGE_DESCRIPTION: 'IMAGE_DESCRIPTION',
  TRANSCRIPTION: 'TRANSCRIPTION',
  TEXT_TO_SPEECH: 'TEXT_TO_SPEECH',
  AUDIO: 'AUDIO',
  VIDEO: 'VIDEO',
  OBJECT_SMALL: 'OBJECT_SMALL',
  OBJECT_LARGE: 'OBJECT_LARGE',
} as const;
```

### Event System

```typescript
export type PluginEvents = {
  [K in keyof EventPayloadMap]?: EventHandler<K>[];
} & {
  [key: string]: ((params: any) => Promise<any>)[]; // Custom events supported
};
```

**Standard Events:**

- World: `WORLD_JOINED`, `WORLD_CONNECTED`, `WORLD_LEFT`
- Entity: `ENTITY_JOINED`, `ENTITY_LEFT`, `ENTITY_UPDATED`
- Room: `ROOM_JOINED`, `ROOM_LEFT`
- Message: `MESSAGE_RECEIVED`, `MESSAGE_SENT`, `MESSAGE_DELETED`
- Voice: `VOICE_MESSAGE_RECEIVED`, `VOICE_MESSAGE_SENT`
- Run: `RUN_STARTED`, `RUN_ENDED`, `RUN_TIMEOUT`
- Action: `ACTION_STARTED`, `ACTION_COMPLETED`
- Evaluator: `EVALUATOR_STARTED`, `EVALUATOR_COMPLETED`
- Model: `MODEL_USED`

### TestSuite Interface

```typescript
export interface TestSuite {
  name: string;
  tests: TestCase[];
}

export interface TestCase {
  name: string;
  fn: (runtime: IAgentRuntime) => Promise<void> | void;
}
```

---

## Section 2: Plugin Scaffolding Template

### CLI Scaffolding

```bash
elizaos create my-plugin --type plugin      # Quick plugin (backend only)
elizaos create my-plugin --type plugin      # Full plugin (with frontend, when prompted)
```

### Quick Plugin Directory Structure

```
plugin-my-plugin/
├── src/
│   ├── index.ts          # Plugin manifest & default export
│   ├── actions/
│   │   └── example.ts    # Action implementations
│   ├── providers/
│   │   └── example.ts    # Provider implementations
│   └── types/
│       └── index.ts      # Custom type definitions
├── package.json
├── tsconfig.json
├── tsup.config.ts
└── README.md
```

### package.json

```json
{
  "name": "@myorg/plugin-custom",
  "version": "0.1.0",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "type": "module",
  "scripts": {
    "build": "tsup",
    "dev": "tsup --watch",
    "test": "bun test"
  },
  "dependencies": {
    "@elizaos/core": "workspace:*"
  },
  "devDependencies": {
    "typescript": "^5.x",
    "tsup": "^8.x",
    "@types/node": "^20.x"
  }
}
```

### tsconfig.json

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022"],
    "rootDir": "./src",
    "outDir": "./dist",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "declaration": true,
    "declarationMap": true,
    "sourceMap": true,
    "types": ["node"]
  },
  "include": ["src/**/*"],
  "exclude": ["node_modules", "dist"]
}
```

### tsup.config.ts

```typescript
import { defineConfig } from 'tsup';

export default defineConfig({
  entry: ['src/index.ts'],
  format: ['esm'],
  dts: true,
  splitting: false,
  sourcemap: true,
  clean: true,
  external: ['@elizaos/core'],
});
```

### src/index.ts (Entry Point)

```typescript
import type { Plugin } from '@elizaos/core';
import { myAction } from './actions/myAction';
import { myProvider } from './providers/myProvider';

export const myPlugin: Plugin = {
  name: 'my-custom-plugin',
  description: 'A custom plugin for elizaOS',
  actions: [myAction],
  providers: [myProvider],
  services: [],
  init: async (config, runtime) => {
    console.log('Plugin initialized');
  },
};

export default myPlugin;
```

---

## Section 3: Component Development Guides

### Action Development

**Pattern**: Actions are user-invokable operations. The agent's LLM decides which action to invoke based on `name`, `similes`, `description`, and `examples`.

**Validation**: `validate()` checks prerequisites (API keys, service availability). Return `true` if the action can execute.

**Handler**: Receives runtime, message, state, options, and optional callback. Use callback for streaming/intermediate responses. Return `ActionResult`.

**Complete Action Example (API Integration)**:

```typescript
import {
  Action,
  ActionResult,
  IAgentRuntime,
  Memory,
  State,
  HandlerCallback,
  logger,
} from '@elizaos/core';

export const generateVideoAction: Action = {
  name: 'TEXT_TO_VIDEO',
  similes: ['CREATE_VIDEO', 'MAKE_VIDEO', 'GENERATE_VIDEO'],
  description: 'Generate a video from a text prompt',

  validate: async (runtime: IAgentRuntime, message: Memory, state?: State): Promise<boolean> => {
    const apiKey = runtime.getSetting('FAL_KEY');
    return !!apiKey;
  },

  handler: async (
    runtime: IAgentRuntime,
    message: Memory,
    state?: State,
    options?: Record<string, unknown>,
    callback?: HandlerCallback
  ): Promise<ActionResult> => {
    try {
      const prompt = message.content.text || '';
      // ... API call logic ...

      if (callback) {
        await callback({
          text: `Video generated: ${videoUrl}`,
          actions: ['TEXT_TO_VIDEO'],
          source: message.content.source,
        });
      }

      return {
        success: true,
        text: `Video generated successfully`,
        data: { videoUrl },
      };
    } catch (error) {
      logger.error('Video generation failed:', error);
      return {
        success: false,
        error: error instanceof Error ? error : new Error(String(error)),
      };
    }
  },

  examples: [
    [
      { name: '{{userName}}', content: { text: 'Create a video of a sunset', actions: [] } },
      {
        name: '{{agentName}}',
        content: { text: 'Generating video...', actions: ['TEXT_TO_VIDEO'] },
      },
    ],
  ],
};
```

### Provider Development

**Pattern**: Providers supply context data during state composition. They run concurrently, ordered by `position`.

```typescript
import { Provider, ProviderResult, IAgentRuntime, Memory, State } from '@elizaos/core';

export const weatherProvider: Provider = {
  name: 'WEATHER_PROVIDER',
  description: 'Provides current weather information',
  position: 10, // Lower = runs earlier

  get: async (runtime: IAgentRuntime, message: Memory, state: State): Promise<ProviderResult> => {
    const apiKey = runtime.getSetting('WEATHER_API_KEY');
    if (!apiKey) return { text: '', values: {}, data: {} };

    const weather = await fetchWeather(apiKey);

    return {
      text: `Current weather: ${weather.description}, ${weather.temp}F`,
      values: { temperature: weather.temp, conditions: weather.description },
      data: { weather },
    };
  },
};
```

### Service Development

**Pattern**: Services are long-running background processes. They use static `start()` factory and instance `stop()` cleanup.

```typescript
import { Service, IAgentRuntime, logger } from '@elizaos/core';

export class PollingService extends Service {
  static serviceType = 'polling';
  capabilityDescription = 'Polls external API for updates';

  private intervalId?: NodeJS.Timeout;

  constructor(protected runtime: IAgentRuntime) {
    super(runtime);
  }

  static async start(runtime: IAgentRuntime): Promise<Service> {
    const service = new PollingService(runtime);
    const interval = parseInt(runtime.getSetting('POLL_INTERVAL') || '60000');

    service.intervalId = setInterval(async () => {
      try {
        await service.poll();
      } catch (error) {
        logger.error('Polling failed:', error);
      }
    }, interval);

    return service;
  }

  private async poll() {
    // ... fetch data, emit events, update memory ...
  }

  async stop(): Promise<void> {
    if (this.intervalId) clearInterval(this.intervalId);
    logger.info('Polling service stopped');
  }
}
```

### Evaluator Development

**Pattern**: Evaluators run after message processing. Use `alwaysRun: true` for analytics, or `validate()` for conditional evaluation.

```typescript
import { Evaluator, IAgentRuntime, Memory, State } from '@elizaos/core';

export const sentimentEvaluator: Evaluator = {
  name: 'SENTIMENT_EVALUATOR',
  description: 'Analyzes message sentiment for relationship tracking',
  alwaysRun: true, // Runs after every message

  validate: async (runtime: IAgentRuntime, message: Memory): Promise<boolean> => {
    return !!message.content.text; // Only run on text messages
  },

  handler: async (runtime: IAgentRuntime, message: Memory, state?: State) => {
    const sentiment = await analyzeSentiment(message.content.text);
    await runtime.createMemory(
      {
        entityId: message.entityId,
        roomId: message.roomId,
        content: { text: `Sentiment: ${sentiment.score}`, metadata: { sentiment } },
      },
      'sentiments'
    );
  },

  examples: [],
};
```

### Event Handler Development

```typescript
const myPlugin: Plugin = {
  name: 'my-plugin',
  events: {
    MESSAGE_RECEIVED: [
      async (params: { runtime: IAgentRuntime; message: Memory }) => {
        logger.info(`Message received: ${params.message.content.text}`);
      },
    ],
    WORLD_JOINED: [
      async (params: { runtime: IAgentRuntime; world: any }) => {
        logger.info(`Joined world: ${params.world.name}`);
      },
    ],
  },
};
```

### HTTP Route Development

```typescript
const myPlugin: Plugin = {
  name: 'my-plugin',
  routes: [
    {
      name: 'webhook-handler',
      type: 'POST',
      path: '/webhook',
      handler: async (req, res, runtime) => {
        const apiKey = req.headers['x-api-key'];
        if (apiKey !== runtime.getSetting('WEBHOOK_KEY')) {
          res.status(401).json({ error: 'Unauthorized' });
          return;
        }
        const data = req.body;
        // Process webhook...
        res.json({ status: 'ok' });
      },
    },
    {
      name: 'health-check',
      type: 'GET',
      path: '/health',
      public: true,
      handler: async (_req, res) => {
        res.json({ status: 'healthy' });
      },
    },
    {
      name: 'file-upload',
      type: 'POST',
      path: '/upload',
      isMultipart: true,
      handler: async (req, res, runtime) => {
        const file = req.file;
        // Process file...
        res.json({ uploaded: true });
      },
    },
    {
      name: 'dashboard',
      type: 'STATIC',
      path: '/dashboard',
      filePath: './frontend/dist',
    },
  ],
};
```

---

## Section 4: Testing Guide

### Test Framework

- **Component Tests**: `bun:test` (Bun's built-in) or Vitest
- **E2E Tests**: Custom elizaOS test runner
- **CLI**: `elizaos test --type component` and `elizaos test --type e2e`

### Mock Utilities

```typescript
// src/__tests__/test-utils.ts
import { mock } from 'bun:test';
import type { IAgentRuntime, Memory, State, UUID, Character } from '@elizaos/core';

export type MockRuntime = Partial<IAgentRuntime> & {
  agentId: UUID;
  character: Character;
  getSetting: ReturnType<typeof mock>;
  useModel: ReturnType<typeof mock>;
  composeState: ReturnType<typeof mock>;
  createMemory: ReturnType<typeof mock>;
  getMemories: ReturnType<typeof mock>;
  getService: ReturnType<typeof mock>;
};

export function createMockRuntime(overrides?: Partial<MockRuntime>): MockRuntime {
  return {
    agentId: 'test-agent-123' as UUID,
    character: {
      name: 'TestAgent',
      bio: 'A test agent',
      id: 'test-character' as UUID,
      ...overrides?.character,
    },
    getSetting: mock((key: string) => {
      const settings: Record<string, string> = {
        TEST_API_KEY: 'test-key-123',
      };
      return settings[key];
    }),
    useModel: mock(async () => ({ content: 'Mock response', success: true })),
    composeState: mock(async () => ({ values: {}, data: {}, text: '' })),
    createMemory: mock(async () => ({ id: 'memory-123' })),
    getMemories: mock(async () => []),
    getService: mock(() => null),
    ...overrides,
  };
}

export function createMockMessage(overrides?: Partial<Memory>): Memory {
  return {
    id: 'msg-123' as UUID,
    entityId: 'entity-123' as UUID,
    roomId: 'room-123' as UUID,
    content: { text: 'Test message', ...overrides?.content },
    ...overrides,
  } as Memory;
}

export function createMockState(overrides?: Partial<State>): State {
  return {
    values: { ...overrides?.values },
    data: overrides?.data || {},
    text: overrides?.text || 'Test state',
  } as State;
}
```

### Action Test Example

```typescript
import { describe, expect, it, mock, beforeEach } from 'bun:test';
import { myAction } from '../actions/myAction';
import { createMockRuntime, createMockMessage, createMockState } from './test-utils';

describe('MyAction', () => {
  let mockRuntime: any;
  let mockMessage: any;
  let mockState: any;

  beforeEach(() => {
    mockRuntime = createMockRuntime({ settings: { MY_API_KEY: 'test-key' } });
    mockMessage = createMockMessage({ content: { text: 'Do the thing' } });
    mockState = createMockState();
  });

  it('should validate when API key present', async () => {
    expect(await myAction.validate(mockRuntime, mockMessage, mockState)).toBe(true);
  });

  it('should fail validation without API key', async () => {
    mockRuntime.getSetting = mock(() => undefined);
    expect(await myAction.validate(mockRuntime, mockMessage, mockState)).toBe(false);
  });

  it('should return success on execution', async () => {
    const result = await myAction.handler(mockRuntime, mockMessage, mockState, {}, mock());
    expect(result.success).toBe(true);
  });

  it('should handle errors gracefully', async () => {
    mockRuntime.getService = mock(() => {
      throw new Error('Service unavailable');
    });
    const result = await myAction.handler(mockRuntime, mockMessage, mockState);
    expect(result.success).toBe(false);
    expect(result.error).toBeDefined();
  });
});
```

### Test Commands

```bash
bun test                                # Run all tests
bun test src/__tests__/actions.test.ts  # Run specific test file
bun test --watch                        # Watch mode
bun test --coverage                     # With coverage
elizaos test --type component           # Component tests via CLI
elizaos test --type e2e                 # E2E tests via CLI
```

---

## Section 5: Publishing Checklist

### Pre-Publish Requirements

| Requirement  | Details                                  |
| ------------ | ---------------------------------------- |
| Package name | Must start with `plugin-`                |
| Description  | Must be custom (not placeholder)         |
| Logo image   | `images/logo.jpg`, 400x400px, max 500KB  |
| Banner image | `images/banner.jpg`, 1280x640px, max 1MB |
| Build output | `dist/` directory with compiled JS       |
| README       | `README.md` present                      |

### Publishing Workflow

```bash
# 1. Authenticate
npm whoami && gh auth status

# 2. Build
bun run build

# 3. Validate
elizaos publish --test
elizaos publish --dry-run  # Optional detailed preview

# 4. Publish (first time only)
elizaos publish
# When prompted, create GitHub PAT with scopes: repo, read:org, workflow

# 5. Future updates (never use elizaos publish again)
npm version patch
bun run build
npm publish
git push origin main
```

### Timeline

- **npm package**: Available immediately
- **GitHub repo**: Created immediately
- **Registry approval**: 1-3 business days

### User Installation (after approval)

```bash
elizaos plugins add plugin-my-plugin
```

---

## Section 6: Pattern Catalog

### Pattern 1: API Integration Plugin

**Components**: Action + Provider
**Example**: fal.ai video generation, weather API
**Pattern**: Action validates API key, calls external API, returns result. Provider supplies cached data to context.

### Pattern 2: Platform Connector Plugin

**Components**: Service + Events + Actions
**Example**: Discord, Twitter, Telegram
**Pattern**: Service maintains persistent connection (WebSocket/polling). Events bridge platform messages to ElizaOS. Actions send messages back to platform.

### Pattern 3: LLM Provider Plugin

**Components**: Models + Config
**Example**: OpenAI, Anthropic, Ollama
**Pattern**: Registers model handlers for `TEXT_SMALL`, `TEXT_LARGE`, `TEXT_EMBEDDING` etc. Uses `priority` to become the preferred handler.

### Pattern 4: Data Provider Plugin

**Components**: Provider + Service
**Example**: Knowledge, price feeds, user context
**Pattern**: Service maintains/updates data cache. Provider surfaces cached data into agent context during state composition.

### Pattern 5: Blockchain Integration Plugin

**Components**: Service + Actions + Provider
**Example**: Solana, EVM
**Pattern**: Service manages wallet/connection. Actions execute transactions. Provider shows balances/state.

### Pattern 6: Webhook/API Plugin

**Components**: Routes + Actions
**Example**: GitHub webhooks, custom APIs
**Pattern**: Routes receive external HTTP calls. Actions process and respond to incoming data.

### Pattern 7: Analytics/Logging Plugin

**Components**: Evaluator + Events
**Example**: Sentiment analysis, usage tracking
**Pattern**: Evaluator with `alwaysRun: true` captures metrics after every message. Events track action/model usage.

### Pattern 8: Database Extension Plugin

**Components**: Schema + Adapter
**Example**: SQL plugin, custom storage
**Pattern**: Defines Drizzle ORM schema via `schema` field. Optionally provides full `IDatabaseAdapter` implementation.

### Pattern 9: Background Task Plugin

**Components**: Service + TaskWorker
**Example**: Scheduled reports, data sync
**Pattern**: Service registers TaskWorkers. Tasks created with `queue`/`repeat` tags for scheduled execution.

### Pattern 10: Frontend Dashboard Plugin

**Components**: Routes (STATIC) + Routes (API) + Service
**Example**: Admin dashboard, analytics UI
**Pattern**: STATIC route serves React/Vite frontend. API routes provide data endpoints. Service manages state.

### Decision Framework

| Need                         | Component            |
| ---------------------------- | -------------------- |
| User triggers an operation   | **Action**           |
| Supply context to agent      | **Provider**         |
| Post-message analysis        | **Evaluator**        |
| Long-running background work | **Service**          |
| External HTTP endpoints      | **Route**            |
| React to system events       | **Event Handler**    |
| Custom AI model              | **Model Handler**    |
| Custom database              | **Adapter + Schema** |

### Anti-Patterns

- **Don't put business logic in providers** - Providers should be read-only context suppliers
- **Don't create actions for internal operations** - Actions are user-facing; use services for internal work
- **Don't skip validation** - Always validate API keys and prerequisites in `validate()`
- **Don't block in event handlers** - Event handlers should be fast; queue heavy work
- **Don't import `@elizaos/core` as a bundled dependency** - Always mark it as external
- **Don't use CommonJS** - ElizaOS requires ESM format
- **Don't hardcode secrets** - Use `runtime.getSetting()` for all configuration

---

## Section 7: Plugin Lifecycle & Initialization

### Loading Order

1. Plugins sorted by `priority` (lower numbers load first, default 0)
2. Dependencies resolved via topological sort (circular deps detected and rejected)
3. Plugin loaded: `import(pluginName)` → auto-install via `bun add` if not found
4. Plugin validated: must have `name` and at least one of: `init`, `services`, `providers`, `actions`, `evaluators`, `description`

### Registration Sequence (per plugin)

1. Database adapter (if provided)
2. Actions registered
3. Evaluators registered
4. Providers registered
5. Models registered
6. Routes registered (namespaced: `/${pluginName}${route.path}`)
7. Events registered
8. Services started (async, queued if runtime not ready)
9. `init(config, runtime)` called

### Message Processing Pipeline

```
Message Receipt
  → Memory Storage
    → State Composition (providers called concurrently by position)
      → Action Selection (validate() on each → LLM picks best from valid ones)
        → beforeAction hooks (return false to block)
          → Action Execution (handler called with callback)
            → afterAction hooks
              → Evaluation (evaluators with alwaysRun=true or validate()=true)
                → afterMessage hooks
                  → Response Generation
```

### Middleware Hooks

```typescript
interface Plugin {
  beforeMessage?: (message: Memory, runtime: IAgentRuntime) => Promise<Memory>;
  afterMessage?: (message: Memory, response: Memory, runtime: IAgentRuntime) => Promise<void>;
  beforeAction?: (action: Action, message: Memory, runtime: IAgentRuntime) => Promise<boolean>;
  afterAction?: (action: Action, result: any, runtime: IAgentRuntime) => Promise<void>;
}
```

---

## Section 8: Environment Variables & Settings

### Convention

- Plugin-specific vars use UPPER_SNAKE_CASE with descriptive prefix
- Access via `runtime.getSetting('KEY_NAME')`
- Settings resolve from: env vars → character config `settings` → character `secrets`
- Validate required settings in `validate()` or `init()`

### Config Pattern with Zod Validation

```typescript
import { z } from 'zod';

const configSchema = z.object({
  MY_API_KEY: z.string().min(1, 'API key is required'),
  MY_ENDPOINT: z.string().url().optional(),
});

export const myPlugin: Plugin = {
  name: 'my-plugin',
  config: {
    MY_API_KEY: process.env.MY_API_KEY,
    MY_ENDPOINT: process.env.MY_ENDPOINT,
  },
  async init(config: Record<string, string>) {
    const validated = await configSchema.parseAsync(config);
    for (const [key, value] of Object.entries(validated)) {
      if (value) process.env[key] = value;
    }
  },
};
```

---

## Appendix: Reference Plugin Examples

### plugin-bootstrap

The canonical reference plugin. Contains 13 actions, 16 providers, 1 evaluator, 2 services, and multiple event handlers. See `packages/plugin-bootstrap/src/index.ts` in the ElizaOS repo.

### plugin-starter

The CLI scaffold template. Demonstrates all component types with minimal implementations. See `packages/plugin-starter/src/plugin.ts`.

### Documentation URLs

- Plugin Development: https://docs.elizaos.ai/plugins/development
- Plugin Components: https://docs.elizaos.ai/plugins/components
- Plugin Architecture: https://docs.elizaos.ai/plugins/architecture
- Plugin Patterns: https://docs.elizaos.ai/plugins/patterns
- Create a Plugin: https://docs.elizaos.ai/guides/create-a-plugin
- Publish a Plugin: https://docs.elizaos.ai/guides/publish-a-plugin
- Registry Overview: https://docs.elizaos.ai/plugin-registry/overview
- Types Reference: https://docs.elizaos.ai/runtime/types-reference
- Runtime Core: https://docs.elizaos.ai/runtime/core
- Events: https://docs.elizaos.ai/runtime/events
- Full Docs: https://docs.elizaos.ai/llms-full.txt
