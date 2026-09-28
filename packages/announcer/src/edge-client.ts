/**
 * Polls the Rust connector's client edge (`crates/connector-client-edge`) for
 * the facts it already proved at startup, per connector#681's re-scope:
 * ADR 0022 forbids the connector pushing an announcement itself, so this
 * sidecar ASKS instead — exactly the "answers when asked" surface ADR 0022
 * carved out.
 *
 * Two answers are polled:
 * - `GET /ilp/identity` (lib.rs ~202-207): this node's client-edge identity
 *   (`keyId` + the ADR 0018 wrap public key). Cheap, unauthenticated, always
 *   available.
 * - The x402 payment-required greeting: triggered by an unpaid `POST /ilp`
 *   addressing a priced route, decoded from the `payment-required` response
 *   header (base64 JSON, client-edge-spec.md §1.4). Its `batch-settlement`
 *   `accepts[]` entries are where the chains, receiving addresses and tokens
 *   come from, and its `extensions.toon` terms are where the route price
 *   comes from — the sidecar never hardcodes them.
 *
 * Both are best-effort: a failure to reach the edge, a non-402 answer, or a
 * malformed header logs and returns `null`/omits the route, exactly like the
 * retired `SelfAnnounceService`'s "never crash the refresh loop" contract.
 * The announcement still goes out with whatever facts WERE resolved.
 *
 * @module edge-client
 */

import type { Logger } from 'pino';
import { encodePrepare } from './oer';

/** `GET /ilp/identity` response shape (client-edge-spec.md §1.2, ADR 0018). */
export interface ClientEdgeIdentity {
  keyId: string;
  publicKey: string;
}

/**
 * One x402 `batch-settlement` entry of the greeting's `accepts[]` (ADR 0074
 * decision 8, ADR 0075 decision 10): the chain it settles on (`network`,
 * CAIP-2 -- `eip155:<chainId>` or `solana:<genesis-hash>`), the token
 * (`asset`) and the receiver a payer's channel names (`payTo`). Since ADR
 * 0075 (connector#1384) these are the whole list: the `toon-channel` entry,
 * and its `TokenNetwork` / TOON-program settlement terms, are gone.
 */
export interface X402BatchSettlementOffer {
  network: string;
  asset: string;
  payTo: string;
  extra: Record<string, unknown>;
}

/** One route's resolved greeting facts. */
export interface RouteGreeting {
  destination: string;
  /** The route's price schedule base, from `extensions.toon.info.price`. */
  price: string;
  /** The path ILP packets are posted to, from `extensions.toon.info.endpoint`. */
  endpoint: string;
  /** Every `batch-settlement` entry of `accepts[]`, one per chain the node settles on. */
  batchSettlements: X402BatchSettlementOffer[];
}

const PAYMENT_REQUIRED_HEADER = 'payment-required';

export interface EdgeClientOptions {
  /** Base URL of the Rust client edge, e.g. `http://connector-rust:4000`. Never advertised. */
  baseUrl: string;
  /** Per-request timeout. */
  timeoutMs: number;
  logger: Logger;
  /** Injectable for tests; defaults to the global `fetch`. */
  fetchImpl?: typeof fetch;
}

function withTimeout(timeoutMs: number): { signal: AbortSignal; cancel: () => void } {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  timer.unref?.();
  return { signal: controller.signal, cancel: () => clearTimeout(timer) };
}

/**
 * `GET /ilp/identity`. Returns `null` (logged) on any failure — this is
 * informational content for the announce, never load-bearing for whether the
 * sidecar keeps running.
 */
export async function fetchIdentity(opts: EdgeClientOptions): Promise<ClientEdgeIdentity | null> {
  const fetchFn = opts.fetchImpl ?? fetch;
  const { signal, cancel } = withTimeout(opts.timeoutMs);
  try {
    const res = await fetchFn(`${opts.baseUrl}/ilp/identity`, { signal });
    if (!res.ok) {
      opts.logger.warn(
        { event: 'edge_identity_failed', status: res.status },
        'GET /ilp/identity did not return 200'
      );
      return null;
    }
    const body = (await res.json()) as Partial<ClientEdgeIdentity>;
    if (typeof body.keyId !== 'string' || typeof body.publicKey !== 'string') {
      opts.logger.warn(
        { event: 'edge_identity_malformed' },
        'GET /ilp/identity returned an unexpected shape'
      );
      return null;
    }
    return { keyId: body.keyId, publicKey: body.publicKey };
  } catch (err) {
    opts.logger.warn(
      { event: 'edge_identity_error', err: errMsg(err) },
      'Failed to reach GET /ilp/identity'
    );
    return null;
  } finally {
    cancel();
  }
}

