/**
 * ClaimStatistics Component Tests - Story 17.6
 */

import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ClaimStatistics, ClaimStats } from './ClaimStatistics';

describe('ClaimStatistics', () => {
  const createStats = (overrides: Partial<ClaimStats> = {}): ClaimStats => ({
    blockchain: 'evm',
    sentCount: 100,
    receivedCount: 95,
    redeemedCount: 90,
    verificationFailures: 5,
    successRate: 0.9,
    ...overrides,
  });

  describe('empty state', () => {
    it('shows no statistics message when stats array is empty', () => {
      render(<ClaimStatistics stats={[]} timeRange="24h" />);

      expect(
        screen.getByText(/No claim statistics available for this time range/i)
      ).toBeInTheDocument();
    });
  });

  describe('header and time range selector', () => {
    it('displays the header title', () => {
      const stats = [createStats()];
      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('Claim Exchange Statistics')).toBeInTheDocument();
    });

    it('displays all time range options', () => {
      const stats = [createStats()];
      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('1 Hour')).toBeInTheDocument();
      expect(screen.getByText('24 Hours')).toBeInTheDocument();
      expect(screen.getByText('7 Days')).toBeInTheDocument();
    });

    it('accepts onTimeRangeChange callback prop', () => {
      const handleTimeRangeChange = vi.fn();
      const stats = [createStats()];

      render(
        <ClaimStatistics stats={stats} timeRange="24h" onTimeRangeChange={handleTimeRangeChange} />
      );

      // Component renders without errors with the callback
      expect(screen.getByText('1 Hour')).toBeInTheDocument();
      expect(screen.getByText('24 Hours')).toBeInTheDocument();
    });
  });

  describe('blockchain statistics cards', () => {
    it('renders XRP statistics card', () => {
      const stats = [
        createStats({
          blockchain: 'evm',
          sentCount: 150,
          receivedCount: 145,
          redeemedCount: 140,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('XRP Ledger')).toBeInTheDocument();
      expect(screen.getByText('XRP')).toBeInTheDocument();
      expect(screen.getByText('150')).toBeInTheDocument(); // sent
      expect(screen.getByText('145')).toBeInTheDocument(); // received
      expect(screen.getByText('140')).toBeInTheDocument(); // redeemed
    });

    it('renders EVM statistics card', () => {
      const stats = [
        createStats({
          blockchain: 'evm',
          sentCount: 200,
          receivedCount: 198,
          redeemedCount: 195,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('EVM Ledger')).toBeInTheDocument();
      expect(screen.getByText('EVM')).toBeInTheDocument();
      expect(screen.getByText('200')).toBeInTheDocument();
      expect(screen.getByText('198')).toBeInTheDocument();
      expect(screen.getByText('195')).toBeInTheDocument();
    });

    it('renders Aptos statistics card', () => {
      const stats = [
        createStats({
          blockchain: 'evm',
          sentCount: 80,
          receivedCount: 79,
          redeemedCount: 78,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('APTOS Ledger')).toBeInTheDocument();
      expect(screen.getByText('APTOS')).toBeInTheDocument();
      expect(screen.getByText('80')).toBeInTheDocument();
      expect(screen.getByText('79')).toBeInTheDocument();
      expect(screen.getByText('78')).toBeInTheDocument();
    });

    it('renders all three blockchain cards together', () => {
      const stats = [
        createStats({ blockchain: 'evm' }),
        createStats({ blockchain: 'evm' }),
        createStats({ blockchain: 'evm' }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('XRP Ledger')).toBeInTheDocument();
      expect(screen.getByText('EVM Ledger')).toBeInTheDocument();
      expect(screen.getByText('APTOS Ledger')).toBeInTheDocument();
    });
  });

  describe('success rate display', () => {
    it('displays success rate as percentage', () => {
      const stats = [
        createStats({
          successRate: 0.953,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('95.3%')).toBeInTheDocument();
    });

    it('shows green indicator for high success rate (>95%)', () => {
      const stats = [
        createStats({
          successRate: 0.97,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      const successRateElement = screen.getByText('97.0%');
      expect(successRateElement).toHaveClass('text-green-600');
      expect(successRateElement).toHaveClass('bg-green-50');
    });

    it('shows yellow indicator for moderate success rate (90-95%)', () => {
      const stats = [
        createStats({
          successRate: 0.92,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      const successRateElement = screen.getByText('92.0%');
      expect(successRateElement).toHaveClass('text-yellow-600');
      expect(successRateElement).toHaveClass('bg-yellow-50');
    });

    it('shows red indicator for low success rate (<90%)', () => {
      const stats = [
        createStats({
          successRate: 0.85,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      const successRateElement = screen.getByText('85.0%');
      expect(successRateElement).toHaveClass('text-red-600');
      expect(successRateElement).toHaveClass('bg-red-50');
    });
  });

  describe('verification failures display', () => {
    it('shows verification failure warning when failures > 0', () => {
      const stats = [
        createStats({
          verificationFailures: 12,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText(/12 verification failures/i)).toBeInTheDocument();
    });

    it('shows singular form for 1 verification failure', () => {
      const stats = [
        createStats({
          verificationFailures: 1,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText(/1 verification failure$/i)).toBeInTheDocument();
    });

    it('does not show verification warning when failures = 0', () => {
      const stats = [
        createStats({
          verificationFailures: 0,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.queryByText(/verification failure/i)).not.toBeInTheDocument();
    });
  });

  describe('claim count labels', () => {
    it('displays sent label', () => {
      const stats = [createStats()];
      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('Sent')).toBeInTheDocument();
    });

    it('displays received label', () => {
      const stats = [createStats()];
      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('Received')).toBeInTheDocument();
    });

    it('displays redeemed label', () => {
      const stats = [createStats()];
      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      expect(screen.getByText('Redeemed')).toBeInTheDocument();
    });
  });

  describe('responsive grid layout', () => {
    it('renders cards in grid layout', () => {
      const stats = [
        createStats({ blockchain: 'evm' }),
        createStats({ blockchain: 'evm' }),
        createStats({ blockchain: 'evm' }),
      ];

      const { container } = render(<ClaimStatistics stats={stats} timeRange="24h" />);

      // Check for grid classes
      const gridContainer = container.querySelector('.grid');
      expect(gridContainer).toBeInTheDocument();
      expect(gridContainer).toHaveClass('grid-cols-1');
      expect(gridContainer).toHaveClass('md:grid-cols-2');
      expect(gridContainer).toHaveClass('lg:grid-cols-3');
    });
  });

  describe('success rate progress bar', () => {
    it('displays progress bar for success rate', () => {
      const stats = [
        createStats({
          successRate: 0.75,
        }),
      ];

      const { container } = render(<ClaimStatistics stats={stats} timeRange="24h" />);

      // Check for progress bar container
      const progressBar = container.querySelector('.bg-gray-200.rounded-full.h-2');
      expect(progressBar).toBeInTheDocument();
    });

    it('sets progress bar width based on success rate', () => {
      const stats = [
        createStats({
          successRate: 0.8,
        }),
      ];

      const { container } = render(<ClaimStatistics stats={stats} timeRange="24h" />);

      const progressFill = container.querySelector('.h-2.rounded-full.transition-all');
      expect(progressFill).toBeInTheDocument();
      // Width should be 80%
      expect(progressFill).toHaveStyle({ width: '80%' });
    });
  });

  describe('zero counts', () => {
    it('handles zero counts gracefully', () => {
      const stats = [
        createStats({
          sentCount: 0,
          receivedCount: 0,
          redeemedCount: 0,
          verificationFailures: 0,
          successRate: 1.0,
        }),
      ];

      render(<ClaimStatistics stats={stats} timeRange="24h" />);

      // All counts should show as 0
      const zeroCounts = screen.getAllByText('0');
      expect(zeroCounts.length).toBeGreaterThanOrEqual(3); // At least 3 zeros for sent/received/redeemed
    });
  });
});
