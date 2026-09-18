import express from 'express';
import cors from 'cors';
import { createSolanaFaucet } from './solana.js';
import { createBaseSepoliaFaucet, baseSepoliaInfo } from './base-sepolia.js';

const app = express();
const PORT = process.env.PORT || 3500;

// Middleware
app.use(cors());
app.use(express.json());
app.use(express.static('public'));

// Solana faucet (null when not configured — a Base-Sepolia-only deploy still works).
const solanaFaucet = createSolanaFaucet();

// Read the mint's authority once, in the BACKGROUND, at boot. The leg mints on
// demand, so it can only serve a mint this faucet is the authority of; that is
// an on-chain fact `createSolanaFaucet` cannot check without dialling (it is
// synchronous by design). Doing it here means a box wired to the wrong mint
// says so in its logs at startup instead of at the first drip request hours
// later. Non-fatal: the drip path awaits the same memoised check and answers
// 503, and a failure caused by an unreachable RPC is not cached.
if (solanaFaucet) {
  solanaFaucet
    .assertMintAuthority()
    .then(() =>
      console.log('   Mint authority confirmed: this faucet can mint its configured USDC.')
    )
    .catch((err) => console.error('⚠️  Solana USDC leg will refuse drips:', err.message));
}

// Base Sepolia faucet (null when BASE_SEPOLIA_FAUCET_KEY unset — route 503s).
// Mints the ungated public mock USDC on the PUBLIC Base Sepolia testnet
// (chainId 84532) + best-effort ETH gas. Mirrors createSolanaFaucet's shape.
const baseSepoliaFaucet = createBaseSepoliaFaucet();

// Serialize Solana drips so concurrent requests don't race the treasury's
// transaction signing / blockhash reuse.
let solanaQueue = Promise.resolve();

// Serialize Base Sepolia drips the same way — the faucet key's nonce is
// read-then-spent, so concurrent mint txs must not race it.
let baseSepoliaQueue = Promise.resolve();

// Health check
app.get('/health', (req, res) => {
  res.json({ status: 'ok' });
});

// Get faucet info
app.get('/api/info', async (req, res) => {
  try {
    res.json({
      // ── Per-chain capability map ──
      //
      // USDC only (toon-meta#310 §4.6, connector#898): this faucet dispenses no
      // native gas/token of any chain — the local-anvil EVM leg (`/api/request`)
      // and the native-SOL leg are gone, not just unconfigured. The Mina leg
      // went with Mina itself (ADR 0065). Each surviving leg advertises only
      // its USDC-drip route.
      chains: {
        solana: solanaFaucet
          ? {
              enabled: true,
              route: '/api/solana/usdc-request',
              ready: true,
              drips: { usdc: String(solanaFaucet.usdcAmount) },
              cooldownHours: String(solanaFaucet.cooldownMs / 3_600_000),
              usdcMint: solanaFaucet.mint,
              rpcUrl: solanaFaucet.rpcUrl,
              mintMode: solanaFaucet.mintMode,
            }
          : { enabled: false, route: '/api/solana/usdc-request', ready: false },
        baseSepolia: baseSepoliaInfo(baseSepoliaFaucet),
      },
    });
  } catch (error) {
    res.status(500).json({
      error: 'Failed to get faucet info',
      message: error.message,
    });
  }
});

// ---------------------------------------------------------------------------
// Solana route — POST /api/solana/usdc-request { address }
//
// USDC only (no SOL leg at all — the SOL-dispensing route it used to sit beside
// is retired, toon-meta#310 §4.6): transfers mock USDC from the devnet treasury.
// The treasury pays the fee + ATA rent, so this succeeds even when the public
// devnet airdrop is dry/rate-limited and even if the recipient holds 0 SOL.
// Recipients get their SOL for gas from the chain's own faucet.
// ---------------------------------------------------------------------------
app.post('/api/solana/usdc-request', (req, res) => {
  if (!solanaFaucet) {
    res.status(503).json({
      error: 'Solana faucet not configured',
      message: 'Set SOLANA_USDC_MINT and mount SOLANA_FAUCET_KEYPAIR to enable the Solana route.',
    });
    return;
  }

  const { address } = req.body || {};
  if (!address || !solanaFaucet.isValidAddress(address)) {
    res.status(400).json({ error: 'Invalid Solana address (expected base58 pubkey)' });
    return;
  }

  // Per-address cooldown: minting is unbounded on-chain (this faucet holds the
  // authority), so this window is the only limit. The drip also spends the
  // faucet's own SOL on the tx fee and a possible ATA rent, but never sends
  // SOL to the recipient.
  const claim = solanaFaucet.claim(address);
  if (!claim.allowed) {
    res
      .status(429)
      .set('Retry-After', String(Math.ceil(claim.retryAfterMs / 1000)))
      .json({
        error: 'Solana drip rate limit',
        message: 'This address already received a Solana drip inside the cooldown window.',
        retryAfterMs: claim.retryAfterMs,
      });
    return;
  }

  console.log(`💧 Solana USDC-only faucet request for ${address}`);
  solanaQueue = solanaQueue
    .then(async () => {
      const result = await solanaFaucet.drip(address);
      console.log(`  ✅ Solana USDC-only request completed for ${address}`);
      res.json({
        success: true,
        chain: 'solana',
        mode: 'usdc-only',
        address,
        transactions: result,
      });
    })
    .catch((error) => {
      solanaFaucet.release(address);
      console.error('❌ Solana USDC-only request failed:', error);
      if (!res.headersSent) {
        if (error.code === 'VALIDATOR_STALLED') {
          res.status(503).json({
            error: 'Solana validator not producing blocks',
            message: error.message,
          });
          return;
        }
        // Misconfiguration, not a failed request: this box cannot mint the
        // token it is pointed at, and no retry will change that. 503 (not 500)
        // for the same reason an unconfigured leg 503s — the fault is the
        // deploy's, and the message names both keys.
        if (error.code === 'MINT_AUTHORITY_MISMATCH') {
          res.status(503).json({
            error: "Solana faucet is not this mint's authority",
            message: error.message,
          });
          return;
        }
        res.status(500).json({
          error: 'Solana USDC-only request failed',
          message: error.message,
        });
      }
    });
});

