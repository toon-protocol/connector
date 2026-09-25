// ---------------------------------------------------------------------------
// Base Sepolia faucet — devnet-USDC mint drip (+ best-effort ETH gas)
// ---------------------------------------------------------------------------
// The PUBLIC Base Sepolia testnet (chainId 84532) hosts the devnet USDC: Circle's
// FiatToken v2.2, the same code as Base mainnet USDC, deployed 2026-09-25 (#1337)
// so a client can deposit without gas through ERC-3009. Its
// `mint(address,uint256)` is MINTER-GATED, and the faucet key is a minter with an
// unlimited allowance. So, unlike the anvil EVM leg (which TRANSFERS pre-minted
// tokens from the deployer), this leg does NOT need the faucet key to hold any
// USDC: it just calls `mint(recipient, amount)`, coining new tokens on demand.
// The faucet key only needs Base Sepolia ETH to pay gas for the mint tx. (The
// mock ERC-20 it replaced, 0x49beE1…a9Ce, had an ungated mint and no ERC-3009.)
//
// It ALSO best-effort drips a little Base Sepolia ETH for gas (mirroring the
// anvil EVM leg, which drips ETH too) — but only when the faucet key holds a
// surplus above a configured reserve. On a low balance the ETH drip is SKIPPED
// (never failing the whole request), because minting fresh testnet USDC is the
// leg's primary job and a user can top up gas from the public Base faucet.
//
// Everything here is OPTIONAL: with BASE_SEPOLIA_FAUCET_KEY unset,
// `createBaseSepoliaFaucet` returns null and the route answers a clear 503, so
// deploys that don't want the Base Sepolia leg still boot fine (mirrors the
// Solana leg).
import { ethers, NonceManager } from 'ethers';
import { createDripLimiter } from './drip-limiter.js';

const BASE_SEPOLIA_RPC_URL = process.env.BASE_SEPOLIA_RPC_URL || 'https://sepolia.base.org';
const BASE_SEPOLIA_CHAIN_ID = Number(process.env.BASE_SEPOLIA_CHAIN_ID || '84532');
// Devnet USDC: Circle FiatToken v2.2 on Base Sepolia, 2026-09-25 (#1337).
const BASE_SEPOLIA_USDC =
  process.env.BASE_SEPOLIA_USDC || '0x0C996d7c934c79a6255254875607Fe69df25C0E1';
const BASE_SEPOLIA_FAUCET_KEY = process.env.BASE_SEPOLIA_FAUCET_KEY || '';
// 6 decimals — real-USDC standard, matches the deployed FiatToken.
const BASE_SEPOLIA_USDC_DECIMALS = Number(process.env.BASE_SEPOLIA_USDC_DECIMALS || '6');
// USDC per drip (whole tokens; default 1,000 USDC = 1000_000000 base units).
const BASE_SEPOLIA_USDC_AMOUNT = process.env.BASE_SEPOLIA_USDC_AMOUNT || '1000';
// Best-effort ETH gas drip per request (whole ETH string; '0' disables it).
const BASE_SEPOLIA_ETH_AMOUNT = process.env.BASE_SEPOLIA_ETH_AMOUNT || '0';
// Keep at least this much ETH in the faucet key — the ETH drip only runs when
// the balance sits comfortably above (reserve + drip), so the key never spends
// itself out of gas for the mints (its primary job).
const BASE_SEPOLIA_ETH_RESERVE = process.env.BASE_SEPOLIA_ETH_RESERVE || '0.01';
// Per-address cooldown (default 24h) — mirrors the on-chain-style daily cap the
// other USDC legs apply, so one address can't drain repeated drips.
const BASE_SEPOLIA_COOLDOWN_MS = Number(
  process.env.BASE_SEPOLIA_COOLDOWN_MS || String(24 * 60 * 60 * 1000)
);

