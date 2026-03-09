import * as React from 'react';
import {
  Users,
  Copy,
  Check,
  ExternalLink,
  RefreshCw,
  Network,
  CheckCircle2,
  Circle,
} from 'lucide-react';
import { usePeers, PeerInfo } from '@/hooks/usePeers';
import { useRoutingTable, RoutingEntry } from '@/hooks/useRoutingTable';
import { Card, CardHeader, CardTitle, CardContent, CardAction } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';
import { cn } from '@/lib/utils';

/**
 * Blockchain explorer URL generators for supported networks.
 */
const BLOCKCHAIN_EXPLORERS = {
  evm: {
    name: 'Base Sepolia',
    url: (address: string) => `https://sepolia.basescan.org/address/${address}`,
  },
  xrp: {
    name: 'XRPL Testnet',
    url: (address: string) => `https://testnet.xrpl.org/accounts/${address}`,
  },
};

/**
 * Truncate an address string for display (first 8 + last 6 chars).
 */
function truncateAddress(address: string): string {
  if (address.length <= 17) return address;
  return `${address.slice(0, 8)}...${address.slice(-6)}`;
}

/**
 * CopyButton — click-to-copy with brief checkmark feedback.
 */
function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = React.useState(false);

  const handleCopy = React.useCallback(async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API unavailable — ignore silently
    }
  }, [text]);

  return (
    <button
      onClick={handleCopy}
      className="inline-flex items-center text-muted-foreground hover:text-foreground transition-colors"
      title={`Copy: ${text}`}
    >
      {copied ? <Check className="h-3 w-3 text-green-500" /> : <Copy className="h-3 w-3" />}
    </button>
  );
}

/**
 * BlockchainAddressLink — displays a truncated address with external link to blockchain explorer.
 */
function BlockchainAddressLink({ address, type }: { address: string; type: 'evm' | 'xrp' }) {
  const explorer = BLOCKCHAIN_EXPLORERS[type];
  const truncated = truncateAddress(address);

  return (
    <a
      href={explorer.url(address)}
      target="_blank"
      rel="noopener noreferrer"
      className="inline-flex items-center gap-1 font-mono text-xs text-cyan-500 hover:text-cyan-400 hover:underline"
      title={`View on ${explorer.name}: ${address}`}
    >
      {truncated}
      <ExternalLink className="h-3 w-3" />
    </a>
  );
}

/**
 * SummaryStats — displays total peers, connected count, and total routes.
 */
