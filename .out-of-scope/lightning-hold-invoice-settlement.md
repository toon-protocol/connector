# A Lightning hold invoice as the claim behind a packet

The connector does not take a Lightning payment as the claim covering a PREPARE, at the client
edge or on a peering. A claim is a voucher on an x402 `batch-settlement` channel and nothing else
(ADR 0075). Selling a route for sats is an app's job: `l402-door` does it today, with no connector
change.

## Why this is out of scope

The decision was made on 2026-10-07 (#1480).

The request rested on one observation: an ILP execution condition is the SHA-256 of a 32-byte
fulfilment, and a Lightning payment hash is the SHA-256 of a 32-byte preimage, so a hold invoice
keyed to a PREPARE's condition settles with the bytes the FULFILL carries. The arithmetic is
right. The rest does not survive the records already accepted here.

- **A PREPARE carries no condition.** ADR 0069 (#1269) took the execution condition off the wire,
  because a value that is the same at every hop and different for every packet is a join key for
  any two hops on a path. Keying a hold invoice to it brings that value back, and on routed
  Lightning it puts the same hash at every Lightning hop too. The payer would also have to compute
  `sha256(derive_fulfillment(shared_secret))` itself and hand it to the connector before the
  packet, which is a new side channel and a round trip per packet.
- **The lock does not remove the trust at a termination.** The fulfilment is derived from the
  secret the sender sealed to the terminating connector (ADR 0019). A terminating connector that
  holds the invoice can open the wrap, derive the preimage and settle without calling the app. The
  buyer's protection there is the same as it is today: it chose that counterparty.
- **At a forwarding hop the connector takes the reject.** A forward pays the next hop with a
  voucher that rides the PREPARE (ADR 0042). A connector covered by a hold invoice would pay
  downstream on PREPARE and collect upstream only on FULFILL. That is the postpay ordering #1145
  deleted, and it reverses the owner's ruling on #1031: value in flight is the sender's risk,
  bounded by a small packet, and the path does not lock value. A client could spend the
  connector's money by sending packets that get rejected.
- **A peering over Lightning is a second kind of channel.** ADR 0075 decision 1 fixes one channel
  type in the binary, and its falsifier fails the build on a second settlement port. A Lightning
  HTLC is also a lock on the peer path, which #1031 refused.
- **Sats are not a token a node declares.** A PREPARE's amount is in a token's base units, and a
  hop converts only between `[[tokens]]` rows on its settlement chains, at a rate it declares (ADR
  0071). Bitcoin on Lightning is neither.

None of this stops sats from buying a route. An app in front of a connector can take a Lightning
payment, hold it if its wallet allows, and pay the route from its own x402 channel. The app
carries the rate and the reject, and prices them. `l402-door` 0.2.0 has that hold mode, and falls
back to a plain invoice when the wallet refuses a hold (Rizful refuses any under 1000 sats).

`connector_domain::condition::fulfillment_matches_condition` stays exported for a caller of that
kind, one that holds a condition from elsewhere. It has no caller in this workspace.

## Prior requests

- #1480: "Research: a Lightning hold invoice keyed to the PREPARE's condition, so sats move only
  on FULFILL"
