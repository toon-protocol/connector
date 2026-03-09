import { useState, useEffect, useCallback } from 'react';

export interface PeerInfo {
  peerId: string;
  ilpAddress: string;
  evmAddress?: string;
  btpUrl?: string;
  connected: boolean;
  petname?: string;
  pubkey?: string;
}

export interface UsePeersResult {
  peers: PeerInfo[];
  loading: boolean;
  error: string | null;
  refresh: () => void;
}

const POLL_INTERVAL_MS = 10_000;

/**
 * Polling hook that fetches GET /api/peers every 10 seconds.
 * Gracefully handles 404/errors (endpoint may not exist in non-agent contexts).
 */
export function usePeers(): UsePeersResult {
  const [peers, setPeers] = useState<PeerInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchPeers = useCallback(async () => {
    try {
      const response = await fetch('/api/peers');
      if (response.status === 404) {
        setLoading(false);
        return;
      }
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      const json = await response.json();
      setPeers(json.peers ?? []);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to fetch peers');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchPeers();
    const interval = setInterval(fetchPeers, POLL_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [fetchPeers]);

  return { peers, loading, error, refresh: fetchPeers };
}
