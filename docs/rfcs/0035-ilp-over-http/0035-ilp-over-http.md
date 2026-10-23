# RFC 0035 — ILP over HTTP

> **Vendored, unmodified.** The body below the marker line is the Interledger
> Foundation's text, reproduced byte for byte. Only this preface was added.
> [ADR 0062](../../adr/0062-an-rfc-is-vendored-verbatim-and-profiled-never-forked.md)
> is why, and why it is never edited to match what this connector does.
>
> - **Upstream:** `interledger/rfcs` → `0035-ilp-over-http/0035-ilp-over-http.md`
> - **Pinned commit:** `1eb8d73b67a1d048f74ded508406a7e1ae1e00d5`
> - **Body SHA-256:** `d764ca8e3ebc6513bcfc796f13a1b1e3c9c7c8ce17f9e5ff8f07fddd57fbba2a`
> - **Licence:** CC BY-SA 4.0 — see [`docs/rfcs/README.md`](../README.md)

## TOON profile

**This is the shape of `POST /ilp` and of the `https://` peer carriage. The
request/response contract below is honoured closely; what TOON adds is how the
request pays for itself.**

- **An unpaid request to a priced route is answered `402`, not rejected.** The
  body is an x402 v2 `PaymentRequired` document (`application/json`), repeated
  base64-encoded in a `Payment-Required` header, quoting the same price a real
  request would be charged (`client-edge-spec.md` §1.4). The BTP mirror is an
  `F06` REJECT carrying byte-identical terms.
- **Payment rides in headers this RFC does not define:**
  `ILP-Payment-Channel-Claim` (base64 JSON) and
  `ILP-Payment-Channel-Claim-Wrapped` (NIP-59-wrapped, HTTP-only and ignored on
  a peer request) on the way in; `Toon-Claim-Ack`, `Toon-Accumulated-Cost` and
  `Toon-Flush-Requested` on the way back
  ([ADR 0042](../../adr/0042-a-packet-carries-its-claim.md);
  `client-edge-spec.md` §1.3).
- **`ILP-Peer-Id` plus `Authorization: Bearer` is a client-edge mechanism and is
  never sufficient to move value.** The identities are `[[client_identities]]`;
  an empty secret makes an identity permissionless; a node configuring none
  refuses any presented `ILP-Peer-Id` with `401`, before the route is looked up
  (`client-edge-spec.md` §1.2). What authorises a **write** is the claim
  ([ADR 0008](../../adr/0008-operator-surface-splits-read-from-write.md)). The
  peer HTTP carriage sends neither header — only the claim.
- **Anonymous is the normal case, not a fallback.** With no `ILP-Peer-Id` the
  connector derives an ephemeral id from the claim's signer, or `http:anon`. No
  prior registration with the operator is required to pay for a terminated route,
  and [ADR 0052](../../adr/0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md)
  makes that a guarantee rather than a convenience.
- **A `GET` on the same `/ilp` path serves the node's self-description**
  ([ADR 0050](../../adr/0050-a-connectors-url-resolves-to-its-self-description.md);
  `self-description-spec.md`), which is how a peering is established from a URL
  at all ([ADR 0058](../../adr/0058-a-peering-is-established-from-a-url.md)).
- **Peers and clients share one listener.** There is no peer bind address;
  `peer_wire_addr` is a tombstone
  ([ADR 0027](../../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)).
  A dedicated peer listener is permitted as defence in depth, but role is still
  decided by the channel binding and the claim, never by which listener the bytes
  arrived on (`peer-carriage-spec.md` §1.10).
- **A body is read up to the node's request limit, which is deliberately not a config knob**
  ([ADR 0082](../../adr/0082-a-route-may-raise-the-request-limit-to-carry-its-payload-limit.md);
  `client-edge-spec.md` §1.1): 2 MiB unless a route's `max_bytes` raises it. `POST /ilp/probe` is a second ingress with the
  same framing, gated by channel recognition and a per-channel rate limit
  answering `403`
  ([ADR 0011](../../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md)).

**Faithful, and load-bearing:** `POST` an OER PREPARE as
`application/octet-stream`; the response is an OER FULFILL or REJECT at HTTP
**200**. A non-2xx is reserved for transport-level failure and never carries an
OER body — an internal routing error surfaces as `200` with a `T00` REJECT rather
than a `500`, and a _rejected claim_ still returns `200`
(`peer-carriage-spec.md` §6.2). Header names are matched case-insensitively per
RFC 9110, with the canonical lower-case spelling pinned by the vectors.

<!-- BEGIN VERBATIM UPSTREAM BODY -->

---
title: ILP Over HTTP
type: working-draft
draft: 3
---

# ILP Over HTTP

> A bilateral communication protocol for server-to-server connections

## Motivation

Scaling Interledger infrastructure to handle large volumes of ILP packets requires horizontally scaling connectors. Using HTTP for bilateral communication enables service providers to leverage standard tools and services for hosting, load balancing, Distributed Denial of Service (DDoS) protection, and monitoring.

## Overview

In an ILP Over HTTP connection, both peers run HTTP servers with accessible HTTPS endpoints. When peering, the peers exchange their respective URLs, authentication tokens or TLS certificates, ILP addresses, and settlement-related details.