/**
 * Trigger and decode the x402 greeting for one route by sending an unpaid
 * `POST /ilp` (a bare PREPARE, no claim header) addressing `destination`.
 * `client-edge-spec.md` §1.4: this connector answers with a `402` carrying
 * the `payment-required` header (base64 JSON) exactly when the route is
 * locally-terminated and priced and no claim was attached — which is always
 * true here, since the sidecar attaches none on purpose.
 *
 * Returns `null` (logged) when the route isn't priced/terminated here (no
 * 402 came back), or the header is missing/malformed.
 */
export async function fetchGreeting(
  destination: string,
  opts: EdgeClientOptions
): Promise<RouteGreeting | null> {
  const fetchFn = opts.fetchImpl ?? fetch;
  const prepare = encodePrepare({
    amount: 0,
    expiresAt: new Date(Date.now() + 30_000),
    greeting: true,
    destination,
    data: Buffer.alloc(0),
  });

  const { signal, cancel } = withTimeout(opts.timeoutMs);
  try {
    const res = await fetchFn(`${opts.baseUrl}/ilp`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: prepare,
      signal,
    });
    if (res.status !== 402) {
      opts.logger.warn(
        { event: 'edge_greeting_not_402', destination, status: res.status },
        'POST /ilp did not answer with a 402 greeting (route may be unpriced or unterminated here)'
      );
      return null;
    }
    const header = res.headers.get(PAYMENT_REQUIRED_HEADER);
    if (!header) {
      opts.logger.warn(
        { event: 'edge_greeting_missing_header', destination },
        '402 response carried no payment-required header'
      );
      return null;
    }
    return parseGreetingHeader(header, destination, opts.logger);
  } catch (err) {
    opts.logger.warn(
      { event: 'edge_greeting_error', destination, err: errMsg(err) },
      'Failed to fetch the x402 greeting'
    );
    return null;
  } finally {
    cancel();
  }
}

/**
 * Decode the base64 `payment-required` header into a {@link RouteGreeting}.
 * Never throws.
 *
 * The route's TOON terms are read from `extensions.toon.info` (x402 v2's
 * extension slot, connector#1384); the channel terms from the
 * `batch-settlement` entries of `accepts[]` alone. A greeting from a
 * connector that predates ADR 0075 -- its terms on a `toon-channel`
 * `accepts[]` entry, no `extensions.toon` -- does not parse, and is logged.
 */
export function parseGreetingHeader(
  header: string,
  destination: string,
  logger: Logger
): RouteGreeting | null {
  try {
    const json = JSON.parse(Buffer.from(header, 'base64').toString('utf8')) as {
      accepts?: unknown[];
      extensions?: { toon?: { info?: Record<string, unknown> } };
    };
    const info = json.extensions?.toon?.info;
    if (!info || typeof info.price !== 'string') {
      logger.warn(
        { event: 'edge_greeting_malformed', destination },
        'payment-required header carried no extensions.toon terms'
      );
      return null;
    }
    const batchSettlements = Array.isArray(json.accepts)
      ? json.accepts.filter(isBatchSettlementOffer)
      : [];
    return {
      destination,
      price: info.price,
      endpoint: typeof info.endpoint === 'string' ? info.endpoint : '/ilp',
      batchSettlements,
    };
  } catch (err) {
    logger.warn(
      { event: 'edge_greeting_decode_failed', destination, err: errMsg(err) },
      'Failed to base64/JSON-decode the payment-required header'
    );
    return null;
  }
}

function isBatchSettlementOffer(value: unknown): value is X402BatchSettlementOffer {
  if (typeof value !== 'object' || value === null) return false;
  const entry = value as Record<string, unknown>;
  return (
    entry.scheme === 'batch-settlement' &&
    typeof entry.network === 'string' &&
    typeof entry.asset === 'string' &&
    typeof entry.payTo === 'string' &&
    typeof entry.extra === 'object' &&
    entry.extra !== null
  );
}

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
