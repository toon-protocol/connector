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
 * - The x402 payment-required greeting (lib.rs ~365-368 documents
 *   `X402SettlementTerms` as exactly the facts a kind:10032 announce would
 *   otherwise carry): triggered by an unpaid `POST /ilp` addressing a priced
 *   route, decoded from the `payment-required` response header (base64 JSON,
 *   client-edge-spec.md §1.4). This is where the settlement contract
 *   addresses, token addresses and route price come from — the sidecar never
 *   hardcodes them.
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

/** The EVM-shaped channel-opening facts (issue #617). */
export interface X402EvmSettlementTerms {
  chain: string;
  settlementAddress: string;
  tokenNetworkRegistry: string;
  tokenNetwork: string;
  tokenAddress: string;
  decimals: number;
}

/** The Solana twin (issue #632). Structurally disjoint from the EVM shape (no `tokenNetworkRegistry`). */
export interface X402SolanaSettlementTerms {
  chain: string;
  settlementAddress: string;
  programId: string;
  tokenAddress: string;
  decimals: number;
}

export type X402ChainSettlementTerms = X402EvmSettlementTerms | X402SolanaSettlementTerms;

export function isEvmSettlementTerms(
  terms: X402ChainSettlementTerms
): terms is X402EvmSettlementTerms {
  return 'tokenNetworkRegistry' in terms;
}

export function isSolanaSettlementTerms(
  terms: X402ChainSettlementTerms
): terms is X402SolanaSettlementTerms {
  return 'programId' in terms;
}

/** One route's resolved greeting facts. */
export interface RouteGreeting {
  destination: string;
  price: string;
  httpEndpoint: string;
  settlement?: X402EvmSettlementTerms;
  settlements: X402ChainSettlementTerms[];
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

/** Decode the base64 `payment-required` header into a {@link RouteGreeting}. Never throws. */
export function parseGreetingHeader(
  header: string,
  destination: string,
  logger: Logger
): RouteGreeting | null {
  try {
    const json: unknown = JSON.parse(Buffer.from(header, 'base64').toString('utf8'));
    const accepts = (json as { accepts?: unknown[] }).accepts;
    const entry = Array.isArray(accepts)
      ? (accepts[0] as Record<string, unknown> | undefined)
      : undefined;
    const extra = entry?.extra as Record<string, unknown> | undefined;
    if (
      !entry ||
      !extra ||
      typeof entry.httpEndpoint !== 'string' ||
      typeof extra.price !== 'string'
    ) {
      logger.warn(
        { event: 'edge_greeting_malformed', destination },
        'payment-required header did not parse to the expected shape'
      );
      return null;
    }
    const settlement = isEvmLike(extra.settlement)
      ? (extra.settlement as X402EvmSettlementTerms)
      : undefined;
    const settlements = Array.isArray(extra.settlements)
      ? (extra.settlements as unknown[]).filter(isChainSettlementLike)
      : [];
    return {
      destination,
      price: extra.price,
      httpEndpoint: entry.httpEndpoint,
      settlement,
      settlements,
    };
  } catch (err) {
    logger.warn(
      { event: 'edge_greeting_decode_failed', destination, err: errMsg(err) },
      'Failed to base64/JSON-decode the payment-required header'
    );
    return null;
  }
}

function isEvmLike(value: unknown): value is X402EvmSettlementTerms {
  return typeof value === 'object' && value !== null && 'tokenNetworkRegistry' in value;
}

function isChainSettlementLike(value: unknown): value is X402ChainSettlementTerms {
  if (typeof value !== 'object' || value === null) return false;
  return 'tokenNetworkRegistry' in value || 'programId' in value;
}

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
