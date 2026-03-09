import * as React from 'react';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { ChannelState } from '@/lib/event-types';
import { Clock, Link2, ArrowRightLeft } from 'lucide-react';
import { formatRelativeTime } from '@/lib/event-types';

/**
 * PaymentChannelCard props interface
 */
export interface PaymentChannelCardProps {
  channel: ChannelState;
  onClick?: () => void;
}

/**
 * Format address for display (truncate with ellipsis)
 */
function formatAddress(address: string): string {
  if (address.length <= 12) return address;
  return `${address.slice(0, 6)}...${address.slice(-4)}`;
}

/**
 * Get channel status badge styling
 */
function getStatusBadge(status: ChannelState['status']): {
  label: string;
  className: string;
} {
  switch (status) {
    case 'opening':
      return { label: 'Opening', className: 'bg-yellow-500' };
    case 'active':
      return { label: 'Active', className: 'bg-green-500' };
    case 'closing':
      return { label: 'Closing', className: 'bg-orange-500' };
    case 'settling':
      return { label: 'Settling', className: 'bg-blue-500 animate-pulse' };
    case 'settled':
      return { label: 'Settled', className: 'bg-gray-500' };
  }
}

/**
 * Format balance value
 */
function formatBalance(value: string): string {
  const num = BigInt(value);
  if (num >= 1_000_000_000n) {
    return `${(Number(num) / 1_000_000_000).toFixed(2)}B`;
  }
  if (num >= 1_000_000n) {
    return `${(Number(num) / 1_000_000).toFixed(2)}M`;
  }
  if (num >= 1_000n) {
    return `${(Number(num) / 1_000).toFixed(2)}K`;
  }
  return num.toLocaleString();
}

/**
 * PaymentChannelCard component - displays payment channel status
 * Story 14.6: Settlement and Balance Visualization
 */
export const PaymentChannelCard = React.memo(function PaymentChannelCard({
  channel,
  onClick,
}: PaymentChannelCardProps) {
  const statusBadge = getStatusBadge(channel.status);
  const lastActivityTs = new Date(channel.lastActivityAt).getTime();

  return (
    <Card
      className="py-4 cursor-pointer hover:border-primary/50 transition-colors"
      onClick={onClick}
    >
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between">
          <CardTitle className="text-sm font-medium flex items-center gap-2">
            <Link2 className="h-4 w-4" />
            <span className="truncate" title={channel.channelId}>
              {formatAddress(channel.channelId)}
            </span>
          </CardTitle>
          <Badge className="text-xs text-white bg-emerald-500">EVM</Badge>
        </div>
        <CardDescription className="flex items-center gap-2">
          <Badge className={`text-xs text-white ${statusBadge.className}`}>
            {statusBadge.label}
          </Badge>
          <span className="text-xs">{channel.tokenSymbol}</span>
        </CardDescription>
      </CardHeader>

      <CardContent className="space-y-3">
        {/* Participants */}
        <div className="text-xs">
          <div className="text-muted-foreground mb-1">Participants</div>
          <div className="flex items-center gap-2 font-mono">
            <span title={channel.participants[0]}>{formatAddress(channel.participants[0])}</span>
            <ArrowRightLeft className="h-3 w-3 text-muted-foreground" />
            <span title={channel.participants[1]}>{formatAddress(channel.participants[1])}</span>
          </div>
        </div>

        {/* Transfers */}
        <div className="grid grid-cols-2 gap-2 text-xs">
          <div>
            <div className="text-muted-foreground">My Transferred</div>
            <div className="font-mono text-green-500">{formatBalance(channel.myTransferred)}</div>
          </div>
          <div>
            <div className="text-muted-foreground">Their Transferred</div>
            <div className="font-mono text-blue-400">{formatBalance(channel.theirTransferred)}</div>
          </div>
        </div>

        {/* Last Activity */}
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          <Clock className="h-3 w-3" />
          Last activity {formatRelativeTime(lastActivityTs)}
        </div>
      </CardContent>
    </Card>
  );
});
