#!/usr/bin/env python3
"""TOON_Network#167: what does an Anyone circuit cost a settlement RPC call?

ADR 0070 decision 4 kept `[settlement.*]` RPC off `socks_proxy` until someone
measured it. This measures it: the same JSON-RPC calls, against the same public
devnet endpoints, direct and through a local `anon` SOCKS port (socks5h), in
five modes, round-robined so every mode meets the same network weather.

  direct-fresh      new TCP+TLS per call, no proxy
  direct-keepalive  one connection reused for a batch of calls, no proxy
  pinned-fresh      socks5h, ONE fixed SOCKS credential for the whole run (so
                    one circuit, per IsolateSOCKSAuth), new TCP+TLS per call
  pinned-keepalive  socks5h, fixed credential, one connection per batch --
                    the shape a pooled reqwest client with a proxy would have
  per-call          socks5h, a FRESH credential per call, so anon may not put
                    two calls on one circuit -- the unlinkable-per-call shape

The connector's RPC clients pool connections, so the *-keepalive rows are the
ones to compare with what a node would see steady-state; the *-fresh rows are
what it sees after the pool drops a connection (and per-call is what it would
see if every call were isolated).

Circuit build time is read from anon's control port (SETEVENTS CIRC): the time
from a circuit's LAUNCHED event to its BUILT event, plus FAILED counts.

Nothing here is simulated. Every call goes over the real network, and a call
that failed is counted as failed rather than retried: the failure rate is part
of the answer.

Requires: python3 (stdlib only) and curl on PATH, and an `anon` client with a
SocksPort (IsolateSOCKSAuth, the default) and a ControlPort with a password.
One way, using this repository's image (see local/anon-image):

    docker run -d --name anon167 --init \
      -v $PWD/anonrc:/etc/anon/anonrc:ro \
      -p 127.0.0.1:19050:9050 -p 127.0.0.1:19051:9051 anon-live:v0.4.10.2

with an anonrc holding `AgreeToTerms 1`, `ClientOnly 1`,
`SocksPort 0.0.0.0:9050 IsolateSOCKSAuth`, `ControlPort 0.0.0.0:9051` and a
`HashedControlPassword` from `anon --AgreeToTerms 1 --hash-password <pw>`.

Usage:
    tools/bench/settlement-rpc-over-anon.py --rounds 12 --out target/bench-167

Results: <out>/calls.jsonl (one line per call), <out>/circuits.jsonl, and the
summary printed at the end (also <out>/summary.txt).
"""

import argparse
import json
import os
import secrets
import socket
import statistics
import subprocess
import threading
import time

SOLANA = "https://api.devnet.solana.com"
BASE = "https://sepolia.base.org"

# The read shapes the settlement backends actually issue on their hot paths:
# a blockhash before every Solana send, slot/status polling while confirming,
# an account read; block number / nonce / receipt polling / gas on EVM.
# Random signature/hash: the answer is `null`, which is still a full round trip.
ZERO_SIG = "1" * 64
SYSTEM_PROGRAM = "11111111111111111111111111111111"
PROBE_ADDR = "0x000000000000000000000000000000000000dEaD"


def solana_calls():
    return [
        ("getLatestBlockhash", [{"commitment": "confirmed"}]),
        ("getSlot", [{"commitment": "confirmed"}]),
        ("getSignatureStatuses", [[ZERO_SIG], {"searchTransactionHistory": False}]),
        ("getAccountInfo", [SYSTEM_PROGRAM, {"encoding": "base64"}]),
    ]


def evm_calls():
    return [
        ("eth_blockNumber", []),
        ("eth_getTransactionCount", [PROBE_ADDR, "pending"]),
        ("eth_getTransactionReceipt", ["0x" + secrets.token_hex(32)]),
        ("eth_gasPrice", []),
    ]


# sepolia.base.org is Base's own public endpoint; the connector README and the
# provider bundle's .env preset name base-sepolia-rpc.publicnode.com, which
# --evm-url measures.
CHAINS = [("solana-devnet", SOLANA, solana_calls), ("base-sepolia", BASE, evm_calls)]
MODES = ["direct-fresh", "direct-keepalive", "pinned-fresh", "pinned-keepalive", "per-call"]

WFMT = (
    '{"http":%{http_code},"total":%{time_total},"connect":%{time_connect},'
    '"tls":%{time_appconnect},"ttfb":%{time_starttransfer},'
    '"num_connects":%{num_connects},"exitcode":%{exitcode},"errmsg":"%{errormsg}"}\n'
)


