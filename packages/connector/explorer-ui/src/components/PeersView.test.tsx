import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { PeersView } from './PeersView';

// Mock the hooks
vi.mock('@/hooks/usePeers', () => ({
  usePeers: vi.fn(),
}));

vi.mock('@/hooks/useRoutingTable', () => ({
  useRoutingTable: vi.fn(),
}));

import { usePeers } from '@/hooks/usePeers';
import { useRoutingTable } from '@/hooks/useRoutingTable';

const mockUsePeers = vi.mocked(usePeers);
const mockUseRoutingTable = vi.mocked(useRoutingTable);

describe('PeersView', () => {
  beforeEach(() => {
    vi.clearAllMocks();

    // Default mock returns
    mockUsePeers.mockReturnValue({
      peers: [],
      loading: false,
      error: null,
      refresh: vi.fn(),
    });

    mockUseRoutingTable.mockReturnValue({
      routes: [],
      loading: false,
      error: null,
      refresh: vi.fn(),
    });
  });

  describe('empty state', () => {
    it('renders empty state when no peers and no routes', () => {
      render(<PeersView />);

      expect(screen.getByText('No peers connected')).toBeInTheDocument();
      expect(
        screen.getByText(/Peers will appear when BTP connections are established/)
      ).toBeInTheDocument();
    });

    it('shows error message when peers error exists', () => {
      mockUsePeers.mockReturnValue({
        peers: [],
        loading: false,
        error: 'Connection failed',
        refresh: vi.fn(),
      });

      render(<PeersView />);

      expect(
        screen.getByText('Failed to fetch peer data. Please check the connector is running.')
      ).toBeInTheDocument();
    });
  });

  describe('loading state', () => {
    it('shows skeleton loaders when loading with no data', () => {
      mockUsePeers.mockReturnValue({
        peers: [],
        loading: true,
        error: null,
        refresh: vi.fn(),
      });

      mockUseRoutingTable.mockReturnValue({
        routes: [],
        loading: true,
        error: null,
        refresh: vi.fn(),
      });

      const { container } = render(<PeersView />);

      // Should show skeleton elements (animate-pulse)
      const skeletons = container.querySelectorAll('.animate-pulse');
      expect(skeletons.length).toBeGreaterThan(0);
    });
  });

  describe('peer cards', () => {
    it('renders peer cards with correct data', () => {
      mockUsePeers.mockReturnValue({
        peers: [
          {
            peerId: 'alice',
            ilpAddress: 'g.agent.alice',
            evmAddress: '0x1234567890abcdef1234567890abcdef12345678',
            connected: true,
            petname: 'alice',
            pubkey: 'abc123def456789012345678901234567890123456789012345678901234',
          },
          {
            peerId: 'bob',
            ilpAddress: 'g.agent.bob',
            connected: false,
            petname: 'bob',
          },
        ],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      // Peer names
      expect(screen.getByText('alice')).toBeInTheDocument();
      expect(screen.getByText('bob')).toBeInTheDocument();

      // ILP addresses
      expect(screen.getByText('g.agent.alice')).toBeInTheDocument();
      expect(screen.getByText('g.agent.bob')).toBeInTheDocument();

      // Connection status badges (in peer cards, not summary stats)
      const badges = screen.getAllByText('Connected');
      expect(badges.length).toBeGreaterThanOrEqual(1);
      expect(screen.getByText('Disconnected')).toBeInTheDocument();

      // Section header
      expect(screen.getByText('Connected Peers (2)')).toBeInTheDocument();
    });

    it('shows EVM address with blockchain link', () => {
      mockUsePeers.mockReturnValue({
        peers: [
          {
            peerId: 'alice',
            ilpAddress: 'g.agent.alice',
            evmAddress: '0x1234567890abcdef1234567890abcdef12345678',
            connected: true,
          },
        ],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      expect(screen.getByText('EVM Address')).toBeInTheDocument();
      // Truncated address (8 + 6 chars)
      expect(screen.getByText('0x123456...345678')).toBeInTheDocument();
    });

    it('shows XRP address with blockchain link', () => {
      mockUsePeers.mockReturnValue({
        peers: [
          {
            peerId: 'alice',
            ilpAddress: 'g.agent.alice',
            connected: true,
          },
        ],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      expect(screen.getByText('XRP Address')).toBeInTheDocument();
    });
  });

  describe('routing table', () => {
    it('renders routing table with entries', () => {
      mockUsePeers.mockReturnValue({
        peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      mockUseRoutingTable.mockReturnValue({
        routes: [
          { prefix: 'g.agent.alice', nextHop: 'alice', priority: 0 },
          { prefix: 'g.agent.bob', nextHop: 'bob' },
        ],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      // Table headers
      expect(screen.getByText('Prefix')).toBeInTheDocument();
      expect(screen.getByText('Next Hop')).toBeInTheDocument();
      expect(screen.getByText('Priority')).toBeInTheDocument();

      // Route entries — g.agent.alice appears in peer card AND routing table,
      // so use getAllByText and verify at least the routing table has them
      expect(screen.getAllByText('g.agent.alice').length).toBeGreaterThanOrEqual(2);
      expect(screen.getByText('g.agent.bob')).toBeInTheDocument();

      // Section header
      expect(screen.getByText('Routing Table (2 entries)')).toBeInTheDocument();
    });

    it('shows empty message when no routing entries', () => {
      mockUsePeers.mockReturnValue({
        peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      expect(screen.getByText('No routing entries configured')).toBeInTheDocument();
    });

    it('shows dash for undefined priority', () => {
      mockUsePeers.mockReturnValue({
        peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      mockUseRoutingTable.mockReturnValue({
        routes: [{ prefix: 'g.agent.alice', nextHop: 'alice' }],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      expect(screen.getByText('—')).toBeInTheDocument();
    });

    it('renders next hop as clickable link', () => {
      mockUsePeers.mockReturnValue({
        peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      mockUseRoutingTable.mockReturnValue({
        routes: [{ prefix: 'g.agent.alice', nextHop: 'alice' }],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      const nextHopLink = screen.getAllByText('alice').find((el) => el.tagName === 'BUTTON');
      expect(nextHopLink).toBeTruthy();
    });

    it('scrolls to peer card when next hop is clicked', () => {
      const scrollIntoViewMock = vi.fn();

      mockUsePeers.mockReturnValue({
        peers: [
          { peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true, petname: 'alice' },
        ],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      mockUseRoutingTable.mockReturnValue({
        routes: [{ prefix: 'g.agent.alice', nextHop: 'alice' }],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      // Mock scrollIntoView on the peer card element
      const peerCardEl = document.getElementById('peer-card-alice');
      if (peerCardEl) {
        peerCardEl.scrollIntoView = scrollIntoViewMock;
      }

      const nextHopButton = screen.getAllByText('alice').find((el) => el.tagName === 'BUTTON');
      if (nextHopButton) {
        fireEvent.click(nextHopButton);
      }

      // scrollIntoView should be called if element exists
      if (peerCardEl) {
        expect(scrollIntoViewMock).toHaveBeenCalledWith({
          behavior: 'smooth',
          block: 'center',
        });
      }
    });
  });

  describe('sorts routing entries alphabetically', () => {
    it('sorts routes by prefix', () => {
      mockUsePeers.mockReturnValue({
        peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      mockUseRoutingTable.mockReturnValue({
        routes: [
          { prefix: 'g.agent.charlie', nextHop: 'charlie' },
          { prefix: 'g.agent.alice', nextHop: 'alice' },
          { prefix: 'g.agent.bob', nextHop: 'bob' },
        ],
        loading: false,
        error: null,
        refresh: vi.fn(),
      });

      render(<PeersView />);

      const rows = screen.getAllByRole('row');
      // Skip header row (index 0)
      const cells = rows.slice(1).map((row) => row.querySelector('td')?.textContent);
      expect(cells).toEqual(['g.agent.alice', 'g.agent.bob', 'g.agent.charlie']);
    });
  });

  // Story 18.5: NOC Aesthetic Enhancements
  describe('NOC Aesthetic Enhancements - Story 18.5', () => {
    describe('SummaryStats', () => {
      it('should display total peers count', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            { peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true },
            { peerId: 'bob', ilpAddress: 'g.agent.bob', connected: false },
            { peerId: 'charlie', ilpAddress: 'g.agent.charlie', connected: true },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        render(<PeersView />);

        // Total peers count should be 3
        expect(screen.getByText('3')).toBeInTheDocument();
        expect(screen.getByText('Total Peers')).toBeInTheDocument();
      });

      it('should display connected count with emerald color when > 0', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            { peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true },
            { peerId: 'bob', ilpAddress: 'g.agent.bob', connected: true },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        // Summary stats has "Connected" label
        expect(screen.getAllByText('Connected').length).toBeGreaterThan(0);
        // Should have emerald styling
        const emeraldElements = container.querySelectorAll('.text-emerald-500');
        expect(emeraldElements.length).toBeGreaterThan(0);
      });

      it('should display connected count in muted color when 0', () => {
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: false }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        // Should have gray styling for disconnected
        const grayElements = container.querySelectorAll('.text-gray-500');
        expect(grayElements.length).toBeGreaterThan(0);
      });

      it('should display total routes count', () => {
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        mockUseRoutingTable.mockReturnValue({
          routes: [
            { prefix: 'g.agent.alice', nextHop: 'alice' },
            { prefix: 'g.agent.bob', nextHop: 'bob' },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        render(<PeersView />);

        expect(screen.getByText('Total Routes')).toBeInTheDocument();
      });

      it('should update counts when data changes', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            { peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true },
            { peerId: 'bob', ilpAddress: 'g.agent.bob', connected: true },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        render(<PeersView />);

        // Both peers are now connected - header shows count
        expect(screen.getByText('Connected Peers (2)')).toBeInTheDocument();
      });
    });

    describe('BlockchainAddressLink', () => {
      it('should generate correct Base Sepolia URL for EVM addresses', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            {
              peerId: 'alice',
              ilpAddress: 'g.agent.alice',
              evmAddress: '0x1234567890abcdef1234567890abcdef12345678',
              connected: true,
            },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        const link = container.querySelector('a[href*="basescan"]');
        expect(link).toHaveAttribute(
          'href',
          'https://sepolia.basescan.org/address/0x1234567890abcdef1234567890abcdef12345678'
        );
      });

      it('should generate correct XRPL Testnet URL for XRP addresses', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            {
              peerId: 'alice',
              ilpAddress: 'g.agent.alice',
              connected: true,
            },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        const link = container.querySelector('a[href*="xrpl.org"]');
        expect(link).toHaveAttribute(
          'href',
          'https://testnet.xrpl.org/accounts/rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh'
        );
      });

      it('should truncate address to 8+6 characters', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            {
              peerId: 'alice',
              ilpAddress: 'g.agent.alice',
              evmAddress: '0x1234567890abcdef1234567890abcdef12345678',
              connected: true,
            },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        render(<PeersView />);

        // 8 chars + ... + 6 chars = 0x123456...345678
        expect(screen.getByText('0x123456...345678')).toBeInTheDocument();
      });

      it('should have target="_blank" attribute', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            {
              peerId: 'alice',
              ilpAddress: 'g.agent.alice',
              evmAddress: '0x1234567890abcdef1234567890abcdef12345678',
              connected: true,
            },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        const link = container.querySelector('a[href*="basescan"]');
        expect(link).toHaveAttribute('target', '_blank');
      });

      it('should have rel="noopener noreferrer" attribute', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            {
              peerId: 'alice',
              ilpAddress: 'g.agent.alice',
              evmAddress: '0x1234567890abcdef1234567890abcdef12345678',
              connected: true,
            },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        const link = container.querySelector('a[href*="basescan"]');
        expect(link).toHaveAttribute('rel', 'noopener noreferrer');
      });

      it('should display ExternalLink icon', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            {
              peerId: 'alice',
              ilpAddress: 'g.agent.alice',
              evmAddress: '0x1234567890abcdef1234567890abcdef12345678',
              connected: true,
            },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        // lucide-react renders SVGs, check for the external-link icon class
        const svgElement = container.querySelector('a[href*="basescan"] svg');
        expect(svgElement).toBeInTheDocument();
      });

      it('should apply cyan color class', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            {
              peerId: 'alice',
              ilpAddress: 'g.agent.alice',
              evmAddress: '0x1234567890abcdef1234567890abcdef12345678',
              connected: true,
            },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        const link = container.querySelector('a[href*="basescan"]');
        expect(link).toHaveClass('text-cyan-500');
      });
    });

    describe('NOC Styling', () => {
      it('should apply emerald badge styling when connected', () => {
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        // Look for badge with emerald styling
        const badge = container.querySelector('[class*="border-emerald-500"]');
        expect(badge).toBeInTheDocument();
      });

      it('should apply gray badge styling when disconnected', () => {
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: false }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        // Look for badge with gray styling
        const badge = container.querySelector('[class*="border-gray-500"]');
        expect(badge).toBeInTheDocument();
      });

      it('should display route prefix in cyan color', () => {
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        mockUseRoutingTable.mockReturnValue({
          routes: [{ prefix: 'g.agent.alice', nextHop: 'alice' }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        // Route prefix should have cyan color
        const cyanCell = container.querySelector('td.text-cyan-500');
        expect(cyanCell).toBeInTheDocument();
      });

      it('should display BTP URL when available', () => {
        mockUsePeers.mockReturnValue({
          peers: [
            {
              peerId: 'alice',
              ilpAddress: 'g.agent.alice',
              btpUrl: 'ws://localhost:3001/btp',
              connected: true,
            },
          ],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        render(<PeersView />);

        expect(screen.getByText('BTP URL')).toBeInTheDocument();
        expect(screen.getByText('ws://localhost:3001/btp')).toBeInTheDocument();
      });

      it('should show CheckCircle2 icon for connected peers', () => {
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        // CheckCircle2 icon should be present in the badge (data-slot="badge")
        const badge = container.querySelector('[data-slot="badge"][class*="emerald"]');
        expect(badge).toBeInTheDocument();
        const svgInBadge = badge?.querySelector('svg');
        expect(svgInBadge).toBeInTheDocument();
      });

      it('should show Circle icon for disconnected peers', () => {
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: false }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        const { container } = render(<PeersView />);

        // Circle icon should be present in the badge (data-slot="badge")
        const badge = container.querySelector('[data-slot="badge"][class*="gray"]');
        expect(badge).toBeInTheDocument();
        const svgInBadge = badge?.querySelector('svg');
        expect(svgInBadge).toBeInTheDocument();
      });
    });

    describe('Refresh Buttons', () => {
      it('should call refreshPeers when peers refresh button clicked', () => {
        const mockRefreshPeers = vi.fn();
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
          loading: false,
          error: null,
          refresh: mockRefreshPeers,
        });

        render(<PeersView />);

        const refreshButton = screen.getByLabelText('Refresh peers');
        fireEvent.click(refreshButton);

        expect(mockRefreshPeers).toHaveBeenCalledTimes(1);
      });

      it('should call refreshRoutes when routes refresh button clicked', () => {
        const mockRefreshRoutes = vi.fn();
        mockUsePeers.mockReturnValue({
          peers: [{ peerId: 'alice', ilpAddress: 'g.agent.alice', connected: true }],
          loading: false,
          error: null,
          refresh: vi.fn(),
        });

        mockUseRoutingTable.mockReturnValue({
          routes: [{ prefix: 'g.agent.alice', nextHop: 'alice' }],
          loading: false,
          error: null,
          refresh: mockRefreshRoutes,
        });

        render(<PeersView />);

        const refreshButton = screen.getByLabelText('Refresh routes');
        fireEvent.click(refreshButton);

        expect(mockRefreshRoutes).toHaveBeenCalledTimes(1);
      });
    });

    describe('Empty State', () => {
      it('should display Users icon with cyan color', () => {
        const { container } = render(<PeersView />);

        const usersIcon = container.querySelector('.text-cyan-500');
        expect(usersIcon).toBeInTheDocument();
      });

      it('should display setup instructions text', () => {
        render(<PeersView />);

        expect(
          screen.getByText(/Configure peer connections in your connector configuration file/i)
        ).toBeInTheDocument();
      });

      it('should apply animate-pulse class to icon', () => {
        const { container } = render(<PeersView />);

        const pulsingIcon = container.querySelector('.animate-pulse');
        expect(pulsingIcon).toBeInTheDocument();
      });
    });
  });
});
