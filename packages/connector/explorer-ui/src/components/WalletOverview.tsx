import * as React from 'react';
import { Link2, Copy, Check, RefreshCw, ExternalLink } from 'lucide-react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { getExplorerUrl } from '@/lib/explorer-links';
import { Badge } from '@/components/ui/badge';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';
import { useCopyToClipboard } from '@/hooks/useCopyToClipboard';
import { formatRelativeTime } from '@/lib/event-types';
import type { WalletBalances } from '@/lib/event-types';

interface WalletOverviewProps {
  data: WalletBalances;
  lastUpdated: number | null;
  onRefresh: () => void;
}

// ---------- Token icon SVGs ----------

function EthIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 256 417" className={className} fill="none" xmlns="http://www.w3.org/2000/svg">
      <path
        d="M127.961 0L125.166 9.5V285.168L127.961 287.958L255.923 212.32L127.961 0Z"
        fill="#687DE3"
      />
      <path d="M127.962 0L0 212.32L127.962 287.958V154.158V0Z" fill="#A4B0F4" />
      <path
        d="M127.961 312.187L126.386 314.107V412.306L127.961 416.905L255.999 236.587L127.961 312.187Z"
        fill="#687DE3"
      />
      <path d="M127.962 416.905V312.187L0 236.587L127.962 416.905Z" fill="#A4B0F4" />
      <path d="M127.961 287.958L255.922 212.32L127.961 154.159V287.958Z" fill="#4E5DA0" />
      <path d="M0 212.32L127.962 287.958V154.159L0 212.32Z" fill="#687DE3" />
    </svg>
  );
}

function AgentIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" className={className} fill="none" xmlns="http://www.w3.org/2000/svg">
      <circle cx="12" cy="12" r="11" stroke="#A855F7" strokeWidth="2" />
      <text x="12" y="16" textAnchor="middle" fontSize="12" fontWeight="bold" fill="#A855F7">
        A
      </text>
    </svg>
  );
}

/** Truncate an address to first 6 + last 4 characters */
function truncateAddress(addr: string): string {
  if (addr.length <= 12) return addr;
  return `${addr.slice(0, 6)}...${addr.slice(-4)}`;
}

/** Copyable address inline component */
function CopyableAddress({ address, explorerUrl }: { address: string; explorerUrl?: string }) {
  const { copy, copied } = useCopyToClipboard();

  if (explorerUrl) {
    return (
      <a
        href={explorerUrl}
        target="_blank"
        rel="noopener noreferrer"
        className="inline-flex items-center gap-1 font-mono text-xs text-blue-500 hover:text-blue-700 hover:underline transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-1 rounded"
        onClick={(e) => {
          e.preventDefault();
          window.open(explorerUrl, '_blank', 'noopener,noreferrer');
        }}
        title={address}
        aria-label="View on blockchain explorer"
      >
        {truncateAddress(address)}
        <ExternalLink className="h-3 w-3" />
      </a>
    );
  }

  // Fallback: plain copy button (existing behavior)
  return (
    <button
      onClick={() => copy(address)}
      className="inline-flex items-center gap-1 font-mono text-xs text-muted-foreground hover:text-foreground transition-colors"
      title={address}
    >
      {truncateAddress(address)}
      {copied ? <Check className="h-3 w-3 text-green-500" /> : <Copy className="h-3 w-3" />}
    </button>
  );
}

/** Status badge for channel status */
function ChannelStatusBadge({ status }: { status: string }) {
  const variant =
    status === 'opened' || status === 'open' || status === 'active'
      ? 'default'
      : status === 'settled' || status === 'closed'
        ? 'secondary'
        : 'outline';
  return <Badge variant={variant}>{status}</Badge>;
}

export const WalletOverview = React.memo(function WalletOverview({
  data,
  lastUpdated,
  onRefresh,
}: WalletOverviewProps) {
  return (
    <Card>
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <CardTitle className="text-sm font-medium flex items-center gap-2">
            <Link2 className="h-4 w-4" />
            On-Chain Wallet
          </CardTitle>
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            {lastUpdated && <span>Updated {formatRelativeTime(lastUpdated)}</span>}
            <button
              onClick={onRefresh}
              className="hover:text-foreground transition-colors"
              title="Refresh balances"
            >
              <RefreshCw className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground mt-1">
          <span>
            Agent: <span className="font-medium text-foreground">{data.agentId}</span>
          </span>
          <span>
            EVM:{' '}
            <CopyableAddress
              address={data.evmAddress}
              explorerUrl={getExplorerUrl(data.evmAddress, 'address') ?? undefined}
            />
          </span>
        </div>
      </CardHeader>

      <CardContent className="space-y-4">
        {/* Balance cards */}
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          {/* ETH Balance */}
          <div className="rounded-lg border p-3">
            <div className="flex items-center gap-1.5 text-xs font-medium text-blue-500 mb-1">
              <EthIcon className="h-3.5 w-3.5" />
              ETH
            </div>
            <div className="text-lg font-bold font-mono">
              {data.ethBalance != null
                ? Number(data.ethBalance).toLocaleString(undefined, { maximumFractionDigits: 4 })
                : '—'}
            </div>
          </div>

          {/* AGENT Token Balance */}
          <div className="rounded-lg border p-3">
            <div className="flex items-center gap-1.5 text-xs font-medium text-purple-500 mb-1">
              <AgentIcon className="h-3.5 w-3.5" />
              AGENT Token
            </div>
            <div className="text-lg font-bold font-mono">
              {data.agentTokenBalance != null
                ? Number(data.agentTokenBalance).toLocaleString(undefined, {
                    maximumFractionDigits: 2,
                  })
                : '—'}
            </div>
          </div>
        </div>

        {/* EVM Channels */}
        {data.evmChannels.length > 0 && (
          <div>
            <h4 className="text-xs font-medium text-muted-foreground mb-2">
              EVM Channels ({data.evmChannels.length})
            </h4>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[20%]">Channel</TableHead>
                  <TableHead className="w-[20%]">Peer</TableHead>
                  <TableHead className="w-[25%] text-right">Deposit</TableHead>
                  <TableHead className="w-[25%] text-right">Transferred</TableHead>
                  <TableHead className="w-[10%] text-right">Status</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {data.evmChannels.map((ch) => (
                  <TableRow key={ch.channelId}>
                    <TableCell>
                      <CopyableAddress
                        address={ch.channelId}
                        explorerUrl={getExplorerUrl(ch.channelId, 'address') ?? undefined}
                      />
                    </TableCell>
                    <TableCell>
                      <CopyableAddress
                        address={ch.peerAddress}
                        explorerUrl={getExplorerUrl(ch.peerAddress, 'address') ?? undefined}
                      />
                    </TableCell>
                    <TableCell className="text-right font-mono text-xs">
                      {Number(ch.deposit).toLocaleString(undefined, { maximumFractionDigits: 4 })}
                    </TableCell>
                    <TableCell className="text-right font-mono text-xs">
                      {Number(ch.transferredAmount).toLocaleString(undefined, {
                        maximumFractionDigits: 4,
                      })}
                    </TableCell>
                    <TableCell className="text-right">
                      <ChannelStatusBadge status={ch.status} />
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}

        {/* No channels message */}
        {data.evmChannels.length === 0 && (
          <p className="text-xs text-muted-foreground text-center py-2">
            No payment channels open yet.
          </p>
        )}
      </CardContent>
    </Card>
  );
});
