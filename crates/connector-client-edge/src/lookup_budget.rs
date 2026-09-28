//! A bound on the chain lookups an anonymous sender can make this
//! connector perform for channels that do not resolve (issue #613).
//!
//! # The hole this closes
//!
//! Issue #611 is what makes ADR/issue #502's *"anonymity is a first-class
//! path"* real: a buyer who has opened a channel on chain pays without the
//! operator hand-editing a config file, because the claim gate resolves the
//! channel from the chain through its settlement backend
//! ([`crate::BatchSettlementChannels`] since ADR 0075; the retired
//! `toon-channel` registry before it). That resolution costs one chain read
//! per **previously-unseen** channel id, and before this module nothing
//! bounded how many a sender could cause.
//!
//! So a sender naming a fresh nonexistent channel id on every request made
//! this connector issue one `eth_call` (or one Solana account read) per
//! request, indefinitely. Every one of those claims was refused -- nothing
//! was paid and nothing was delivered -- and that is exactly what made it
//! attractive: the sender spends a packet, this connector spends a unit of
//! its own metered settlement-RPC budget, and the exchange is free in one
//! direction only.
//!
//! # Why this shapes rather than caches, and why it shapes rather than drops
//!
//! Two wrong answers, and the second is the subtler one.
//!
//! **Not a cache.** Memoising "no such channel" for N seconds -- which #611
//! declined deliberately -- makes the buyer this whole path exists for
//! poison their own next N seconds: they open a channel and write a second
//! later, so their first attempt is a miss by construction. The feature
//! would work for everybody except its intended user. Nothing here
//! memoises a negative answer. What is metered is the *asking*.
//!
//! **Not a drop, either.** The first cut of this module refused outright
//! once its window was spent, and that is the same defect wearing different
//! clothes. Refusing at a ceiling of *C* lookups per window hands any
//! sender able to sustain *C* requests per window a switch that turns #611
//! off for **every** new buyer, for as long as they care to hold it down --
//! no keypair, no valid signature (the resolution runs before
//! [`crate::ClientClaimGate`] ever checks one), no funds. Compare the two
//! failure modes honestly: with no bound at all, a flooder costs this node
//! one chain read per request **and the feature keeps working**; with a
//! dropping bound, the same flooder costs this node nothing and the feature
//! is entirely off. That is a worse trade, and it was available at a lower
//! attacker rate than the attack it was built to stop.
//!
//! So the node-wide bound **shapes**: it is a leaky bucket, and a lookup
//! that arrives with the bucket in arrears **waits for its slot** rather
//! than being refused. The RPC protection is unchanged, and that is the only
//! thing the bound was ever for -- but a legitimate buyer arriving during a
//! flood is *delayed*, not denied. Only a request whose slot is further out
//! than [`max_wait`](UnresolvableLookupBudgetPolicy::max_wait) is refused,
//! which is what keeps the waiting room bounded and keeps a packet's own
//! deadline reachable.
//!
//! What "the RPC protection is unchanged" means exactly, since it is the
//! number an operator sizes an endpoint against: the **sustained** rate is
//! [`total`](UnresolvableLookupBudgetPolicy::total) per
//! [`window`](UnresolvableLookupBudgetPolicy::window), and any *single*
//! window can see up to **`total` burst plus `total` drain**, i.e. roughly
//! twice the rate, when a flood arrives at an idle node. That is inherent
//! to tolerating a burst at all -- an idle node has to serve an arriving
//! crowd immediately or the shaper would be a throttle on ordinary traffic
//! -- and it is what `a_flood_still_costs_the_chain_only_the_drain_rate`
//! allows for. Size an endpoint against the sustained figure and expect one
//! window of twice it.
//!
//! # What #654 already bounds, and why it cannot bound this
//!
//! The retired `toon-channel` registry's liveness policy (deleted with it,
//! issue #1384) looked like it should already cover this, and it did not.
//! Every one of its protections -- `refresh_after`, `serve_stale_until`,
//! `min_reattempt_interval`, and the single-flight in-flight marker -- hung
//! off a *memo entry*, and the registry only ever inserted one for a
//! channel the chain vouched for; a channel that resolves to nothing is
//! `memo.remove`d, never inserted. The consequence is stronger than "a
//! fresh id each time escapes the interval": **even the same nonexistent
//! id, presented two hundred times in a row under a ten-minute
//! `min_reattempt_interval`, costs two hundred chain reads**, because there
//! is never an entry for the interval to be recorded on.
//! `the_same_nonexistent_channel_is_not_bounded_by_the_liveness_policy` in
//! `crate::channels` measures exactly that rather than asserting it.
//!
//! # What identity this budgets against, and what that is worth
//!
//! A probe budgets per recognized channel
//! (`connector_runtime`'s `ProbeRateLimiter`, whose per-identity shape this
//! follows). An unresolvable lookup has no recognized channel -- that is
//! what makes it unresolvable -- so the identity has to come from
//! somewhere else, and the honest summary is that every candidate is weak.
//! What was available at this seam, and why the choice landed where it did:
//!
//! * **The transport source address is not available here at all.** Nothing
//!   in this crate's axum handlers takes a `ConnectInfo<SocketAddr>` and
//!   nothing plumbs a peer address onto the packet path. Adding that would
//!   not help either: on the deployed boxes the client edge sits behind
//!   nginx, so the peer address is the proxy's and every anonymous buyer in
//!   the world would share one bucket -- with the attacker in it. The
//!   remedy for *that* would be trusting a forwarded-for header, which this
//!   repo trusts nowhere, and which would be strictly worse than what is
//!   used below: an attacker-settable free-text field that needs no keypair
//!   even to look plausible. Rate-limiting per *real* peer address at that
//!   nginx is a genuine defence -- the only sybil-resistant axis available
//!   at this layer -- it simply is not something this crate can reach. See
//!   `client-edge-spec.md` §1.3 for that and for the durable fix.
//! * **The voucher's signer is not known, and the sender is not verified.**
//!   A voucher names no signer: its channel's voucher signer is read from
//!   the chain by the very lookup being budgeted, so nothing before it can
//!   say whose key signed. Recovering *a* key from the signature proves
//!   nothing except that the sender can run one `ecrecover`, and would swap
//!   one amplifier for another: an anonymous request would buy an
//!   elliptic-curve operation instead of a hashmap increment. So
//!   [`connector_domain::client_claim::ClientClaim::signer_key`] is a label
//!   for grouping and attribution, never a credential.
//!
//! **The residual weakness, stated plainly.** A keypair is free, so an
//! adaptive attacker declares a fresh signer on every request and the
//! per-signer shaping never binds them. What the per-signer axis buys is
//! that an attacker must *become* adaptive: a flooder rotating a handful of
//! identities is shaped to `per_signer` lookups per window each, so holding
//! the node-wide bucket in arrears at all takes `total / per_signer`
//! distinct declared signers, sustained. That is a cost, not a bound, and a
//! cheap one -- but it turns the cheapest version of the attack into one
//! that is loud in a log (hundreds of distinct declared signers a minute is
//! a signature) and reachable by the per-address limiter that belongs at
//! the proxy.
//!
//! Against a genuinely sybil flood the node-wide shaper is what remains,
//! and what it guarantees is a floor rather than an exclusion: admissions
//! continue at the drain rate however hard the drain is pushed, so an
//! honest buyer's share of them is their share of arrivals, and a client
//! that retries gets through rather than being locked out for the duration.
//! The feature degrades in latency, not in availability.
//!
//! # Why the per-signer axis binds only under contention
//!
//! Because the signer is read rather than authenticated, anyone can declare
//! anyone else's address. A per-signer bound enforced unconditionally would
//! therefore be a targeted denial of service against a *known* buyer: spend
//! their allowance with a handful of nonsense channel ids, and their
//! genuine first write is refused.
//!
//! The per-signer axis is therefore consulted only once the node-wide
//! bucket is already in **arrears** -- i.e. its whole burst of `total` is
//! spent. That does **not remove** the aim, and an earlier draft of this
//! doc was wrong to claim it did. It *prices* it: an attacker must first
//! push the node-wide bucket into arrears, which costs `total` requests
//! inside one window before the first request aimed at anybody bites.
//! Usage accrues on the per-signer axis whether or not the node is
//! contended, so that price is paid up front rather than avoided. What is
//! bought is that an idle node never refuses anyone for their declared
//! identity, and that the attack on one named buyer costs the same
//! sustained flood as the attack on everybody -- at which point it is the
//! flood, and not the aim, that an operator is looking at.
//!
//! # What consumes it
//!
//! A slot is claimed **before** the chain is touched -- the point is to
//! prevent the read, not to notice it afterwards, and claiming it under the
//! same lock that decides it is what makes a burst of simultaneous requests
//! bind rather than all pass the check at once.
//!
//! A slot is **given back** when the lookup resolves the channel, so a busy
//! legitimate node onboarding real buyers never throttles itself: only
//! lookups that came back with nothing, or that failed, leave a mark. A
//! failed lookup consumes deliberately -- the RPC was spent either way, and
//! a node whose endpoint is down must not keep paying for the discovery.
//! A lookup the backend could not complete is reported as a lookup failure
//! (`ChannelLookupFailed`), distinguishably from a lookup this budget
//! withheld, so the two refusals stay separately countable.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many unresolvable lookups one declared signer may cause per window
/// once the node-wide bucket is in arrears.
///
/// Twenty. A legitimate buyer needs *one* resolution, and it is given back
/// when it succeeds, so the only thing a well-behaved sender ever spends
/// here is the handful of attempts between opening a channel and the chain
/// reporting it; twenty leaves room for that and for a client that retries
/// too eagerly. Read the other way -- which is the way that matters -- it
/// is [`DEFAULT_UNRESOLVABLE_LOOKUPS_TOTAL`] divided by thirty, so pushing
/// the node-wide drain into arrears takes thirty distinct declared signers
/// sustained rather than one.
pub const DEFAULT_UNRESOLVABLE_LOOKUPS_PER_SIGNER: u32 = 20;

