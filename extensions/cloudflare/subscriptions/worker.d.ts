/// <reference types="@cloudflare/workers-types" />

export declare const PRIVATE_ROUTING_HEADER: "x-incurs-private";
export declare const PRIVATE_ROUTING_VALUE: "incurs-subscriptions-v1";
export declare const SCOPE_HEADER: "x-incurs-scope";
export declare const RETENTION_MS: number;
export declare const MAX_RETAINED_EVENTS: number;
export declare const MAX_SUBSCRIBER_EVENTS: number;
export declare const MAX_SUBSCRIBER_BYTES: number;
export declare const HEARTBEAT_MS: number;

export type Awaitable<T> = T | Promise<T>;

export interface TrustedSubscriptionScope {
  key: string;
}

export interface ChangeEnvelope {
  schema_version: 1;
  source_id: string;
  resource_uris: string[];
  revision: string;
  kind: "updated" | "deleted";
  occurred_at: string;
}

export interface AppendResult {
  cursor: string;
  source_id: string;
  deduplicated: boolean;
}

export interface KnownSourceIdsPage {
  source_ids: string[];
  next_after: string | null;
}

export interface SubscriptionChangeData {
  cursor: string;
  event: ChangeEnvelope;
}

export interface SubscriptionResetData {
  cursor: string;
  resource_uris: string[];
}

export interface SubscriptionHeartbeatData {
  cursor: string;
}

export interface SubscriptionChangeFrame {
  event: "change";
  data: SubscriptionChangeData;
}

export interface SubscriptionResetFrame {
  event: "reset";
  data: SubscriptionResetData;
}

export interface SubscriptionHeartbeatFrame {
  event: "heartbeat";
  data: SubscriptionHeartbeatData;
}

export type SubscriptionFrame = SubscriptionChangeFrame | SubscriptionResetFrame | SubscriptionHeartbeatFrame;

export type ScopeResolver<Env = Record<string, unknown>, Ctx = ExecutionContext> = (
  request: Request,
  env: Env,
  context: Ctx,
) => Awaitable<TrustedSubscriptionScope | null | undefined>;

export type Authorizer<Env = Record<string, unknown>, Ctx = ExecutionContext> = (
  request: Request,
  env: Env,
  context: Ctx,
) => Awaitable<boolean>;

export interface IncursSubscriptionWorkerOptions<Env = Record<string, unknown>, Ctx = ExecutionContext> {
  bindingName?: string;
  authorize?: Authorizer<Env, Ctx>;
  resolveScope?: ScopeResolver<Env, Ctx>;
}

export interface IncursSubscriptionWorker<Env = Record<string, unknown>, Ctx = ExecutionContext> {
  fetch(request: Request, env: Env, context: Ctx): Promise<Response>;
}

export declare function createIncursSubscriptionWorker<
  Env = Record<string, unknown>,
  Ctx = ExecutionContext,
>(options?: IncursSubscriptionWorkerOptions<Env, Ctx>): IncursSubscriptionWorker<Env, Ctx>;

export declare class IncursSubscriptionDelivery {
  constructor(state: DurableObjectState);

  state: DurableObjectState;
  sessions: Map<number, unknown>;
  nextSession: number;
  ready: Promise<void>;

  readonly sql: unknown;

  migrate(): Promise<void>;
  fetch(request: Request): Promise<Response>;
  append(scope: string, event: ChangeEnvelope): Response;
  nextCursor(scope: string): number;
  currentCursor(scope: string): number;
  knownSourceIds(scope: string, url: URL): Response;
  listen(scope: string, url: URL): Response;
  replay(scope: string, filters: string[], cursor: number): SubscriptionFrame[];
  resetFrame(scope: string, resourceUris: string[]): SubscriptionResetFrame;
  prune(scope: string, now: number): void;
  broadcast(scope: string, frame: SubscriptionFrame): void;
  enqueueToSession(id: number, session: unknown, frame: SubscriptionFrame): boolean;
  dropSession(id: number, session?: unknown): void;
  allKnownSourceIds(scope: string): string[];
}
