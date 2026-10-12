# Configuration is one typed file with no environment-variable layer

**Status:** Accepted. Extended by [0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md), which adds a runtime table that can never shadow the file. Amended by [0079](0079-a-controllers-key-is-revoked-by-an-edit-and-may-sign-a-read.md) for one thing: the **contents** of `[operator] controller_keys_file` are read again on every signed request, so a controller's key is revoked without a restart. The setting naming that file, and everything else, is still read once.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

The connector reads a single configuration file, deserializes and validates it once at boot,
and holds the result as an immutable value for the process lifetime. There is no
environment-variable override layer. Secrets are referenced by location — a file path or a key
management identifier — and never written inline.

## Why

Two configuration surfaces means a precedence model, and a precedence model means a class of
bug where the deployed value is not the value anyone read. The existing connector carries 2,600
lines of configuration types, a 1,227-line loader, a 567-line environment validator and 56
distinct environment variables — and the devnet boxes are configured with hand-tuned
bind-mounted YAML that uses none of them. The environment layer is a maintained surface with no
users.

Validating once into an immutable value also removes a whole category of question at every
call site. Nothing downstream asks whether a field is present, in range, or mutually consistent
with another field, because a configuration that reaches the runtime has already answered that.
Anything genuinely operational — routes, channels, peers — changes through the operator surface
instead, where the change is authenticated and audited.

## Consequences

Desugaring happens at load. The existing `child-expander` already establishes the pattern:
a `children:` block is expanded into ordinary routes before the routing table exists, "so the
packet path stays topology-blind". Every convenience in the file is resolved the same way, and
the runtime sees only primitives. Complexity is absorbed at the boundary rather than carried
into the core.

Every deployment needs a mounted file, which is already how these nodes are deployed. Purely
environmental configuration is no longer possible, and that is intentional.

Reload is a restart. Since the configuration value is immutable, changing the file means
restarting the process — acceptable because the things that need to change while running have
been moved to the operator surface by design.

## Update (issue #1057) — `apex` and `[[children]]` are removed; the desugaring principle is not

The "Consequences" section above cites the child-expander as this record's worked example of
load-time desugaring:

> Desugaring happens at load. The existing `child-expander` already establishes the pattern: a
> `children:` block is expanded into ordinary routes before the routing table exists, "so the packet
> path stays topology-blind."

**That example is removed. The pattern it illustrates is not.**

### What they were

`apex` was an ILP address prefix. Each `[[children]]` entry — `{name, handler_url, price?,
transport?}` — desugared at load into an ordinary route at `<apex>.<name>`, always an app route and
never a peer route. `apex` was read only when `children` was non-empty, so on its own it was inert.

### Why they go

**Nothing uses them.** No committed configuration in this repository sets either key — not
`deploy/connector-rust/connector.toml`, not the local stack, not either Linode box. Every one writes
explicit `[[routes]]`. The form was ported from the TypeScript `child-expander` whose source is no
longer in this repository, and no operator adopted it.

**Keeping it would have made it a compatibility question.** This repo's configuration is being written
down as a specification for the first time. A convenience form that reaches that document forces a
second implementer to decide whether to implement it — converting thirty lines of unused sugar into an
interoperability obligation. Deleting it is cheaper than specifying it, and re-adding sugar later is
trivial and would arrive with a record saying why, which is strictly better than existing because
TypeScript had it.

**`apex` was also a name collision waiting to be documented.** In this repository "apex" otherwise
names the retired devnet apex box (`g.toon.apex`, retired by issue #872). A glossary entry for the
config key would have arrived pre-confused.

### What survives

**The pattern.** Desugaring at load, so the runtime sees only primitives and the packet path stays
topology-blind, remains this record's rule. It loses its example, not its force — and it has work
waiting for it: the node self-description (issue #1060) and the re-homing of `[announce]`'s greeting
fields (ADR 0046, issue #1074) are both shaped by it.

**Both keys are parsed to be rejected by name**, per this record's own standing rule that a removed
config key is never silently dropped — the `peer_wire_addr` / `ceiling` / `[peer_sale]` precedent. An
operator whose committed TOML still sets `apex` or `[[children]]` stops at boot, by name, rather than
loading with the key ignored.

## Update (issue #1485) — the contents of `controller_keys_file` are read again

[0079](0079-a-controllers-key-is-revoked-by-an-edit-and-may-sign-a-read.md) makes one exception to "immutable for the process lifetime": the **contents** of the file
`[operator] controller_keys_file` names are read again on every signed request, so that a
controller's key is revoked by an edit with no restart. The setting that names the file is still
read once, as is everything else, `write_keys` and `write_keys_file` included. A file that stops
parsing after boot is not a crash and not a stale list: it yields no restricted keys until repaired.