/// How many unresolvable lookups this connector will perform per window in
/// total, whoever asks.
///
/// Six hundred a minute -- ten a second sustained -- which is the right
/// default for the endpoint this connector is actually deployed against,
/// and **too high for a metered plan**. Both halves of that matter, so here
/// is the arithmetic rather than an adjective.
///
/// Ten `eth_call`s a second is 260 compute units a second on Alchemy's
/// published schedule (26 CU for an `eth_call`). Sustained for a day that
/// is 864,000 lookups, i.e. **22.5M CU a day, or 674M CU a month** --
/// against a 300M/month free allowance, **225% of the plan**, on discovery
/// traffic alone and before the node's own settlement work. On throughput
/// it is 260 CU/s against a free-tier cap around 330 CU/s: 79% of it, again
/// leaving little for anything else. (An earlier version of this paragraph
/// quoted 864,000 as *compute units* rather than lookups and concluded
/// "single-digit percent of a free plan". It was wrong by a factor of 26,
/// in the direction that flatters the default, which is the direction a
/// number like this must never be wrong in.)
///
/// The default is nonetheless 600, because the endpoint that matters here
/// is a **self-hosted** one -- what the devnet boxes run, and what any
/// operator serious about a paid write path ends up running -- where ten
/// reads a second is free and the shaping is worth having. What follows for
/// everyone else is not a caveat but an instruction:
///
/// **On a metered plan, lower `unresolvable_lookup_budget_total`.** Staying
/// inside a 300M CU/month allowance on `eth_call` alone means at most
/// `300M / 30 / 24 / 60 / 26` ~= **267 a minute**, and that is the whole
/// allowance; a node that also settles from the same plan wants a fraction
/// of it. Lowering it costs nothing an honest buyer notices -- a resolution
/// that succeeds returns its slot, so the drain is only ever spent on
/// discovery that found nothing -- and it lowers the rate a flooder must
/// sustain to make anyone wait by the same factor.
///
/// The first draft of this constant was sixty a minute, which protected an
/// endpoint nobody was worried about while letting one request a second put
/// every new buyer in a queue. Ten times that raises the rate a sender must
/// sustain by the same factor; what it does not do, and did not do at
/// sixty, is make the number safe to ignore on a plan that bills.
pub const DEFAULT_UNRESOLVABLE_LOOKUPS_TOTAL: u32 = 600;