// Minimal ABI: the ungated mint + the reads we use for balances/metadata.
const MOCK_USDC_ABI = [
  'function mint(address to, uint256 amount)',
  'function balanceOf(address account) view returns (uint256)',
  'function decimals() view returns (uint8)',
  'function symbol() view returns (string)',
];

// Pure drip executor — extracted so it can be unit-tested with a FAKE token
// contract + provider (no network). Given a `token` (with `mint(to, amount)`),
// a `provider` (with `getBalance`), and a `signer` (with `sendTransaction`), it:
//   1. mints `usdcAmount` base units to `recipient` (ungated — no faucet USDC
//      balance needed), then
//   2. best-effort drips `ethAmount` wei of gas ETH ONLY when the faucet key's
//      balance sits above (ethReserve + ethAmount). A low balance, or a send
//      failure, is SKIPPED — never failing the request, since the mint landed.
// Returns the { usdc, eth } shape the route serialises to the client.
export async function executeBaseSepoliaDrip({
  token,
  provider,
  signer,
  faucetAddress,
  recipient,
  usdcAmount,
  usdcAmountLabel,
  ethAmount,
  ethAmountLabel,
  ethReserve,
  tokenAddress,
  log = () => {},
}) {
  // 1. Mint fresh USDC to the recipient.
  const mintTx = await token.mint(recipient, usdcAmount);
  log(`  📤 Minting ${usdcAmountLabel} USDC → ${recipient}: ${mintTx.hash}`);
  await mintTx.wait();

  // 2. Best-effort ETH gas drip.
  let eth = { dripped: false, skipped: true, reason: 'ETH drip disabled (amount 0)' };
  if (ethAmount > 0n) {
    try {
      const balance = await provider.getBalance(faucetAddress);
      if (balance >= ethReserve + ethAmount) {
        const ethTx = await signer.sendTransaction({ to: recipient, value: ethAmount });
        log(`  📤 Dripping ${ethAmountLabel} ETH → ${recipient}: ${ethTx.hash}`);
        await ethTx.wait();
        eth = { dripped: true, hash: ethTx.hash, amount: ethAmountLabel };
      } else {
        eth = {
          dripped: false,
          skipped: true,
          reason: `faucet ETH balance below reserve+drip; mint still succeeded`,
        };
        log(`  ⏭️  ETH drip skipped (balance below reserve+drip)`);
      }
    } catch (ethErr) {
      // Never let a gas-drip failure fail the request — the mint succeeded.
      eth = { dripped: false, skipped: true, reason: `ETH drip failed: ${ethErr.message}` };
      log(`  ⚠️  Base Sepolia ETH drip failed (mint still succeeded): ${ethErr.message}`);
    }
  }

  return {
    usdc: { hash: mintTx.hash, amount: usdcAmountLabel, symbol: 'USDC', token: tokenAddress },
    eth,
  };
}