Each ILP Prepare packet is sent as the body of an HTTP request to the peer's server endpoint. The peer asynchronously returns ILP Fulfill or Reject packets in the body of a separate HTTP request. 

## Specification

This is a minimal protocol built on HTTP. HTTP/2 is HIGHLY RECOMMENDED for performance reasons, although HTTP/1.1 MAY also be used. Implementations SHOULD support HTTP version negotiation via Application Protocol Negotiation (ALPN).

### Authentication

Peers MAY use any standard HTTP authentication mechanism to authenticate incoming requests. TLS Client Certificates are RECOMMENDED between peers for security and performance, though bearer tokens such as JSON Web Tokens (JWTs) or Macaroons MAY be used instead. Basic authentication (username and password) is NOT RECOMMENDED, because of the additional delay introduced by securely hashing the password.

### Send ILP Prepare

#### Request

```http
POST /ilp HTTP/x.x
Host: bob.example
Accept: application/octet-stream
Content-Type: application/octet-stream
Authorization: Bearer zxcljvoizuu09wqqpowipoalksdflksjdgxclvkjl0s909asdf
Callback-Url: https://alice.example/incoming/ilp
Request-Id: 42ee09c8-a6de-4ae3-8a47-4732b0cbb07b

< Body: Binary OER-Encoded ILP Prepare Packet >
```

- **Path** &mdash; A connector MAY specify any HTTP path for their peer to send ILP packets to.
- **Host Header** &mdash; The standard [HTTP Host Header](https://tools.ietf.org/html/rfc2616#section-14.23) indicating the domain of the HTTP server the request is sent to.
- **Content-Type / Accept Headers** &mdash; MUST be set to `application/octet-stream`.
- **Body** &mdash; ILP Prepare encoded using OER, as specified in [RFC 27: Interledger Protocol V4](../0027-interledger-protocol-4/0027-interledger-protocol-4.md).
- **Callback URL Header** &mdash; Callback URL of the origin connector to send an HTTP request with the ILP Fulfill/Reject. Required unless peers exchange the callback URL out-of-band.
- **Request Id Header** &mdash; UUIDv4 to uniquely identify this ILP Prepare, and correlate the corresponding ILP Fulfill/Reject.

#### Response

If the request is semantically valid, the recipient MUST respond immediately that the ILP Prepare is accepted for processing, even if the packet will ultimately be rejected:

```http
HTTP/x.x 202 Accepted
```

### ILP Fulfill/Reject Reply

#### Request

```http
POST /incoming/ilp HTTP/x.x
Host: alice.example
Content-Type: application/octet-stream
Authorization: Bearer zxcljvoizuu09wqqpowipoalksdflksjdgxclvkjl0s909asdf
Request-Id: 42ee09c8-a6de-4ae3-8a47-4732b0cbb07b

< Body: Binary OER-Encoded ILP Fulfill or Reject Packet >
```

- **Path** &mdash; HTTP path from the callback URL in the original request carrying the ILP Prepare.
- **Host Header** &mdash; The standard [HTTP Host Header](https://tools.ietf.org/html/rfc2616#section-14.23) indicating the domain of the HTTP server the Request is sent to.
- **Content-Type Header** &mdash; MUST be set to `application/octet-stream`.
- **Request Id Header** &mdash; Request ID from the corresponding ILP Prepare, which is a UUIDv4, matching this reply to the original request.
- **Body** &mdash; ILP Packet encoded using OER, as specified in [RFC 27: Interledger Protocol V4](../0027-interledger-protocol-4/0027-interledger-protocol-4.md).

#### Response

```http
HTTP/x.x 200 OK
```

If the request ID doesn't correspond to an in-flight ILP Prepare, or a reply was already processed, the connector should ignore it and return an error:

```http
HTTP/x.x 400 Bad Request
```

#### Retry Behavior

If the recipient of the ILP Fulfill/Reject responds with a `5xx` status or no HTTP response is received within a given timeout, the sender of the ILP Fulfill/Reject SHOULD retry sending the request.

The sender of the ILP Fulfill/Reject MUST conclude retrying after receiving a response with a `2xx` status or `4xx` error.

The sender of the ILP Fulfill/Reject SHOULD ensure there are multiple attempts to deliver the reply packet to the peer before the corresponding ILP Prepare expires.

#### Idempotence

An ILP Fulfill packet corresponds to a commitment which affects financial accounting balances. If an HTTP request carrying the ILP reply fails, such as due to a network connection error, retrying delivery of the ILP reply with [idempotence](https://en.wikipedia.org/wiki/Idempotence) can prevent balance inconsistencies between peers.

The sender of the ILP Prepare should only process the first ILP reply they receive corresponding to the original ILP Prepare packet.

### Error Handling

An endpoint MAY return standard HTTP errors, including but not limited to: a malformed or unauthenticated request, rate limiting, or an unresponsive upstream service. Connectors SHOULD relay an ILP Reject packet back to the original sender with an appropriate [Final or Temporary error code](../0027-interledger-protocol-4/0027-interledger-protocol-4.md#error-codes). Server errors (status codes 500-599) SHOULD be translated into ILP Reject packets with `T00: Temporary Error` codes.