/// The window both rates are expressed over -- a minute, matching
/// `connector_runtime`'s probe rate limiter, since both are answering the
/// same question about the same kind of caller.
pub const DEFAULT_UNRESOLVABLE_LOOKUP_WINDOW: Duration = Duration::from_secs(60);

/// How long a lookup may wait for its slot before it is refused instead.
///
/// Two seconds: long enough that a legitimate buyer arriving mid-flood is
/// served on the attempt they made rather than told to come back, short
/// enough to sit inside a packet's own deadline and to bound the waiting
/// room (at the default drain rate it holds about twenty). An operator
/// whose clients run tighter timeouts should shorten it, at the cost of
/// refusing sooner under load. The one value it may not take is zero, which
/// turns the shaper back into the dropper this module's doc describes, and
/// which the config layer refuses.
pub const DEFAULT_UNRESOLVABLE_LOOKUP_MAX_WAIT: Duration = Duration::from_millis(2_000);

/// The longest window this shaper will honour, whatever it is handed.
///
/// A day. Nothing sensible is expressed over longer -- a rate limit whose
/// window outlives the process it runs in is not a rate limit -- and the
/// clamp is defensive rather than a policy: [`UnresolvableLookupBudgetPolicy`]
/// is public, so a caller that is not the config layer (which refuses
/// anything above this outright) can hand over a window whose arithmetic
/// overflows an `Instant`. The config layer's refusal is the real answer;
/// this is the guard for everybody who did not go through it.
pub const MAX_UNRESOLVABLE_LOOKUP_WINDOW: Duration = Duration::from_secs(86_400);

/// How many unresolvable channel lookups this connector will perform per
/// window, per declared signer and in total, and how long one may wait for
/// its turn (issue #613).
///
/// Two leaky buckets and a wait ceiling. A leaky bucket rather than the
/// fixed window `connector_runtime`'s `ProbeRateLimiter` uses, and the
/// divergence is the point rather than an accident: a probe that is refused
/// costs its own sender a probe, while a lookup that is refused costs a
/// *third party* -- the new buyer who has done nothing -- their entire
/// ability to pay this node. A bound whose overflow behaviour is "wait"
/// rather than "no" is the only shape that protects the endpoint without
/// handing a flooder an off switch for the feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnresolvableLookupBudgetPolicy {
    /// Per declared signer, consulted only while the node-wide bucket is in
    /// arrears -- see this module's own doc for what that condition does
    /// and does not buy.
    pub per_signer: u32,
    /// Node-wide, always. The drain rate the chain actually sees, and the
    /// only figure an adaptive sender cannot raise.
    pub total: u32,
    /// What both rates are expressed over. A bucket idle for this long
    /// admits exactly `total` (or `per_signer`) lookups back to back before
    /// anything waits, and then paces the rest at the rate.
    pub window: Duration,
    /// The longest a lookup will wait for its slot. Past this it is refused
    /// -- the only refusal this type produces.
    pub max_wait: Duration,
}

impl Default for UnresolvableLookupBudgetPolicy {
    fn default() -> UnresolvableLookupBudgetPolicy {
        UnresolvableLookupBudgetPolicy {
            per_signer: DEFAULT_UNRESOLVABLE_LOOKUPS_PER_SIGNER,
            total: DEFAULT_UNRESOLVABLE_LOOKUPS_TOTAL,
            window: DEFAULT_UNRESOLVABLE_LOOKUP_WINDOW,
            max_wait: DEFAULT_UNRESOLVABLE_LOOKUP_MAX_WAIT,
        }
    }
}

impl UnresolvableLookupBudgetPolicy {
    /// The window every derived figure is computed from, clamped into a
    /// range whose arithmetic cannot overflow an [`Instant`] -- see
    /// [`MAX_UNRESOLVABLE_LOOKUP_WINDOW`], and the same reasoning as the
    /// rate flooring below: this struct is public, and a public struct's
    /// fields are inputs.
    fn effective_window(&self) -> Duration {
        self.window
            .clamp(Duration::from_millis(1), MAX_UNRESOLVABLE_LOOKUP_WINDOW)
    }

    /// The gap between two node-wide admissions in the steady state.
    ///
    /// `total` is floored at one rather than trusted: this struct is
    /// public, so a caller that is not the config layer (which refuses a
    /// zero) can still construct one, and a rate of zero per window is a
    /// division by zero rather than a policy.
    fn node_interval(&self) -> Duration {
        self.effective_window() / self.total.max(1)
    }

    /// The per-signer twin of [`Self::node_interval`], floored for the same
    /// reason.
    fn signer_interval(&self) -> Duration {
        self.effective_window() / self.per_signer.max(1)
    }

    /// The arrears a bucket serves without making anybody wait -- one
    /// window's worth *less one interval*, which is exactly what makes an
    /// idle bucket admit `rate` lookups back to back and the next one wait.
    /// One window flat would admit `rate + 1`, which is harmless and
    /// confusing; a knob documented as "600 per minute" should admit 600.
    fn tolerance(&self, interval: Duration) -> Duration {
        self.effective_window().saturating_sub(interval)
    }
}

