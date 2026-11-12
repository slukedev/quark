//! Wire-level behaviour real clients depend on, found by driving Quark with raw sockets:
//! stream headers (RFC 6120 §4), addressing errors (§8.3), stream management edge cases
//! (XEP-0198) and text that has to survive a round trip.

use std::time::Duration;

use crate::blind_common;

use blind_common::*;
use quark_testkit::quark_core::{ns, Element};
use quark_testkit::{message, Client, TestServer, DOMAIN};

async fn server() -> anyhow::Result<TestServer> {
    let s = TestServer::start().await?;
    s.add_user("alice", "password1").await?;
    s.add_user("bob", "password1").await?;
    Ok(s)
}

/// The server's header and features (or stream error) for a header written by the test.
async fn open_raw(s: &TestServer, header: &str) -> anyhow::Result<(Client, Element)> {
    let mut c = Client::connect_raw(s.c2s(), DOMAIN).await?;
    c.open_with(header).await?;
    let first = c.recv().await?;
    Ok((c, first))
}

#[tokio::test]
async fn stream_headers_in_every_legal_style_are_accepted() -> anyhow::Result<()> {
    let s = server().await?;
    // A prefix-free header with no default namespace (RFC 6120 §4.8.2's second example).
    let (_, f) = open_raw(&s, &format!("<stream xmlns='{}' to='{DOMAIN}' version='1.0'>", ns::STREAM)).await?;
    assert!(f.is("features", ns::STREAM), "{f}");
    // A trailing dot is part of no domain (RFC 7622 §3.2); case doesn't matter.
    let (_, f) = open_raw(&s, &header("QUARK.test.")).await?;
    assert!(f.is("features", ns::STREAM), "{f}");
    // Our header goes back `to` the bare JID the client named (RFC 6120 §4.7.2).
    let h =
        format!("<stream:stream xmlns='{}' xmlns:stream='{}' from='alice@{DOMAIN}/x' to='{DOMAIN}' version='1.0'>", ns::CLIENT, ns::STREAM);
    let (c, _) = open_raw(&s, &h).await?;
    assert_eq!(c.header_attr("to"), Some(format!("alice@{DOMAIN}").as_str()));
    Ok(())
}

#[tokio::test]
async fn a_second_stream_header_ends_the_stream_instead_of_swallowing_it() -> anyhow::Result<()> {
    let s = server().await?;
    let mut a = s.login("alice", "password1").await?;
    a.send_raw(&header(DOMAIN)).await?;
    a.send_raw("<iq type='get' id='p'><ping xmlns='urn:xmpp:ping'/></iq>").await?;
    let e = a.expect(|e| e.is("error", ns::STREAM)).await?;
    assert!(stream_error_condition(&e).is_some(), "{e}");
    Ok(())
}

#[tokio::test]
async fn unaddressable_stanzas_are_answered_not_dropped() -> anyhow::Result<()> {
    let s = server().await?;
    let mut a = s.login("alice", "password1").await?;
    // A `to` that isn't a JID: jid-malformed back to the sender (RFC 6120 §8.3.3.8).
    a.send_raw("<iq type='get' id='bad' to='a@b@quark.test'><ping xmlns='urn:xmpp:ping'/></iq>").await?;
    let r = a.expect(|e| e.attr("id") == Some("bad")).await?;
    assert_eq!(stanza_error(&r), Some(("modify".into(), "jid-malformed".into())), "{r}");
    assert_eq!(r.attr("from"), None, "{r}");
    // An IQ without an id can't be matched to an answer (RFC 6120 §8.1.3).
    a.send_raw("<iq type='get'><ping xmlns='urn:xmpp:ping'/></iq>").await?;
    let r = a.expect(|e| e.name() == "iq").await?;
    assert_eq!(stanza_error(&r).map(|e| e.1), Some("bad-request".into()), "{r}");
    Ok(())
}

#[tokio::test]
async fn carriage_returns_survive_delivery() -> anyhow::Result<()> {
    let s = server().await?;
    let mut a = s.online("alice", "password1", "a").await?;
    let mut b = s.online("bob", "password1", "b").await?;
    a.send(&message(&format!("bob@{DOMAIN}"), "chat", "one\r\ntwo")).await?;
    let m = b.expect(|e| e.name() == "message").await?;
    assert_eq!(m.child_text("body", ns::CLIENT).as_deref(), Some("one\r\ntwo"));
    Ok(())
}

#[tokio::test]
async fn a_stale_ack_is_ignored() -> anyhow::Result<()> {
    let s = server().await?;
    let mut a = s.online("alice", "password1", "a").await?;
    drain(&mut a).await?;
    assert!(sm_enable(&mut a, false).await?.is("enabled", SM));
    let mut b = s.online("bob", "password1", "b").await?;
    for i in 0..3 {
        b.send(&message(&format!("alice@{DOMAIN}"), "chat", &format!("m{i}"))).await?;
    }
    for _ in 0..3 {
        a.expect(|e| e.name() == "message").await?;
    }
    // XEP-0198 §6 only forbids acking more than was sent; going back confirms nothing new.
    a.send_raw("<a xmlns='urn:xmpp:sm:3' h='3'/><a xmlns='urn:xmpp:sm:3' h='2'/>").await?;
    let r = a.iq_get(Some(DOMAIN), Element::new("ping", ns::PING)).await?;
    assert_eq!(r.attr("type"), Some("result"), "{r}");
    Ok(())
}

#[tokio::test]
async fn a_resume_claiming_too_much_fails_softly_and_the_client_can_bind() -> anyhow::Result<()> {
    let s = server().await?;
    let mut a = s.online("alice", "password1", "phone").await?;
    drain(&mut a).await?;
    let en = sm_enable(&mut a, true).await?;
    let previd = en.attr("id").unwrap_or_default().to_string();
    a.drop_connection();

    let mut c = authed_client(&s, "alice", "password1").await?;
    c.send(&Element::new("resume", SM).attr_set("previd", &previd).attr_set("h", "10")).await?;
    let f = c.expect(|e| e.ns() == SM).await?;
    assert!(f.is("failed", SM) && f.has_child("handled-count-too-high", SM), "{f}");
    assert!(f.attr("h").is_some(), "the client should learn what we handled: {f}");
    // XEP-0198 §5: the server SHOULD allow binding a resource after a failed resume.
    c.bind(Some("phone")).await?;
    let r = c.iq_get(Some(DOMAIN), Element::new("ping", ns::PING)).await?;
    assert_eq!(r.attr("type"), Some("result"), "{r}");
    c.expect_silence(Duration::from_millis(200)).await?;
    Ok(())
}

#[tokio::test]
async fn probing_channel_binding_types_is_not_a_failed_sign_in() -> anyhow::Result<()> {
    let s = server().await?;
    // Clients without tls-exporter try tls-unique on every -PLUS mechanism before falling back.
    // Ten of those a minute used to lock the address out, correct passwords and all.
    for _ in 0..6 {
        let mut c = tls_client(&s).await?;
        for m in ["SCRAM-SHA-512-PLUS", "SCRAM-SHA-256-PLUS", "SCRAM-SHA-1-PLUS"] {
            let first = b64("p=tls-unique,,n=alice,r=abcdefghijklmnop");
            c.send(&Element::new("auth", ns::SASL).attr_set("mechanism", m).text(first)).await?;
            let f = c.expect(|e| e.ns() == ns::SASL).await?;
            assert_eq!(sasl_failure_condition(&f).as_deref(), Some("not-authorized"), "{f}");
        }
        c.auth_scram("SCRAM-SHA-256", "alice", "password1").await?;
    }
    Ok(())
}
