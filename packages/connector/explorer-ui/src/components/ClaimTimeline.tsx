/**
 * ClaimTimeline Component - Story 17.6
 *
 * Displays the lifecycle of a claim exchange operation as a vertical timeline.
 * Shows CLAIM_SENT → CLAIM_RECEIVED → CLAIM_REDEEMED progression.
 */

import * as React from 'react';
import { Badge } from '@/components/ui/badge';
import { Separator } from '@/components/ui/separator';
import {
  ClaimBlockchain,
  formatClaimAmount,
  formatRelativeTime,
  getBlockchainBadgeColor,
} from '../lib/event-types';
import { Check, X, ExternalLink } from 'lucide-react';

/**
 * Claim timeline event interface
 */
export interface ClaimTimelineEvent {
  type: 'CLAIM_SENT' | 'CLAIM_RECEIVED' | 'CLAIM_REDEEMED';
  timestamp: string | number;
  success: boolean;
  verified?: boolean;
  blockchain: ClaimBlockchain;
  amount?: string;
  messageId?: string;
  channelId?: string;
  txHash?: string;
  error?: string;
  peerId?: string;
  details: Record<string, unknown>;
}

/**
 * ClaimTimeline component props
 */
export interface ClaimTimelineProps {
  /** Claim message ID for correlation */
  messageId: string;
  /** Array of claim events in chronological order */
  events: ClaimTimelineEvent[];
}

/**
 * Get blockchain explorer URL for transaction hash
 */
function getBlockchainExplorerUrl(
  blockchain: ClaimBlockchain,
  txHash: string
): { url: string; label: string } {
  // Base L2 explorer (EVM only)
  return {
    url: `https://basescan.org/tx/${txHash}`,
    label: 'View on BaseScan',
  };
}

/**
 * Get status icon and color for event
 */
function getEventStatusDisplay(event: ClaimTimelineEvent): {
  icon: React.ReactNode;
  color: string;
  label: string;
} {
  if (event.type === 'CLAIM_SENT') {
    return event.success
      ? { icon: <Check className="h-4 w-4" />, color: 'text-green-500', label: 'Sent' }
      : { icon: <X className="h-4 w-4" />, color: 'text-red-500', label: 'Send Failed' };
  }

  if (event.type === 'CLAIM_RECEIVED') {
    return event.verified
      ? { icon: <Check className="h-4 w-4" />, color: 'text-green-500', label: 'Verified' }
      : { icon: <X className="h-4 w-4" />, color: 'text-red-500', label: 'Verification Failed' };
  }

  if (event.type === 'CLAIM_REDEEMED') {
    return event.success
      ? { icon: <Check className="h-4 w-4" />, color: 'text-green-500', label: 'Redeemed' }
      : { icon: <X className="h-4 w-4" />, color: 'text-red-500', label: 'Redemption Failed' };
  }

  return { icon: <span>○</span>, color: 'text-gray-500', label: 'Unknown' };
}

/**
 * Get connection line color based on event success
 */
function getConnectionLineColor(event: ClaimTimelineEvent, nextEvent?: ClaimTimelineEvent): string {
  if (!nextEvent) return 'border-gray-300';

  // If current event succeeded and next event exists, line is green
  if (event.type === 'CLAIM_SENT' && event.success) return 'border-green-500';
  if (event.type === 'CLAIM_RECEIVED' && event.verified) return 'border-green-500';

  // If current event failed, line is red
  if (event.type === 'CLAIM_SENT' && !event.success) return 'border-red-500';
  if (event.type === 'CLAIM_RECEIVED' && !event.verified) return 'border-red-500';

  return 'border-gray-300';
}

/**
 * Timeline event node component
 */