/// Which of [`UnresolvableLookupBudgetPolicy`]'s two axes refused a lookup.
/// Kept apart because they lead an operator to different actions:
/// [`LookupBudgetBound::Signer`] names a sender to look at, while
/// [`LookupBudgetBound::Node`] says the drain is saturated regardless of
/// who saturated it -- which, given a sender may declare any signer they
/// like, is what a sybil walk of the id space looks like from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupBudgetBound {
    /// This declared signer's own bucket is further behind than
    /// [`UnresolvableLookupBudgetPolicy::max_wait`], on a node whose
    /// node-wide bucket is already in arrears.
    Signer,
    /// The node-wide bucket is further behind than
    /// [`UnresolvableLookupBudgetPolicy::max_wait`].
    Node,
}

impl LookupBudgetBound {
    /// The structured-log field value for this bound, so a `bound=node`
    /// line and a `bound=signer` line can be counted apart without parsing
    /// English.
    pub fn as_str(&self) -> &'static str {
        match self {
            LookupBudgetBound::Signer => "signer",
            LookupBudgetBound::Node => "node",
        }
    }
}

/// This connector declined to look a channel up: the queue for lookups that
/// do not resolve is longer than it will hold one for (issue #613).
///
/// Deliberately **not** a `crate::ChannelLookupFailed` and deliberately not
/// an unknown channel. All three refuse the claim; all three mean something
/// different, and an operator reading a log has to be able to tell them
/// apart because they lead to three different actions -- fix your endpoint,
/// nothing (a sender named a channel that is not there), and look at who is
/// saturating your discovery drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookupBudgetExhausted {
    /// Which axis was saturated.
    pub bound: LookupBudgetBound,
    /// The rate that axis is shaped to, so a refusal can quote a number
    /// rather than a policy name the sender cannot see.
    pub allowance: u32,
    /// The window that rate is expressed over.
    pub window: Duration,
    /// How long this node was prepared to hold the lookup for before
    /// refusing it -- and therefore, roughly, how far behind the queue is.
    pub max_wait: Duration,
}

impl std::fmt::Display for LookupBudgetExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let axis = match self.bound {
            LookupBudgetBound::Signer => "this signer's share of",
            LookupBudgetBound::Node => "this node's",
        };
        write!(
            f,
            "{axis} discovery drain of {} lookups per {} s is saturated, and its queue is already \
             longer than the {} ms this node will hold a lookup for",
            self.allowance,
            self.window.as_secs(),
            self.max_wait.as_millis()
        )
    }
}

impl std::error::Error for LookupBudgetExhausted {}

/// One admitted unresolvable lookup, already counted against both axes.
///
/// Held by the caller across the chain read and handed back to
/// [`UnresolvableLookupBudget::refund`] if the channel resolved. There is
/// deliberately no `Drop` refund: dropping one is how a lookup that found
/// nothing, or failed, *keeps* its charge, and that is the common case
/// rather than an error path.
#[derive(Debug)]
#[must_use = "an admitted lookup is charged until it is refunded or dropped"]
pub(crate) struct LookupReservation {
    signer: String,
}

/// A slot claimed on both axes, and how long the caller must hold off
/// before using it.
#[derive(Debug)]
struct Admission {
    wait: Duration,
    reservation: LookupReservation,
}

/// One axis's leaky bucket, as a single instant: the earliest moment a
/// lookup could proceed if no burst were tolerated (the GCRA "theoretical
/// arrival time").
///
/// Everything the shaper needs is a comparison between this and `now`. A
/// bucket whose instant is at or before `now` is idle and behaves exactly
/// like one that has never been used, which is what makes an entry safe to
/// drop from the per-signer map.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    admit_at: Instant,
}

impl Bucket {
    fn new(now: Instant) -> Bucket {
        Bucket { admit_at: now }
    }

    /// Whether this bucket carries no history at all at `now`, and so
    /// behaves exactly like one that has never been used. What makes an
    /// entry safe to drop from the per-signer map.
    fn idle(&self, now: Instant) -> bool {
        self.admit_at <= now
    }

    /// How long a lookup arriving at `now` must wait for its slot, given
    /// that `tolerance` of arrears is served immediately (the burst).
    fn wait(&self, now: Instant, tolerance: Duration) -> Duration {
        self.admit_at
            .saturating_duration_since(now)
            .saturating_sub(tolerance)
    }

    /// Claim a slot, advancing the bucket by one emission interval.
    ///
    /// `checked_add` rather than `+`: `Instant + Duration` panics on
    /// overflow, and a bucket is advanced once per admitted lookup for the
    /// life of the process. Saturating leaves the bucket where it already
    /// was, which reads as "no further arrears" -- the only safe direction,
    /// since the alternative is a panic on the packet path.
    fn take(&mut self, now: Instant, interval: Duration) {
        let from = self.admit_at.max(now);
        self.admit_at = from.checked_add(interval).unwrap_or(from);
    }

    /// Give a slot back, never rewinding further than `tolerance` before
    /// `now` -- the arrears a bucket may hold anyway, so a refund can
    /// restore a burst but never manufacture one larger than the policy.
    fn give_back(&mut self, now: Instant, interval: Duration, tolerance: Duration) {
        let floor = now.checked_sub(tolerance).unwrap_or(now);
        let rewound = self.admit_at.checked_sub(interval).unwrap_or(floor);
        self.admit_at = rewound.max(floor);
    }
}

#[derive(Debug)]
struct BudgetState {
    node: Bucket,
    per_signer: HashMap<String, Bucket>,
}

/// The shaper on lookups for channels this connector has never resolved
/// (issue #613).
///
/// See this module's own doc for what is being defended, why it shapes
/// rather than drops, what the identity is worth, and why the per-signer
/// axis is consulted only under contention.
#[derive(Debug)]
pub struct UnresolvableLookupBudget {
    policy: UnresolvableLookupBudgetPolicy,
    /// A plain [`Mutex`] rather than an `RwLock`, exactly as
    /// `connector_runtime`'s `ProbeRateLimiter` reasons: every access here
    /// mutates, so there is no read-only path for a reader/writer lock to
    /// be better at. Never held across an await -- [`Self::admit`] decides
    /// under it and [`Self::reserve`] sleeps outside it.
    state: Mutex<BudgetState>,
}