// ---------------------------------------------------------------------------
// Base Sepolia route — POST /api/base-sepolia/request { address }
//
// Mints the ungated mock USDC on the PUBLIC Base Sepolia testnet (chainId 84532)
// to `address`, and best-effort drips a little Base Sepolia ETH for gas when the
// faucet key holds a surplus. Because mint() is ungated the faucet key holds no
// USDC — it coins fresh tokens on demand; it only needs Base Sepolia ETH for
// gas. Returns a clear 503 when the leg isn't configured (so the Solana leg
// still works), and honours the same per-address cooldown as that leg.
// ---------------------------------------------------------------------------
app.post('/api/base-sepolia/request', (req, res) => {
  if (!baseSepoliaFaucet) {
    res.status(503).json({
      error: 'Base Sepolia faucet not configured',
      message:
        'Set BASE_SEPOLIA_FAUCET_KEY (an EVM key funded with Base Sepolia ETH for gas) to enable the Base Sepolia route.',
    });
    return;
  }

  const { address } = req.body || {};
  if (!address || !baseSepoliaFaucet.isValidAddress(address)) {
    res.status(400).json({ error: 'Invalid Ethereum address' });
    return;
  }

  // Per-address cooldown: claim BEFORE enqueueing (reserves the slot so
  // concurrent requests for the same address cannot double-drip); released
  // below if the drip fails.
  const claim = baseSepoliaFaucet.claim(address);
  if (!claim.allowed) {
    res
      .status(429)
      .set('Retry-After', String(Math.ceil(claim.retryAfterMs / 1000)))
      .json({
        error: 'Base Sepolia drip rate limit',
        message: 'This address already received a Base Sepolia drip inside the cooldown window.',
        retryAfterMs: claim.retryAfterMs,
      });
    return;
  }

  console.log(`💧 Base Sepolia faucet request for ${address}`);
  baseSepoliaQueue = baseSepoliaQueue
    .then(async () => {
      const result = await baseSepoliaFaucet.drip(address);
      console.log(`  ✅ Base Sepolia faucet request completed for ${address}`);
      res.json({ success: true, chain: 'base-sepolia', address, transactions: result });
    })
    .catch((error) => {
      // A failed drip must not burn the address's cooldown.
      baseSepoliaFaucet.release(address);
      console.error('❌ Base Sepolia faucet request failed:', error);
      if (!res.headersSent) {
        res.status(500).json({
          error: 'Base Sepolia faucet request failed',
          message: error.message,
        });
      }
    });
});

// Start server
app.listen(PORT, () => {
  console.log('');
  console.log('═══════════════════════════════════════════════');
  console.log('   🚰 Token Faucet');
  console.log('═══════════════════════════════════════════════');
  console.log(`   Port:          ${PORT}`);
  console.log(`   Solana:        ${solanaFaucet ? 'enabled' : 'disabled'}`);
  console.log(
    `   Base Sepolia:  ${baseSepoliaFaucet ? `mint ${baseSepoliaFaucet.usdcAmount} USDC (chainId ${baseSepoliaFaucet.chainId}, ungated mint)` : 'disabled (503)'}`
  );
  console.log('═══════════════════════════════════════════════');
  console.log('');

  console.log('✅ Faucet is running!');
  console.log(`   UI: http://localhost:${PORT}`);
  console.log('');
});
