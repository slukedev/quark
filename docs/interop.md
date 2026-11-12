# Interoperability testing

Quark's own suite (`crates/testkit/tests`) drives a real server over the wire, but with a client
written for Quark. Real apps are built on other libraries, each with its own habits, so Quark is
also checked against those.

## XMPP Interop Testing (Smack)

The [XMPP Interop Testing](https://xmpp-interop-testing.github.io/) project packages Smack's
integration tests for servers; Openfire, ejabberd and Prosody run it too. CI runs it on every push
([`.github/workflows/interop.yml`](../.github/workflows/interop.yml)). To run it locally against a
Quark listening on `localhost:5222` with an admin account `admin@localhost`:

```sh
curl -sSfL -o sint.jar https://github.com/XMPP-Interop-Testing/smack-sint-server-extensions/releases/download/v1.8.0/smack-sint-server-extensions-1.8.0-jar-with-dependencies.jar
# The 1.8.0 jar lists a resolver service that only exists for Java 18+ in a jar not marked
# multi-release; without this, Java 18 and newer abort at the first DNS lookup.
zip -q -d sint.jar META-INF/services/java.net.spi.InetAddressResolverProvider
java -Dsinttest.service=localhost -Dsinttest.host=127.0.0.1 \
  -Dsinttest.securityMode=required -Dsinttest.acceptAllCertificates=true \
  -Dsinttest.adminAccountUsername=admin -Dsinttest.adminAccountPassword=adminpassword \
  -Dsinttest.enabledConnections=tcp -jar sint.jar
```

`-Dsinttest.enabledTests=MultiUserChatOccupantIntegrationTest` runs one class. Each test's
stanzas are logged under `-DlogDir=…` with the debugger
`org.igniterealtime.smack.inttest.util.ModifiedStandardSinttestDebuggerMetaFactory`.

## Other client libraries

Before a release, it's worth signing in with the libraries below and walking through the
features they use. Anything that misbehaves is a bug report with the library's wire log.

| Library | Apps | Worth checking |
|---|---|---|
| slixmpp, aioxmpp | Poezio, bots | SCRAM-PLUS fallback on TLS 1.2, MUC, MAM, PEP publish-options, push |
| Converse.js, Strophe.js, xmpp.js | web chat | WebSocket and BOSH behind a TLS proxy, XEP-0156 discovery, CORS, subscription flow, OMEMO |
| libstrophe | Profanity | TLS 1.2 and 1.3, stream resumption, resource conflicts |
| python-nbxmpp | Gajim | SASL2, bookmarks, OMEMO, vCard4 |
| tokio-xmpp, xmpp-rs | Rust clients | sign-in, MUC autojoin via bookmarks, MAM |
| mellium, go-xmpp | Go clients, go-sendxmpp | STARTTLS quirks, SASL2 + Bind2, FAST |
| loudmouth | mcabber | PLAIN, legacy session |

Things these found that Quark's own tests didn't, all covered by protocol tests now:

- go-xmpp sends a newline after `<starttls/>` and waits for features after SASL2 success.
- Most -PLUS clients default to `tls-unique` on TLS 1.2, which rustls can't do, so -PLUS is only
  offered on TLS 1.3.
- slixmpp tries every -PLUS mechanism before plain SCRAM; those tries mustn't count as wrong
  passwords.
- Converse.js answers every `subscribed` with another `subscribe`.
- Smack waits for the MUC self-presence to carry the id of the presence that caused it.