impl UnresolvableLookupBudget {
    pub fn new(policy: UnresolvableLookupBudgetPolicy) -> UnresolvableLookupBudget {
        let now = Instant::now();
        UnresolvableLookupBudget {
            policy,
            state: Mutex::new(BudgetState {
                node: Bucket::new(now),
                per_signer: HashMap::new(),
            }),
        }
    }

    pub fn policy(&self) -> UnresolvableLookupBudgetPolicy {
        self.policy
    }

    /// Admit one lookup for a channel this connector has never resolved,
    /// waiting for its slot if the drain is in arrears, and refusing only
    /// if that wait would exceed
    /// [`UnresolvableLookupBudgetPolicy::max_wait`].
    ///
    /// The waiting happens here rather than being handed back to the caller
    /// so that "a lookup costs a slot" stays one statement at the call
    /// site. The lock is released before the sleep.
    pub(crate) async fn reserve(
        &self,
        signer: &str,
    ) -> Result<LookupReservation, LookupBudgetExhausted> {
        let admission = self.admit(signer, Instant::now())?;
        if !admission.wait.is_zero() {
            tokio::time::sleep(admission.wait).await;
        }
        Ok(admission.reservation)
    }

    /// The decision half of [`Self::reserve`], with the clock passed in:
    /// everything that reads or writes the shaper's state, and nothing that
    /// waits. Split out so a test can drive a window's worth of arrears
    /// without sleeping through one, which is what keeps every measurement
    /// below schedule-independent.
    fn admit(&self, signer: &str, now: Instant) -> Result<Admission, LookupBudgetExhausted> {
        let node_interval = self.policy.node_interval();
        let signer_interval = self.policy.signer_interval();
        // The burst either bucket tolerates, so a node that has been idle
        // serves an arriving crowd immediately and shapes only once it is
        // genuinely saturated.
        let node_tolerance = self.policy.tolerance(node_interval);
        let signer_tolerance = self.policy.tolerance(signer_interval);

        let mut state = self.state.lock().expect("lookup budget lock poisoned");

        let node_wait = state.node.wait(now, node_tolerance);
        if node_wait > self.policy.max_wait {
            return Err(self.refusal(LookupBudgetBound::Node, self.policy.total));
        }
        // "In arrears" means the node's whole burst is gone and an arriving
        // lookup would actually have to wait -- not merely that some
        // request has been served, which on a leaky bucket is true a
        // microsecond after the node starts up. Read before the bucket is
        // advanced, so a request is never contended by its own arrival.
        let contended = node_wait > Duration::ZERO;

        // Pruned here rather than on a timer: the key space is text a
        // sender chooses, so a sender declaring a fresh signer per request
        // would otherwise grow this map without bound -- trading the
        // RPC-spend hole for a memory one. An idle bucket is
        // indistinguishable from an absent one, so dropping it changes no
        // decision, and only an admitted lookup inserts, which is itself
        // shaped -- so the map stays proportional to the allowance rather
        // than to how long the process has been running.
        if state.per_signer.len() > self.policy.total as usize {
            state.per_signer.retain(|_, bucket| !bucket.idle(now));
        }

        let bucket = state
            .per_signer
            .entry(signer.to_string())
            .or_insert_with(|| Bucket::new(now));
        let signer_wait = bucket.wait(now, signer_tolerance);
        // Usage accrues on this axis whether or not the node is contended,
        // so that a flooder cannot bank a full per-signer burst per
        // identity while the node is quiet and spend it all the moment it
        // is not. Only the *refusal* waits for contention.
        if contended && signer_wait > self.policy.max_wait {
            return Err(self.refusal(LookupBudgetBound::Signer, self.policy.per_signer));
        }
        bucket.take(now, signer_interval);
        state.node.take(now, node_interval);

        let wait = if contended {
            node_wait.max(signer_wait)
        } else {
            node_wait
        };
        Ok(Admission {
            wait,
            reservation: LookupReservation {
                signer: signer.to_string(),
            },
        })
    }

    fn refusal(&self, bound: LookupBudgetBound, allowance: u32) -> LookupBudgetExhausted {
        LookupBudgetExhausted {
            bound,
            allowance,
            window: self.policy.window,
            max_wait: self.policy.max_wait,
        }
    }

    /// Give back a slot whose lookup resolved the channel after all.
    ///
    /// This is what keeps the shaper off a legitimate node's back: an
    /// operator whose connector is onboarding real anonymous buyers as fast
    /// as they arrive spends nothing here, because every one of those
    /// lookups came back with a channel. Only lookups that found nothing --
    /// or that could not be completed -- leave a mark.
    ///
    /// Unlike the fixed window this replaced, a refund cannot be lost to a
    /// boundary, because there is no boundary: a lookup that took longer
    /// than a whole window to come back still rewinds its own bucket by
    /// exactly the interval it advanced it, clamped so that it can restore
    /// a burst and never invent one.
    pub(crate) fn refund(&self, reservation: LookupReservation) {
        let now = Instant::now();
        let node_interval = self.policy.node_interval();
        let signer_interval = self.policy.signer_interval();
        let mut state = self.state.lock().expect("lookup budget lock poisoned");
        state
            .node
            .give_back(now, node_interval, self.policy.tolerance(node_interval));
        if let Some(bucket) = state.per_signer.get_mut(&reservation.signer) {
            bucket.give_back(now, signer_interval, self.policy.tolerance(signer_interval));
        }
    }

    /// How far behind the node-wide drain currently is, past the burst it
    /// tolerates -- i.e. what a lookup arriving now would wait. For a
    /// test, never for a decision.
    #[cfg(test)]
    pub(crate) fn queued_for(&self) -> Duration {
        let now = Instant::now();
        let tolerance = self.policy.tolerance(self.policy.node_interval());
        self.state
            .lock()
            .expect("lookup budget lock poisoned")
            .node
            .wait(now, tolerance)
    }