const SummaryStats = React.memo(function SummaryStats({
  peers,
  routes,
}: {
  peers: PeerInfo[];
  routes: RoutingEntry[];
}) {
  const connectedCount = React.useMemo(() => peers.filter((p) => p.connected).length, [peers]);

  return (
    <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
      {/* Total Peers */}
      <div className="rounded-lg border border-border bg-card pl-0 overflow-hidden hover:border-primary/30 transition-colors">
        <div className="flex items-stretch">
          <div className="w-1 bg-blue-500 shrink-0" />
          <div className="flex items-center gap-3 px-4 py-3">
            <Users className="h-5 w-5 text-blue-500 shrink-0" />
            <div>
              <div className="text-2xl font-bold font-mono tabular-nums leading-tight">
                {peers.length}
              </div>
              <div className="text-xs text-muted-foreground mt-0.5">Total Peers</div>
            </div>
          </div>
        </div>
      </div>

      {/* Connected Count */}
      <div className="rounded-lg border border-border bg-card pl-0 overflow-hidden hover:border-primary/30 transition-colors">
        <div className="flex items-stretch">
          <div
            className={cn('w-1 shrink-0', connectedCount > 0 ? 'bg-emerald-500' : 'bg-gray-500')}
          />
          <div className="flex items-center gap-3 px-4 py-3">
            <CheckCircle2
              className={cn(
                'h-5 w-5 shrink-0',
                connectedCount > 0 ? 'text-emerald-500' : 'text-gray-500'
              )}
            />
            <div>
              <div
                className={cn(
                  'text-2xl font-bold font-mono tabular-nums leading-tight',
                  connectedCount > 0 ? 'text-emerald-500' : ''
                )}
              >
                {connectedCount}
              </div>
              <div className="text-xs text-muted-foreground mt-0.5">Connected</div>
            </div>
          </div>
        </div>
      </div>

      {/* Total Routes */}
      <div className="rounded-lg border border-border bg-card pl-0 overflow-hidden hover:border-primary/30 transition-colors">
        <div className="flex items-stretch">
          <div
            className={cn('w-1 shrink-0', routes.length > 0 ? 'bg-cyan-500' : 'bg-cyan-500/50')}
          />
          <div className="flex items-center gap-3 px-4 py-3">
            <Network
              className={cn(
                'h-5 w-5 shrink-0',
                routes.length > 0 ? 'text-cyan-500' : 'text-cyan-500/50'
              )}
            />
            <div>
              <div className="text-2xl font-bold font-mono tabular-nums leading-tight">
                {routes.length}
              </div>
              <div className="text-xs text-muted-foreground mt-0.5">Total Routes</div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
});

/**
 * PeerCard — displays a single peer's information with NOC styling.
 */
const PeerCard = React.memo(function PeerCard({ peer, id }: { peer: PeerInfo; id: string }) {
  return (
    <Card
      id={id}
      className="rounded-lg border border-border bg-card/80 border-border/50 p-4 space-y-3 hover:border-primary/50 transition-colors"
    >
      {/* Peer ID + Connection Status */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span
            className={cn(
              'w-2 h-2 rounded-full shrink-0',
              peer.connected ? 'bg-emerald-500' : 'bg-gray-500'
            )}
          />
          <span className="font-mono text-sm font-medium truncate">
            {peer.petname || peer.peerId}
          </span>
        </div>
        <Badge
          variant="outline"
          className={cn(
            'text-xs',
            peer.connected
              ? 'border-emerald-500 text-emerald-500 bg-emerald-500/10'
              : 'border-gray-500 text-gray-500 bg-gray-500/10'
          )}
        >
          {peer.connected ? (
            <>
              <CheckCircle2 className="h-3 w-3 mr-1" />
              Connected
            </>
          ) : (
            <>
              <Circle className="h-3 w-3 mr-1" />
              Disconnected
            </>
          )}
        </Badge>
      </div>

      {/* ILP Address */}
      {peer.ilpAddress && (
        <div className="space-y-0.5">
          <div className="text-xs text-muted-foreground">ILP Address</div>
          <div className="flex items-center gap-1">
            <span className="font-mono text-xs text-cyan-500 break-all">{peer.ilpAddress}</span>
            <CopyButton text={peer.ilpAddress} />
          </div>
        </div>
      )}

      {/* EVM Address - Blockchain Explorer Link */}
      {peer.evmAddress && (
        <div className="space-y-0.5">
          <div className="text-xs text-muted-foreground">EVM Address</div>
          <BlockchainAddressLink address={peer.evmAddress} type="evm" />
        </div>
      )}

      {/* BTP URL */}
      {peer.btpUrl && (
        <div className="space-y-0.5">
          <div className="text-xs text-muted-foreground">BTP URL</div>
          <span className="font-mono text-xs text-muted-foreground break-all">{peer.btpUrl}</span>
        </div>
      )}

      {/* Pubkey (if different from petname/peerId) */}
      {peer.pubkey && (
        <div className="space-y-0.5">
          <div className="text-xs text-muted-foreground">Pubkey</div>
          <div className="flex items-center gap-1">
            <span className="font-mono text-xs" title={peer.pubkey}>
              {truncateAddress(peer.pubkey)}
            </span>
            <CopyButton text={peer.pubkey} />
          </div>
        </div>
      )}
    </Card>
  );
});

/**
 * RoutingTable — displays routing table entries with NOC styling.
 */
const RoutingTableView = React.memo(function RoutingTableView({
  routes,
  onPeerClick,
}: {
  routes: RoutingEntry[];
  onPeerClick: (peerId: string) => void;
}) {
  const sorted = React.useMemo(
    () => [...routes].sort((a, b) => a.prefix.localeCompare(b.prefix)),
    [routes]
  );

  if (sorted.length === 0) {
    return <p className="text-sm text-muted-foreground py-4">No routing entries configured</p>;
  }

  return (
    <div className="rounded-lg border border-border overflow-hidden">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead className="w-[50%]">Prefix</TableHead>
            <TableHead className="w-[35%]">Next Hop</TableHead>
            <TableHead className="w-[15%] text-right">Priority</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {sorted.map((route, idx) => (
            <TableRow key={`${route.prefix}-${idx}`}>
              <TableCell className="font-mono text-xs text-cyan-500">{route.prefix}</TableCell>
              <TableCell>
                <button
                  className="font-mono text-xs text-blue-400 hover:text-blue-300 hover:underline transition-colors"
                  onClick={() => onPeerClick(route.nextHop)}
                >
                  {route.nextHop}
                </button>
              </TableCell>
              <TableCell className="text-right">
                {route.priority !== undefined ? (
                  <Badge variant="outline" className="text-xs">
                    {route.priority}
                  </Badge>
                ) : (
                  <span className="text-xs text-muted-foreground">—</span>
                )}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
});

/**
 * PeersView — main component showing connected peers and routing table.
 * Story 15.6: Peers & Routing Table View
 * Story 18.5: NOC Aesthetic Enhancement
 */
export const PeersView = React.memo(function PeersView() {
  const { peers, loading: peersLoading, error: peersError, refresh: refreshPeers } = usePeers();
  const { routes, loading: routesLoading, refresh: refreshRoutes } = useRoutingTable();

  const isLoading = peersLoading || routesLoading;

  const handlePeerClick = React.useCallback((peerId: string) => {
    // Find the peer card element and scroll to it
    const el = document.getElementById(`peer-card-${peerId}`);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      // Brief highlight effect
      el.classList.add('ring-2', 'ring-primary');
      setTimeout(() => el.classList.remove('ring-2', 'ring-primary'), 2000);
    }
  }, []);

  // Loading state
  if (isLoading && peers.length === 0 && routes.length === 0) {
    return (
      <div className="space-y-6">
        {/* Skeleton summary stats */}
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          {Array.from({ length: 3 }).map((_, i) => (
            <div key={i} className="rounded-lg border border-border bg-card pl-0 overflow-hidden">
              <div className="flex items-stretch">
                <div className="w-1 bg-muted animate-pulse shrink-0" />
                <div className="flex items-center gap-3 px-4 py-3">
                  <div className="h-5 w-5 bg-muted animate-pulse rounded" />
                  <div>
                    <div className="h-7 w-10 bg-muted animate-pulse rounded mb-1" />
                    <div className="h-3 w-20 bg-muted animate-pulse rounded" />
                  </div>
                </div>
              </div>
            </div>
          ))}
        </div>
        {/* Skeleton peer cards */}
        <div>
          <div className="h-4 w-32 bg-muted animate-pulse rounded mb-3" />
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
            {Array.from({ length: 3 }).map((_, i) => (
              <div key={i} className="rounded-lg border border-border bg-card p-4 space-y-3">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <div className="h-2 w-2 bg-muted animate-pulse rounded-full" />
                    <div className="h-4 w-24 bg-muted animate-pulse rounded" />
                  </div>
                  <div className="h-5 w-16 bg-muted animate-pulse rounded" />
                </div>
                <div className="space-y-2">
                  <div className="h-3 w-12 bg-muted animate-pulse rounded" />
                  <div className="h-3 w-40 bg-muted animate-pulse rounded" />
                </div>
                <div className="space-y-2">
                  <div className="h-3 w-16 bg-muted animate-pulse rounded" />
                  <div className="h-3 w-28 bg-muted animate-pulse rounded" />
                </div>
              </div>
            ))}
          </div>
        </div>
        {/* Skeleton routing table */}
        <div>
          <div className="h-4 w-28 bg-muted animate-pulse rounded mb-3" />
          <div className="rounded-lg border border-border p-4 space-y-2">
            {Array.from({ length: 3 }).map((_, i) => (
              <div key={i} className="h-4 w-full bg-muted animate-pulse rounded" />
            ))}
          </div>
        </div>
      </div>
    );
  }

  // Empty state with NOC aesthetic
  if (peers.length === 0 && routes.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-16 text-muted-foreground">
        <Users className="h-12 w-12 text-cyan-500 mb-4 animate-pulse" />
        <p className="text-lg font-medium">No peers connected</p>
        <p className="text-sm mt-1 max-w-md text-center">
          Peers will appear when BTP connections are established. Configure peer connections in your
          connector configuration file.
        </p>
        {peersError && (
          <p className="text-xs mt-4 text-destructive">
            Failed to fetch peer data. Please check the connector is running.
          </p>
        )}
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* Summary Stats */}
      <SummaryStats peers={peers} routes={routes} />

      {/* Connected Peers */}
      <Card className="bg-card/80 border-border/50">
        <CardHeader>
          <CardTitle className="text-sm font-medium text-muted-foreground">
            Connected Peers ({peers.length})
          </CardTitle>
          <CardAction>
            <Button
              variant="ghost"
              size="icon"
              onClick={refreshPeers}
              aria-label="Refresh peers"
              className="h-8 w-8"
            >
              <RefreshCw className="h-4 w-4" />
            </Button>
          </CardAction>
        </CardHeader>
        <CardContent>
          {peers.length === 0 ? (
            <p className="text-sm text-muted-foreground py-4">No peers connected</p>
          ) : (
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
              {peers.map((peer) => (
                <PeerCard
                  key={peer.peerId}
                  peer={peer}
                  id={`peer-card-${peer.petname || peer.peerId}`}
                />
              ))}
            </div>
          )}
        </CardContent>
      </Card>

      {/* Routing Table */}
      <Card className="bg-card/80 border-border/50">
        <CardHeader>
          <CardTitle className="text-sm font-medium text-muted-foreground">
            Routing Table ({routes.length} entries)
          </CardTitle>
          <CardAction>
            <Button
              variant="ghost"
              size="icon"
              onClick={refreshRoutes}
              aria-label="Refresh routes"
              className="h-8 w-8"
            >
              <RefreshCw className="h-4 w-4" />
            </Button>
          </CardAction>
        </CardHeader>
        <CardContent>
          <RoutingTableView routes={routes} onPeerClick={handlePeerClick} />
        </CardContent>
      </Card>
    </div>
  );
});
