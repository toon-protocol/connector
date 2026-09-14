# anon v0.4.10.2 -- the daemon BOTH hidden-service topologies here run, and the
# release `toon-client` pins as ANON_VERSION.
#
# WHY THIS FILE EXISTS AT ALL. ghcr.io/anyone-protocol/ator-protocol publishes
# no image for it: its tag list stops at v0.4.9.7, which is from October 2024.
# So this overlays the official release binary onto that image, which keeps the
# image contract -- the `anond` user, /var/lib/anon and its ownership, the
# `docker-entrypoint` -- identical to the one those topologies used before.
# The binary is statically linked, so nothing else has to come with it.
#
# THE VERSION IS THE WHOLE POINT (issue #1284). Upstream renamed the
# hidden-service TLD between these two releases and the rename is total in both
# directions: v0.4.9.7 routes `.onion` and does not contain the string
# `.anyone`; v0.4.10.2 routes `.anyone` and contains ZERO occurrences of
# `.onion`, refusing an address spelled that way. A daemon at the older pin
# therefore publishes an address `toon-client` refuses and cannot resolve the
# one it dials. The connector accepts both spellings -- `is_onion_endpoint`,
# ADR 0070 as amended -- because it dials whatever ITS operator's daemon
# speaks; this file is that daemon, and it is the current one.
#
# ONE image, built once and used by `local/onion` (the peering rehearsal) and
# `local/anyone` (the client-edge one). A second copy of this file is a second
# pinned version to drift.
FROM ghcr.io/anyone-protocol/ator-protocol:v0.4.9.7

ARG ANON_VERSION=v0.4.10.2
# sha256 of anon-live-linux-amd64.zip, the same figure toon-client records in
# `packages/client/src/cli/anon-daemon.ts`. Checked BEFORE the binary is ever
# executed: an unverified download is not an upgrade.
ARG ANON_SHA256=9c6498b8d27de54d78842a1b854979a605f9c140ccc34f2b4c267bf094eaeb17

USER root
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends curl unzip ca-certificates; \
    curl -fsSL -o /tmp/anon.zip \
      "https://github.com/anyone-protocol/ator-protocol/releases/download/${ANON_VERSION}/anon-live-linux-amd64.zip"; \
    echo "${ANON_SHA256}  /tmp/anon.zip" | sha256sum -c -; \
    unzip -o -j /tmp/anon.zip -d /tmp/anon; \
    install -m 0755 /tmp/anon/anon /usr/local/bin/anon; \
    install -m 0644 /tmp/anon/geoip  /usr/local/share/anon/geoip; \
    install -m 0644 /tmp/anon/geoip6 /usr/local/share/anon/geoip6; \
    rm -rf /tmp/anon /tmp/anon.zip; \
    apt-get purge -y curl unzip; \
    apt-get autoremove -y; \
    rm -rf /var/lib/apt/lists/*; \
    # Fail the BUILD, not the first bring-up, if the wrong release was fetched.
    anon --version | grep -q "0.4.10.2"