    /// How many declared signers this shaper is currently tracking -- for
    /// the test that its map cannot be grown without bound.
    #[cfg(test)]
    fn tracked_signers(&self) -> usize {
        self.state
            .lock()
            .expect("lookup budget lock poisoned")
            .per_signer
            .len()
    }
}

impl Default for UnresolvableLookupBudget {
    fn default() -> UnresolvableLookupBudget {
        UnresolvableLookupBudget::new(UnresolvableLookupBudgetPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(per_signer: u32, total: u32) -> UnresolvableLookupBudgetPolicy {
        UnresolvableLookupBudgetPolicy {
            per_signer,
            total,
            window: Duration::from_secs(60),
            max_wait: Duration::from_millis(2_000),
        }
    }

    /// Drive `count` admissions at `now`, round-robin over `signers`,
    /// dropping every reservation -- i.e. every lookup resolved nothing,
    /// which is the attack's shape. Reports (admitted, refused).
    fn flood(
        budget: &UnresolvableLookupBudget,
        signers: &[&str],
        count: usize,
        now: Instant,
    ) -> (usize, usize) {
        let mut admitted = 0;
        let mut refused = 0;
        for request in 0..count {
            match budget.admit(signers[request % signers.len()], now) {
                Ok(_) => admitted += 1,
                Err(_) => refused += 1,
            }
        }
        (admitted, refused)
    }

    /// An idle node serves exactly a window's worth back to back without
    /// making anybody wait -- the shaper is invisible until the drain is
    /// genuinely saturated, and "600 per minute" admits 600.
    #[test]
    fn an_idle_node_admits_exactly_the_rate_without_waiting() {
        let budget = UnresolvableLookupBudget::new(policy(1_000, 60));
        let now = Instant::now();

        for request in 0..60 {
            let admission = budget
                .admit("evm:0xaaaa", now)
                .unwrap_or_else(|_| panic!("burst request {request} is admitted"));
            assert_eq!(admission.wait, Duration::ZERO, "request {request} waited");
        }
        assert!(
            !budget.admit("evm:0xaaaa", now).unwrap().wait.is_zero(),
            "and the very next one waits"
        );
    }

    /// Past the burst the drain shapes rather than refuses: a lookup waits
    /// for its slot, and the wait grows one emission interval at a time.
    #[test]
    fn past_the_burst_a_lookup_waits_rather_than_being_refused() {
        // Sixty per sixty seconds is one a second, so each arrival past the
        // burst waits a second longer than the last -- big enough to assert
        // exactly rather than approximately.
        let budget = UnresolvableLookupBudget::new(UnresolvableLookupBudgetPolicy {
            per_signer: 1_000,
            total: 60,
            window: Duration::from_secs(60),
            max_wait: Duration::from_secs(5),
        });
        let now = Instant::now();
        for _ in 0..60 {
            let _ = budget.admit("evm:0xaaaa", now).expect("the burst");
        }

        for expected in 1..=5u64 {
            let admission = budget
                .admit("evm:0xaaaa", now)
                .expect("still inside the wait ceiling");
            assert_eq!(
                admission.wait,
                Duration::from_secs(expected),
                "each further arrival waits one more interval"
            );
        }

        // ...and only past the ceiling is anything refused at all.
        let refused = budget
            .admit("evm:0xaaaa", now)
            .expect_err("past the wait ceiling");
        assert_eq!(refused.bound, LookupBudgetBound::Node);
    }

    /// **The property the first cut of this module got wrong**, and the one
    /// it was reshaped for: a flood cannot switch the feature off. However
    /// hard the drain is pushed, it keeps draining -- so an honest buyer
    /// arriving one interval later is admitted, rather than locked out for
    /// as long as the flood lasts.
    #[test]
    fn a_flood_reopens_every_interval_rather_than_closing() {
        let budget = UnresolvableLookupBudget::new(policy(1_000, 600));
        let start = Instant::now();

        // Ten thousand requests at one instant: far past the burst, far
        // past the wait ceiling, as hard as a flood can push.
        let (admitted, refused) = flood(&budget, &["evm:0xbad"], 10_000, start);
        assert!(admitted > 0 && refused > 0, "{admitted} / {refused}");

        // A tenth of a second later -- one emission interval -- the honest
        // buyer arrives and is admitted. Under a bound that refuses at a
        // ceiling this buyer is denied for the whole flood; under one that
        // shapes, the queue has moved on by exactly one slot and they are
        // in it.
        let admission = budget
            .admit("evm:0xhonest", start + Duration::from_millis(100))
            .expect("an honest buyer is queued, not denied");
        assert!(
            admission.wait <= Duration::from_millis(2_000),
            "and their wait is bounded: {:?}",
            admission.wait
        );
    }

    /// The chain still sees only the drain rate, which is the whole point
    /// of the bound. Six hundred thousand arrivals over one window admit
    /// about one window's worth of lookups and no more.
    #[test]
    fn a_flood_still_costs_the_chain_only_the_drain_rate() {
        let budget = UnresolvableLookupBudget::new(policy(1_000, 600));
        let start = Instant::now();

        let mut admitted = 0;
        for millisecond in 0..60_000u64 {
            let now = start + Duration::from_millis(millisecond);
            for _ in 0..10 {
                if budget.admit("evm:0xbad", now).is_ok() {
                    admitted += 1;
                }
            }
        }

        // One burst, plus one window of drain, plus whatever the wait
        // ceiling lets sit ahead of the drain at any one moment.
        assert!(
            admitted <= 600 * 2 + 32,
            "600k arrivals in one window admitted {admitted} lookups"
        );
        assert!(
            admitted >= 600,
            "and the drain did keep running: {admitted}"
        );
    }

    /// The per-signer axis is what makes a flooder rotating a handful of
    /// identities pay for it: each identity is shaped to its own rate, so a
    /// few of them cannot hold the node-wide drain in arrears. This is the
    /// measured attack that sank the first cut -- eight rotating declared
    /// signers, flooding for a whole window.
    #[test]
    fn a_few_rotating_signers_cannot_hold_the_drain_in_arrears() {
        let budget = UnresolvableLookupBudget::new(policy(20, 600));
        let start = Instant::now();
        let signers = [
            "evm:0x01", "evm:0x02", "evm:0x03", "evm:0x04", "evm:0x05", "evm:0x06", "evm:0x07",
            "evm:0x08",
        ];

        for millisecond in 0..60_000u64 {
            let now = start + Duration::from_millis(millisecond);
            flood(&budget, &signers, 10, now);
        }

        // Still inside the flood, an honest buyer arrives and is served at
        // once: the eight flooders ran out of their own rates long ago, so
        // the node-wide drain never stayed saturated.
        let now = start + Duration::from_millis(59_999);
        let admission = budget
            .admit("evm:0xhonest", now)
            .expect("an honest buyer is admitted");
        assert!(
            admission.wait < Duration::from_millis(10),
            "and served promptly rather than queued behind the flood: {:?}",
            admission.wait
        );
    }

    /// Two senders do not share a rate: on a drain that is genuinely in
    /// arrears, the one that has spent its own share is refused while the
    /// one that has spent nothing is admitted.
    #[test]
    fn two_signers_have_independent_allowances() {
        // Ten a second node-wide, one a second each: so a single signer
        // cannot take more than a tenth of the drain once it is contended.
        let budget = UnresolvableLookupBudget::new(UnresolvableLookupBudgetPolicy {
            per_signer: 60,
            total: 600,
            window: Duration::from_secs(60),
            max_wait: Duration::from_millis(2_000),
        });
        let start = Instant::now();
        let loud = "evm:0xloud";
        let quiet = "evm:0xquiet";

        // The loud sender takes the whole node-wide burst on its own, which
        // takes it far past its own share of it.
        let (admitted, refused) = flood(&budget, &[loud], 600, start);
        assert_eq!((admitted, refused), (600, 0), "the burst is not refused");

        // Now the drain is in arrears, so shares start to matter.
        let aimed = budget
            .admit(loud, start)
            .expect_err("the loud sender is far past its own share");
        assert_eq!(aimed.bound, LookupBudgetBound::Signer);

        // ...and a sender that has spent nothing is admitted, waiting only
        // for the node-wide queue it did not create.
        let admission = budget
            .admit(quiet, start)
            .expect("a second sender has its own share and has spent none of it");
        assert!(admission.wait <= Duration::from_millis(2_000));
    }

    /// A resolution that succeeded gives its slot back, so a node
    /// onboarding real buyers never throttles itself however many it
    /// onboards. A drain of one per window, spent and refunded fifty times.
    #[test]
    fn a_refunded_slot_costs_nothing() {
        let budget = UnresolvableLookupBudget::new(policy(1, 1));
        let now = Instant::now();

        for attempt in 0..50 {
            let admission = budget
                .admit("evm:0x4444", now)
                .unwrap_or_else(|_| panic!("charge {attempt} is admitted"));
            budget.refund(admission.reservation);
        }
        assert_eq!(budget.queued_for(), Duration::ZERO);
    }

    /// A lookup that took longer than a whole window to come back still
    /// gets its slot returned. Under the fixed window this replaced, a
    /// refund whose window had rolled was silently dropped, so a slow but
    /// perfectly healthy endpoint drained the budget on its *successes*.
    #[test]
    fn a_refund_survives_a_lookup_that_outlived_a_window() {
        let budget = UnresolvableLookupBudget::new(policy(10, 10));
        let signer = "evm:0x5555";
        let start = Instant::now();

        // Spend all but one of the burst, then take the last slot -- the
        // one whose lookup is going to be slow.
        for _ in 0..9 {
            let _ = budget.admit(signer, start).expect("the burst");
        }
        let slow = budget
            .admit(signer, start)
            .expect("the last of the burst")
            .reservation;
        // The bucket is now exactly full: one more is past the ceiling.
        assert!(budget.admit(signer, start).is_err(), "the burst is spent");

        // The slow lookup comes back a very long time later and succeeds.
        // Its slot must come back with it.
        budget.refund(slow);
        assert_eq!(
            budget.queued_for(),
            Duration::ZERO,
            "a slow success gives its slot back rather than losing it to a boundary"
        );
    }

    /// An idle node never refuses anyone for their declared identity, so
    /// there is nothing for a sender who wants to spend a *named* buyer's
    /// share to aim with until they have first pushed the node-wide drain
    /// into arrears. That prices the aim; it does not remove it.
    #[test]
    fn a_signers_axis_does_not_bind_while_the_node_is_uncontended() {
        // A per-signer rate of one against a node-wide rate of a thousand:
        // the victim is forty times past their own share and is admitted
        // anyway, because nothing is scarce.
        let budget = UnresolvableLookupBudget::new(policy(1, 1_000));
        let victim = "evm:0x3333";
        let now = Instant::now();

        for attempt in 0..40 {
            assert!(
                budget.admit(victim, now).is_ok(),
                "charge {attempt} is admitted: a per-signer rate of one does not bind an \
                 uncontended node"
            );
        }
    }

    /// ...and the honest price of that, measured rather than asserted: an
    /// aimed attack costs a whole node-wide burst first, and only then does
    /// the victim's own share bite. This test exists so that nobody reads
    /// the paragraph above as "the aim is gone" -- it is not, it costs
    /// `total` requests inside a window.
    #[test]
    fn aiming_at_a_named_buyer_costs_a_whole_node_wide_burst_first() {
        let budget = UnresolvableLookupBudget::new(policy(4, 64));
        let victim = "evm:0xvictim";
        let now = Instant::now();

        // Eight requests under the victim's name -- twice their share, and
        // refused nothing, because the node is not contended.
        for request in 0..8 {
            assert!(
                budget.admit(victim, now).is_ok(),
                "request {request} is below contention"
            );
        }
        // The rest of the node-wide burst, under a throwaway identity.
        let (admitted, refused) = flood(&budget, &["evm:0xthrowaway"], 56, now);
        assert_eq!((admitted, refused), (56, 0), "the burst is not refused");

        // Only now, with the node in arrears, does the victim pay for the
        // eight requests somebody else made under their name.
        let aimed = budget
            .admit(victim, now)
            .expect_err("the victim's own share is spent");
        assert_eq!(aimed.bound, LookupBudgetBound::Signer);
    }

    /// The identity space is text a sender picks, so the map keyed by it
    /// must not become a second, quieter way to spend this node's
    /// resources.
    #[test]
    fn a_sybil_walk_does_not_grow_the_map_without_bound() {
        let budget = UnresolvableLookupBudget::new(policy(20, 60));
        let start = Instant::now();

        for request in 0..200_000u64 {
            // Spread over many windows, so earlier entries go idle.
            let now = start + Duration::from_millis(request * 5);
            let _ = budget.admit(&format!("evm:0x{request:040x}"), now);
        }

        let tracked = budget.tracked_signers();
        assert!(
            tracked <= 4 * 60,
            "200k distinct signers left {tracked} entries behind"
        );
    }

    /// The two refusals say which they are, in a message an operator reads
    /// rather than a code they have to look up.
    #[test]
    fn each_bound_says_which_one_it_is() {
        let budget = UnresolvableLookupBudget::new(policy(20, 600));
        let node = budget.refusal(LookupBudgetBound::Node, 600);
        let signer = budget.refusal(LookupBudgetBound::Signer, 20);

        assert!(node.to_string().contains("this node's discovery drain"));
        assert!(signer.to_string().contains("this signer's share"));
        assert_ne!(node.to_string(), signer.to_string());
        assert_eq!(LookupBudgetBound::Node.as_str(), "node");
        assert_eq!(LookupBudgetBound::Signer.as_str(), "signer");
    }

    /// The hold itself, end to end through [`UnresolvableLookupBudget::reserve`]
    /// rather than through `admit` -- so the one genuinely new runtime
    /// behaviour this module introduces is exercised rather than worked
    /// around.
    ///
    /// Two things are proven here and nowhere else. That a lookup past the
    /// burst really does *wait* its slot out and then proceed, rather than
    /// the wait being a number nobody acts on; and that this crate needs a
    /// **time-enabled tokio runtime**, since `reserve` sleeps -- a fact that
    /// would otherwise surface as a panic in whatever process first mounted
    /// this crate's router on a runtime built without `enable_time`.
    ///
    /// `start_paused` rather than a real second of wall time: tokio
    /// auto-advances its own clock when the runtime is idle, so the sleep is
    /// real, its length is asserted exactly, and the test takes microseconds.
    #[tokio::test(start_paused = true)]
    async fn a_lookup_past_the_burst_is_held_for_its_slot_and_then_proceeds() {
        // Sixty per sixty seconds is one a second, so the first arrival past
        // the burst waits exactly one second.
        let budget = UnresolvableLookupBudget::new(policy(1_000, 60));
        for request in 0..60 {
            let _ = budget
                .reserve("evm:0xaaaa")
                .await
                .unwrap_or_else(|_| panic!("burst request {request}"));
        }

        let started = tokio::time::Instant::now();
        let _held = budget
            .reserve("evm:0xaaaa")
            .await
            .expect("held for a slot, not refused");
        let waited = tokio::time::Instant::now().duration_since(started);

        assert!(
            waited >= Duration::from_millis(900),
            "the lookup was supposed to be held for about a second: {waited:?}"
        );
        assert!(
            waited <= Duration::from_millis(1_100),
            "...and for about a second, not for the whole window: {waited:?}"
        );
    }

    /// ...and a hold that would exceed the ceiling is refused rather than
    /// served late, which is what keeps the waiting room bounded. The pair
    /// of tests is the whole contract of the wait ceiling.
    #[tokio::test(start_paused = true)]
    async fn a_lookup_past_the_wait_ceiling_is_refused_rather_than_held() {
        let budget = UnresolvableLookupBudget::new(UnresolvableLookupBudgetPolicy {
            per_signer: 1_000,
            total: 60,
            window: Duration::from_secs(60),
            // One interval's grace, so the second arrival past the burst is
            // already too far out.
            max_wait: Duration::from_millis(1_100),
        });
        for _ in 0..60 {
            let _ = budget.reserve("evm:0xaaaa").await.expect("the burst");
        }
        let _held = budget.reserve("evm:0xaaaa").await.expect("one second out");

        let refused = budget
            .reserve("evm:0xaaaa")
            .await
            .expect_err("two seconds out is past the ceiling");
        assert_eq!(refused.bound, LookupBudgetBound::Node);
    }

    /// A policy built by hand rather than by the config layer -- which
    /// refuses a zero -- must shape rather than divide by zero.
    #[test]
    fn a_zero_rate_is_floored_rather_than_dividing_by_zero() {
        let budget = UnresolvableLookupBudget::new(UnresolvableLookupBudgetPolicy {
            per_signer: 0,
            total: 0,
            window: Duration::from_secs(60),
            max_wait: Duration::from_millis(2_000),
        });
        let now = Instant::now();

        // A floored rate of one per window: one slot, then the next
        // arrival's is a whole window out and is refused rather than
        // panicking -- and refused on the node axis, so no per-signer entry
        // is banked for a caller that was never charged.
        assert!(budget.admit("evm:0xaaaa", now).is_ok(), "the one slot");
        let refused = budget
            .admit("evm:0xaaaa", now)
            .expect_err("a whole window out is past any wait ceiling");
        assert_eq!(refused.bound, LookupBudgetBound::Node);
        assert_eq!(budget.tracked_signers(), 1, "one entry, from the admission");
    }
}