const TimelineNode: React.FC<{
  event: ClaimTimelineEvent;
  nextEvent?: ClaimTimelineEvent;
  isLast: boolean;
}> = ({ event, nextEvent, isLast }) => {
  const [expanded, setExpanded] = React.useState(false);
  const statusDisplay = getEventStatusDisplay(event);
  const lineColor = getConnectionLineColor(event, nextEvent);
  const timestamp =
    typeof event.timestamp === 'number' ? event.timestamp : new Date(event.timestamp).getTime();

  const explorerLink =
    event.type === 'CLAIM_REDEEMED' && event.txHash
      ? getBlockchainExplorerUrl(event.blockchain, event.txHash)
      : null;

  return (
    <div className="flex gap-4">
      {/* Timeline indicator */}
      <div className="flex flex-col items-center">
        <div
          className={`flex h-8 w-8 items-center justify-center rounded-full border-2 ${
            statusDisplay.color === 'text-green-500'
              ? 'border-green-500 bg-green-50'
              : statusDisplay.color === 'text-red-500'
                ? 'border-red-500 bg-red-50'
                : 'border-gray-300 bg-gray-50'
          }`}
        >
          <span className={statusDisplay.color}>{statusDisplay.icon}</span>
        </div>
        {!isLast && <div className={`w-0.5 flex-1 border-l-2 ${lineColor} min-h-[40px]`} />}
      </div>

      {/* Event content */}
      <div className="flex-1 pb-6">
        <div className="flex items-center gap-2 mb-2">
          <h4 className="text-sm font-medium">{event.type.replace(/_/g, ' ')}</h4>
          <Badge
            variant="outline"
            className={`text-xs border ${getBlockchainBadgeColor(event.blockchain)}`}
          >
            {event.blockchain.toUpperCase()}
          </Badge>
          <span className={`text-sm ${statusDisplay.color}`}>{statusDisplay.label}</span>
        </div>

        <p className="text-xs text-muted-foreground mb-2">{formatRelativeTime(timestamp)}</p>

        {/* Key details */}
        <div className="text-xs space-y-1">
          {event.amount && (
            <div className="flex gap-2">
              <span className="text-muted-foreground">Amount:</span>
              <span className="font-mono">{formatClaimAmount(event.amount, event.blockchain)}</span>
            </div>
          )}
          {event.peerId && (
            <div className="flex gap-2">
              <span className="text-muted-foreground">Peer:</span>
              <span className="font-mono">{event.peerId}</span>
            </div>
          )}
          {event.channelId && (
            <div className="flex gap-2">
              <span className="text-muted-foreground">Channel:</span>
              <span className="font-mono">{event.channelId.slice(0, 16)}...</span>
            </div>
          )}
          {event.error && (
            <div className="flex gap-2">
              <span className="text-red-500">Error:</span>
              <span className="text-red-500 font-mono">{event.error}</span>
            </div>
          )}
        </div>

        {/* Blockchain explorer link for redeemed events */}
        {explorerLink && (
          <a
            href={explorerLink.url}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-1 text-xs text-blue-500 hover:text-blue-600 hover:underline mt-2"
          >
            <ExternalLink className="h-3 w-3" />
            {explorerLink.label}
          </a>
        )}

        {/* Expandable details */}
        {Object.keys(event.details).length > 0 && (
          <button
            onClick={() => setExpanded(!expanded)}
            className="text-xs text-blue-500 hover:text-blue-600 mt-2"
          >
            {expanded ? 'Hide details' : 'Show details'}
          </button>
        )}

        {expanded && (
          <div className="mt-2 p-2 bg-muted rounded text-xs font-mono">
            <pre className="whitespace-pre-wrap">{JSON.stringify(event.details, null, 2)}</pre>
          </div>
        )}
      </div>
    </div>
  );
};

/**
 * ClaimTimeline component
 *
 * Displays a vertical timeline showing the lifecycle of a claim exchange:
 * CLAIM_SENT → CLAIM_RECEIVED → CLAIM_REDEEMED
 *
 * @example
 * ```tsx
 * <ClaimTimeline
 *   messageId="msg_123456"
 *   events={[
 *     {
 *       type: 'CLAIM_SENT',
 *       timestamp: Date.now(),
 *       success: true,
 *       blockchain: 'xrp',
 *       amount: '1000000',
 *       details: {}
 *     },
 *     {
 *       type: 'CLAIM_RECEIVED',
 *       timestamp: Date.now() + 1000,
 *       success: true,
 *       verified: true,
 *       blockchain: 'xrp',
 *       details: {}
 *     }
 *   ]}
 * />
 * ```
 */
export function ClaimTimeline({ messageId, events }: ClaimTimelineProps): React.ReactElement {
  if (events.length === 0) {
    return (
      <div className="p-4 text-center text-sm text-muted-foreground">
        No claim events found for message ID: {messageId}
      </div>
    );
  }

  return (
    <div className="w-full">
      <div className="mb-4">
        <h3 className="text-sm font-medium mb-1">Claim Lifecycle</h3>
        <p className="text-xs text-muted-foreground">
          Message ID:{' '}
          <span className="font-mono" title={messageId}>
            {messageId.slice(0, 16)}...
          </span>
        </p>
      </div>

      <Separator className="my-4" />

      <div className="space-y-0">
        {events.map((event, index) => (
          <TimelineNode
            key={`${event.type}-${event.timestamp}`}
            event={event}
            nextEvent={events[index + 1]}
            isLast={index === events.length - 1}
          />
        ))}
      </div>
    </div>
  );
}
