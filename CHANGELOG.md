# Changelog

## Unreleased

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
