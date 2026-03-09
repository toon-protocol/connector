# ElizaOS Complete Plugin System Research

> Extracted from https://docs.elizaos.ai/llms-full.txt and all associated documentation
> pages (plugins/architecture, plugins/components, plugins/patterns, plugins/schemas,
> plugins/webhooks-and-routes, runtime/core, runtime/events, runtime/memory, runtime/models,
> runtime/providers, runtime/services, runtime/types-reference, guides/create-a-plugin,
> guides/publish-a-plugin, guides/test-a-project, cli-reference/test).
>
> Date: 2026-02-09

---

## Table of Contents

1. [Complete Plugin Interface](#1-complete-plugin-interface)
2. [Action Interface](#2-action-interface)
3. [Provider Interface](#3-provider-interface)
4. [Evaluator Interface](#4-evaluator-interface)
5. [Service Abstract Class](#5-service-abstract-class)
6. [ModelHandler Type](#6-modelhandler-type)
7. [IAgentRuntime Interface](#7-iagentruntime-interface)
8. [Memory, Content, State Types](#8-memory-content-state-types)
9. [Event System](#9-event-system)
10. [HTTP Routes](#10-http-routes)
11. [Task System](#11-task-system)
12. [Plugin Lifecycle](#12-plugin-lifecycle)
13. [Testing Patterns](#13-testing-patterns)
14. [Publishing and Registry](#14-publishing-and-registry)
15. [Database Schemas for Plugins](#15-database-schemas-for-plugins)
16. [Supporting Types](#16-supporting-types)

---

## 1. Complete Plugin Interface

```typescript
export type Plugin = {
  name: string;
  description?: string;
  priority?: number;
  dependencies?: string[];
  testDependencies?: string[];
  config?: Record<string, any>;

  // Components
  actions?: Action[];
  evaluators?: Evaluator[];
  providers?: Provider[];
  services?: Service[];

  // Extensions
  adapter?: IDatabaseAdapter;
  models?: Record<string, ModelHandler>;
  routes?: Route[];
  events?: PluginEvents;
  schema?: Record<string, any>; // Drizzle ORM schema for plugin tables

  // Lifecycle hooks
  init?: (config: any, runtime: IAgentRuntime) => Promise<void>;
  start?: (runtime: IAgentRuntime) => Promise<void>;
  stop?: (runtime: IAgentRuntime) => Promise<void>;

  // Message middleware hooks
  beforeMessage?: (message: Memory, runtime: IAgentRuntime) => Promise<Memory>;
  afterMessage?: (message: Memory, response: Memory, runtime: IAgentRuntime) => Promise<void>;

  // Action middleware hooks
  beforeAction?: (action: Action, message: Memory, runtime: IAgentRuntime) => Promise<boolean>;
  afterAction?: (action: Action, result: any, runtime: IAgentRuntime) => Promise<void>;
};
```

### Plugin Loading Priority Levels

| Priority | Category        |
| -------- | --------------- |
| -100     | Databases       |
| -50      | Model Providers |
| 0        | Core Plugins    |
| 50       | Features        |
| 100      | Platforms       |

### Component Registration Sequence

The runtime registers plugin components in this strict order:

1. Database adapter
2. Actions
3. Evaluators
4. Providers
5. Models
6. Routes
7. Events
8. Services (queued if runtime not ready)

### Minimal Plugin Export

```typescript
import type { Plugin } from '@elizaos/core';

export const myPlugin: Plugin = {
  name: 'my-custom-plugin',
  description: 'A custom plugin for elizaOS',
  actions: [myAction],
  providers: [myProvider],
  services: [MyService],
  init: async (config, runtime) => {
    console.log('Plugin initialized');
  },
};

export default myPlugin;
```

---

## 2. Action Interface

```typescript
interface Action {
  name: string;
  description: string;
  similes?: string[];
  examples?: Array<
    Array<{
      name: string; // "{{user}}" or "{{agent}}" or literal name
      content: {
        text: string;
        actions?: string[]; // e.g. ['TEXT_TO_VIDEO']
      };
    }>
  >;
  validate: (runtime: IAgentRuntime, message: Memory) => Promise<boolean>;
  handler: (
    runtime: IAgentRuntime,
    message: Memory,
    state: State | undefined,
    options?: HandlerOptions,
    callback?: HandlerCallback
  ) => Promise<ActionResult>;
}
```

### ActionResult

```typescript
export interface ActionResult {
  success: boolean;
  text?: string;
  values?: Record<string, unknown>; // Key-value pairs merged into state
  data?: unknown; // Raw data payload
  error?: Error;
}
```

### HandlerCallback

```typescript
export type HandlerCallback = (response: Content, files?: any) => Promise<Memory[]>;
```

### HandlerOptions

```typescript
interface HandlerOptions {
  actionContext?: ActionContext;
  actionPlan?: {
    totalSteps: number;
    currentStep: number;
    steps: Array<{
      action: string;
      status: 'pending' | 'completed' | 'failed';
      result?: ActionResult;
      error?: string;
    }>;
    thought: string;
  };
  [key: string]: unknown;
}
```

### ActionContext

```typescript
export interface ActionContext {
  previousResults: ActionResult[];
  getPreviousResult?: (actionName: string) => ActionResult | undefined;
  currentStep: number;
  totalSteps: number;
}
```

### How validate() Works

The runtime calls `action.validate?.(runtime, message)` on each registered action to filter which actions can handle the current message. Only actions returning `true` are candidates. The runtime then uses an LLM to select the best action from the valid candidates.

### How similes Work

`similes` are alternative trigger names for the action. They allow the LLM to match user intent to the action even when the user uses different phrasing. For example, an action named `TEXT_TO_VIDEO` might have similes `['CREATE_VIDEO', 'MAKE_VIDEO', 'GENERATE_VIDEO', 'VIDEO_FROM_TEXT']`.

### examples Array Format

```typescript
examples: [
  [
    // Conversation 1 - array of messages
    { name: '{{user}}', content: { text: 'Create video: dolphins jumping' } },
    { name: '{{agent}}', content: { text: 'Creating video!', actions: ['TEXT_TO_VIDEO'] } },
  ],
  [
    // Conversation 2
    { name: '{{user}}', content: { text: 'Make a video of a sunset' } },
    {
      name: '{{agent}}',
      content: { text: 'Generating your video now.', actions: ['TEXT_TO_VIDEO'] },
    },
  ],
];
```

### Complete Action Example with Callback Usage

```typescript
const generateVideoAction: Action = {
  name: 'TEXT_TO_VIDEO',
  similes: ['CREATE_VIDEO', 'MAKE_VIDEO', 'GENERATE_VIDEO', 'VIDEO_FROM_TEXT'],
  description: 'Generate a video from text using MiniMax Hailuo-02',
  validate: async (runtime: IAgentRuntime, message: Memory) => {
    const falKey = runtime.getSetting('FAL_KEY');
    if (!falKey) {
      logger.error('FAL_KEY not found in environment variables');
      return false;
    }
    return true;
  },
  handler: async (
    runtime: IAgentRuntime,
    message: Memory,
    state: State | undefined,
    options: any,
    callback?: HandlerCallback
  ): Promise<ActionResult> => {
    try {
      // Send immediate feedback via callback
      await callback?.({
        text: 'Starting to process your request...',
        source: message.content.source,
      });

      const result = await performOperation();

      // Send completion feedback
      await callback?.({
        text: `Created: ${result.title}\nView: ${result.url}`,
        source: message.content.source,
      });

      return {
        success: true,
        text: `Created: ${result.title}`,
        data: { id: result.id, url: result.url },
      };
    } catch (error) {
      await callback?.({
        text: `Failed: ${error.message}`,
        source: message.content.source,
      });
      return {
        success: false,
        text: `Failed: ${error.message}`,
        error: error instanceof Error ? error : new Error(String(error)),
      };
    }
  },
  examples: [
    [
      { name: '{{user}}', content: { text: 'Create video: dolphins jumping' } },
      { name: '{{agent}}', content: { text: 'Creating video!', actions: ['TEXT_TO_VIDEO'] } },
    ],
  ],
};
```

### Action Error Recovery with Retry Pattern

```typescript
export const apiAction: Action = {
  name: 'API_CALL',
  handler: async (runtime, message, state, options, callback) => {
    const maxRetries = 3;
    let lastError: Error | null = null;

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
      try {
        await callback?.({ text: `Attempt ${attempt}/${maxRetries}...` });
        const result = await callExternalAPI({
          endpoint: state.endpoint,
          data: state.data,
          timeout: 5000,
        });
        return { success: true, text: 'API call successful', data: result };
      } catch (error) {
        lastError = error as Error;
        if (attempt < maxRetries) {
          await callback?.({ text: `Attempt ${attempt} failed, retrying...` });
          await new Promise((r) => setTimeout(r, 1000 * attempt));
        }
      }
    }
    return {
      success: false,
      text: `API call failed after ${maxRetries} attempts`,
      error: lastError,
    };
  },
};
```

### Accessing Previous Action Results (Chaining)

```typescript
async handler(runtime, message, state, options, callback): Promise<ActionResult> {
  const context = options?.context as ActionContext;
  const previousResult = context?.getPreviousResult?.('CREATE_LINEAR_ISSUE');

  if (previousResult?.data?.issueId) {
    const issueId = previousResult.data.issueId;
    // Continue with logic using previous result
  }
}
```

---

## 3. Provider Interface

```typescript
interface Provider {
  name: string;
  description?: string;
  dynamic?: boolean; // Only executed when explicitly requested (not in default state)
  private?: boolean; // Internal-only, not included in default state assembly
  position?: number; // Execution order (lower runs first)

  get: (runtime: IAgentRuntime, message: Memory, state?: State) => Promise<ProviderResult>;
}

interface ProviderResult {
  values: Record<string, any>; // Key-value pairs merged into state for templates
  data: Record<string, any>; // Structured data accessible via state.data.providers[name]
  text: string; // Textual context injected into LLM prompt
}
```

### Provider Categories

- **Standard Providers**: Run automatically in default state composition.
- **Dynamic Providers** (`dynamic: true`): Execute only upon explicit request.
- **Private Providers** (`private: true`): Internal only, unavailable in standard state assembly.

### Built-in Providers

| Provider        | Dynamic | Position | Purpose                              |
| --------------- | ------- | -------- | ------------------------------------ |
| ACTIONS         | No      | -1       | Lists available actions              |
| CHARACTER       | No      | Default  | Agent personality and behavior       |
| RECENT_MESSAGES | No      | 100      | Conversation history                 |
| ACTION_STATE    | No      | 150      | Chained action execution state       |
| FACTS           | Yes     | Default  | Context-relevant stored knowledge    |
| RELATIONSHIPS   | Yes     | Default  | Social graph and interaction history |
| TIME            | No      | Default  | Current UTC timestamp                |
| CAPABILITIES    | No      | Default  | Service capabilities                 |

### How Providers Inject Data into Context

Providers are called during `composeState()`. The runtime:

1. Selects relevant providers (filters by dynamic/private flags)
2. Executes them concurrently (ordered by `position`)
3. Aggregates results into the State object
4. Caches composed state

```typescript
async composeState(
  message: Memory,
  includeList: string[] | null = null,
  onlyInclude: boolean = false,
  skipCache: boolean = false
): Promise<State>
```

Later-positioned providers can access earlier results through the `state` parameter:

```typescript
const dependentProvider: Provider = {
  name: 'DEPENDENT',
  position: 200,
  get: async (runtime, message, state) => {
    const characterData = state?.data?.providers?.CHARACTER?.data;
    // use characterData...
    return { values: {}, data: {}, text: '' };
  },
};
```

### Custom Provider Example

```typescript
const customDataProvider: Provider = {
  name: 'CUSTOM_DATA',
  description: 'Custom data from external source',
  dynamic: true,
  position: 150,
  get: async (runtime, message, state) => {
    try {
      const customData = await runtime.getService('customService')?.getData();
      if (!customData) {
        return { values: {}, data: {}, text: '' };
      }
      return {
        values: { customData: customData.summary },
        data: { customData },
        text: `Custom data: ${customData.summary}`,
      };
    } catch (error) {
      runtime.logger.error('Error in custom provider:', error);
      return { values: {}, data: {}, text: '' };
    }
  },
};
```

---

## 4. Evaluator Interface

```typescript
interface Evaluator {
  name: string;
  description: string;
  similes?: string[];
  alwaysRun?: boolean;
  examples: Array<{
    prompt: string;
    messages: Memory[];
    outcome: string;
  }>;
  validate: (runtime: IAgentRuntime, message: Memory) => Promise<boolean>;
  handler: (runtime: IAgentRuntime, message: Memory, state: State) => Promise<any>;
}
```

### alwaysRun vs validate()

- `alwaysRun: true`: Evaluator executes after every message processing, regardless of content.
- `validate()`: Called to determine if the evaluator should run for this particular message. Returns boolean.
- Evaluators run post-processing through `EvaluatorOrchestrator.runEvaluators()` after message processing completes, receiving the original message, response, and state.

### What Evaluators Can Do

- Extract and store facts/knowledge from conversations
- Log analytics and metrics
- Modify agent state based on conversation patterns
- Trigger follow-up actions
- Assess response quality

---

## 5. Service Abstract Class

```typescript
abstract class Service {
  static serviceType: ServiceType; // Unique identifier string
  capabilityDescription: string; // Human-readable description
  config?: ServiceConfig; // Optional configuration

  constructor(runtime?: IAgentRuntime);

  static async start(runtime: IAgentRuntime): Promise<Service>; // Static factory
  abstract stop(): Promise<void>; // Instance cleanup
}
```

### ServiceType Registry

Core types in `@elizaos/core`:

```typescript
const ServiceType = {
  TASK: 'task',
  DATABASE: 'database',
  // ... additional core types
} as const;
```

Plugins extend via module augmentation:

```typescript
declare module '@elizaos/core' {
  interface ServiceTypeRegistry {
    DISCORD: 'discord';
    TELEGRAM: 'telegram';
    TWITTER: 'twitter';
    SEARCH: 'search';
    IMAGE_GENERATION: 'image_generation';
    TRANSCRIPTION: 'transcription';
  }
}
```

### Known Service Types

| Type            | Purpose                   | Plugin                     |
| --------------- | ------------------------- | -------------------------- |
| TASK            | Background job execution  | @elizaos/core              |
| DATABASE        | Database operations       | @elizaos/core              |
| MESSAGE_SERVICE | Real-time communication   | @elizaos/core              |
| TRANSCRIPTION   | Audio-to-text             | @elizaos/plugin-openai     |
| VIDEO           | Video processing          | @elizaos/plugin-video      |
| BROWSER         | Web automation            | @elizaos/plugin-browser    |
| PDF             | PDF handling              | @elizaos/plugin-pdf        |
| REMOTE_FILES    | Cloud storage (S3)        | @elizaos/plugin-s3         |
| WEB_SEARCH      | Search functionality      | @elizaos/plugin-web-search |
| EMAIL           | Email operations          | @elizaos/plugin-email      |
| WALLET          | Cryptocurrency management | @elizaos/plugin-evm        |

### Service Lifecycle

1. **Registration**: Service registered during plugin initialization
2. **Queuing**: Queued for startup
3. **Initialization**: Runtime prepares environment
4. **Start**: `start()` static method executes (factory returns instance)
5. **Running**: Active processing phase
6. **Stop**: Graceful shutdown initiated via `stop()`
7. **Cleanup**: Resources released

### Accessing Services

```typescript
// Get service by type
const discord = runtime.getService('discord');

// Type-safe access
const searchService = runtime.getService<SearchService>('search');
const results = await searchService.search('elizaOS');

// Check availability
runtime.hasService('discord');

// Get all services
const services = runtime.getAllServices(); // Map<ServiceTypeName, Service[]>

// Get registered types
const types = runtime.getRegisteredServiceTypes();
```

### Service-to-Service Communication

```typescript
class NotificationService extends Service {
  static serviceType = 'notification' as const;
  capabilityDescription = 'Cross-platform notifications';

  async notify(message: string) {
    const discord = this.runtime.getService('discord');
    if (discord) await discord.sendMessage(channelId, message);

    const telegram = this.runtime.getService('telegram');
    if (telegram) await telegram.sendMessage(chatId, message);
  }
}
```

### Platform Integration Service Example

```typescript
class DiscordService extends Service {
  static serviceType = 'discord' as const;
  capabilityDescription = 'Discord bot integration';
  private client: Discord.Client;

  constructor(private runtime: IAgentRuntime) {
    super(runtime);
  }

  static async start(runtime: IAgentRuntime): Promise<Service> {
    const service = new DiscordService(runtime);
    await service.initialize();
    return service;
  }

  private async initialize() {
    const token = this.runtime.getSetting('DISCORD_API_TOKEN');
    if (!token) {
      this.runtime.logger.warn('Discord token not found');
      return;
    }
    this.client = new Discord.Client({
      intents: [
        /* ... */
      ],
    });
    this.setupEventHandlers();
    await this.client.login(token);
  }

  private setupEventHandlers() {
    this.client.on('messageCreate', async (message) => {
      const memory = await this.convertToMemory(message);
      await this.runtime.processActions(memory, []);
    });
  }

  async stop() {
    await this.client?.destroy();
  }
}
```

### Error Handling: Retry with Exponential Backoff

```typescript
private async connectWithRetry(maxRetries = 3) {
  for (let i = 0; i < maxRetries; i++) {
    try {
      await this.client.connect();
      this.runtime.logger.info('Service connected successfully');
      return;
    } catch (error) {
      this.runtime.logger.error(`Connection attempt ${i + 1} failed:`, error);
      if (i < maxRetries - 1) {
        const delay = Math.pow(2, i) * 1000;
        await new Promise(resolve => setTimeout(resolve, delay));
      } else {
        throw error;
      }
    }
  }
}
```

---

## 6. ModelHandler Type

```typescript
type ModelHandler<T = any, R = any> = (runtime: IAgentRuntime, params: T) => Promise<R>;

interface ModelRegistration {
  type: ModelTypeName;
  handler: ModelHandler;
  provider: string;
  priority: number; // Higher priority preferred
}
```

### ModelTypeName Values

| Constant         | String Value         |
| ---------------- | -------------------- |
| TEXT_SMALL       | `'text:small'`       |
| TEXT_MEDIUM      | `'text:medium'`      |
| TEXT_LARGE       | `'text:large'`       |
| TEXT_EMBEDDING   | `'text:embedding'`   |
| IMAGE_GENERATION | `'image:generation'` |
| IMAGE_ANALYSIS   | `'image:analysis'`   |
| SPEECH_TO_TEXT   | `'speech:to:text'`   |
| TEXT_TO_SPEECH   | `'text:to:speech'`   |
| CODE_GENERATION  | `'code:generation'`  |
| CLASSIFICATION   | `'classification'`   |

### Model Registration and Usage

```typescript
// Registration (typically in a Service's initialize method)
runtime.registerModel(
  ModelType.TEXT_LARGE,
  this.handleTextGeneration.bind(this),
  'openai',
  100 // priority
);

// Usage
const result = await runtime.useModel(ModelType.TEXT_LARGE, {
  prompt: '...',
  temperature: 0.7,
});

// With preferred provider
const result = await runtime.useModel(ModelType.TEXT_LARGE, params, 'anthropic');
```

### Provider Selection Strategy

The runtime prioritizes providers by:

1. Specified preference (if `preferredProvider` argument given)
2. Registered priority (higher number wins)
3. Sequential fallback (try next provider on failure)

---

## 7. IAgentRuntime Interface

```typescript
interface IAgentRuntime {
  // Identity
  readonly agentId: UUID;
  readonly character: Character;

  // Registered components
  readonly actions: Action[];
  readonly evaluators: Evaluator[];
  readonly providers: Provider[];
  readonly plugins: Plugin[];
  services: Service[];

  // Initialization
  initialize(): Promise<void>;

  // Message processing pipeline
  processActions(message: Memory, responses: Memory[], state?: State): Promise<void>;
  composeState(
    message: Memory,
    includeList?: string[] | null,
    onlyInclude?: boolean,
    skipCache?: boolean
  ): Promise<State>;
  evaluate(message: Memory, state?: State): Promise<void>;

  // Component registration
  registerAction(action: Action): void;
  registerProvider(provider: Provider): void;
  registerEvaluator(evaluator: Evaluator): void;
  registerService(ServiceClass: typeof Service): Promise<Service>;

  // Service access
  getService<T extends Service>(name: ServiceTypeName): T | null;
  getAllServices(): Map<ServiceTypeName, Service[]>;
  getRegisteredServiceTypes(): string[];
  getServicesByType<T>(): T[];
  getServiceLoadPromise(): Promise<void>;
  hasService(name: string): boolean;

  // Model operations
  useModel<T>(type: ModelTypeName, params: any, preferredProvider?: string): Promise<T>;
  registerModel(
    type: ModelTypeName,
    handler: ModelHandler,
    provider?: string,
    priority?: number
  ): void;
  getModel(type: ModelTypeName, provider?: string): ModelHandler;

  // Memory operations
  createMemory(memory: Memory, tableName: string, unique?: boolean): Promise<UUID>;
  getMemories(params: MemoryRetrievalOptions): Promise<Memory[]>;
  searchMemories(params: MemorySearchOptions): Promise<Memory[]>;
  getMemoryById(id: UUID): Promise<Memory | null>;
  deleteMemory(id: UUID): Promise<void>;

  // Embedding
  queueEmbeddingGeneration(memory: Memory, priority?: string): void;
  addEmbeddingToMemory(memory: Memory): Promise<Memory>;

  // Entity management
  createEntity(entity: any): Promise<UUID>;
  updateEntity(entity: any): Promise<void>;
  getEntity(id: UUID): Promise<any>;

  // Relationship management
  createRelationship(relationship: any): Promise<UUID>;
  getRelationships(params: any): Promise<any[]>;

  // Knowledge
  createFact(fact: any): Promise<UUID>;
  searchFacts(params: any): Promise<any[]>;

  // Run tracking
  createRunId(): UUID;
  startRun(roomId?: UUID): void;
  endRun(): void;
  getCurrentRunId(): UUID | null;
  getActionResults(messageId: UUID): ActionResult[];

  // Connection management
  ensureConnections(params: any): Promise<void>;
  ensureConnection(params: any): Promise<void>;

  // World management
  updateWorld(world: any): Promise<void>;

  // Messaging
  registerSendHandler(handler: any): void;
  sendMessageToTarget(message: any, target: TargetInfo): Promise<void>;

  // Settings
  getSetting(key: string): string | undefined;

  // Event system
  emit(event: string, data: any): void;
  on(event: string, callback: Function): void;
  once(event: string, callback: Function): void;
  off(event: string, callback: Function): void;
  removeAllListeners(event?: string): void;

  // Tasks
  createTask(task: Task): Promise<UUID>;
  getTasks(params: any): Promise<Task[]>;
  getTask(id: UUID): Promise<Task | null>;
  updateTask(id: UUID, updates: any): Promise<void>;
  deleteTask(id: UUID): Promise<void>;
  registerTaskWorker(worker: TaskWorker): void;
  getTaskWorker(name: string): TaskWorker | undefined;

  // Lifecycle
  stop(): Promise<void>;

  // Type guard
  hasElizaOS(): boolean;

  // Logging
  logger: Logger;

  // Database
  databaseAdapter: IDatabaseAdapter;

  // Cache
  cacheManager: CacheManager;
}
```

### IElizaOS (Top-level orchestrator)

```typescript
interface IElizaOS {
  handleMessage(
    agentId: UUID | IAgentRuntime,
    message: Message,
    options?: HandleMessageOptions
  ): Promise<HandleMessageResult>;

  handleMessages(
    messages: Array<{ agentId: UUID; message: Message }>
  ): Promise<HandleMessageResult[]>;

  getAgent(agentId: UUID): IAgentRuntime | undefined;
  getAgents(): Map<UUID, IAgentRuntime>;
}

interface HandleMessageOptions {
  onResponse?: (content: Content) => Promise<void>;
  onStreamChunk?: (chunk: string, messageId: UUID) => Promise<void>;
  onError?: (error: Error) => Promise<void>;
  onComplete?: () => Promise<void>;
  skipEvaluators?: boolean;
  skipActions?: boolean;
}

interface HandleMessageResult {
  messageId: UUID;
  userMessage: Memory;
  processing?: {
    text: string;
    actions?: ActionResult[];
    evaluations?: unknown[];
  };
  error?: Error;
}
```

---

## 8. Memory, Content, State Types

### Memory

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
```

### Content

```typescript
interface Content {
  text?: string;
  actions?: string[];
  inReplyTo?: UUID;
  source?: string;
  metadata?: any;
  [key: string]: any;
}
```

### MemoryMetadata

```typescript
interface MemoryMetadata {
  [key: string]: unknown;
  type?: string;
  importance?: number;
  lastAccessed?: number;
  accessCount?: number;
}
```

### MemoryType Enum

```typescript
enum MemoryType {
  MESSAGE = 'message',
  FACT = 'fact',
  DOCUMENT = 'document',
  RELATIONSHIP = 'relationship',
  GOAL = 'goal',
  TASK = 'task',
  ACTION = 'action',
}
```

### State

```typescript
interface State {
  values: Record<string, any>;
  data: StateData;
  text: string;
  [key: string]: unknown;
}
```

### StateData

```typescript
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

### ActionPlan

```typescript
interface ActionPlan {
  thought: string;
  totalSteps: number;
  currentStep: number;
  steps: ActionPlanStep[];
}

interface ActionPlanStep {
  action: string;
  status: 'pending' | 'completed' | 'failed';
  error?: string;
  result?: ActionResult;
}
```

### Memory Retrieval and Search

```typescript
interface MemoryRetrievalOptions {
  roomId?: UUID;
  entityId?: UUID;
  limit?: number;
  before?: number;
  after?: number;
  types?: MemoryType[];
}

interface MemorySearchOptions extends MemoryRetrievalOptions {
  query: string;
  threshold?: number;
  embedding?: number[];
}

interface EmbeddingSearchResult {
  memory: Memory;
  similarity: number;
}
```

---

## 9. Event System

### Event Type Strings

```
// World Events
'world:joined'
'world:connected'
'world:left'

// Entity Events
'entity:joined'
'entity:left'
'entity:updated'

// Room Events
'room:joined'
'room:left'
'room:updated'

// Message Events
'message:received'
'message:sent'
'message:deleted'
'message:updated'

// Voice Events
'voice:message:received'
'voice:message:sent'
'voice:started'
'voice:ended'

// Run Events
'run:started'
'run:completed'
'run:failed'
'run:timeout'

// Action Events
'action:started'
'action:completed'
'action:failed'

// Evaluator Events
'evaluator:started'
'evaluator:completed'
'evaluator:failed'

// Model Events
'model:used'
'model:failed'

// Service Events
'service:started'
'service:stopped'
'service:error'
```

### PluginEvents Type

```typescript
export type PluginEvents = {
  [K in keyof EventPayloadMap]?: EventHandler<K>[];
} & {
  [key: string]: ((params: any) => Promise<any>)[];
};
```

### Plugin Event Registration

```typescript
const myPlugin: Plugin = {
  name: 'my-plugin',
  events: {
    [EventType.MESSAGE_RECEIVED]: [handleMessageReceived, logMessage],
    [EventType.ACTION_COMPLETED]: [processActionResult],
    [EventType.RUN_COMPLETED]: [cleanupRun],
  },
};
```

### Event Payload Interfaces

```typescript
interface RunEventPayload extends EventPayload {
  runId: UUID;
  agentId: UUID;
  roomId?: UUID;
  startTime: number;
  endTime?: number;
  status: 'started' | 'completed' | 'failed' | 'timeout';
  error?: string;
}

interface ActionEventPayload extends EventPayload {
  action: string;
  content: Content;
  roomId: UUID;
  messageId: UUID;
  status: 'started' | 'completed' | 'failed';
  result?: ActionResult;
  error?: string;
  duration?: number;
}

interface EvaluatorEventPayload extends EventPayload {
  evaluator: string;
  roomId: UUID;
  messageId: UUID;
  status: 'started' | 'completed' | 'failed';
  result?: unknown;
  duration?: number;
}

interface ModelEventPayload extends EventPayload {
  modelType: ModelTypeName;
  provider: string;
  model: string;
  tokens?: TokenUsage;
  duration?: number;
  cached?: boolean;
}

interface EmbeddingGenerationPayload extends EventPayload {
  memoryId: UUID;
  priority: 'high' | 'normal' | 'low';
  status: 'requested' | 'completed' | 'failed';
  error?: string;
}

interface MessageStreamChunkPayload {
  messageId: UUID;
  chunk: string;
  index: number;
  channelId: string;
  agentId: UUID;
}
```

### Primary Event Payloads (Conceptual)

| Event Category | Payload Fields                                                |
| -------------- | ------------------------------------------------------------- |
| Message        | runtime, message, room, user, callback                        |
| World          | runtime, world, metadata                                      |
| Entity         | runtime, entity, action, changes                              |
| Action         | runtime, action, message, state, result, error                |
| Model          | runtime, modelType, provider, params, result, error, duration |

### Runtime Event Methods

```typescript
runtime.emit(EventType.MESSAGE_RECEIVED, payload);
runtime.on('message:received', handler);
runtime.once('message:received', handler);
runtime.off('message:received', handler);
runtime.removeAllListeners('message:received');
```

Custom events are supported through module declaration extension.

---

## 10. HTTP Routes

### Route Type

```typescript
export type Route = {
  type: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE' | 'STATIC';
  path: string;
  handler?: (req: RouteRequest, res: RouteResponse, runtime: IAgentRuntime) => Promise<void>;
  filePath?: string; // For STATIC type routes serving frontend assets
  public?: boolean; // Skip authentication
  name?: string; // Route identifier
  isMultipart?: boolean; // Accept file uploads via req.file
};
```

### Route Authentication

```typescript
// In route handler:
const apiKey = req.headers['x-api-key'];
const expectedKey = runtime.getSetting('WEBHOOK_API_KEY');

if (apiKey !== expectedKey) {
  res.status(401).json({ error: 'Unauthorized' });
  return;
}
```

Service-specific webhooks (like GitHub) can verify signatures from request headers.

### Plugin Route Registration

```typescript
const myPlugin: Plugin = {
  name: 'my-plugin',
  routes: [
    {
      type: 'POST',
      path: '/webhook/github',
      handler: async (req, res, runtime) => {
        // Handle webhook...
        res.json({ status: 'ok' });
      },
    },
    {
      type: 'GET',
      path: '/api/data',
      public: true,
      handler: async (req, res, runtime) => {
        res.json({ data: '...' });
      },
    },
    {
      type: 'STATIC',
      path: '/dashboard',
      filePath: './frontend/dist',
    },
  ],
};
```

### Sending Messages as an Agent (from Routes)

```typescript
// POST to /api/messaging/submit
const serverUrl = runtime.getSetting('SERVER_URL');
await fetch(`${serverUrl}/api/messaging/submit`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    channel_id: targetChannel,
    author_id: runtime.agentId,
    content: 'Message text',
    metadata: { source: 'webhook' },
    source_type: 'agent_response',
  }),
});
```

### Best Practices

- Validate all inputs
- Implement authentication for sensitive endpoints
- Verify service signatures
- Return appropriate HTTP status codes
- Log errors appropriately
- Process asynchronously when possible
- Maintain response times under 5 seconds

---

## 11. Task System

### Task Interface

```typescript
interface Task {
  id?: UUID;
  name: string; // Must match registered TaskWorker.name
  description: string;
  roomId?: UUID;
  worldId?: UUID;
  entityId?: UUID;
  tags: string[]; // Control tags: 'queue', 'repeat', 'immediate'
  metadata?: TaskMetadata;
  updatedAt?: number;
}

interface TaskMetadata {
  updateInterval?: number; // For recurring tasks (milliseconds)
  options?: { name: string; description: string }[];
  [key: string]: unknown;
}
```

### TaskWorker Interface

```typescript
interface TaskWorker {
  name: string;

  execute: (runtime: IAgentRuntime, options: Record<string, unknown>, task: Task) => Promise<void>;

  validate?: (runtime: IAgentRuntime, message: Memory, state?: State) => Promise<boolean>;
}
```

### Task Tags

| Tag       | Behavior                             |
| --------- | ------------------------------------ |
| queue     | Task eligible for execution          |
| repeat    | Persists after execution (recurring) |
| immediate | Execute as soon as possible          |
| Custom    | For filtering and organization       |

### Task Usage

```typescript
// Register worker
runtime.registerTaskWorker({
  name: 'SEND_DAILY_REPORT',
  execute: async (runtime, options, task) => {
    const report = await generateReport(runtime);
    await sendReport(report, options.recipientId);
  },
});

// Create one-time task
await runtime.createTask({
  name: 'SEND_DAILY_REPORT',
  description: 'Send daily analytics report',
  tags: ['queue'],
  metadata: { recipientId: 'user-123' },
});

// Create recurring task
await runtime.createTask({
  name: 'SYNC_EXTERNAL_DATA',
  description: 'Sync data from external API every hour',
  tags: ['queue', 'repeat'],
  metadata: { updateInterval: 1000 * 60 * 60 },
});
```

---

## 12. Plugin Lifecycle

### Loading Order

1. Plugins sorted by `priority` (lower numbers load first)
2. Dependencies resolved (runtime ensures dependencies loaded before dependent plugins)
3. For each plugin, components registered in strict order:
   adapter -> actions -> evaluators -> providers -> models -> routes -> events -> services

### Registration Sequence

```typescript
async registerPlugin(plugin: Plugin) {
  // 1. Register database adapter
  if (plugin.adapter) this.registerAdapter(plugin.adapter);

  // 2. Register actions
  plugin.actions?.forEach(a => this.registerAction(a));

  // 3. Register evaluators
  plugin.evaluators?.forEach(e => this.registerEvaluator(e));

  // 4. Register providers
  plugin.providers?.forEach(p => this.registerProvider(p));

  // 5. Register models
  if (plugin.models) {
    for (const [type, handler] of Object.entries(plugin.models)) {
      this.registerModel(type, handler, plugin.name, plugin.priority);
    }
  }

  // 6. Register routes
  // 7. Register events
  // 8. Register services (queued if runtime not ready)
  plugin.services?.forEach(s => this.registerService(s));

  // 9. Call init hook
  await plugin.init?.(plugin.config || {}, this);
}
```

### Message Processing Pipeline

```
Message Receipt
  -> Memory Storage
    -> State Composition (providers called concurrently by position)
      -> Action Selection (validate() on each -> LLM picks best from valid)
        -> beforeAction hooks (return false to block)
          -> Action Execution (handler called with callback)
            -> afterAction hooks
              -> Evaluation (evaluators with alwaysRun=true or validate()=true)
                -> afterMessage hooks
                  -> Response Generation
```

### Runtime Initialization Sequence

1. Runtime instantiation with character and configuration
2. Character personality loading
3. Plugin registration in priority order
4. Background service startup
5. Ready state for message processing

### Middleware Hook Behaviors

- `beforeMessage`: Modify/validate messages before processing. Returns modified Memory.
- `afterMessage`: Log, analyze, or store conversations post-response.
- `beforeAction`: Return `false` to block action execution, `true` to allow.
- `afterAction`: Process results after action completes.

---

## 13. Testing Patterns

### Test Framework

- **Component Tests**: Vitest framework in `__tests__/` or `src/__tests__/` directory
- **E2E Tests**: Custom elizaOS test runner in `e2e/` directory

### CLI Commands

```bash
elizaos test                         # All tests
elizaos test --type component        # Component only
elizaos test --type e2e              # E2E only
elizaos test --name "multi-agent"    # Filter by name (case sensitive)
elizaos test --skip-build            # Skip build phase
elizaos test --skip-type-check       # Skip TS type checking
elizaos test --type e2e --port 4000  # Custom port for e2e
```

### Test Directory Structure

```
src/__tests__/
  ├── test-utils.ts        # Shared mocks and utilities
  ├── actions.test.ts      # Action validation and handler tests
  ├── providers.test.ts    # Provider result generation tests
  └── services.test.ts     # Service initialization and cleanup tests
```

### Component Test Pattern (Vitest)

```typescript
import { describe, it, expect } from 'vitest';

describe('MyAction', () => {
  it('should validate with correct settings', async () => {
    const mockRuntime = {
      getSetting: (key: string) => (key === 'API_KEY' ? 'test-key' : undefined),
    } as IAgentRuntime;

    const mockMessage = {
      content: { text: 'test message' },
      entityId: 'user-1',
      roomId: 'room-1',
    } as Memory;

    const result = await myAction.validate(mockRuntime, mockMessage);
    expect(result).toBe(true);
  });

  it('should fail validation without API key', async () => {
    const mockRuntime = {
      getSetting: () => undefined,
    } as IAgentRuntime;

    const result = await myAction.validate(mockRuntime, {} as Memory);
    expect(result).toBe(false);
  });
});
```

### E2E Test Pattern

Uses `createTestAgent` with character configuration for full workflow validation. Tests verify agent initialization, loaded agents match expected setup, and action registration is correct.

### Test Coverage Areas

- Action validation and handler logic
- Provider result generation and error handling
- Service initialization and cleanup
- Examples structure validation
- E2E integration testing with runtime context

---

## 14. Publishing and Registry

### Required Directory Structure

```
plugin-name/
├── src/index.ts
├── images/
│   ├── logo.jpg          (400x400px, max 500KB)
│   └── banner.jpg        (1280x640px, max 1MB)
├── package.json
├── README.md
└── dist/
```

### Validation Requirements

- Plugin name must start with `plugin-`
- Description must be custom (not auto-generated placeholder)
- Both image assets (logo.jpg and banner.jpg) must be present
- Plugin must build successfully (`bun run build`)

### Publishing Commands

```bash
# Test/validate without publishing
elizaos publish --test
elizaos publish --dry-run     # generates registry files locally

# Actual publication
elizaos publish
```

### What Happens on Publish

1. Package published to npm (available immediately)
2. GitHub repository created (available immediately)
3. Registry pull request created (reviewed by elizaOS team in 1-3 business days)

### Updating After Initial Publish

```bash
npm version patch
bun run build
npm publish
git push origin main
```

Registry automatically syncs with npm updates. `elizaos publish` is only for the initial release.

### GitHub Token Scopes Required

`repo`, `read:org`, `workflow`

---

## 15. Database Schemas for Plugins

Plugins define custom tables using Drizzle ORM and register them via the `schema` field:

```typescript
import { pgTable, uuid, varchar, jsonb, timestamp, index } from 'drizzle-orm/pg-core';

export const myTable = pgTable(
  'my_table',
  {
    id: uuid('id').primaryKey().defaultRandom(),
    agentId: uuid('agent_id').notNull(),    // Include for agent-specific data
    key: varchar('key', { length: 255 }).notNull(),
    value: jsonb('value').notNull(),
    createdAt: timestamp('created_at').defaultNow().notNull(),
  },
  (table) => [
    index('idx_my_table_agent_key').on(table.agentId, table.key),
  ]
);

export const myPlugin: Plugin = {
  name: 'my-plugin',
  schema: { myTable },
  actions: [...],
};
```

Tables without `agentId` fields enable data sharing across all agents.

### IDatabaseAdapter (Key Methods)

```typescript
interface IDatabaseAdapter {
  db: any; // Drizzle ORM instance

  init(): Promise<void>;

  // Memory operations
  createMemory(memory: Memory, tableName?: string, unique?: boolean): Promise<UUID>;
  getMemories(params: MemoryRetrievalOptions): Promise<Memory[]>;
  searchMemories(params: MemorySearchOptions): Promise<Memory[]>;
  getMemoryById(id: UUID): Promise<Memory | null>;
  deleteMemory(id: UUID): Promise<void>;
  updateMemory(memory: Memory): Promise<void>;
  batchCreateMemories(memories: Memory[]): Promise<void>;

  // Entity management
  createEntity(entity: any): Promise<UUID>;
  updateEntity(entity: any): Promise<void>;
  getEntity(id: UUID): Promise<any>;

  // Relationships
  createRelationship(relationship: any): Promise<UUID>;
  getRelationships(params: any): Promise<any[]>;

  // Tasks
  getTasks(params: any): Promise<Task[]>;
  updateTask(task: Task): Promise<void>;

  // Knowledge/Facts
  createFact(fact: any): Promise<UUID>;
  searchFacts(params: any): Promise<any[]>;
}
```

---

## 16. Supporting Types

### UUID

```typescript
type UUID = string;
```

### TokenUsage

```typescript
interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  cachedTokens?: number;
}
```

### TargetInfo

```typescript
interface TargetInfo {
  source: string;
  roomId: UUID;
  channelId?: string;
  serverId?: string;
  entityId?: UUID;
  threadId?: string;
}
```

### ControlMessage

```typescript
interface ControlMessage {
  type: 'control';
  payload: {
    action: 'disable_input' | 'enable_input';
    target?: string;
    [key: string]: unknown;
  };
  roomId: UUID;
}
```

### TextStreamResult and TextStreamChunk

```typescript
interface TextStreamResult {
  text: string;
  usage?: TokenUsage;
  [Symbol.asyncIterator](): AsyncIterator<TextStreamChunk>;
}

interface TextStreamChunk {
  text: string;
  isFirst?: boolean;
  isLast?: boolean;
}
```

### Log Types

```typescript
interface Log {
  id: UUID;
  agentId: UUID;
  roomId?: UUID;
  entityId?: UUID;
  type: 'action' | 'evaluator' | 'model' | 'embedding';
  body: LogBody;
  createdAt: number;
}

type LogBody = ActionLogBody | EvaluatorLogBody | ModelLogBody | EmbeddingLogBody;

interface ActionLogBody {
  type: 'action';
  action: string;
  input: Content;
  output?: ActionResult;
  duration: number;
  success: boolean;
  error?: string;
}

interface ModelLogBody {
  type: 'model';
  modelType: ModelTypeName;
  provider: string;
  model: string;
  tokens: TokenUsage;
  duration: number;
  cached: boolean;
}
```

### AgentRunSummary

```typescript
interface AgentRunSummary {
  runId: UUID;
  agentId: UUID;
  roomId?: UUID;
  startTime: number;
  endTime: number;
  status: RunStatus;
  actionsExecuted: number;
  messagesProcessed: number;
  tokensUsed: TokenUsage;
  errors: string[];
}

type RunStatus = 'completed' | 'failed' | 'timeout' | 'cancelled';
```

### HealthStatus

```typescript
interface HealthStatus {
  alive: boolean;
  responsive: boolean;
  memoryUsage?: number;
  uptime?: number;
  lastActivity?: number;
}
```

### SOCKET_MESSAGE_TYPE

```typescript
enum SOCKET_MESSAGE_TYPE {
  ROOM_JOINING = 1,
  SEND_MESSAGE = 2,
  MESSAGE = 3,
  ACK = 4,
  THINKING = 5,
  CONTROL = 6,
}
```

### IStreamExtractor

```typescript
interface IStreamExtractor {
  done: boolean; // readonly
  push(chunk: string): string;
}
```

### Character Interface

```typescript
interface Character {
  name: string;
  bio: string | string[];
  id?: UUID;
  username?: string;
  system?: string;
  templates?: object;
  adjectives?: string[];
  topics?: string[];
  knowledge?: any[];
  messageExamples?: any[][];
  postExamples?: string[];
  style?: object;
  plugins?: string[];
  settings?: object;
  secrets?: object;
}
```

### TEE Types

```typescript
enum TEEMode {
  OFF = 'OFF',
  LOCAL = 'LOCAL',
  DOCKER = 'DOCKER',
  PRODUCTION = 'PRODUCTION',
}

interface TeeAgent {
  id: string;
  agentId: string;
  agentName: string;
  createdAt: number;
  publicKey: string;
  attestation: string;
}

interface RemoteAttestationQuote {
  quote: string;
  timestamp: number;
  mrEnclave?: string;
  mrSigner?: string;
}

interface TeePluginConfig {
  mode: TEEMode;
  vendor?: string;
  vendorConfig?: Record<string, unknown>;
}
```

### Plugin Scaffolding Structure

**Quick Plugin (Backend Only):**

```
plugin-my-plugin/
├── src/
│   ├── index.ts           # Plugin manifest
│   ├── actions/
│   ├── providers/
│   └── types/
├── package.json
├── tsconfig.json
└── tsup.config.ts
```

**Full Plugin (with Frontend):**

```
plugin-my-plugin/
├── src/
│   ├── index.ts
│   ├── actions/
│   ├── providers/
│   └── types/
├── frontend/              # React components
├── public/                # Static assets
├── package.json
├── tsconfig.json
├── tsup.config.ts
├── vite.config.ts
└── tailwind.config.js
```

### Build Configuration

TypeScript targets ES2022 with strict mode, declaration generation, and source maps. Build uses tsup with ESM format, `@elizaos/core` as external dependency, and automatic type generation.

### Development Commands

```bash
elizaos create my-plugin --type plugin   # Scaffold new plugin
elizaos dev           # or bun run dev   # Watch mode with hot reloading
bun run build                            # Production build to dist/
bun test                                 # Execute test suite
bun test --watch                         # Continuous testing
```