def curl_batch(url, bodies, proxy, max_time):
    """One curl process, len(bodies) transfers joined by --next so the
    connection is reused between them. Returns one result dict per body."""
    args = ["curl", "-sS", "--http1.1"]
    for i, body in enumerate(bodies):
        if i:
            args.append("--next")
        args += [
            "-m", str(max_time), "--connect-timeout", str(max_time),
            "-H", "content-type: application/json",
            "-d", json.dumps(body), "-o", "-", "-w", "\n@@W@@" + WFMT,
        ]
        if proxy:
            args += ["--proxy", proxy]
        args.append(url)
    p = subprocess.run(args, capture_output=True, text=True)
    out = []
    # Parse: each transfer prints <body>\n@@W@@<json>\n
    parts = p.stdout.split("\n@@W@@")
    bodies_out = [parts[0]] + [x.split("\n", 1)[1] if "\n" in x else "" for x in parts[1:-1]]
    metas = [x.split("\n", 1)[0] for x in parts[1:]]
    for i in range(len(bodies)):
        meta = {}
        if i < len(metas):
            try:
                meta = json.loads(metas[i])
            except json.JSONDecodeError:
                meta = {"parse_error": metas[i][:200]}
        body = bodies_out[i] if i < len(bodies_out) else ""
        meta["body"] = body[:300]
        out.append(meta)
    if len(metas) < len(bodies):
        for m in out[len(metas):]:
            m.setdefault("exitcode", p.returncode or -1)
            m.setdefault("errmsg", p.stderr.strip()[:200])
    return out


def classify(r):
    if r.get("exitcode", 0) != 0:
        return "curl:%s %s" % (r.get("exitcode"), (r.get("errmsg") or "")[:60])
    if r.get("http") != 200:
        return "http:%s" % r.get("http")
    try:
        j = json.loads(r.get("body") or "")
    except json.JSONDecodeError:
        return "badjson"
    if "error" in j:
        return "rpc:%s" % j["error"].get("code")
    return "ok"


class Control:
    """Minimal anon/tor control-port client: authenticate, subscribe to CIRC,
    record LAUNCHED->BUILT durations and FAILED reasons."""

    def __init__(self, host, port, password, sink):
        self.s = socket.create_connection((host, port), timeout=10)
        self.f = self.s.makefile("rwb")
        self.sink = sink
        self.launched = {}
        self.cmd('AUTHENTICATE "%s"' % password)
        self.cmd("SETEVENTS CIRC BUILDTIMEOUT_SET")
        self.s.settimeout(None)
        threading.Thread(target=self.loop, daemon=True).start()

    def cmd(self, line):
        self.f.write((line + "\r\n").encode())
        self.f.flush()
        resp = self.f.readline().decode().strip()
        if not resp.startswith("250"):
            raise RuntimeError("control: %s -> %s" % (line, resp))

    def loop(self):
        for raw in self.f:
            line = raw.decode(errors="replace").strip()
            now = time.time()
            if line.startswith("650 CIRC "):
                f = line.split()
                cid, status = f[2], f[3]
                purpose = next((x.split("=", 1)[1] for x in f if x.startswith("PURPOSE=")), "")
                if status == "LAUNCHED":
                    self.launched[cid] = now
                elif status in ("BUILT", "FAILED"):
                    t0 = self.launched.pop(cid, None)
                    reason = next((x.split("=", 1)[1] for x in f if x.startswith("REASON=")), "")
                    self.sink({"t": now, "circ": cid, "status": status, "purpose": purpose,
                               "build_s": (now - t0) if t0 else None, "reason": reason})
            elif line.startswith("650 BUILDTIMEOUT_SET"):
                self.sink({"t": now, "buildtimeout": line[4:]})


