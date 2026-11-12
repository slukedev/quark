# Quark

XMPP in a box. One Rust binary (or one small container) that is a complete, modern chat server:
accounts and invites, group chats, history on every device, photos and files, push to phones,
calls with a built-in relay, federation, and web chat. Settings change live, certificates reload
themselves, and everything is managed through one JSON admin API, which is how
[Gluon](https://github.com/reallukedev/gluon) runs it.

Works with the apps people use: Conversations, Monal, Siskin, Gajim, Dino, Cheogram, Movim,
Converse.js, Profanity, Poezio and the rest. Compatibility is checked against real client code,
not just Quark's own test client (see [Tested with](#tested-with)).

Quark runs on Linux. It is free software under the GNU GPL, version 3 or later.

## Run it

With Docker:

```sh
docker build -t quark .
docker run -d --name quark \
  -e QUARK_DOMAIN=chat.example.com -e QUARK_ADMINS=you@chat.example.com \
  -v quark-config:/etc/quark -v quark-data:/var/lib/quark \
  -p 5222:5222 -p 5223:5223 -p 5269:5269 -p 5280:5280 \
  -p 3478:3478 -p 3478:3478/udp -p 49160-49200:49160-49200/udp \
  quark
echo '{"cmd":"account.create","args":{"host":"chat.example.com","user":"you","password":"a long password","role":"admin"}}' \
  | docker exec -i quark quark ctl
```

Or download a binary for x86_64 or aarch64 Linux from the
[releases](https://github.com/slukedev/quark/releases), or build one (Rust 1.89 or newer):

```sh
cargo build --release
./target/release/quark --config quark.toml
```

On first start without a config, Quark writes a starter one from `QUARK_DOMAIN`/`QUARK_ADMINS`.
Put a real certificate in `/etc/quark/certs/<domain>.crt` + `.key` (Gluon copies Caddy's for
you); until then Quark uses a self-signed one.

Point DNS (and ideally SRV records `_xmpp-client._tcp`, `_xmpps-client._tcp`,
`_xmpp-server._tcp`) at the box, and a TLS proxy such as Caddy at port 5280 for web chat, uploads
and invite pages.

## With Gluon

Gluon installs Quark as its default chat server (Apps → Chat server), creates the first admin
account, publishes the chat domain through Caddy, copies the certificate in, and gives you
screens for accounts, devices, invites with QR codes, group chats, settings and calls. It talks to
Quark with `quark ctl` inside the container; see [docs/admin-api.md](docs/admin-api.md) and
[docs/gluon-contract.md](docs/gluon-contract.md).

## What it supports

**Core (RFC 6120/6121/7622):** STARTTLS and direct TLS (XEP-0368), SASL PLAIN and
SCRAM-SHA-1/256/512 with `-PLUS` channel binding (tls-exporter, tls-server-end-point,
XEP-0440) and downgrade detection, rosters with versioning and pre-approval, the full presence
subscription state machine, message routing with priorities.

**Modern sign-in:** SASL2 (XEP-0388), Bind2 (XEP-0386) with inline carbons/CSI/stream
management, FAST tokens (XEP-0484) for one-round-trip reconnects, in-band registration
(XEP-0077) with invites (XEP-0401, XEP-0379, XEP-0445).

**Mobile:** stream management with resumption (XEP-0198), client state indication (XEP-0352)
with server-side batching, push (XEP-0357) with priority hints Monal and Siskin use, message
carbons (XEP-0280), offline messages (XEP-0160).

**History:** message archive management (XEP-0313, incl. extended and full-text search), stable
stanza ids (XEP-0359), retention that follows the history setting.

**Group chats:** XEP-0045 with archives, occupant ids (XEP-0421), self-ping (XEP-0410),
moderation (XEP-0425), room avatars, persistent rooms, direct and mediated invites.

**Personal eventing:** PEP/PubSub (XEP-0163/0060) with entity caps (XEP-0115), publish-options
and access models: OMEMO keys, avatars (XEP-0084, synced with vCard XEP-0398), nicknames,
bookmarks 2 (XEP-0402, synced with legacy XEP-0048/0411), vCard4.

**Files and web:** HTTP file upload (XEP-0363) with quotas and optional photo metadata stripping,
WebSocket (RFC 7395), BOSH (XEP-0206), host-meta (XEP-0156), invite landing pages with QR codes.

**Calls:** a built-in STUN/TURN relay (RFC 8489/8656) with external service discovery
(XEP-0215) handing out short-lived credentials; or point it at coturn.

**Federation:** server-to-server with SRV and direct TLS, PKIX certificates and SASL EXTERNAL,
dialback (XEP-0220/0185), stream management on links.

**Also:** blocking (XEP-0191), spam reports (XEP-0377), vcard-temp (XEP-0054), private storage
(XEP-0049), disco (XEP-0030/0128), contact addresses (XEP-0157), ad-hoc commands with service
administration (XEP-0050/0133), ping, time, version, last activity, account export/import
(XEP-0227).

## Beyond other servers

- Every chat setting applies live; nothing needs a restart. Certificates hot-reload from a folder.
- One typed JSON admin API (Unix socket, `quark ctl`, optional HTTP) and an MCP server
  (`quark mcp`) so AI assistants can manage the server with the same tools.
- Built-in call relay, so calls work across networks without a separate coturn.
- No passwords stored, only SCRAM keys; usernames can't be probed through SCRAM.
- Write batching per connection, CSI batching on phones, backpressure on slow clients and fast
  ones, bounded queues everywhere, compressed archives with SQLite FTS5 search.
- Uploads can strip location and camera data from photos as they stream in (`http.strip_photo_metadata`).
- Stranger filter: link-carrying messages from unknown remote senders are rate-limited and never
  wake phones.

## Tested with

Besides its own protocol suite (about 600 tests that drive a real server over the wire), Quark is
checked with the libraries real apps are built on:

| Client code | Used by | Covers |
|---|---|---|
| [XMPP Interop Testing](https://xmpp-interop-testing.github.io/) (Smack) | Smack-based apps; the suite Openfire, ejabberd and Prosody run | ~900 tests across RFC 6120/6121, MUC, PubSub, MAM, SM, SASL2, FAST, Bind2 ([CI](.github/workflows/interop.yml)) |
| slixmpp, aioxmpp | Poezio, bots | 140+ scenarios: every XEP above, two libraries talking to each other |
| Converse.js, Strophe.js, xmpp.js | web chat | WebSocket, BOSH, XEP-0156 discovery, CORS, OMEMO, uploads in a real browser |
| libstrophe | Profanity | STARTTLS, direct TLS, TLS 1.2 and 1.3, stream resumption |
| python-nbxmpp | Gajim | SASL2, bookmarks, OMEMO, vCard4 |
| tokio-xmpp, xmpp-rs | Rust clients | sign-in, MUC, bookmarks, MAM, resumption |
| mellium, go-xmpp | Go clients, go-sendxmpp | every SASL mechanism, SASL2, Bind2, FAST |
| loudmouth | mcabber | legacy sign-in and sessions |

[docs/interop.md](docs/interop.md) explains how to run these. Found an app that misbehaves?
[Open an issue](https://github.com/slukedev/quark/issues/new?template=bug.yml) with the app and
its version.

## Layout

`crates/core` (XML, JIDs, stanzas), `crates/server` (the server and `quark` binary),
`crates/mcp` (MCP tools), `crates/testkit` (a real server on random ports plus a raw client;
the protocol test suite lives in `crates/testkit/tests`). See
[docs/architecture.md](docs/architecture.md), [docs/config.md](docs/config.md),
[docs/testing.md](docs/testing.md).

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Contributions are welcome: read [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Copyright (C) 2026 Luke and contributors.

Quark is free software: you can redistribute it and/or modify it under the terms of the GNU
General Public License as published by the Free Software Foundation, either version 3 of the
License, or (at your option) any later version. It is distributed in the hope that it will be
useful, but WITHOUT ANY WARRANTY; see [LICENSE](LICENSE) for details.
