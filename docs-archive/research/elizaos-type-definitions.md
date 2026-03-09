# ElizaOS Plugin System - Complete Type Definitions Reference

> Extracted from `elizaos/eliza` repository, `develop` branch (default).
> Source: `packages/core/src/types/` directory.
> Date: 2026-02-09

## Table of Contents

1. [Repository Structure](#repository-structure)
2. [Primitives](#primitives-typesprimitivests)
3. [Memory Types](#memory-types-typesmemorysts)
4. [State Types](#state-types-typesstatetsts)
5. [Component Types (Action, Provider, Evaluator)](#component-types-typescomponentsts)
6. [Plugin Interface](#plugin-interface-typesplugints)
7. [Service Types](#service-types-typesservicets)
8. [Event System](#event-system-typesevents)
9. [IAgentRuntime Interface](#iagentruntimeinterface-typesruntimets)
10. [Environment Types](#environment-types-typesenvironmentts)
11. [Model Types](#model-types-typesmodelts)
12. [Task Types](#task-types-typestaskts)
13. [Messaging Types](#messaging-types-typesmessagingts)
14. [Database Adapter Interface](#database-adapter-interface-typesdatabasets)
15. [Character / Agent Types](#character--agent-types-typesagentts)
16. [Testing Types](#testing-types-typestestingts)
17. [Settings Types](#settings-types-typessettingsts)
18. [ElizaOS Orchestrator](#elizaos-orchestrator-typeselizaoists)
19. [Knowledge Types](#knowledge-types-typesknowledgets)
20. [Streaming Types](#streaming-types-typesstreamingts)
21. [Plugin Loader](#plugin-loader-plugints)
22. [Runtime Implementation (AgentRuntime class)](#runtime-implementation)
23. [Reference Plugins](#reference-plugins)

---

## Repository Structure

The default branch is `develop`. Key packages:

```
packages/
  core/                    # Core framework with all type definitions
  cli/                     # CLI tooling (elizaos create, start, dev, etc.)
  plugin-bootstrap/        # Reference plugin (actions, providers, evaluators, services, events)
  plugin-starter/          # Starter template for new plugins (what "elizaos create --type plugin" generates)
  plugin-dummy-services/   # Dummy service implementations for testing
  plugin-quick-starter/    # Quick starter template
  plugin-sql/              # SQL database adapter plugin
  server/                  # Server package
  client/                  # Client package
  app/                     # Application package
  api-client/              # API client
  elizaos/                 # ElizaOS orchestrator
  config/                  # Configuration
  service-interfaces/      # Service interface definitions
  test-utils/              # Test utilities
  project-starter/         # Project starter template
  project-tee-starter/     # TEE project starter template
```

Core types are in `packages/core/src/types/` with these files:

```
types/
  index.ts           # Barrel re-export of all type modules
  primitives.ts      # UUID, Content, Media, Metadata
  environment.ts     # Entity, Room, World, Component, ChannelType, Role
  state.ts           # State, StateData, ActionPlan
  memory.ts          # Memory, MemoryType, MemoryScope, MemoryMetadata
  knowledge.ts       # KnowledgeItem, DirectoryItem
  agent.ts           # Character, Agent, AgentStatus
  components.ts      # Action, Provider, Evaluator, ActionResult, HandlerCallback, Handler
  plugin.ts          # Plugin, Route, RouteRequest, RouteResponse, PluginEvents, Project
  service.ts         # Service (abstract class), ServiceType, ServiceTypeRegistry
  model.ts           # ModelType, ModelParamsMap, ModelResultMap, GenerateTextParams
  database.ts        # IDatabaseAdapter, Log types
  events.ts          # EventType, EventPayloadMap, EventHandler, all payload types
  task.ts            # Task, TaskWorker, TaskMetadata
  runtime.ts         # IAgentRuntime interface
  messaging.ts       # TargetInfo, SendHandlerFunction, ControlMessage, MessageResult
  testing.ts         # TestCase, TestSuite
  settings.ts        # RuntimeSettings, Setting, WorldSettings, OnboardingConfig
  elizaos.ts         # IElizaOS, HandleMessageOptions, HandleMessageResult
  streaming.ts       # IStreamExtractor, IStreamingRetryState
  tee.ts             # TEE-related types
```

---

## Primitives (`types/primitives.ts`)

```typescript
import type { ChannelType } from './environment';

export type UUID = `${string}-${string}-${string}-${string}-${string}`;

export function asUUID(id: string): UUID {
  if (!id || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) {
    throw new Error(`Invalid UUID format: ${id}`);
  }
  return id as UUID;
}

export interface Content {
  thought?: string;
  text?: string;
  actions?: string[];
  providers?: string[];
  source?: string;
  target?: string;
  url?: string;
  inReplyTo?: UUID;
  attachments?: Media[];
  channelType?: ChannelType;
  mentionContext?: MentionContext;
  responseMessageId?: UUID;
  [key: string]: unknown;
}

export interface MentionContext {
  isMention: boolean;
  isReply: boolean;
  isThread: boolean;
  mentionType?: 'platform_mention' | 'reply' | 'thread' | 'none';
}

export type Media = {
  id: string;
  url: string;
  title?: string;
  source?: string;
  description?: string;
  text?: string;
  contentType?: ContentType;
};

export enum ContentType {
  IMAGE = 'image',
  VIDEO = 'video',
  AUDIO = 'audio',
  DOCUMENT = 'document',
  LINK = 'link',
}

export type Metadata = Record<string, unknown>;
```

---

## Memory Types (`types/memory.ts`)

```typescript
import type { Content, UUID } from './primitives';

export type MemoryTypeAlias = string;

export enum MemoryType {
  DOCUMENT = 'document',
  FRAGMENT = 'fragment',
  MESSAGE = 'message',
  DESCRIPTION = 'description',
  CUSTOM = 'custom',
}

export type MemoryScope = 'shared' | 'private' | 'room';

export interface BaseMetadata {
  type: MemoryTypeAlias;
  source?: string;
  sourceId?: UUID;
  scope?: MemoryScope;
  timestamp?: number;
  tags?: string[];
}

export interface DocumentMetadata extends BaseMetadata {
  type: MemoryType.DOCUMENT;
}

export interface FragmentMetadata extends BaseMetadata {
  type: MemoryType.FRAGMENT;
  documentId: UUID;
  position: number;
}

export interface MessageMetadata extends BaseMetadata {
  type: MemoryType.MESSAGE;
}

export interface DescriptionMetadata extends BaseMetadata {
  type: MemoryType.DESCRIPTION;
}

export interface CustomMetadata extends BaseMetadata {
  [key: string]: unknown;
}

export type MemoryMetadata =
  | DocumentMetadata
  | FragmentMetadata
  | MessageMetadata
  | DescriptionMetadata
  | CustomMetadata;

export interface Memory {
  id?: UUID;
  entityId: UUID;
  agentId?: UUID;
  createdAt?: number;
  content: Content;
  embedding?: number[];
  roomId: UUID;
  worldId?: UUID;
  unique?: boolean;
  similarity?: number;
  metadata?: MemoryMetadata;
}

export interface MessageMemory extends Memory {
  metadata: MessageMetadata;
  content: Content & {
    text: string;
  };
}
```

---

## State Types (`types/state.ts`)

```typescript
import type { ActionResult } from './components';
import type { Entity, Room, World } from './environment';

export interface ActionPlanStep {
  action: string;
  status: 'pending' | 'completed' | 'failed';
  error?: string;
  result?: ActionResult;
}

export interface ActionPlan {
  thought: string;
  totalSteps: number;
  currentStep: number;
  steps: ActionPlanStep[];
}

export interface StateData {
  room?: Room;
  world?: World;
  entity?: Entity;
  providers?: Record<string, Record<string, unknown>>;
  actionPlan?: ActionPlan;
  actionResults?: ActionResult[];
  [key: string]: unknown;
}

export interface State {
  [key: string]: unknown;
  values: {
    [key: string]: unknown;
  };
  data: StateData;
  text: string;
}
```

---

## Component Types (`types/components.ts`)

```typescript
import type { Memory } from './memory';
import type { Content } from './primitives';
import type { IAgentRuntime } from './runtime';
import type { State } from './state';

export interface ActionExample {
  name: string;
  content: Content;
}

export type HandlerCallback = (response: Content) => Promise<Memory[]>;

export type Handler = (
  runtime: IAgentRuntime,
  message: Memory,
  state?: State,
  options?: HandlerOptions,
  callback?: HandlerCallback,
  responses?: Memory[]
) => Promise<ActionResult | void | undefined>;

export type Validator = (
  runtime: IAgentRuntime,
  message: Memory,
  state?: State
) => Promise<boolean>;

export interface Action {
  similes?: string[];
  description: string;
  examples?: ActionExample[][];
  handler: Handler;
  name: string;
  validate: Validator;
  [key: string]: unknown;
}

export interface EvaluationExample {
  prompt: string;
  messages: Array<ActionExample>;
  outcome: string;
}

export interface Evaluator {
  alwaysRun?: boolean;
  description: string;
  similes?: string[];
  examples: EvaluationExample[];
  handler: Handler;
  name: string;
  validate: Validator;
}

export interface ProviderResult {
  text?: string;
  values?: Record<string, unknown>;
  data?: Record<string, unknown>;
}

export interface Provider {
  name: string;
  description?: string;
  dynamic?: boolean;
  position?: number;
  private?: boolean;
  get: (runtime: IAgentRuntime, message: Memory, state: State) => Promise<ProviderResult>;
}

export interface ActionResult {
  text?: string;
  values?: Record<string, unknown>;
  data?: Record<string, unknown>;
  success: boolean;
  error?: string | Error;
}

export interface ActionContext {
  previousResults: ActionResult[];
  getPreviousResult?: (actionName: string) => ActionResult | undefined;
}

export interface HandlerOptions {
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

---

## Plugin Interface (`types/plugin.ts`)

```typescript
import type { Character } from './agent';
import type { Action, Evaluator, Provider } from './components';
import type { IDatabaseAdapter } from './database';
import type { EventHandler, EventPayloadMap } from './events';
import type { ModelParamsMap, PluginModelResult } from './model';
import type { IAgentRuntime } from './runtime';
import type { Service } from './service';
import type { TestSuite } from './testing';

export interface RouteRequest {
  body?: unknown;
  params?: Record<string, string>;
  query?: Record<string, unknown>;
  headers?: Record<string, string | string[] | undefined>;
  method?: string;
  path?: string;
  url?: string;
}

export interface RouteResponse {
  status: (code: number) => RouteResponse;
  json: (data: unknown) => RouteResponse;
  send: (data: unknown) => RouteResponse;
  end: () => RouteResponse;
  setHeader?: (name: string, value: string | string[]) => RouteResponse;
  headersSent?: boolean;
}

export type Route = {
  type: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE' | 'STATIC';
  path: string;
  filePath?: string;
  public?: boolean;
  name?: string extends { public: true } ? string : string | undefined;
  handler?: (req: RouteRequest, res: RouteResponse, runtime: IAgentRuntime) => Promise<void>;
  isMultipart?: boolean;
};

export type PluginEvents = {
  [K in keyof EventPayloadMap]?: EventHandler<K>[];
};

export type RuntimeEventStorage = PluginEvents & {
  [key: string]: ((params: unknown) => Promise<void>)[] | undefined;
};

export interface Plugin {
  name: string;
  description: string;

  init?: (config: Record<string, string>, runtime: IAgentRuntime) => Promise<void>;

  config?: { [key: string]: string | number | boolean | null | undefined };

  services?: (typeof Service)[];

  componentTypes?: {
    name: string;
    schema: Record<string, unknown>;
    validator?: (data: unknown) => boolean;
  }[];

  actions?: Action[];
  providers?: Provider[];
  evaluators?: Evaluator[];
  adapter?: IDatabaseAdapter;
  models?: {
    [K in keyof ModelParamsMap]?: (
      runtime: IAgentRuntime,
      params: ModelParamsMap[K]
    ) => Promise<PluginModelResult<K>>;
  };
  events?: PluginEvents;
  routes?: Route[];
  tests?: TestSuite[];

  dependencies?: string[];
  testDependencies?: string[];
  priority?: number;
  schema?: Record<string, unknown>;
}

export interface ProjectAgent {
  character: Character;
  init?: (runtime: IAgentRuntime) => Promise<void>;
  plugins?: Plugin[];
  tests?: TestSuite | TestSuite[];
}

export interface Project {
  agents: ProjectAgent[];
}
```

---

## Service Types (`types/service.ts`)

```typescript
import type { Metadata } from './primitives';
import type { IAgentRuntime } from './runtime';

export interface ServiceTypeRegistry {
  TRANSCRIPTION: 'transcription';
  VIDEO: 'video';
  BROWSER: 'browser';
  PDF: 'pdf';
  REMOTE_FILES: 'aws_s3';
  WEB_SEARCH: 'web_search';
  EMAIL: 'email';
  TEE: 'tee';
  TASK: 'task';
  WALLET: 'wallet';
  LP_POOL: 'lp_pool';
  TOKEN_DATA: 'token_data';
  MESSAGE_SERVICE: 'message_service';
  MESSAGE: 'message';
  POST: 'post';
  UNKNOWN: 'unknown';
}

export type ServiceTypeName = ServiceTypeRegistry[keyof ServiceTypeRegistry];

export type ServiceTypeValue<K extends keyof ServiceTypeRegistry> = ServiceTypeRegistry[K];

export type IsValidServiceType<T extends string> = T extends ServiceTypeName ? true : false;

export type TypedServiceClass<T extends ServiceTypeName> = {
  new (runtime?: IAgentRuntime): Service;
  serviceType: T;
  start(runtime: IAgentRuntime): Promise<Service>;
};

export interface ServiceClassMap {
  // Core services will be added here, plugins extend via module augmentation
}

export type ServiceInstance<T extends ServiceTypeName> = T extends keyof ServiceClassMap
  ? InstanceType<ServiceClassMap[T]>
  : Service;

export type ServiceRegistry<T extends ServiceTypeName = ServiceTypeName> = Map<T, Service>;

export const ServiceType = {
  TRANSCRIPTION: 'transcription',
  VIDEO: 'video',
  BROWSER: 'browser',
  PDF: 'pdf',
  REMOTE_FILES: 'aws_s3',
  WEB_SEARCH: 'web_search',
  EMAIL: 'email',
  TEE: 'tee',
  TASK: 'task',
  WALLET: 'wallet',
  LP_POOL: 'lp_pool',
  TOKEN_DATA: 'token_data',
  MESSAGE_SERVICE: 'message_service',
  MESSAGE: 'message',
  POST: 'post',
  UNKNOWN: 'unknown',
} as const satisfies ServiceTypeRegistry;

export abstract class Service {
  protected runtime!: IAgentRuntime;

  constructor(runtime?: IAgentRuntime) {
    if (runtime) {
      this.runtime = runtime;
    }
  }

  abstract stop(): Promise<void>;

  static serviceType: string;

  abstract capabilityDescription: string;

  config?: Metadata;

  static async start(_runtime: IAgentRuntime): Promise<Service> {
    throw new Error('Not implemented');
  }

  static async stop(_runtime: IAgentRuntime): Promise<void> {
    throw new Error('Not implemented');
  }

  static registerSendHandlers?(runtime: IAgentRuntime, service: Service): void;
}

export interface TypedService<
  ConfigType extends Metadata = Metadata,
  InputType = unknown,
  ResultType = unknown,
> extends Service {
  config?: ConfigType;
  process(input: InputType): Promise<ResultType>;
}

export function getTypedService<
  ConfigType extends Metadata = Metadata,
  InputType = unknown,
  ResultType = unknown,
>(
  runtime: IAgentRuntime,
  serviceType: ServiceTypeName
): TypedService<ConfigType, InputType, ResultType> | null {
  return runtime.getService<TypedService<ConfigType, InputType, ResultType>>(serviceType);
}

export interface ServiceError {
  code: string;
  message: string;
  details?: Record<string, unknown> | string | number | boolean | null;
  cause?: Error;
}

export function createServiceError(error: unknown, code = 'UNKNOWN_ERROR'): ServiceError {
  if (error instanceof Error) {
    return {
      code,
      message: error.message,
      cause: error,
    };
  }
  return {
    code,
    message: String(error),
  };
}
```

---

## Event System (`types/events.ts`)

```typescript
import type { HandlerCallback } from './components';
import type { Entity, Room, World } from './environment';
import type { Memory } from './memory';
import type { ControlMessage } from './messaging';
import type { ModelTypeName } from './model';
import type { Content, UUID } from './primitives';
import type { IAgentRuntime } from './runtime';

export enum EventType {
  // World events
  WORLD_JOINED = 'WORLD_JOINED',
  WORLD_CONNECTED = 'WORLD_CONNECTED',
  WORLD_LEFT = 'WORLD_LEFT',

  // Entity events
  ENTITY_JOINED = 'ENTITY_JOINED',
  ENTITY_LEFT = 'ENTITY_LEFT',
  ENTITY_UPDATED = 'ENTITY_UPDATED',

  // Room events
  ROOM_JOINED = 'ROOM_JOINED',
  ROOM_LEFT = 'ROOM_LEFT',

  // Message events
  MESSAGE_RECEIVED = 'MESSAGE_RECEIVED',
  MESSAGE_SENT = 'MESSAGE_SENT',
  MESSAGE_DELETED = 'MESSAGE_DELETED',

  // Channel events
  CHANNEL_CLEARED = 'CHANNEL_CLEARED',

  // Voice events
  VOICE_MESSAGE_RECEIVED = 'VOICE_MESSAGE_RECEIVED',
  VOICE_MESSAGE_SENT = 'VOICE_MESSAGE_SENT',

  // Interaction events
  REACTION_RECEIVED = 'REACTION_RECEIVED',
  POST_GENERATED = 'POST_GENERATED',
  INTERACTION_RECEIVED = 'INTERACTION_RECEIVED',

  // Run events
  RUN_STARTED = 'RUN_STARTED',
  RUN_ENDED = 'RUN_ENDED',
  RUN_TIMEOUT = 'RUN_TIMEOUT',

  // Action events
  ACTION_STARTED = 'ACTION_STARTED',
  ACTION_COMPLETED = 'ACTION_COMPLETED',

  // Evaluator events
  EVALUATOR_STARTED = 'EVALUATOR_STARTED',
  EVALUATOR_COMPLETED = 'EVALUATOR_COMPLETED',

  // Model events
  MODEL_USED = 'MODEL_USED',

  // Embedding events
  EMBEDDING_GENERATION_REQUESTED = 'EMBEDDING_GENERATION_REQUESTED',
  EMBEDDING_GENERATION_COMPLETED = 'EMBEDDING_GENERATION_COMPLETED',
  EMBEDDING_GENERATION_FAILED = 'EMBEDDING_GENERATION_FAILED',

  // Control events
  CONTROL_MESSAGE = 'CONTROL_MESSAGE',
}

export enum PlatformPrefix {
  DISCORD = 'DISCORD',
  TELEGRAM = 'TELEGRAM',
  TWITTER = 'TWITTER',
}

export interface EventPayload {
  runtime: IAgentRuntime;
  source: string;
  onComplete?: () => void;
}

export interface WorldPayload extends EventPayload {
  world: World;
  rooms: Room[];
  entities: Entity[];
}

export interface EntityPayload extends EventPayload {
  entityId: UUID;
  worldId?: UUID;
  roomId?: UUID;
  metadata?: {
    originalId: string;
    username: string;
    displayName?: string;
    [key: string]: unknown;
  };
}

export interface MessagePayload extends EventPayload {
  message: Memory;
  callback?: HandlerCallback;
}

export interface ChannelClearedPayload extends EventPayload {
  roomId: UUID;
  channelId: string;
  memoryCount: number;
}

export interface InvokePayload extends EventPayload {
  worldId: UUID;
  userId: string;
  roomId: UUID;
  callback?: HandlerCallback;
}

export interface RunEventPayload extends EventPayload {
  runId: UUID;
  messageId: UUID;
  roomId: UUID;
  entityId: UUID;
  startTime: number;
  status: 'started' | 'completed' | 'timeout';
  endTime?: number;
  duration?: number;
  error?: string;
}

export interface ActionEventPayload extends EventPayload {
  roomId: UUID;
  world: UUID;
  content: Content;
  messageId?: UUID;
}

export interface EvaluatorEventPayload extends EventPayload {
  evaluatorId: UUID;
  evaluatorName: string;
  startTime?: number;
  completed?: boolean;
  error?: Error;
}

export interface ModelEventPayload extends EventPayload {
  provider: string;
  type: ModelTypeName;
  prompt: string;
  tokens?: {
    prompt: number;
    completion: number;
    total: number;
  };
}

export interface EmbeddingGenerationPayload extends EventPayload {
  memory: Memory;
  priority?: 'high' | 'normal' | 'low';
  retryCount?: number;
  maxRetries?: number;
  embedding?: number[];
  error?: Error | string | unknown;
  runId?: UUID;
}

export interface ControlMessagePayload extends EventPayload {
  message: ControlMessage;
}

export interface EventPayloadMap {
  [EventType.WORLD_JOINED]: WorldPayload;
  [EventType.WORLD_CONNECTED]: WorldPayload;
  [EventType.WORLD_LEFT]: WorldPayload;
  [EventType.ENTITY_JOINED]: EntityPayload;
  [EventType.ENTITY_LEFT]: EntityPayload;
  [EventType.ENTITY_UPDATED]: EntityPayload;
  [EventType.MESSAGE_RECEIVED]: MessagePayload;
  [EventType.MESSAGE_SENT]: MessagePayload;
  [EventType.MESSAGE_DELETED]: MessagePayload;
  [EventType.VOICE_MESSAGE_RECEIVED]: MessagePayload;
  [EventType.VOICE_MESSAGE_SENT]: MessagePayload;
  [EventType.CHANNEL_CLEARED]: ChannelClearedPayload;
  [EventType.REACTION_RECEIVED]: MessagePayload;
  [EventType.POST_GENERATED]: InvokePayload;
  [EventType.INTERACTION_RECEIVED]: MessagePayload;
  [EventType.RUN_STARTED]: RunEventPayload;
  [EventType.RUN_ENDED]: RunEventPayload;
  [EventType.RUN_TIMEOUT]: RunEventPayload;
  [EventType.ACTION_STARTED]: ActionEventPayload;
  [EventType.ACTION_COMPLETED]: ActionEventPayload;
  [EventType.EVALUATOR_STARTED]: EvaluatorEventPayload;
  [EventType.EVALUATOR_COMPLETED]: EvaluatorEventPayload;
  [EventType.MODEL_USED]: ModelEventPayload;
  [EventType.EMBEDDING_GENERATION_REQUESTED]: EmbeddingGenerationPayload;
  [EventType.EMBEDDING_GENERATION_COMPLETED]: EmbeddingGenerationPayload;
  [EventType.EMBEDDING_GENERATION_FAILED]: EmbeddingGenerationPayload;
  [EventType.CONTROL_MESSAGE]: ControlMessagePayload;
}

export type EventHandler<T extends keyof EventPayloadMap> = (
  payload: EventPayloadMap[T]
) => Promise<void>;
```

---

## IAgentRuntime Interface (`types/runtime.ts`)

```typescript
import type { Character } from './agent';
import type { Action, Evaluator, Provider, ActionResult } from './components';
import { HandlerCallback } from './components';
import type { IDatabaseAdapter } from './database';
import type { IElizaOS } from './elizaos';
import type { Entity, Room, World, ChannelType } from './environment';
import type { Logger } from '../logger';
import { Memory, MemoryMetadata } from './memory';
import type { SendHandlerFunction, TargetInfo } from './messaging';
import type { IMessageService } from '../services/message-service';
import type {
  ModelParamsMap,
  ModelResultMap,
  ModelTypeName,
  GenerateTextOptions,
  GenerateTextResult,
  GenerateTextParams,
  TextGenerationModelType,
} from './model';
import type { Plugin, RuntimeEventStorage, Route } from './plugin';
import type { Content, UUID } from './primitives';
import type { Service, ServiceTypeName } from './service';
import type { State } from './state';
import type { TaskWorker } from './task';
import type { EventPayloadMap, EventHandler, EventPayload } from './events';

export interface IAgentRuntime extends IDatabaseAdapter {
  // Properties
  agentId: UUID;
  character: Character;
  initPromise: Promise<void>;
  messageService: IMessageService | null;
  providers: Provider[];
  actions: Action[];
  evaluators: Evaluator[];
  plugins: Plugin[];
  services: Map<ServiceTypeName, Service[]>;
  events: RuntimeEventStorage;
  fetch?: typeof fetch | null;
  routes: Route[];
  logger: Logger;
  stateCache: Map<string, State>;
  elizaOS?: IElizaOS;

  // Methods
  registerPlugin(plugin: Plugin): Promise<void>;
  initialize(options?: { skipMigrations?: boolean }): Promise<void>;
  getConnection(): Promise<unknown>;

  getService<T extends Service>(service: ServiceTypeName | string): T | null;
  getServicesByType<T extends Service>(service: ServiceTypeName | string): T[];
  getAllServices(): Map<ServiceTypeName, Service[]>;
  registerService(service: typeof Service): Promise<void>;
  getServiceLoadPromise(serviceType: ServiceTypeName): Promise<Service>;
  getRegisteredServiceTypes(): ServiceTypeName[];
  hasService(serviceType: ServiceTypeName | string): boolean;
  hasElizaOS(): this is IAgentRuntime & { elizaOS: IElizaOS };

  registerDatabaseAdapter(adapter: IDatabaseAdapter): void;
  setSetting(key: string, value: string | boolean | null, secret?: boolean): void;
  getSetting(key: string): string | boolean | number | null;
  getConversationLength(): number;

  processActions(
    message: Memory,
    responses: Memory[],
    state?: State,
    callback?: HandlerCallback,
    options?: { onStreamChunk?: (chunk: string, messageId?: UUID) => Promise<void> }
  ): Promise<void>;

  getActionResults(messageId: UUID): ActionResult[];

  evaluate(
    message: Memory,
    state?: State,
    didRespond?: boolean,
    callback?: HandlerCallback,
    responses?: Memory[]
  ): Promise<Evaluator[] | null>;

  registerProvider(provider: Provider): void;
  registerAction(action: Action): void;
  registerEvaluator(evaluator: Evaluator): void;

  ensureConnections(entities: Entity[], rooms: Room[], source: string, world: World): Promise<void>;
  ensureConnection(params: {
    entityId: UUID;
    roomId: UUID;
    userName?: string;
    name?: string;
    worldName?: string;
    source?: string;
    channelId?: string;
    messageServerId?: UUID;
    type?: ChannelType | string;
    worldId: UUID;
    userId?: UUID;
    metadata?: Record<string, unknown>;
  }): Promise<void>;

  ensureParticipantInRoom(entityId: UUID, roomId: UUID): Promise<void>;
  ensureWorldExists(world: World): Promise<void>;
  ensureRoomExists(room: Room): Promise<void>;

  composeState(
    message: Memory,
    includeList?: string[],
    onlyInclude?: boolean,
    skipCache?: boolean
  ): Promise<State>;

  // Text generation overload (auto-streams via context)
  useModel(
    modelType: TextGenerationModelType,
    params: GenerateTextParams,
    provider?: string
  ): Promise<string>;

  // Generic fallback for other model types
  useModel<T extends keyof ModelParamsMap, R = ModelResultMap[T]>(
    modelType: T,
    params: ModelParamsMap[T],
    provider?: string
  ): Promise<R>;

  generateText(input: string, options?: GenerateTextOptions): Promise<GenerateTextResult>;

  registerModel(
    modelType: ModelTypeName | string,
    handler: (runtime: IAgentRuntime, params: Record<string, unknown>) => Promise<unknown>,
    provider: string,
    priority?: number
  ): void;

  getModel(
    modelType: ModelTypeName | string
  ): ((runtime: IAgentRuntime, params: Record<string, unknown>) => Promise<unknown>) | undefined;

  registerEvent<T extends keyof EventPayloadMap>(event: T, handler: EventHandler<T>): void;
  registerEvent<P extends EventPayload = EventPayload>(
    event: string,
    handler: (params: P) => Promise<void>
  ): void;

  getEvent<T extends keyof EventPayloadMap>(event: T): EventHandler<T>[] | undefined;
  getEvent(event: string): ((params: EventPayload) => Promise<void>)[] | undefined;

  emitEvent<T extends keyof EventPayloadMap>(
    event: T | T[],
    params: EventPayloadMap[T]
  ): Promise<void>;
  emitEvent(event: string | string[], params: EventPayload): Promise<void>;

  registerTaskWorker(taskHandler: TaskWorker): void;
  getTaskWorker(name: string): TaskWorker | undefined;

  stop(): Promise<void>;

  addEmbeddingToMemory(memory: Memory): Promise<Memory>;
  queueEmbeddingGeneration(memory: Memory, priority?: 'high' | 'normal' | 'low'): Promise<void>;
  getAllMemories(): Promise<Memory[]>;
  clearAllAgentMemories(): Promise<void>;
  updateMemory(memory: Partial<Memory> & { id: UUID; metadata?: MemoryMetadata }): Promise<boolean>;

  // Run tracking
  createRunId(): UUID;
  startRun(roomId?: UUID): UUID;
  endRun(): void;
  getCurrentRunId(): UUID;

  // Easy/compat wrappers
  getEntityById(entityId: UUID): Promise<Entity | null>;
  getRoom(roomId: UUID): Promise<Room | null>;
  createEntity(entity: Entity): Promise<boolean>;
  createRoom(room: Room): Promise<UUID>;
  addParticipant(entityId: UUID, roomId: UUID): Promise<boolean>;
  getRooms(worldId: UUID): Promise<Room[]>;
  registerSendHandler(source: string, handler: SendHandlerFunction): void;
  sendMessageToTarget(target: TargetInfo, content: Content): Promise<void>;
  updateWorld(world: World): Promise<void>;
}
```

---

## Environment Types (`types/environment.ts`)

```typescript
import type { Metadata, UUID } from './primitives';

export interface Component {
  id: UUID;
  entityId: UUID;
  agentId: UUID;
  roomId: UUID;
  worldId: UUID;
  sourceEntityId: UUID;
  type: string;
  createdAt: number;
  data: Metadata;
}

export interface Entity {
  id?: UUID;
  names: string[];
  metadata: Metadata;
  agentId: UUID;
  components?: Component[];
}

export enum Role {
  OWNER = 'OWNER',
  ADMIN = 'ADMIN',
  NONE = 'NONE',
}

export type World = {
  id: UUID;
  name?: string;
  agentId: UUID;
  messageServerId?: UUID;
  /** @deprecated Use messageServerId instead. */
  serverId?: UUID;
  metadata?: {
    ownership?: {
      ownerId: string;
    };
    roles?: {
      [entityId: UUID]: Role;
    };
    [key: string]: unknown;
  };
};

export enum ChannelType {
  SELF = 'SELF',
  DM = 'DM',
  GROUP = 'GROUP',
  VOICE_DM = 'VOICE_DM',
  VOICE_GROUP = 'VOICE_GROUP',
  FEED = 'FEED',
  THREAD = 'THREAD',
  WORLD = 'WORLD',
  FORUM = 'FORUM',
  /** @deprecated Use DM or GROUP instead */
  API = 'API',
}

export type Room = {
  id: UUID;
  name?: string;
  agentId?: UUID;
  source: string;
  type: ChannelType;
  channelId?: string;
  messageServerId?: UUID;
  /** @deprecated Use messageServerId instead. */
  serverId?: UUID;
  worldId?: UUID;
  metadata?: Metadata;
};

export type RoomMetadata = {
  [key: string]: unknown;
};

export interface Participant {
  id: UUID;
  entity: Entity;
}

export interface Relationship {
  id: UUID;
  sourceEntityId: UUID;
  targetEntityId: UUID;
  agentId: UUID;
  tags: string[];
  metadata: Metadata;
  createdAt?: string;
}
```

---

## Model Types (`types/model.ts`)

```typescript
import type { IAgentRuntime } from './runtime';

export type ModelTypeName = (typeof ModelType)[keyof typeof ModelType] | string;

export const ModelType = {
  SMALL: 'TEXT_SMALL', // kept for backwards compatibility
  MEDIUM: 'TEXT_LARGE', // kept for backwards compatibility
  LARGE: 'TEXT_LARGE', // kept for backwards compatibility
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

export type TextGenerationModelType =
  | typeof ModelType.TEXT_SMALL
  | typeof ModelType.TEXT_LARGE
  | typeof ModelType.TEXT_REASONING_SMALL
  | typeof ModelType.TEXT_REASONING_LARGE
  | typeof ModelType.TEXT_COMPLETION;

export type GenerateTextParams = {
  prompt: string;
  maxTokens?: number;
  minTokens?: number;
  temperature?: number;
  topP?: number;
  topK?: number;
  minP?: number;
  seed?: number;
  repetitionPenalty?: number;
  frequencyPenalty?: number;
  presencePenalty?: number;
  stopSequences?: string[];
  user?: string | null;
  responseFormat?: { type: 'json_object' | 'text' } | string;
  stream?: boolean;
  onStreamChunk?: (chunk: string, messageId?: string) => void | Promise<void>;
};

export interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface TextStreamChunk {
  text: string;
  done: boolean;
}

export interface TextStreamResult {
  textStream: AsyncIterable<string>;
  text: Promise<string>;
  usage: Promise<TokenUsage | undefined>;
  finishReason: Promise<string | undefined>;
}

export interface GenerateTextOptions extends Omit<GenerateTextParams, 'prompt'> {
  includeCharacter?: boolean;
  modelType?: TextGenerationModelType;
}

export interface GenerateTextResult {
  text: string;
}

export interface TokenizeTextParams {
  prompt: string;
  modelType: ModelTypeName;
}

export interface DetokenizeTextParams {
  tokens: number[];
  modelType: ModelTypeName;
}

export interface TextEmbeddingParams {
  text: string;
}

export interface ImageGenerationParams {
  prompt: string;
  size?: string;
  count?: number;
}

export interface ImageDescriptionParams {
  imageUrl: string;
  prompt?: string;
}

export interface TranscriptionParams {
  audioUrl: string;
  prompt?: string;
}

export interface TextToSpeechParams {
  text: string;
  voice?: string;
  speed?: number;
}

export interface AudioProcessingParams {
  audioUrl: string;
  processingType: string;
}

export interface VideoProcessingParams {
  videoUrl: string;
  processingType: string;
}

export type JSONSchema = {
  type: string;
  properties?: Record<string, JSONSchema | { type: string }>;
  required?: string[];
  items?: JSONSchema;
  [key: string]: unknown;
};

export interface ObjectGenerationParams {
  prompt: string;
  schema?: JSONSchema;
  output?: 'object' | 'array' | 'enum';
  enumValues?: string[];
  modelType?: ModelTypeName;
  temperature?: number;
  stopSequences?: string[];
}

export interface ModelParamsMap {
  [ModelType.TEXT_SMALL]: GenerateTextParams;
  [ModelType.TEXT_LARGE]: GenerateTextParams;
  [ModelType.TEXT_EMBEDDING]: TextEmbeddingParams | string | null;
  [ModelType.TEXT_TOKENIZER_ENCODE]: TokenizeTextParams;
  [ModelType.TEXT_TOKENIZER_DECODE]: DetokenizeTextParams;
  [ModelType.TEXT_REASONING_SMALL]: GenerateTextParams;
  [ModelType.TEXT_REASONING_LARGE]: GenerateTextParams;
  [ModelType.IMAGE]: ImageGenerationParams;
  [ModelType.IMAGE_DESCRIPTION]: ImageDescriptionParams | string;
  [ModelType.TRANSCRIPTION]: TranscriptionParams | Buffer | string;
  [ModelType.TEXT_TO_SPEECH]: TextToSpeechParams | string;
  [ModelType.AUDIO]: AudioProcessingParams;
  [ModelType.VIDEO]: VideoProcessingParams;
  [ModelType.OBJECT_SMALL]: ObjectGenerationParams;
  [ModelType.OBJECT_LARGE]: ObjectGenerationParams;
  [ModelType.TEXT_COMPLETION]: GenerateTextParams;
}

export interface ModelResultMap {
  [ModelType.TEXT_SMALL]: string;
  [ModelType.TEXT_LARGE]: string;
  [ModelType.TEXT_EMBEDDING]: number[];
  [ModelType.TEXT_TOKENIZER_ENCODE]: number[];
  [ModelType.TEXT_TOKENIZER_DECODE]: string;
  [ModelType.TEXT_REASONING_SMALL]: string;
  [ModelType.TEXT_REASONING_LARGE]: string;
  [ModelType.IMAGE]: { url: string }[];
  [ModelType.IMAGE_DESCRIPTION]: { title: string; description: string };
  [ModelType.TRANSCRIPTION]: string;
  [ModelType.TEXT_TO_SPEECH]: Buffer | ArrayBuffer | Uint8Array;
  [ModelType.AUDIO]: Buffer | ArrayBuffer | Uint8Array | Record<string, unknown>;
  [ModelType.VIDEO]: Buffer | ArrayBuffer | Uint8Array | Record<string, unknown>;
  [ModelType.OBJECT_SMALL]: Record<string, unknown>;
  [ModelType.OBJECT_LARGE]: Record<string, unknown>;
  [ModelType.TEXT_COMPLETION]: string;
}

export type StreamableModelType =
  | typeof ModelType.TEXT_SMALL
  | typeof ModelType.TEXT_LARGE
  | typeof ModelType.TEXT_REASONING_SMALL
  | typeof ModelType.TEXT_REASONING_LARGE
  | typeof ModelType.TEXT_COMPLETION;

export type PluginModelResult<K extends keyof ModelResultMap> = K extends StreamableModelType
  ? ModelResultMap[K] | TextStreamResult
  : ModelResultMap[K];

export interface ModelHandler<TParams = Record<string, unknown>, TResult = unknown> {
  handler: (runtime: IAgentRuntime, params: TParams) => Promise<TResult>;
  provider: string;
  priority?: number;
  registrationOrder?: number;
}
```

---

## Task Types (`types/task.ts`)

```typescript
import type { Memory } from './memory';
import type { UUID } from './primitives';
import type { IAgentRuntime } from './runtime';
import type { State } from './state';

export interface TaskWorker {
  name: string;
  execute: (
    runtime: IAgentRuntime,
    options: { [key: string]: unknown },
    task: Task
  ) => Promise<void>;
  validate?: (runtime: IAgentRuntime, message: Memory, state: State) => Promise<boolean>;
}

export type TaskMetadata = {
  updateInterval?: number;
  options?: {
    name: string;
    description: string;
  }[];
  [key: string]: unknown;
};

export interface Task {
  id?: UUID;
  name: string;
  updatedAt?: number;
  metadata?: TaskMetadata;
  description: string;
  roomId?: UUID;
  worldId?: UUID;
  entityId?: UUID;
  tags: string[];
}
```

---

## Messaging Types (`types/messaging.ts`)

```typescript
import type { Content, UUID } from './primitives';
import type { IAgentRuntime } from './runtime';
import type { Memory } from './memory';

export interface TargetInfo {
  source: string;
  roomId?: UUID;
  channelId?: string;
  serverId?: string;
  entityId?: UUID;
  threadId?: string;
}

export type SendHandlerFunction = (
  runtime: IAgentRuntime,
  target: TargetInfo,
  content: Content
) => Promise<void>;

export enum SOCKET_MESSAGE_TYPE {
  ROOM_JOINING = 1,
  SEND_MESSAGE = 2,
  MESSAGE = 3,
  ACK = 4,
  THINKING = 5,
  CONTROL = 6,
}

export const MESSAGE_STREAM_EVENT = {
  messageStreamChunk: 'messageStreamChunk',
  messageStreamError: 'messageStreamError',
  messageBroadcast: 'messageBroadcast',
} as const;

export type MessageStreamEventType =
  (typeof MESSAGE_STREAM_EVENT)[keyof typeof MESSAGE_STREAM_EVENT];

export interface MessageStreamChunkPayload {
  messageId: UUID;
  chunk: string;
  index: number;
  channelId: string;
  agentId: UUID;
}

export interface MessageStreamErrorPayload {
  messageId: UUID;
  channelId: string;
  agentId: UUID;
  error: string;
  partialText?: string;
}

export interface ControlMessage {
  type: 'control';
  payload: {
    action: 'disable_input' | 'enable_input';
    target?: string;
    [key: string]: unknown;
  };
  roomId: UUID;
}

export interface MessageHandlerOptions {
  onResponse?: (content: Content) => Promise<void>;
  onError?: (error: Error) => Promise<void>;
  onComplete?: () => Promise<void>;
}

export interface MessageResult {
  messageId: UUID;
  userMessage?: Memory;
  agentResponses?: Content[];
  usage?: {
    inputTokens: number;
    outputTokens: number;
    model: string;
  };
}
```

---

## Database Adapter Interface (`types/database.ts`)

```typescript
export interface IDatabaseAdapter {
  db: unknown;
  initialize(config?: Record<string, string | number | boolean | null>): Promise<void>;
  init(): Promise<void>;
  runPluginMigrations?(
    plugins: Array<{
      name: string;
      schema?: Record<string, string | number | boolean | null | Record<string, unknown>>;
    }>,
    options?: { verbose?: boolean; force?: boolean; dryRun?: boolean }
  ): Promise<void>;
  runMigrations?(migrationsPaths?: string[]): Promise<void>;
  isReady(): Promise<boolean>;
  close(): Promise<void>;
  getConnection(): Promise<unknown>;
  withEntityContext?<T>(entityId: UUID | null, callback: () => Promise<T>): Promise<T>;

  // Agent CRUD
  getAgent(agentId: UUID): Promise<Agent | null>;
  getAgents(): Promise<Partial<Agent>[]>;
  createAgent(agent: Partial<Agent>): Promise<boolean>;
  updateAgent(agentId: UUID, agent: Partial<Agent>): Promise<boolean>;
  deleteAgent(agentId: UUID): Promise<boolean>;

  ensureEmbeddingDimension(dimension: number): Promise<void>;

  // Entity operations
  getEntitiesByIds(entityIds: UUID[]): Promise<Entity[] | null>;
  getEntitiesForRoom(roomId: UUID, includeComponents?: boolean): Promise<Entity[]>;
  createEntities(entities: Entity[]): Promise<boolean>;
  updateEntity(entity: Entity): Promise<void>;

  // Component operations
  getComponent(
    entityId: UUID,
    type: string,
    worldId?: UUID,
    sourceEntityId?: UUID
  ): Promise<Component | null>;
  getComponents(entityId: UUID, worldId?: UUID, sourceEntityId?: UUID): Promise<Component[]>;
  createComponent(component: Component): Promise<boolean>;
  updateComponent(component: Component): Promise<void>;
  deleteComponent(componentId: UUID): Promise<void>;

  // Memory operations
  getMemories(params: {
    entityId?: UUID;
    agentId?: UUID;
    count?: number;
    offset?: number;
    unique?: boolean;
    tableName: string;
    start?: number;
    end?: number;
    roomId?: UUID;
    worldId?: UUID;
  }): Promise<Memory[]>;
  getMemoryById(id: UUID): Promise<Memory | null>;
  getMemoriesByIds(ids: UUID[], tableName?: string): Promise<Memory[]>;
  getMemoriesByRoomIds(params: {
    tableName: string;
    roomIds: UUID[];
    limit?: number;
  }): Promise<Memory[]>;
  getCachedEmbeddings(params: {
    query_table_name: string;
    query_threshold: number;
    query_input: string;
    query_field_name: string;
    query_field_sub_name: string;
    query_match_count: number;
  }): Promise<{ embedding: number[]; levenshtein_score: number }[]>;
  searchMemories(params: {
    embedding: number[];
    match_threshold?: number;
    count?: number;
    unique?: boolean;
    tableName: string;
    query?: string;
    roomId?: UUID;
    worldId?: UUID;
    entityId?: UUID;
  }): Promise<Memory[]>;
  createMemory(memory: Memory, tableName: string, unique?: boolean): Promise<UUID>;
  updateMemory(memory: Partial<Memory> & { id: UUID; metadata?: MemoryMetadata }): Promise<boolean>;
  deleteMemory(memoryId: UUID): Promise<void>;
  deleteManyMemories(memoryIds: UUID[]): Promise<void>;
  deleteAllMemories(roomId: UUID, tableName: string): Promise<void>;
  countMemories(roomId: UUID, unique?: boolean, tableName?: string): Promise<number>;

  // Logging
  log(params: {
    body: { [key: string]: unknown };
    entityId: UUID;
    roomId: UUID;
    type: string;
  }): Promise<void>;
  getLogs(params: {
    entityId?: UUID;
    roomId?: UUID;
    type?: string;
    count?: number;
    offset?: number;
  }): Promise<Log[]>;
  deleteLog(logId: UUID): Promise<void>;
  getAgentRunSummaries?(params: {
    limit?: number;
    roomId?: UUID;
    status?: RunStatus | 'all';
    from?: number;
    to?: number;
    entityId?: UUID;
  }): Promise<AgentRunSummaryResult>;

  // World operations
  createWorld(world: World): Promise<UUID>;
  getWorld(id: UUID): Promise<World | null>;
  removeWorld(id: UUID): Promise<void>;
  getAllWorlds(): Promise<World[]>;
  updateWorld(world: World): Promise<void>;

  // Room operations
  getRoomsByIds(roomIds: UUID[]): Promise<Room[] | null>;
  createRooms(rooms: Room[]): Promise<UUID[]>;
  deleteRoom(roomId: UUID): Promise<void>;
  deleteRoomsByWorldId(worldId: UUID): Promise<void>;
  updateRoom(room: Room): Promise<void>;
  getRoomsForParticipant(entityId: UUID): Promise<UUID[]>;
  getRoomsForParticipants(userIds: UUID[]): Promise<UUID[]>;
  getRoomsByWorld(worldId: UUID): Promise<Room[]>;

  // Participant operations
  removeParticipant(entityId: UUID, roomId: UUID): Promise<boolean>;
  getParticipantsForEntity(entityId: UUID): Promise<Participant[]>;
  getParticipantsForRoom(roomId: UUID): Promise<UUID[]>;
  isRoomParticipant(roomId: UUID, entityId: UUID): Promise<boolean>;
  addParticipantsRoom(entityIds: UUID[], roomId: UUID): Promise<boolean>;
  getParticipantUserState(roomId: UUID, entityId: UUID): Promise<'FOLLOWED' | 'MUTED' | null>;
  setParticipantUserState(
    roomId: UUID,
    entityId: UUID,
    state: 'FOLLOWED' | 'MUTED' | null
  ): Promise<void>;

  // Relationship operations
  createRelationship(params: {
    sourceEntityId: UUID;
    targetEntityId: UUID;
    tags?: string[];
    metadata?: Metadata;
  }): Promise<boolean>;
  updateRelationship(relationship: Relationship): Promise<void>;
  getRelationship(params: {
    sourceEntityId: UUID;
    targetEntityId: UUID;
  }): Promise<Relationship | null>;
  getRelationships(params: { entityId: UUID; tags?: string[] }): Promise<Relationship[]>;

  // Cache operations
  getCache<T>(key: string): Promise<T | undefined>;
  setCache<T>(key: string, value: T): Promise<boolean>;
  deleteCache(key: string): Promise<boolean>;

  // Task operations
  createTask(task: Task): Promise<UUID>;
  getTasks(params: { roomId?: UUID; tags?: string[]; entityId?: UUID }): Promise<Task[]>;
  getTask(id: UUID): Promise<Task | null>;
  getTasksByName(name: string): Promise<Task[]>;
  updateTask(id: UUID, task: Partial<Task>): Promise<void>;
  deleteTask(id: UUID): Promise<void>;

  getMemoriesByWorldId(params: {
    worldId: UUID;
    count?: number;
    tableName?: string;
  }): Promise<Memory[]>;
}
```

---

## Character / Agent Types (`types/agent.ts`)

```typescript
import type { DirectoryItem } from './knowledge';
import type { Content, UUID } from './primitives';
import type { State } from './state';

export interface MessageExample {
  name: string;
  content: Content;
}

export type TemplateType =
  | string
  | ((options: { state: State | { [key: string]: string } }) => string);

export interface Character {
  id?: UUID;
  name: string;
  username?: string;
  system?: string;
  templates?: {
    [key: string]: TemplateType;
  };
  bio: string | string[];
  messageExamples?: MessageExample[][];
  postExamples?: string[];
  topics?: string[];
  adjectives?: string[];
  knowledge?: (string | { path: string; shared?: boolean } | DirectoryItem)[];
  plugins?: string[];
  settings?: {
    [key: string]: string | boolean | number | Record<string, unknown>;
  };
  secrets?: {
    [key: string]: string | boolean | number;
  };
  style?: {
    all?: string[];
    chat?: string[];
    post?: string[];
  };
}

export enum AgentStatus {
  ACTIVE = 'active',
  INACTIVE = 'inactive',
}

export interface Agent extends Character {
  enabled?: boolean;
  status?: AgentStatus;
  createdAt: number;
  updatedAt: number;
}
```

---

## Testing Types (`types/testing.ts`)

```typescript
import type { IAgentRuntime } from './runtime';

export interface TestCase {
  name: string;
  fn: (runtime: IAgentRuntime) => Promise<void> | void;
}

export interface TestSuite {
  name: string;
  tests: TestCase[];
}
```

---

## Settings Types (`types/settings.ts`)

```typescript
export interface RuntimeSettings {
  [key: string]: string | undefined;
}

export interface Setting {
  name: string;
  description: string;
  usageDescription: string;
  value: string | boolean | null;
  required: boolean;
  public?: boolean;
  secret?: boolean;
  validation?: (value: string | boolean | null) => boolean;
  dependsOn?: string[];
  onSetAction?: (value: string | boolean | null) => string;
  visibleIf?: (settings: { [key: string]: Setting }) => boolean;
}

export interface WorldSettings {
  [key: string]: Setting;
}

export interface OnboardingConfig {
  settings: {
    [key: string]: Omit<Setting, 'value'>;
  };
}
```

---

## ElizaOS Orchestrator (`types/elizaos.ts`)

```typescript
import type { UUID, Content } from './primitives';
import type { Memory } from './memory';
import type { IAgentRuntime } from './runtime';
import type { Character } from './agent';
import type { State } from './state';
import type {
  MessageProcessingOptions,
  MessageProcessingResult,
} from '../services/message-service';

export interface BatchOperation {
  agentId: UUID;
  operation: 'message' | 'action' | 'evaluate';
  payload: any;
}

export interface BatchResult {
  agentId: UUID;
  success: boolean;
  result?: any;
  error?: Error;
}

export interface ReadonlyRuntime {
  getAgent(id: UUID): IAgentRuntime | undefined;
  getAgents(): IAgentRuntime[];
  getState(agentId: UUID): State | undefined;
}

export interface HealthStatus {
  alive: boolean;
  responsive: boolean;
  memoryUsage?: number;
  uptime?: number;
}

export interface AgentUpdate {
  id: UUID;
  character: Partial<Character>;
}

export interface HandleMessageOptions extends MessageProcessingOptions {
  onResponse?: (content: Content) => Promise<void>;
  onError?: (error: Error) => Promise<void>;
  onComplete?: () => Promise<void>;
}

export interface HandleMessageResult {
  messageId: UUID;
  userMessage: Memory;
  processing?: MessageProcessingResult;
}

export interface IElizaOS {
  handleMessage(
    agentId: UUID,
    message: Partial<Memory> & {
      entityId: UUID;
      roomId: UUID;
      content: Content;
      worldId?: UUID;
    },
    options?: HandleMessageOptions
  ): Promise<HandleMessageResult>;

  getAgent(agentId: UUID): IAgentRuntime | undefined;
}
```

---

## Knowledge Types (`types/knowledge.ts`)

```typescript
import type { MemoryMetadata } from './memory';
import type { Content, UUID } from './primitives';

export type KnowledgeItem = {
  id: UUID;
  content: Content;
  metadata?: MemoryMetadata;
};

export interface DirectoryItem {
  directory: string;
  shared?: boolean;
}
```

---

## Streaming Types (`types/streaming.ts`)

```typescript
export interface IStreamExtractor {
  readonly done: boolean;
  push(chunk: string): string;
  reset(): void;
  flush?(): string;
}

export interface IStreamingRetryState {
  getStreamedText: () => string;
  isComplete: () => boolean;
  reset: () => void;
}
```

---

## Plugin Loader (`plugin.ts`)

Key functions from `packages/core/src/plugin.ts`:

```typescript
// Validate plugin shape
export function isValidPluginShape(obj: unknown): obj is Plugin;

// Validate plugin structure, returns { isValid: boolean; errors: string[] }
export function validatePlugin(plugin: unknown): { isValid: boolean; errors: string[] };

// Load plugin by name (dynamic import) with auto-install via Bun
export async function loadAndPreparePlugin(pluginName: string): Promise<Plugin | null>;

// Normalize plugin name: '@elizaos/plugin-discord' -> 'discord'
export function normalizePluginName(pluginName: string): string;

// Topological sort of plugins respecting dependencies
export function resolvePluginDependencies(
  availablePlugins: Map<string, Plugin>,
  isTestMode?: boolean
): Plugin[];

// Load a plugin by name or validate a provided Plugin object
export async function loadPlugin(nameOrPlugin: string | Plugin): Promise<Plugin | null>;

// Full resolution with dependency loading
export async function resolvePlugins(
  plugins: (string | Plugin)[],
  isTestMode?: boolean
): Promise<Plugin[]>;
```

Plugin loading behavior:

- Attempts `import(pluginName)` first
- Falls back to auto-install via `bun add pluginName` if allowed
- Searches for exports: camelCase function name, `.default`, then all exports
- Validates plugin shape (must have `name` and at least one of: `init`, `services`, `providers`, `actions`, `evaluators`, `description`)
- Dependencies resolved via topological sort with circular dependency detection
- Test dependencies only resolved when `isTestMode = true`

---

## Runtime Implementation

The `AgentRuntime` class in `packages/core/src/runtime.ts` implements `IAgentRuntime`. Key aspects:

### Constructor

```typescript
constructor(opts: {
  conversationLength?: number;
  agentId?: UUID;
  character?: Character;
  plugins?: Plugin[];
  fetch?: typeof fetch;
  adapter?: IDatabaseAdapter;
  settings?: RuntimeSettings;
  allAvailablePlugins?: Plugin[];
})
```

### Plugin Registration Flow

When `registerPlugin(plugin)` is called, the runtime:

1. Checks for duplicate plugin names (skips if already registered)
2. Pushes plugin to `this.plugins` array
3. Calls `plugin.init(config, runtime)` if defined
4. Registers `plugin.adapter` if defined
5. Registers all `plugin.actions` via `registerAction(action)`
6. Registers all `plugin.evaluators` via `registerEvaluator(evaluator)`
7. Registers all `plugin.providers` via `registerProvider(provider)`
8. Registers all `plugin.models` via `registerModel(modelType, handler, pluginName, priority)`
9. Registers all `plugin.routes` (namespaced with plugin name: `/${pluginName}${route.path}`)
10. Registers all `plugin.events` via `registerEvent(eventName, handler)`
11. Registers all `plugin.services` asynchronously via `registerService(ServiceClass)`

---

## Reference Plugins

### plugin-bootstrap (Full Reference)

The bootstrap plugin (`packages/plugin-bootstrap/src/index.ts`) demonstrates the canonical plugin structure:

```typescript
export const bootstrapPlugin: Plugin = {
  name: 'bootstrap',
  description: 'Agent bootstrap with basic actions and evaluators',
  actions: [
    actions.replyAction,
    actions.followRoomAction,
    actions.unfollowRoomAction,
    actions.ignoreAction,
    actions.noneAction,
    actions.muteRoomAction,
    actions.unmuteRoomAction,
    actions.sendMessageAction,
    actions.updateEntityAction,
    actions.choiceAction,
    actions.updateRoleAction,
    actions.updateSettingsAction,
    actions.generateImageAction,
  ],
  events, // Handles MESSAGE_RECEIVED, WORLD_JOINED, WORLD_CONNECTED, etc.
  evaluators: [evaluators.reflectionEvaluator],
  providers: [
    providers.evaluatorsProvider,
    providers.anxietyProvider,
    providers.timeProvider,
    providers.entitiesProvider,
    providers.relationshipsProvider,
    providers.choiceProvider,
    providers.factsProvider,
    providers.roleProvider,
    providers.settingsProvider,
    providers.attachmentsProvider,
    providers.providersProvider,
    providers.actionsProvider,
    providers.actionStateProvider,
    providers.characterProvider,
    providers.recentMessagesProvider,
    providers.worldProvider,
  ],
  services: [TaskService, EmbeddingGenerationService],
};
```

### plugin-starter (Scaffold Template)

The starter plugin (`packages/plugin-starter/src/plugin.ts`) is what `elizaos create --type plugin` generates:

```typescript
export const starterPlugin: Plugin = {
  name: 'plugin-starter',
  description: 'Plugin starter for elizaOS',
  config: {
    EXAMPLE_PLUGIN_VARIABLE: process.env.EXAMPLE_PLUGIN_VARIABLE,
  },
  async init(config: Record<string, string>) {
    // Validate config with zod schema
    const validatedConfig = await configSchema.parseAsync(config);
    for (const [key, value] of Object.entries(validatedConfig)) {
      if (value) process.env[key] = value;
    }
  },
  models: {
    [ModelType.TEXT_SMALL]: async (_runtime, { prompt, stopSequences = [] }: GenerateTextParams) => {
      return 'placeholder response';
    },
    [ModelType.TEXT_LARGE]: async (_runtime, { prompt, ... }: GenerateTextParams) => {
      return 'placeholder response';
    },
  },
  routes: [
    {
      name: 'hello-world-route',
      path: '/helloworld',
      type: 'GET',
      handler: async (_req: RouteRequest, res: RouteResponse) => {
        res.json({ message: 'Hello World!' });
      },
    },
  ],
  events: {
    MESSAGE_RECEIVED: [
      async (params) => { /* handle message received */ },
    ],
    VOICE_MESSAGE_RECEIVED: [
      async (params) => { /* handle voice message */ },
    ],
    WORLD_CONNECTED: [
      async (params) => { /* handle world connected */ },
    ],
    WORLD_JOINED: [
      async (params) => { /* handle world joined */ },
    ],
  },
  services: [StarterService],
  actions: [helloWorldAction],
  providers: [helloWorldProvider],
  // dependencies: ['@elizaos/plugin-knowledge'],
};
```

### Starter Service Example

```typescript
export class StarterService extends Service {
  static serviceType = 'starter';
  capabilityDescription = 'This is a starter service...';

  constructor(protected runtime: IAgentRuntime) {
    super(runtime);
  }

  static async start(runtime: IAgentRuntime) {
    const service = new StarterService(runtime);
    return service;
  }

  static async stop(runtime: IAgentRuntime) {
    const service = runtime.getService(StarterService.serviceType);
    if (!service) throw new Error('Starter service not found');
    service.stop();
  }

  async stop() {
    // cleanup
  }
}
```

### Starter Action Example

```typescript
const helloWorldAction: Action = {
  name: 'HELLO_WORLD',
  similes: ['GREET', 'SAY_HELLO'],
  description: 'Responds with a simple hello world message',

  validate: async (
    _runtime: IAgentRuntime,
    _message: Memory,
    _state: State | undefined
  ): Promise<boolean> => {
    return true;
  },

  handler: async (
    _runtime: IAgentRuntime,
    message: Memory,
    _state: State | undefined,
    _options: any,
    callback?: HandlerCallback,
    _responses?: Memory[]
  ): Promise<ActionResult> => {
    try {
      const response = 'Hello world!';
      if (callback) {
        await callback({
          text: response,
          actions: ['HELLO_WORLD'],
          source: message.content.source,
        });
      }
      return {
        text: response,
        success: true,
        data: {
          actions: ['HELLO_WORLD'],
          source: message.content.source,
        },
      };
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error : new Error(String(error)),
      };
    }
  },

  examples: [
    [
      {
        name: '{{userName}}',
        content: { text: 'hello', actions: [] },
      },
      {
        name: '{{agentName}}',
        content: { text: 'Hello world!', actions: ['HELLO_WORLD'] },
      },
    ],
  ],
};
```

### Starter Provider Example

```typescript
const helloWorldProvider: Provider = {
  name: 'HELLO_WORLD_PROVIDER',
  description: 'A simple example provider',

  get: async (
    _runtime: IAgentRuntime,
    _message: Memory,
    _state: State | undefined
  ): Promise<ProviderResult> => {
    return {
      text: 'I am a provider',
      values: {},
      data: {},
    };
  },
};
```

---

## Core Public API Exports

From `packages/core/src/index.ts`, the public API re-exports:

- All types from `./types` (barrel export)
- Utilities from `./utils`
- Character schemas from `./schemas/character`
- Character utilities from `./character`
- Environment utilities from `./utils/environment`
- Buffer utilities from `./utils/buffer`
- Streaming utilities from `./utils/streaming`
- Path utilities from `./utils/paths`
- Actions from `./actions`
- Database from `./database`
- Entities from `./entities`
- Logger from `./logger`
- Memory from `./memory`
- Prompts from `./prompts`
- Roles from `./roles`
- Runtime from `./runtime`
- Secrets from `./secrets`
- Settings from `./settings`
- Services from `./services`
- Message service from `./services/message-service`
- Default message service from `./services/default-message-service`
- Search (BM25) from `./search`
- ElizaOS from `./elizaos`
- Streaming context from `./streaming-context`
- Request context from `./request-context`
- Server health from `./utils/server-health`

Everything is importable from `@elizaos/core`.
