// Tests for the Base Sepolia mock-USDC mint leg.
//
// We don't hit a live Base Sepolia RPC here — executeBaseSepoliaDrip takes the
// token contract, provider and signer as plain injected objects, so the tests
// feed it fakes that record what they were called with. This exercises the
// three things the leg promises: the mint call is built correctly (recipient +
// amount + token), the per-address rate limit is honoured, and the ETH gas drip
// is best-effort (skipped on a low balance or a send failure, never failing the
// mint).
//
// Run: node --test
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ethers } from 'ethers';

import { executeBaseSepoliaDrip, baseSepoliaInfo } from '../src/base-sepolia.js';
import { createDripLimiter } from '../src/drip-limiter.js';

const RECIPIENT = '0x70997970C51812dc3A010C7d01b50e0d17dc79C8';
const TOKEN = '0x0C996d7c934c79a6255254875607Fe69df25C0E1';
const FAUCET = '0x6bafedaF18FF62f0a63dd0148bafa163204627F6';
const USDC_AMOUNT = ethers.parseUnits('1000', 6); // 1000_000000 base units

// A fake token contract whose mint() records its args and returns a tx stub.
function fakeToken() {
  const calls = [];
  return {
    calls,
    async mint(to, amount) {
      calls.push({ to, amount });
      return { hash: '0xMINT', wait: async () => ({ status: 1 }) };
    },
  };
}

test('mint call is built with the right recipient and amount', async () => {
  const token = fakeToken();
  const result = await executeBaseSepoliaDrip({
    token,
    provider: { getBalance: async () => 0n },
    signer: { sendTransaction: async () => assert.fail('ETH drip must not run when amount is 0') },
    faucetAddress: FAUCET,
    recipient: RECIPIENT,
    usdcAmount: USDC_AMOUNT,
    usdcAmountLabel: '1000',
    ethAmount: 0n, // disabled
    ethAmountLabel: '0',
    ethReserve: ethers.parseEther('0.01'),
    tokenAddress: TOKEN,
  });

  assert.equal(token.calls.length, 1);
  assert.equal(token.calls[0].to, RECIPIENT);
  assert.equal(token.calls[0].amount, USDC_AMOUNT); // exactly 1000_000000
  assert.equal(result.usdc.hash, '0xMINT');
  assert.equal(result.usdc.amount, '1000');
  assert.equal(result.usdc.symbol, 'USDC');
  assert.equal(result.usdc.token, TOKEN);
  // ETH drip disabled → skipped, mint still succeeded.
  assert.equal(result.eth.dripped, false);
  assert.equal(result.eth.skipped, true);
});

test('ETH gas drip runs when the faucet balance is above reserve+drip', async () => {
  const token = fakeToken();
  const sent = [];
  const result = await executeBaseSepoliaDrip({
    token,
    provider: { getBalance: async () => ethers.parseEther('0.05') }, // plenty
    signer: {
      async sendTransaction(tx) {
        sent.push(tx);
        return { hash: '0xETH', wait: async () => ({ status: 1 }) };
      },
    },
    faucetAddress: FAUCET,
    recipient: RECIPIENT,
    usdcAmount: USDC_AMOUNT,
    usdcAmountLabel: '1000',
    ethAmount: ethers.parseEther('0.001'),
    ethAmountLabel: '0.001',
    ethReserve: ethers.parseEther('0.01'),
    tokenAddress: TOKEN,
  });

  assert.equal(sent.length, 1);
  assert.equal(sent[0].to, RECIPIENT);
  assert.equal(sent[0].value, ethers.parseEther('0.001'));
  assert.equal(result.eth.dripped, true);
  assert.equal(result.eth.hash, '0xETH');
});

test('ETH gas drip is skipped (mint still succeeds) when the balance is too low', async () => {
  const token = fakeToken();
  const result = await executeBaseSepoliaDrip({
    token,
    provider: { getBalance: async () => ethers.parseEther('0.005') }, // below reserve+drip
    signer: { sendTransaction: async () => assert.fail('ETH must not be sent on a low balance') },
    faucetAddress: FAUCET,
    recipient: RECIPIENT,
    usdcAmount: USDC_AMOUNT,
    usdcAmountLabel: '1000',
    ethAmount: ethers.parseEther('0.001'),
    ethAmountLabel: '0.001',
    ethReserve: ethers.parseEther('0.01'),
    tokenAddress: TOKEN,
  });

  // Mint landed regardless.
  assert.equal(token.calls.length, 1);
  assert.equal(result.usdc.hash, '0xMINT');
  // ETH drip skipped, not errored.
  assert.equal(result.eth.dripped, false);
  assert.equal(result.eth.skipped, true);
  assert.match(result.eth.reason, /below reserve/);
});

test('a failing ETH gas drip does not fail the whole request', async () => {
  const token = fakeToken();
  const result = await executeBaseSepoliaDrip({
    token,
    provider: { getBalance: async () => ethers.parseEther('0.05') },
    signer: {
      async sendTransaction() {
        throw new Error('rpc down');
      },
    },
    faucetAddress: FAUCET,
    recipient: RECIPIENT,
    usdcAmount: USDC_AMOUNT,
    usdcAmountLabel: '1000',
    ethAmount: ethers.parseEther('0.001'),
    ethAmountLabel: '0.001',
    ethReserve: ethers.parseEther('0.01'),
    tokenAddress: TOKEN,
  });

  // Mint still succeeded; ETH failure is swallowed into a skipped result.
  assert.equal(result.usdc.hash, '0xMINT');
  assert.equal(result.eth.dripped, false);
  assert.equal(result.eth.skipped, true);
  assert.match(result.eth.reason, /ETH drip failed: rpc down/);
});

test('rate limit honoured: a second drip inside the cooldown is refused', () => {
  const limiter = createDripLimiter({ cooldownMs: 24 * 60 * 60 * 1000 });
  assert.equal(limiter.claim(RECIPIENT).allowed, true);
  const second = limiter.claim(RECIPIENT);
  assert.equal(second.allowed, false);
  assert.ok(second.retryAfterMs > 0);
  // release() rolls back so a FAILED drip costs no cooldown.
  limiter.release(RECIPIENT);
  assert.equal(limiter.claim(RECIPIENT).allowed, true);
});

test('baseSepoliaInfo advertises the route (enabled + disabled shapes)', () => {
  assert.deepEqual(baseSepoliaInfo(null), {
    enabled: false,
    route: '/api/base-sepolia/request',
    ready: false,
  });

  const info = baseSepoliaInfo({
    chainId: 84532,
    usdcAmount: '1000',
    ethAmount: '0',
    tokenAddress: TOKEN,
    faucetKey: FAUCET,
    rpcUrl: 'https://sepolia.base.org',
  });
  assert.equal(info.enabled, true);
  assert.equal(info.route, '/api/base-sepolia/request');
  assert.equal(info.ready, true);
  assert.equal(info.chainId, 84532);
  assert.equal(info.tokenAddress, TOKEN);
  assert.equal(info.drips.usdc, '1000');
  assert.equal(info.mintMode, 'ungated-mint');
});
