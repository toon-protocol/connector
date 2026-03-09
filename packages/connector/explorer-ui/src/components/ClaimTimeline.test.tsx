/**
 * ClaimTimeline Component Tests - Story 17.6
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ClaimTimeline, ClaimTimelineEvent } from './ClaimTimeline';

describe('ClaimTimeline', () => {
  const createClaimEvent = (overrides: Partial<ClaimTimelineEvent> = {}): ClaimTimelineEvent => ({
    type: 'CLAIM_SENT',
    timestamp: Date.now(),
    success: true,
    blockchain: 'evm',
    amount: '1000000',
    messageId: 'msg_test123',
    peerId: 'peer-alice',
    details: {},
    ...overrides,
  });

  describe('empty state', () => {
    it('shows no events message when events array is empty', () => {
      render(<ClaimTimeline messageId="msg_123" events={[]} />);

      expect(screen.getByText(/No claim events found/i)).toBeInTheDocument();
      expect(screen.getByText(/msg_123/i)).toBeInTheDocument();
    });
  });

  describe('CLAIM_SENT event', () => {
    it('renders CLAIM_SENT event with success status', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_SENT',
          success: true,
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      expect(screen.getByText('CLAIM SENT')).toBeInTheDocument();
      expect(screen.getByText('Sent')).toBeInTheDocument();
      expect(screen.getByText('XRP')).toBeInTheDocument();
    });

    it('renders CLAIM_SENT event with failure status', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_SENT',
          success: false,
          error: 'BTP connection failed',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      expect(screen.getByText('Send Failed')).toBeInTheDocument();
      expect(screen.getByText(/Error:/)).toBeInTheDocument();
      expect(screen.getByText('BTP connection failed')).toBeInTheDocument();
    });
  });

  describe('CLAIM_RECEIVED event', () => {
    it('renders CLAIM_RECEIVED event with verified status', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_RECEIVED',
          success: true,
          verified: true,
          channelId: 'ch_abc123',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      expect(screen.getByText('CLAIM RECEIVED')).toBeInTheDocument();
      expect(screen.getByText('Verified')).toBeInTheDocument();
      expect(screen.getByText(/Channel:/)).toBeInTheDocument();
    });

    it('renders CLAIM_RECEIVED event with verification failure', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_RECEIVED',
          success: false,
          verified: false,
          error: 'Invalid signature',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      expect(screen.getByText('Verification Failed')).toBeInTheDocument();
      expect(screen.getByText('Invalid signature')).toBeInTheDocument();
    });
  });

  describe('CLAIM_REDEEMED event', () => {
    it('renders CLAIM_REDEEMED event with success status', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_REDEEMED',
          success: true,
          txHash: '0xabc123def456',
          blockchain: 'evm',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      expect(screen.getByText('CLAIM REDEEMED')).toBeInTheDocument();
      expect(screen.getByText('Redeemed')).toBeInTheDocument();
      expect(screen.getByText('EVM')).toBeInTheDocument();
    });

    it('renders blockchain explorer link for redeemed XRP claim', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_REDEEMED',
          success: true,
          txHash: 'ABC123DEF456',
          blockchain: 'evm',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      const link = screen.getByText('View on XRPScan');
      expect(link).toBeInTheDocument();
      expect(link.closest('a')).toHaveAttribute('href', 'https://xrpscan.com/tx/ABC123DEF456');
    });

    it('renders blockchain explorer link for redeemed EVM claim', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_REDEEMED',
          success: true,
          txHash: '0xabc123',
          blockchain: 'evm',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      const link = screen.getByText('View on BaseScan');
      expect(link).toBeInTheDocument();
      expect(link.closest('a')).toHaveAttribute('href', 'https://basescan.org/tx/0xabc123');
    });

    it('renders blockchain explorer link for redeemed Aptos claim', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_REDEEMED',
          success: true,
          txHash: '0xdef789',
          blockchain: 'evm',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      const link = screen.getByText('View on Aptos Explorer');
      expect(link).toBeInTheDocument();
      expect(link.closest('a')).toHaveAttribute(
        'href',
        'https://explorer.aptoslabs.com/txn/0xdef789'
      );
    });

    it('renders CLAIM_REDEEMED event with failure status', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_REDEEMED',
          success: false,
          error: 'Insufficient gas',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      expect(screen.getByText('Redemption Failed')).toBeInTheDocument();
      expect(screen.getByText('Insufficient gas')).toBeInTheDocument();
    });
  });

  describe('complete claim lifecycle', () => {
    it('renders complete timeline: sent → received → redeemed', () => {
      const now = Date.now();
      const events = [
        createClaimEvent({
          type: 'CLAIM_SENT',
          timestamp: now,
          success: true,
        }),
        createClaimEvent({
          type: 'CLAIM_RECEIVED',
          timestamp: now + 1000,
          success: true,
          verified: true,
        }),
        createClaimEvent({
          type: 'CLAIM_REDEEMED',
          timestamp: now + 5000,
          success: true,
          txHash: '0xabc',
        }),
      ];

      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      expect(screen.getByText('CLAIM SENT')).toBeInTheDocument();
      expect(screen.getByText('CLAIM RECEIVED')).toBeInTheDocument();
      expect(screen.getByText('CLAIM REDEEMED')).toBeInTheDocument();
    });
  });

  describe('blockchain badges', () => {
    it('displays XRP blockchain badge', () => {
      const events = [createClaimEvent({ blockchain: 'evm' })];

      render(<ClaimTimeline messageId="msg_123" events={events} />);

      expect(screen.getByText('XRP')).toBeInTheDocument();
    });

    it('displays EVM blockchain badge', () => {
      const events = [createClaimEvent({ blockchain: 'evm' })];

      render(<ClaimTimeline messageId="msg_123" events={events} />);

      expect(screen.getByText('EVM')).toBeInTheDocument();
    });

    it('displays Aptos blockchain badge', () => {
      const events = [createClaimEvent({ blockchain: 'evm' })];

      render(<ClaimTimeline messageId="msg_123" events={events} />);

      expect(screen.getByText('APTOS')).toBeInTheDocument();
    });
  });

  describe('claim details', () => {
    it('displays claim amount for XRP', () => {
      const events = [
        createClaimEvent({
          blockchain: 'evm',
          amount: '1000000', // 1 XRP in drops
        }),
      ];

      render(<ClaimTimeline messageId="msg_123" events={events} />);

      expect(screen.getByText(/1\.000000 XRP/)).toBeInTheDocument();
    });

    it('displays claim amount for EVM', () => {
      const events = [
        createClaimEvent({
          blockchain: 'evm',
          amount: '1000000000000000000', // 1 ETH in wei
        }),
      ];

      render(<ClaimTimeline messageId="msg_123" events={events} />);

      expect(screen.getByText(/1\.000000 ETH/)).toBeInTheDocument();
    });

    it('displays peer ID', () => {
      const events = [
        createClaimEvent({
          peerId: 'peer-bob',
        }),
      ];

      render(<ClaimTimeline messageId="msg_123" events={events} />);

      expect(screen.getByText(/Peer:/)).toBeInTheDocument();
      expect(screen.getByText('peer-bob')).toBeInTheDocument();
    });

    it('displays channel ID when available', () => {
      const events = [
        createClaimEvent({
          type: 'CLAIM_RECEIVED',
          channelId: 'ch_abc123def456',
          verified: true,
        }),
      ];

      render(<ClaimTimeline messageId="msg_123" events={events} />);

      expect(screen.getByText(/Channel:/)).toBeInTheDocument();
      // Channel ID is truncated
      expect(screen.getByText(/ch_abc123def456\.\.\./)).toBeInTheDocument();
    });
  });

  describe('message ID display', () => {
    it('displays message ID in header', () => {
      const events = [createClaimEvent()];
      render(<ClaimTimeline messageId="msg_test123" events={events} />);

      // Message ID should be displayed (truncated)
      expect(screen.getByText(/msg_test123/)).toBeInTheDocument();
    });

    it('shows message ID in empty state', () => {
      render(<ClaimTimeline messageId="msg_full123" events={[]} />);

      // Full message ID shown in empty state message
      expect(screen.getByText(/msg_full123/)).toBeInTheDocument();
    });
  });
});
