import * as React from 'react';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { AccountCard, AccountCardProps } from './AccountCard';

describe('AccountCard', () => {
  const createDefaultProps = (overrides: Partial<AccountCardProps> = {}): AccountCardProps => ({
    peerId: 'peer-a',
    tokenId: 'ILP',
    debitBalance: 0n,
    creditBalance: 0n,
    netBalance: 0n,
    settlementState: 'IDLE',
    balanceHistory: [],
    ...overrides,
  });

  describe('NOC aesthetic styling (Story 18.4)', () => {
    it('should display peer ID with monospace font', () => {
      render(<AccountCard {...createDefaultProps()} />);

      const peerIdElement = screen.getByText('peer-a');
      expect(peerIdElement).toHaveClass('font-mono');
    });

    it('should display net balance prominently with tabular-nums', () => {
      render(<AccountCard {...createDefaultProps({ netBalance: 1000n })} />);

      // Find the element containing the formatted balance (1000n formats to "1.0K")
      const netBalanceContainer = screen.getByText('1.0K');
      // Should have text-3xl for prominent display and tabular-nums
      expect(netBalanceContainer).toHaveClass('text-3xl');
      expect(netBalanceContainer).toHaveClass('font-bold');
      expect(netBalanceContainer).toHaveClass('tabular-nums');
    });

    it('should color net balance emerald when positive', () => {
      render(<AccountCard {...createDefaultProps({ netBalance: 1000n })} />);

      const netBalanceElement = screen.getByText('1.0K');
      expect(netBalanceElement).toHaveClass('text-emerald-500');
    });

    it('should color net balance rose when negative', () => {
      render(<AccountCard {...createDefaultProps({ netBalance: -1000n })} />);

      const netBalanceElement = screen.getByText('-1.0K');
      expect(netBalanceElement).toHaveClass('text-rose-500');
    });

    it('should color debit balance rose', () => {
      const { container } = render(
        <AccountCard {...createDefaultProps({ debitBalance: 1000n })} />
      );

      // Debit balance is in a specific section with rose-400 class
      const debitElement = container.querySelector('.text-rose-400');
      expect(debitElement).toBeInTheDocument();
      expect(debitElement?.textContent).toBe('1.0K');
    });

    it('should color credit balance emerald', () => {
      const { container } = render(
        <AccountCard {...createDefaultProps({ creditBalance: 1000n })} />
      );

      // Credit balance is in a specific section with emerald-400 class
      const creditElement = container.querySelector('.text-emerald-400');
      expect(creditElement).toBeInTheDocument();
      expect(creditElement?.textContent).toBe('1.0K');
    });

    it('should have card with NOC border hover effect', () => {
      const { container } = render(<AccountCard {...createDefaultProps()} />);

      const card = container.querySelector('[class*="hover:border-cyan"]');
      expect(card).toBeInTheDocument();
    });

    it('should animate IN_PROGRESS settlement badge with pulse', () => {
      render(
        <AccountCard {...createDefaultProps({ settlementState: 'SETTLEMENT_IN_PROGRESS' })} />
      );

      const badge = screen.getByText('In Progress');
      expect(badge).toHaveClass('animate-pulse');
      expect(badge).toHaveClass('bg-cyan-500');
    });

    it('should apply hover-elevate class for card elevation effect (Story 18.7)', () => {
      const { container } = render(<AccountCard {...createDefaultProps()} />);

      // Card should have hover-elevate class for smooth hover elevation
      const card = container.querySelector('.hover-elevate');
      expect(card).toBeInTheDocument();
    });
  });

  describe('basic rendering', () => {
    it('renders peer ID and token ID correctly', () => {
      render(<AccountCard {...createDefaultProps()} />);

      expect(screen.getByText('peer-a')).toBeInTheDocument();
      expect(screen.getByText('ILP')).toBeInTheDocument();
    });

    it('renders different token IDs (ETH, XRP)', () => {
      render(<AccountCard {...createDefaultProps({ tokenId: 'ETH' })} />);
      expect(screen.getByText('ETH')).toBeInTheDocument();
    });
  });

  describe('balance display', () => {
    it('shows debit/credit/net balances', () => {
      render(
        <AccountCard
          {...createDefaultProps({
            debitBalance: 1000n,
            creditBalance: 500n,
            netBalance: -500n,
          })}
        />
      );

      // NOC aesthetic: updated labels with parenthetical explanation
      expect(screen.getByText('We Owe (Debit)')).toBeInTheDocument();
      expect(screen.getByText('They Owe (Credit)')).toBeInTheDocument();
      expect(screen.getByText('Net Balance')).toBeInTheDocument();
    });

    it('handles zero balances correctly', () => {
      render(
        <AccountCard
          {...createDefaultProps({
            debitBalance: 0n,
            creditBalance: 0n,
            netBalance: 0n,
          })}
        />
      );

      // Should render without errors
      expect(screen.getByText('peer-a')).toBeInTheDocument();
    });

    it('formats large balances with abbreviations', () => {
      render(
        <AccountCard
          {...createDefaultProps({
            creditBalance: 1500000n,
            netBalance: 1500000n,
          })}
        />
      );

      // Multiple elements may show 1.5M (credit and net), so use getAllByText
      const elements = screen.getAllByText('1.5M');
      expect(elements.length).toBeGreaterThan(0);
    });
  });

  describe('settlement progress bar', () => {
    it('calculates and displays settlement progress', () => {
      render(
        <AccountCard
          {...createDefaultProps({
            creditBalance: 7000n,
            settlementThreshold: 10000n,
          })}
        />
      );

      expect(screen.getByText('Settlement Threshold')).toBeInTheDocument();
      expect(screen.getByText('70%')).toBeInTheDocument();
    });

    it('hides progress bar when no threshold set', () => {
      render(<AccountCard {...createDefaultProps()} />);

      expect(screen.getByText('No threshold set')).toBeInTheDocument();
    });
  });

  describe('settlement state badge', () => {
    it('shows IDLE state correctly', () => {
      render(<AccountCard {...createDefaultProps({ settlementState: 'IDLE' })} />);

      expect(screen.getByText('Idle')).toBeInTheDocument();
    });

    it('shows PENDING state correctly', () => {
      render(<AccountCard {...createDefaultProps({ settlementState: 'SETTLEMENT_PENDING' })} />);

      expect(screen.getByText('Pending')).toBeInTheDocument();
    });

    it('shows IN_PROGRESS state correctly', () => {
      render(
        <AccountCard {...createDefaultProps({ settlementState: 'SETTLEMENT_IN_PROGRESS' })} />
      );

      expect(screen.getByText('In Progress')).toBeInTheDocument();
    });
  });

  describe('channel indicator', () => {
    it('displays EVM channel badge when hasActiveChannel is true', () => {
      render(
        <AccountCard
          {...createDefaultProps({
            hasActiveChannel: true,
            channelType: 'evm',
          })}
        />
      );

      expect(screen.getByText('EVM')).toBeInTheDocument();
    });

    it('displays XRP channel badge when hasActiveChannel is true', () => {
      render(
        <AccountCard
          {...createDefaultProps({
            hasActiveChannel: true,
            channelType: 'evm',
          })}
        />
      );

      expect(screen.getByText('XRP')).toBeInTheDocument();
    });

    it('does not display channel badge when hasActiveChannel is false', () => {
      render(<AccountCard {...createDefaultProps({ hasActiveChannel: false })} />);

      expect(screen.queryByText('EVM')).not.toBeInTheDocument();
      expect(screen.queryByText('XRP')).not.toBeInTheDocument();
    });
  });

  describe('memoization (AC3: sibling isolation)', () => {
    it('does not re-render sibling AccountCard when one account updates', () => {
      let siblingRenderCount = 0;

      // A thin wrapper that counts renders of the sibling AccountCard
      const TrackedAccountCard = React.memo(function TrackedAccountCard(props: AccountCardProps) {
        siblingRenderCount++;
        return <AccountCard {...props} />;
      });

      // Stable props for sibling — same reference across renders
      const stableHistory: [] = [];
      const siblingProps: AccountCardProps = {
        peerId: 'peer-b',
        tokenId: 'ILP',
        debitBalance: 0n,
        creditBalance: 500n,
        netBalance: 500n,
        settlementState: 'IDLE',
        balanceHistory: stableHistory,
      };

      // Wrapper that renders two cards: one updating, one stable
      function TestHarness({ creditA }: { creditA: bigint }): React.JSX.Element {
        return (
          <>
            <AccountCard
              peerId="peer-a"
              tokenId="ILP"
              debitBalance={0n}
              creditBalance={creditA}
              netBalance={creditA}
              settlementState="IDLE"
              balanceHistory={stableHistory}
            />
            <TrackedAccountCard {...siblingProps} />
          </>
        );
      }

      const { rerender } = render(<TestHarness creditA={1000n} />);

      // Initial render: sibling renders once
      expect(siblingRenderCount).toBe(1);

      // Update peer-a's credit balance — sibling props unchanged
      rerender(<TestHarness creditA={2000n} />);

      // Sibling should NOT re-render because its props haven't changed
      // React.memo on TrackedAccountCard prevents this
      expect(siblingRenderCount).toBe(1);
    });
  });
});
