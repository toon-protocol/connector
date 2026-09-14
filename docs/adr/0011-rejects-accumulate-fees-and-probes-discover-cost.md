# Rejects accumulate fees; a probe is how cost is discovered

**Status:** Accepted, amended by [0042](0042-a-packet-carries-its-claim.md) (fee honesty is bounded, not self-enforcing), extended by [0044](0044-a-probe-answers-what-a-route-costs-and-what-it-does.md) (a probe also answers what a route _does_) and by [0065-price](0065-a-price-is-a-schedule-over-payload-length.md) (a price may vary with payload length, so cacheability moves to the published schedule). Fee accumulation, the probe, and the sum-never-breakdown rule are unchanged.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

Every reject carries a running total of the fees of the hops it has passed through: each hop
adds its own fee before passing the reject upstream. Cost discovery is then a packet you expect
to be rejected — a probe — and the reject that comes back states what the path costs. Probes
are not a distinct packet type, and fee accumulation is not a special mode.

## Why not a quoting protocol

Interledger removed ILQP because a quote is computed over a path chosen at quote time, which
need not be the path a real packet later takes; the answer is precise about a route you did not
use. A probe has no such gap. It is an ordinary packet, routed by the ordinary routing table,
accumulating the fees of the hops that actually carried it.

No single participant can answer the question any other way. Fees are per peering relation —
bilateral, local and private — and no hop can see past its own next hop. End-to-end cost
therefore requires either a global view of the graph or a traversal of it, and traversal is the
one that cannot go stale.

## Properties this inherits from earlier decisions

**The answer is cacheable.** Because ADR 0010 makes fees flat per packet rather than
proportional, a path's cost is a constant that does not vary with the amount being sent. One
probe yields a figure good until the topology or a fee changes. Under a percentage spread the
client would have to re-probe per amount.

> **Extended by [ADR 0065](0065-a-price-is-a-schedule-over-payload-length.md) (issue #984).**
> A terminating route's **price** may now vary with the packet's payload length, so a probe's
> figure is exact for a packet its own size and not for every size. Cacheability is kept, and
> moved: the terminating node **publishes its schedule** -- `extra.price` and
> `extra.pricePerKib` on the greeting, the same pair per prefix on the self-description -- so
> one free, unauthenticated read answers every size, and a sender computes
> `cost(len) = probe_cost − charge(probe_len) + charge(len)` without a second round trip.
> A **fee** is untouched and still flat, so everything this clause says about the carrying
> hops holds exactly as written.

**Understating a fee is unprofitable.** Because ADR 0004 moves value on fulfilment, a hop that
advertises a low fee to attract traffic and then rejects the real packet earns nothing and has
spent its own bandwidth. Honesty needs no enforcement.

**Returning a sum leaks nothing.** The total is what a caller must know in order to use the
path. The per-hop breakdown is not, and is never returned, so topology and individual pricing
stay private.

## Consequences

Making accumulation a property of all rejects rather than of probes specifically means a client
rejected for any reason — expiry, no route, a ceiling — also learns what that path would have
cost. That is strictly more information for strictly less protocol.

A probe traverses the network and pays nothing, so it is accepted only from a sender that
already holds an open payment channel with this connector, and is rate-limited per that
identity. A sender without a channel is rejected at ingress without being forwarded. This costs
legitimate users nothing, since a channel is required to send real traffic regardless, while an
abuser must fund a channel per identity to sustain it.

This closes the price-discovery gap opened by removing `announcePrice` with discovery (ADR 0006) and the x402 greeting. Neither is reinstated.

## Update (ADR 0042)

**"Honesty needs no enforcement" no longer follows, and is replaced rather than dropped.** That
property was inherited from ADR 0004: under postpay, a hop advertising a low fee to attract traffic
and then rejecting the real packet earned nothing.
[ADR 0042](0042-a-packet-carries-its-claim.md) retires that headline — a packet now carries its
claim — so such a hop banks the covering claim and refuses to carry. The hop is at least always one
the operator chose: [ADR 0043](0043-purchasable-peering-is-removed.md) removed the purchasable
peering that would otherwise have let a stranger advertise the bait itself.

**Fee honesty is now bounded rather than self-enforcing.** Two mechanisms ADR 0042 already requires
do the work: the sender's own packet sizing, and the per-peer cap on what this connector will
forward in one packet. Both bound a single dishonest hop to one packet's worth, which is what
postpay used to give for free. The property survives; it stopped being free.

**Everything else in this record is unchanged.** Fees still accumulate on every reject, a probe is
still an ordinary packet rather than a mode, the returned sum still leaks no per-hop breakdown, and
the answer is still cacheable because ADR 0010's fees are flat. Probe economics in particular need
no revision: a probe carries a small amount by construction, so what a forwarding connector now
covers on one is a micropayment, against an abuser who must still fund a channel per identity and is
still rate-limited per that identity.