// Returns a faucet object, or null if the Base Sepolia leg is not configured for
// this deploy (BASE_SEPOLIA_FAUCET_KEY unset). Mirrors createSolanaFaucet's
// shape. Throws (fail-loud) only when a key IS configured but is malformed —
// that is operator misconfiguration we must not paper over.
export function createBaseSepoliaFaucet() {
  if (!BASE_SEPOLIA_FAUCET_KEY) {
    console.log(
      'ℹ️  Base Sepolia faucet disabled: BASE_SEPOLIA_FAUCET_KEY not set (route will 503).'
    );
    return null;
  }

  const provider = new ethers.JsonRpcProvider(BASE_SEPOLIA_RPC_URL, {
    chainId: BASE_SEPOLIA_CHAIN_ID,
    name: 'base-sepolia',
  });

  let signer;
  let wallet;
  try {
    wallet = new ethers.Wallet(BASE_SEPOLIA_FAUCET_KEY);
    // NonceManager so serialized drips get sequential nonces even if a tx is
    // still propagating when the next one is signed.
    signer = new NonceManager(wallet.connect(provider));
  } catch {
    // Don't echo the key or the raw error (which may embed it).
    throw new Error('BASE_SEPOLIA_FAUCET_KEY is not a valid EVM private key.');
  }

  const token = new ethers.Contract(BASE_SEPOLIA_USDC, MOCK_USDC_ABI, signer);
  const usdcAmount = ethers.parseUnits(BASE_SEPOLIA_USDC_AMOUNT, BASE_SEPOLIA_USDC_DECIMALS);
  const ethAmount = ethers.parseEther(BASE_SEPOLIA_ETH_AMOUNT);
  const ethReserve = ethers.parseEther(BASE_SEPOLIA_ETH_RESERVE);

  // Per-address off-chain cooldown (claim BEFORE the drip; released on failure).
  const limiter = createDripLimiter({ cooldownMs: BASE_SEPOLIA_COOLDOWN_MS });

  console.log('✅ Base Sepolia faucet enabled (ungated mock-USDC mint)');
  console.log(`   RPC:        ${BASE_SEPOLIA_RPC_URL} (chainId ${BASE_SEPOLIA_CHAIN_ID})`);
  console.log(`   USDC token: ${BASE_SEPOLIA_USDC}`);
  console.log(`   Faucet key: ${wallet.address}`);
  console.log(
    `   Per drip:   ${BASE_SEPOLIA_USDC_AMOUNT} USDC (mint)` +
      (ethAmount > 0n ? ` + up to ${BASE_SEPOLIA_ETH_AMOUNT} ETH (best-effort gas)` : '')
  );

  return {
    rpcUrl: BASE_SEPOLIA_RPC_URL,
    chainId: BASE_SEPOLIA_CHAIN_ID,
    tokenAddress: BASE_SEPOLIA_USDC,
    faucetKey: wallet.address,
    usdcAmount: BASE_SEPOLIA_USDC_AMOUNT,
    ethAmount: BASE_SEPOLIA_ETH_AMOUNT,
    cooldownMs: BASE_SEPOLIA_COOLDOWN_MS,

    isValidAddress(address) {
      return ethers.isAddress(address);
    },

    // Per-address cooldown wrappers (route claims before enqueue, releases on
    // failure — mirrors the Solana leg's concurrency-safe reservation).
    claim(address) {
      return limiter.claim(ethers.getAddress(address));
    },
    release(address) {
      limiter.release(ethers.getAddress(address));
    },

    async drip(address) {
      const recipient = ethers.getAddress(address);
      return executeBaseSepoliaDrip({
        token,
        provider,
        signer,
        faucetAddress: wallet.address,
        recipient,
        usdcAmount,
        usdcAmountLabel: BASE_SEPOLIA_USDC_AMOUNT,
        ethAmount,
        ethAmountLabel: BASE_SEPOLIA_ETH_AMOUNT,
        ethReserve,
        tokenAddress: BASE_SEPOLIA_USDC,
        log: (msg) => console.log(msg),
      });
    },
  };
}

// Capability descriptor surfaced at /api/info. `faucet` is the live
// createBaseSepoliaFaucet() (or null when unconfigured). Mirrors the Solana
// fragment in index.js's /api/info chains map.
export function baseSepoliaInfo(faucet) {
  if (!faucet) {
    return { enabled: false, route: '/api/base-sepolia/request', ready: false };
  }
  return {
    enabled: true,
    route: '/api/base-sepolia/request',
    ready: true,
    chainId: faucet.chainId,
    drips: {
      usdc: faucet.usdcAmount,
      ...(faucet.ethAmount !== '0' ? { eth: `${faucet.ethAmount} (best-effort)` } : {}),
    },
    tokenAddress: faucet.tokenAddress,
    faucetKey: faucet.faucetKey,
    rpcUrl: faucet.rpcUrl,
    mintMode: 'ungated-mint',
  };
}