def pct(xs, p):
    if not xs:
        return float("nan")
    xs = sorted(xs)
    k = (len(xs) - 1) * p / 100
    lo, hi = int(k), min(int(k) + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--socks", default="127.0.0.1:19050")
    ap.add_argument("--control", default="127.0.0.1:19051")
    ap.add_argument("--password", default="measure")
    ap.add_argument("--rounds", type=int, default=12)
    ap.add_argument("--batch", type=int, default=8, help="calls per mode per chain per round")
    ap.add_argument("--max-time", type=int, default=60, help="curl -m, seconds")
    ap.add_argument("--out", default="target/bench-167")
    ap.add_argument("--solana-url", default=SOLANA)
    ap.add_argument("--evm-url", default=BASE)
    ap.add_argument("--only", choices=[c[0] for c in CHAINS], help="measure one chain only")
    a = ap.parse_args()
    chains = [(n, a.solana_url if n == "solana-devnet" else a.evm_url, mk)
              for n, _, mk in CHAINS if a.only in (None, n)]

    os.makedirs(a.out, exist_ok=True)
    calls_f = open(os.path.join(a.out, "calls.jsonl"), "a")
    circ_f = open(os.path.join(a.out, "circuits.jsonl"), "a")
    lock = threading.Lock()

    def circ_sink(rec):
        with lock:
            circ_f.write(json.dumps(rec) + "\n")
            circ_f.flush()

    ch, cp = a.control.split(":")
    Control(ch, int(cp), a.password, circ_sink)

    pinned_cred = "pinned-" + secrets.token_hex(4)
    started = time.time()

    for rnd in range(a.rounds):
        for chain, url, mk in chains:
            for mode in MODES:
                methods = mk()
                reqs = [methods[i % len(methods)] for i in range(a.batch)]
                bodies = [{"jsonrpc": "2.0", "id": i + 1, "method": m, "params": p}
                          for i, (m, p) in enumerate(reqs)]
                t_wall = time.time()
                if mode == "direct-fresh":
                    res = [curl_batch(url, [b], None, a.max_time)[0] for b in bodies]
                elif mode == "direct-keepalive":
                    res = curl_batch(url, bodies, None, a.max_time)
                elif mode == "pinned-fresh":
                    px = "socks5h://%s:x@%s" % (pinned_cred, a.socks)
                    res = [curl_batch(url, [b], px, a.max_time)[0] for b in bodies]
                elif mode == "pinned-keepalive":
                    px = "socks5h://%s:x@%s" % (pinned_cred, a.socks)
                    res = curl_batch(url, bodies, px, a.max_time)
                else:  # per-call
                    res = []
                    for b in bodies:
                        px = "socks5h://call-%s:x@%s" % (secrets.token_hex(6), a.socks)
                        res.append(curl_batch(url, [b], px, a.max_time)[0])
                for i, r in enumerate(res):
                    r.update({"round": rnd, "chain": chain, "mode": mode,
                              "method": reqs[i][0], "outcome": classify(r), "t": t_wall})
                    calls_f.write(json.dumps(r) + "\n")
                calls_f.flush()
                oks = sum(1 for r in res if r["outcome"] == "ok")
                print("round %2d %-13s %-16s ok %d/%d  wall %.1fs" %
                      (rnd, chain, mode, oks, len(res), time.time() - t_wall), flush=True)

    print("elapsed %.0fs" % (time.time() - started))
    time.sleep(2)
    summarize(a.out)


def summarize(out):
    calls = [json.loads(l) for l in open(os.path.join(out, "calls.jsonl"))]
    circs = [json.loads(l) for l in open(os.path.join(out, "circuits.jsonl"))]
    lines = []
    hdr = "%-13s %-16s %5s %6s %7s %7s %7s %7s  %s" % (
        "chain", "mode", "n", "fail%", "p50", "p95", "p99", "max", "failures")
    lines.append(hdr)
    for chain, _, _ in CHAINS:
        for mode in MODES:
            rs = [c for c in calls if c["chain"] == chain and c["mode"] == mode]
            if not rs:
                continue
            ok = [c["total"] for c in rs if c["outcome"] == "ok"]
            bad = [c["outcome"] for c in rs if c["outcome"] != "ok"]
            kinds = {}
            for b in bad:
                kinds[b] = kinds.get(b, 0) + 1
            lines.append("%-13s %-16s %5d %5.1f%% %6.3fs %6.3fs %6.3fs %6.3fs  %s" % (
                chain, mode, len(rs), 100.0 * len(bad) / len(rs),
                pct(ok, 50), pct(ok, 95), pct(ok, 99), max(ok) if ok else float("nan"),
                ", ".join("%s x%d" % kv for kv in sorted(kinds.items())) or "-"))
    built = [c["build_s"] for c in circs if c.get("status") == "BUILT" and c.get("build_s")]
    failed = [c for c in circs if c.get("status") == "FAILED"]
    lines.append("")
    lines.append("circuits: built %d, failed %d; build time p50 %.3fs p95 %.3fs p99 %.3fs max %.3fs" % (
        len(built), len(failed), pct(built, 50), pct(built, 95), pct(built, 99),
        max(built) if built else float("nan")))
    reasons = {}
    for c in failed:
        reasons[c.get("reason")] = reasons.get(c.get("reason"), 0) + 1
    if reasons:
        lines.append("circuit failure reasons: %s" % ", ".join("%s x%d" % kv for kv in reasons.items()))
    bt = [c["buildtimeout"] for c in circs if "buildtimeout" in c]
    if bt:
        lines.append("last BUILDTIMEOUT_SET: %s" % bt[-1])
    text = "\n".join(lines)
    print(text)
    open(os.path.join(out, "summary.txt"), "w").write(text + "\n")


if __name__ == "__main__":
    import sys
    if len(sys.argv) == 3 and sys.argv[1] == "--summarize":
        summarize(sys.argv[2])
    else:
        main()
