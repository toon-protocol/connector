# A controller's key is revoked by an edit, and may sign a read

**Status:** Accepted (owner decision, 2026-10-12, #1485) — **built**. It **amends** [0078](0078-a-controllers-write-key-carries-a-scope-and-two-limits.md): that record's refused option "reread the key file on every write" is now the decision, and its decision 11 gains a second way in. It **amends** [0009](0009-one-typed-config-file-no-environment-layer.md) for the contents of one file and nothing else. It **extends** [0008](0008-operator-surface-splits-read-from-write.md): a read may be signed, and a bearer token still never moves value.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

**Falsifier:** none — the record claims nothing absent that a pattern over the tree could find: what it refuses (a reload of `connector.toml`, of `write_keys` or of `write_keys_file`) is the absence of a second caller of the one re-read, which no symbol names.

**The contents of `controller_keys_file` are read again on every signed request, and a read signed
by a write key is accepted in place of the bearer token.** Both are what a controller that holds a
key and nothing else needs: its operator can stop it at once, and it never has to be handed a secret
that every other reader shares.

## Context

[0078](0078-a-controllers-write-key-carries-a-scope-and-two-limits.md) gave a controller a
restricted key and kept two things as they were. Revoking the key stayed an edit and a restart,
because [0009](0009-one-typed-config-file-no-environment-layer.md) makes config immutable for the
process lifetime. And every read stayed behind the bearer token
([0008](0008-operator-surface-splits-read-from-write.md)).

`toon` is the first tool built on 0078 (toon-protocol/toon_cli#208), and it needs both to move.
It revokes an agent key by rewriting the list, and the revoke has to bind the key's next request:
restarting a connector to stop an agent takes the node down for everyone it serves, and an operator
who must do that will put it off. And its agent is never given the bearer token. The token is one
secret shared by every reader, so it cannot be revoked for one of them, and an agent that held it
would keep every read after its key was gone.

## Decision

1. **The contents of `controller_keys_file` are read again on every request that presents a
   signature.** A request with none is refused before the file is touched. An entry
   removed is an unlisted key at that key's next request, and an entry added or changed binds the
   same way. Nothing is watched and nothing is cached past a request, except that contents equal
   to the last ones parsed are not parsed again.
2. **Only the contents are live.** Which file `controller_keys_file` names, `write_keys`,
   `write_keys_file`, the bearer token and every other key in `connector.toml` are config, read
   once at boot (0009). A node that named no `controller_keys_file` at boot has no restricted keys
   until it is restarted with one. An operator's key is still revoked by an edit and a restart.
3. **A file that cannot be read, or no longer parses under the rules it is held to at boot, yields
   no restricted keys.** Every restricted key is then answered `401`, as an unlisted key, until the
   file is repaired, and the breakage is one error in the log naming the file, not one per
   request. The last list
   that parsed is not served, because that would keep honouring a key its operator may have just
   tried to revoke. A key on `write_keys` is not affected. At boot the same file is still a refusal
   to start. One boot rule lives outside the file and is not applied again: a limit stated on a
   node with no `state_dir`. An entry that gains a limit there is listed and has every packet and
   deposit refused, because its count cannot be journaled.
4. **The day's totals do not follow the file.** They are journaled by key (0078 decision 8), so
   removing an entry and adding it back does not reset its day, and raising a limit takes effect
   against what is already counted.
5. **A read with no `Authorization` header may be signed instead.** It carries the same RFC 9421
   signature as a write, over `@method`, `@path` and the `content-digest` of an empty body, by any
   key that may sign a write: an operator's, or a controller's that is listed and not expired. A
   request that does present an `Authorization` header is judged by the token alone. The
   signature covers the path and not the query, as a write's does.
6. **A scope says nothing about reads.** A scope is a list of named writes (0078 decision 1), and
   any listed key reads everything the bearer token reads. A restricted key past its expiry is
   refused a read `403` naming the expiry; a key gone from the file is refused `401`.
7. **A signed read is not spent and not retained.** It is good until its own `expires`, is not in
   `GET /audit-log`, and does not move `toon_writes_refused_total`. Replay rejection and the audit
   log belong to writes (OP-05, OP-02). A replayed read tells its replayer what the bearer token
   would, until the `expires` its signer chose or until the key is revoked. No largest window is
   enforced, on reads or on writes.
8. **A bearer token still never moves value** (OP-03). Write authority now confers read authority.
   Read authority confers nothing new.

## Considered options

- **Keep the restart**, as 0078 decided. Refused by decision 1. It makes stopping a controller cost
  an outage, and it was chosen before anything was built against it.
- **A signed write that reloads the file**, or a signal. Either adds a step between the edit and
  the revoke that can be forgotten or fail, and a reload endpoint is one more write to scope.
- **Watch the file.** A watcher is a second source of truth about what the file holds, with its own
  lag and its own failure, on a surface whose traffic is a few requests a second at most.
- **Serve the last good list when the file breaks.** Refused by decision 3.
- **Reload `write_keys_file` too.** Not asked for. An operator's key is the one that can withdraw
  every channel, and the operator controls the box it is revoked on.
- **A bearer token per reader**, or scoped tokens. 0078 already refused scoped tokens. A second
  secret per controller is a second thing to revoke beside a key it already holds.
- **Scopes for reads.** Not asked for. `toon` keeps the scopes of its supervisor's own requests in
  its own record, and the bearer token has always read everything.
- **Spend a read's signature as a write's is spent.** Two reads of one path signed in the same
  second carry the same signature, so a controller that polls would be refused its own request.

## Consequences

- Whoever can write `controller_keys_file` can grant a key at once, with no restart to notice. The
  file deserves the permissions of the config that names it.
- A rewrite that is not atomic can be read half-written, and then decision 3 refuses every
  restricted key for that request. Write the new list beside the old and rename it into place.
- Every signed request reads a small file, synchronously and under a lock, on a node that names
  one. That is `POST /packets` from a write key and no other packet: a packet arriving on the
  client edge or a peering is not a signed request and never reaches it
  ([0015](0015-connector-is-a-rust-rewrite.md)'s packet path is untouched). The same handler already
  takes the replay lock and appends a journal line before it hands a packet on.
- A deposit whose key leaves the file between its write being authenticated and its deposit being
  counted is refused, not treated as an operator's: only a key on `write_keys` is ever unlimited.
- The setting and the wire shape of every existing request are unchanged, so this is not a
  breaking deploy.
