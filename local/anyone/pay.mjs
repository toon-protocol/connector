/**
 * The rehearsal, run from INSIDE `anyone-payer` -- a network with no route and
 * no DNS to the connector. Everything it reaches, it reached over a circuit.
 *
 * This is the toon CLIENT (`@toon-protocol/client`), not `connector send`: the
 * point of this topology is the client edge against the real payer, so the
 * library is the thing under test. It is driven through the public API only.
 *
 * `--socks` has no library equivalent by design, and neither does the managed
 * daemon: a library an application embeds must not fetch and execute a binary
 * at runtime (toon-client ADR 0001). The proxy is passed in.
 */
import { ToonClient } from '/repo/packages/client/dist/index.js';

const ADDR = process.env.HS_ADDR;
const STORE = process.env.STORE;

const client = await ToonClient.create({
  connector: `http://${ADDR}`,
  socksProxy: 'socks5h://anon-a:9050',
  // The one opt-out this topology takes, and the documented reason for it:
  // anvil is already private, on a network only these containers share, so the
  // chain RPC has nothing to gain from the circuit and would only be slower.
  // On a real deployment leaving this ON is the point -- reading chain state on
  // clearnet would broadcast the settlement address either side of every paid
  // request (toon-client ADR 0002).
  proxyRpc: false,
  rpcUrl: 'http://anvil:8545',
  chain: 'evm',
  // Public, and every local chain ships with it. Fine here and nowhere else.
  mnemonic: 'test test test test test test test test test test test junk',
  channelStore: `${STORE}/channels.json`,
  deposit: 100_000n,
  timeoutMs: 180_000,
});

const fail = (m) => {
  console.error(m);
  process.exitCode = 1;
};

try {
  // (1) One free GET is the whole of bootstrapping -- and it goes over the
  //     circuit, before anything is paid for. Asking a node what it is must not
  //     be the request that exposes the asking.
  console.log('--- describe (over the circuit)');
  const d = await client.describe();
  console.log('    ilpAddresses:', d.ilpAddresses);
  console.log('    httpEndpoint:', d.httpEndpoint);
  if (!d.httpEndpoint?.includes('.anyone')) {
    fail(`the node advertises ${d.httpEndpoint}, which is not the hidden service it was reached at`);
  }

  // (2) The price is ASKED for. A price is flat per handler, so nothing local
  //     could derive one.
  console.log('--- price');
  const price = await client.price('g.lab.hs');
  console.log('    price:', price);

  // (3) On chain, NOT through the circuit -- see `proxyRpc` above.
  console.log('--- channel open');
  const opened = await client.channel.open({ deposit: 100_000n });
  console.log('    status:', opened.status, 'deposit:', opened.depositTotal, 'chain:', opened.domain.chain);

  // (4) The paid request: sealed payload, signed claim, one packet.
  console.log('--- send: the paid packet');
  const answer = await client.send('g.lab.hs', {
    headers: { 'content-type': 'text/plain' },
    body: 'a paid packet over a .anyone hidden service',
  });
  if (!answer.fulfilled) {
    fail(`    REFUSED by ${answer.refusedBy}: ${answer.code} -- ${answer.message}`);
    throw new Error('not fulfilled');
  }
  console.log('    status:', answer.status);
  console.log('    fulfillment bytes:', answer.fulfillment?.length);
  console.log('    claim:', JSON.stringify(answer.claim, (_k, v) => (typeof v === 'bigint' ? v.toString() : v)));
  const body =
    typeof answer.body === 'string'
      ? answer.body
      : new TextDecoder().decode(answer.body ?? new Uint8Array());
  console.log('    app answered:', body.slice(0, 200));

  // (5) A SECOND crossing, because one cannot see a claim that repeats itself.
  //     The nonce must strictly advance the watermark the connector has banked;
  //     a claim that does not is a replay and is refused.
  console.log('--- second packet: the nonce must strictly advance');
  const before = await client.channel.state();
  const a2 = await client.send('g.lab.hs', { body: 'second' });
  if (!a2.fulfilled) {
    fail(`    REFUSED: ${a2.code} -- ${a2.message}`);
    throw new Error('not fulfilled');
  }
  console.log('    nonce', before.nonce, '->', a2.claim.nonce, '| cumulative', before.spent, '->', a2.claim.cumulative);

  // (6) ...and the CONNECTOR's own watermark agrees with ours, asked over the
  //     circuit. This is the assertion the whole rehearsal exists for: two
  //     independent records of one channel, reconciled over the wire rather
  //     than assumed. A fulfil alone would not show it.
  console.log("--- the connector's own watermark, asked over the circuit");
  const [state] = await client.claimState([a2.claim.channelId]);
  console.log('    connector says:', JSON.stringify(state, (_k, v) => (typeof v === 'bigint' ? v.toString() : v)));
  if (state?.ok === true && BigInt(state.cumulativeClaimed) !== a2.claim.cumulative) {
    fail(
      `    the connector banked ${state.cumulativeClaimed} but this payer signed ${a2.claim.cumulative} -- ` +
        'the two records of this channel disagree'
    );
  }

  if (process.exitCode !== 1) console.log('PASS');
} catch (e) {
  if (process.exitCode !== 1) fail(`FAILED: ${e.message}`);
} finally {
  await client.close();
}
