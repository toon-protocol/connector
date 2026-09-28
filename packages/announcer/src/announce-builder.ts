/**
 * Assembles the sidecar's kind:10032 {@link IlpPeerInfo} content from what it
 * polled off the Rust edge (connector#681's re-scope) — the sidecar's own
 * analogue of the retired TypeScript connector's
 * `discovery/self-announce-builder.ts`, minus everything that builder
 * derived from a live `ConnectorConfig` the Rust connector has no equivalent
 * of. Every settlement/contract/price fact here comes from the edge's own
 * answers (`fetchIdentity` / `fetchGreeting`); only the PUBLIC endpoints
 * (`/rust/ilp`, the BTP URL) and the addressing are sidecar config, because
 * the edge has no way to know its own public-facing hostname behind TLS
 * termination — same reasoning the retired builder's header gives for why
 * `selfAnnounce`'s endpoint fields were always operator overrides, never
 * inferred.
 *
 * @module announce-builder
 */

import type { ClientEdgeIdentity, RouteGreeting } from './edge-client';
import type { IlpPeerInfo, OperatorNotice } from './event';

/** Static (never edge-derived) parts of the announcement — sidecar config. */
export interface AnnounceStaticConfig {
  /** Primary ILP address to advertise, e.g. `g.toon`. */
  ilpAddress: string;
  /** All addresses this announce covers (primary first), e.g. `[g.toon, g.toon.relay, g.toon.ario, g.toon.store]`. */
  ilpAddresses: string[];
  /** Public ILP-over-HTTP ingress, e.g. `https://proxy.devnet.toonprotocol.dev/rust/ilp`. */
  httpEndpoint: string;
  /** Public BTP WebSocket endpoint, e.g. `wss://proxy.devnet.toonprotocol.dev/rust/ilp/btp` (connector#680). */
  btpEndpoint: string;
  /** Public Nostr relay WS URL for free reads, if advertised. */
  relayUrl?: string;
  assetCode: string;
  assetScale: number;
  /** The address a client should PUBLISH (Nostr writes) to. */
  routePublish: string;
  /** The address a client should STORE (blob uploads) to. */
  routeStore: string;
  /**
   * The x402 greeting's Solana `batch-settlement` entry names its chain by
   * CAIP-2 genesis hash (`solana:<hash>`), not by cluster name. Kind:10032
   * consumers key chains as `solana:<cluster>`, so every `solana:*` network
   * is announced under this id (default `solana:devnet`, matching this
   * fleet's only deployed cluster).
   */
  solanaChainId: string;
  /**
   * Operator notice (toon#183's `IlpPeerInfo.notice`), sourced straight from
   * config — never composed or inferred here. `undefined` (the overwhelming
   * common case) omits the key entirely; see {@link buildAnnouncementInfo}.
   */
  notice?: OperatorNotice;
}

/**
 * Build the sidecar's kind:10032 content from its static config plus
 * whatever the edge answered. `identity` and each entry of `greetings` may
 * be `null`/absent (a poll failure) — every field they would have populated
 * is simply omitted, exactly like the retired builder degrades gracefully
 * on a resolver failure.
 */
export function buildAnnouncementInfo(
  config: AnnounceStaticConfig,
  identity: ClientEdgeIdentity | null,
  greetings: RouteGreeting[]
): IlpPeerInfo {
  const supportedChains: string[] = [];
  const settlementAddresses: Record<string, string> = {};
  const preferredTokens: Record<string, string> = {};
  const routePrices: Record<string, string> = {};

  // CAIP-2 `eip155:<id>` is announced as TOON's `evm:<id>`; any `solana:*`
  // as the configured cluster id. Anything else passes through unchanged.
  const chainOf = (network: string): string => {
    if (network.startsWith('eip155:')) return `evm:${network.slice('eip155:'.length)}`;
    if (network === 'solana' || network.startsWith('solana:')) return config.solanaChainId;
    return network;
  };

  for (const greeting of greetings) {
    routePrices[greeting.destination] = greeting.price;
    // ADR 0075 (connector#1384): the greeting's `batch-settlement` entries
    // are the only channel terms there are. There is no TokenNetwork or
    // TOON program to announce any more, so `tokenNetworks` is gone.
    for (const offer of greeting.batchSettlements) {
      const chainId = chainOf(offer.network);
      if (!supportedChains.includes(chainId)) supportedChains.push(chainId);
      settlementAddresses[chainId] = offer.payTo;
      preferredTokens[chainId] = offer.asset;
    }
  }

  const info: IlpPeerInfo = {
    ilpAddress: config.ilpAddress,
    ...(config.ilpAddresses.length > 1 ? { ilpAddresses: config.ilpAddresses } : {}),
    btpEndpoint: config.btpEndpoint,
    httpEndpoint: config.httpEndpoint,
    ...(config.relayUrl ? { relayUrl: config.relayUrl } : {}),
    assetCode: config.assetCode,
    assetScale: config.assetScale,
    ...(supportedChains.length > 0 ? { supportedChains } : {}),
    ...(Object.keys(settlementAddresses).length > 0 ? { settlementAddresses } : {}),
    ...(Object.keys(preferredTokens).length > 0 ? { preferredTokens } : {}),
    ...(Object.keys(routePrices).length > 0 ? { routePrices } : {}),
    ...(identity ? { edgeIdentity: { keyId: identity.keyId, publicKey: identity.publicKey } } : {}),
    routes: { publish: config.routePublish, store: config.routeStore },
    ...(config.notice ? { notice: config.notice } : {}),
  };

  return info;
}
