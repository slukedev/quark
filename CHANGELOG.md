# Changelog

## 0.2.0 - 2026-11-12

Quark is now free software under the GNU GPL, version 3 or later, and targets Linux.

### Works with more apps

- go-xmpp (go-sendxmpp) signs in: whitespace after `<starttls/>` is tolerated, and SASL2 success
  is followed by stream features even with Bind2 or inline resumption.
- Profanity, mellium and other libraries on TLS 1.2 sign in: -PLUS mechanisms are only offered
  where `tls-exporter` exists (TLS 1.3).
- Clients that probe -PLUS mechanisms (slixmpp, Poezio) are no longer locked out after a few good
  logins; only wrong secrets count as failed sign-ins.
- Converse.js no longer loops forever after adding a contact.
- Plain WebSocket or BOSH streams are refused at `<open/>` when TLS is required.
- Messages to malformed JIDs and IQs without an id get errors instead of silence; failed stream
  resumptions let the client bind; stale acks are ignored; every legal stream header style is
  accepted; carriage returns survive delivery.

### Standards

- Stanzas to a bare JID keep their `to`, and error messages to a bare JID are dropped
  (RFC 6121 §8.5.2.1).
- Account disco#info only answers people allowed to see the account's presence; disco on a PEP
  node describes the node; pubsub sent to the domain no longer lands in the sender's PEP.
- Stored subscription requests keep their id and extensions.
- MUC: your own presence reflects the id you sent (Smack's leave no longer hangs), and the error
  conditions for kicking, banning and silencing match XEP-0045.
- FAST tokens follow XEP-0484's two-slot rotation and its error conditions.
- Bind2 sessions get offline messages from the archive instead of twice (XEP-0386 §3.2).
- Admins may set any non-empty password through XEP-0133 commands, an account lists the server's
  commands to itself, and "list online users" asks how many first (§4.21).
- Another user's missing vCard is an error, not an empty card (XEP-0054 §3.3).
- Closing a stream waits briefly for the client to hang up, so clients don't lose the closing
  tag or a stream error to the TLS close.

### History and group chats

- Your own group chat lines no longer appear in your personal history.
- Chat states and markers in group chats no longer use up the message allowance.
- Unconfirmed OMEMO messages without a fallback body are redelivered like any other.

### Faster

- Archiving no longer makes the sender wait for the database: rows are committed in batches.

### Project

- CI runs the XMPP Interop Testing suite; cargo-deny checks licenses and advisories.
- Dependencies updated past open advisories in rustls, rustls-webpki, rmcp, rustix and SQLite.
- Release binaries for x86_64 and aarch64 Linux.
