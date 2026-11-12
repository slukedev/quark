//! Ad-hoc commands (XEP-0050) for admins (XEP-0133) and invites.

use crate::common;

use common::*;
use quark_testkit::quark_core::{ns, Element};
use quark_testkit::{Client, TestServer};

const ADMIN: &str = "http://jabber.org/protocol/admin";

async fn command_nodes(c: &mut Client, domain: &str) -> anyhow::Result<Vec<String>> {
    let r = c.iq_get(Some(domain), Element::new("query", ns::DISCO_ITEMS).attr_set("node", ns::COMMANDS)).await?;
    Ok(r.get_child("query", ns::DISCO_ITEMS)
        .map(|q| q.children().filter_map(|i| i.attr("node").map(str::to_string)).collect())
        .unwrap_or_default())
}

/// Runs a command in one step with these fields; returns the `<command/>`.
async fn run(c: &mut Client, domain: &str, node: &str, fields: &[(&str, &[&str])]) -> anyhow::Result<Element> {
    let r = run_iq(c, domain, node, fields).await?;
    r.get_child("command", ns::COMMANDS).cloned().ok_or_else(|| anyhow::anyhow!("refused: {r}"))
}

/// [`run`], returning the final IQ (a result or an error).
async fn run_iq(c: &mut Client, domain: &str, node: &str, fields: &[(&str, &[&str])]) -> anyhow::Result<Element> {
    let first = c.iq_set(Some(domain), Element::new("command", ns::COMMANDS).attr_set("node", node).attr_set("action", "execute")).await?;
    let cmd = first.get_child("command", ns::COMMANDS).cloned().ok_or_else(|| anyhow::anyhow!("refused: {first}"))?;
    if cmd.attr("status") == Some("completed") {
        return Ok(first);
    }
    let mut x = Element::new("x", ns::DATA_FORMS).attr_set("type", "submit");
    for (var, vals) in fields {
        let mut f = Element::new("field", ns::DATA_FORMS).attr_set("var", *var);
        for v in *vals {
            f.append(Element::new("value", ns::DATA_FORMS).text(*v));
        }
        x.append(f);
    }
    let submit = Element::new("command", ns::COMMANDS)
        .attr_set("node", node)
        .attr_set("sessionid", cmd.attr("sessionid").unwrap_or_default())
        .attr_set("action", "complete")
        .child(x);
    c.iq_set(Some(domain), submit).await
}

fn note(cmd: &Element) -> String {
    cmd.child_text("note", ns::COMMANDS).unwrap_or_default()
}

#[tokio::test]
async fn members_see_invites_and_admins_see_everything() -> anyhow::Result<()> {
    let s = TestServer::start().await?;
    s.add_user("sam", "password-1").await?;
    s.add_admin("kim", "password-2").await?;
    let mut sam = s.login("sam", "password-1").await?;
    let mut kim = s.login("kim", "password-2").await?;
    assert_eq!(command_nodes(&mut sam, s.domain()).await?, ["urn:xmpp:invite#invite"]);
    let all = command_nodes(&mut kim, s.domain()).await?;
    assert!(all.contains(&format!("{ADMIN}#add-user")) && all.contains(&"urn:xmpp:invite#create-account".to_string()), "{all:?}");

    let r = sam.iq_set(Some(s.domain()), Element::new("command", ns::COMMANDS).attr_set("node", format!("{ADMIN}#add-user"))).await?;
    assert_eq!(condition(&r), "forbidden");
    let r = sam.iq_set(Some(s.domain()), Element::new("command", ns::COMMANDS).attr_set("node", "nope")).await?;
    assert_eq!(condition(&r), "item-not-found");
    Ok(())
}

#[tokio::test]
async fn admins_manage_accounts() -> anyhow::Result<()> {
    let s = TestServer::start().await?;
    let d = s.domain().to_string();
    s.add_admin("kim", "password-2").await?;
    let mut kim = s.login("kim", "password-2").await?;

    let lee = format!("lee@{d}");
    let done = run(
        &mut kim,
        &d,
        &format!("{ADMIN}#add-user"),
        &[("accountjid", &[&lee]), ("password", &["password-3"]), ("password-verify", &["password-3"])],
    )
    .await?;
    assert_eq!(done.attr("status"), Some("completed"), "{done}");
    assert!(can_sign_in(&s, "lee", "password-3").await, "{}", note(&done));

    let count = run(&mut kim, &d, &format!("{ADMIN}#get-registered-users-num"), &[]).await?;
    assert!(count.to_xml().contains("<value>2</value>"), "{count}");

    let mut lee_phone = s.online("lee", "password-3", "phone").await?;
    let off = run(&mut kim, &d, &format!("{ADMIN}#disable-user"), &[("accountjids", &[&lee, "ghost"])]).await?;
    assert!(note(&off).contains("ghost@"), "{off}");
    assert_eq!(stream_error(&mut lee_phone).await, "not-authorized");
    assert!(!can_sign_in(&s, "lee", "password-3").await);
    run(&mut kim, &d, &format!("{ADMIN}#reenable-user"), &[("accountjids", &[&lee])]).await?;
    assert!(can_sign_in(&s, "lee", "password-3").await);

    let mut lee_phone = s.online("lee", "password-3", "phone").await?;
    let announce =
        run(&mut kim, &d, &format!("{ADMIN}#announce"), &[("subject", &["Maintenance"]), ("announcement", &["Back in 5"])]).await?;
    assert!(note(&announce).contains("Sent to"), "{announce}");
    let m = lee_phone.expect(|e| e.name() == "message").await?;
    assert_eq!(m.child_text("body", ns::CLIENT).as_deref(), Some("Back in 5"));

    run(&mut kim, &d, &format!("{ADMIN}#delete-user"), &[("accountjids", &[&lee])]).await?;
    assert_eq!(stream_error(&mut lee_phone).await, "not-authorized");
    assert!(!s.srv.accounts.exists(&d, "lee").await?);
    Ok(())
}

#[tokio::test]
async fn commands_people_cant_run_have_no_info() -> anyhow::Result<()> {
    let s = TestServer::start().await?;
    s.add_user("sam", "password-1").await?;
    s.add_admin("kim", "password-2").await?;
    let mut sam = s.login("sam", "password-1").await?;
    let mut kim = s.login("kim", "password-2").await?;
    let info = || Element::new("query", ns::DISCO_INFO).attr_set("node", format!("{ADMIN}#add-user"));
    assert_eq!(condition(&sam.iq_get(Some(s.domain()), info()).await?), "item-not-found");
    let r = kim.iq_get(Some(s.domain()), info()).await?;
    assert!(r.get_child("query", ns::DISCO_INFO).is_some_and(|q| q.has_child("identity", ns::DISCO_INFO)), "{r}");
    Ok(())
}

#[tokio::test]
async fn admin_set_passwords_are_kept_as_typed() -> anyhow::Result<()> {
    let s = TestServer::start().await?;
    let d = s.domain().to_string();
    s.add_admin("kim", "password-2").await?;
    let mut kim = s.login("kim", "password-2").await?;
    let lee = format!("lee@{d}");
    run(&mut kim, &d, &format!("{ADMIN}#add-user"), &[("accountjid", &[&lee]), ("password", &[" spaced out "])]).await?;
    assert!(can_sign_in(&s, "lee", " spaced out ").await);
    run(&mut kim, &d, &format!("{ADMIN}#change-user-password"), &[("accountjid", &[&lee]), ("password", &["  new one  "])]).await?;
    assert!(can_sign_in(&s, "lee", "  new one  ").await);
    assert!(!can_sign_in(&s, "lee", "new one").await);
    Ok(())
}

#[tokio::test]
async fn admins_make_account_invites() -> anyhow::Result<()> {
    let s = TestServer::start().await?;
    let d = s.domain().to_string();
    s.add_admin("kim", "password-2").await?;
    let mut kim = s.login("kim", "password-2").await?;
    let done = run(&mut kim, &d, "urn:xmpp:invite#create-account", &[("username", &["max"]), ("admin", &["1"]), ("days", &["2"])]).await?;
    let x = done.to_xml();
    assert!(x.contains(&format!("xmpp:max@{d}?register;preauth=")), "{x}");
    let bad = run(&mut kim, &d, "urn:xmpp:invite#create-account", &[("days", &["0"])]).await?;
    assert!(note(&bad).contains("between"), "{bad}");
    Ok(())
}

#[tokio::test]
async fn failed_admin_commands_say_why_and_admins_choose_any_password() -> anyhow::Result<()> {
    let s = TestServer::start().await?;
    let d = s.domain().to_string();
    s.add_admin("kim", "password-2").await?;
    let mut kim = s.login("kim", "password-2").await?;
    let lee = format!("lee@{d}");
    let add = format!("{ADMIN}#add-user");

    // XEP-0133 leaves password rules to the admin: a short one is their call.
    let r = run(&mut kim, &d, &add, &[("accountjid", &[&lee]), ("password", &["pw"]), ("password-verify", &["pw"])]).await?;
    assert_eq!(r.get_child("note", ns::COMMANDS).and_then(|n| n.attr("type")), Some("info"), "{r}");
    assert!(can_sign_in(&s, "lee", "pw").await);
    run(&mut kim, &d, &format!("{ADMIN}#change-user-password"), &[("accountjid", &[&lee]), ("password", &["x"])]).await?;
    assert!(can_sign_in(&s, "lee", "x").await);

    // A command that can't be done completes with an error note (XEP-0133's examples), which is
    // where admin clients look.
    let error_note = |c: &Element| c.get_child("note", ns::COMMANDS).filter(|n| n.attr("type") == Some("error")).is_some();
    let r = run(&mut kim, &d, &add, &[("accountjid", &[&lee]), ("password", &["password-3"])]).await?;
    assert!(error_note(&r), "already there: {r}");
    let r = run(&mut kim, &d, &add, &[("accountjid", &["max"]), ("password", &["one"]), ("password-verify", &["two"])]).await?;
    assert!(error_note(&r), "mismatched: {r}");
    let r = run(&mut kim, &d, &add, &[("accountjid", &["max"])]).await?;
    assert!(error_note(&r), "an empty password: {r}");
    assert!(!s.srv.accounts.exists(&d, "max").await?);
    let r = run(&mut kim, &d, &format!("{ADMIN}#delete-user"), &[("accountjids", &["ghost"])]).await?;
    assert!(error_note(&r), "{r}");

    // People choosing their own passwords still need 8 characters.
    let mut lee_c = s.login("lee", "x").await?;
    let r = lee_c
        .iq_set(
            Some(&d),
            Element::new("query", ns::REGISTER).text_child("username", ns::REGISTER, "lee").text_child("password", ns::REGISTER, "short"),
        )
        .await?;
    assert_eq!(r.attr("type"), Some("error"), "{r}");
    Ok(())
}

#[tokio::test]
async fn an_account_lists_the_servers_commands_to_itself_and_nothing_to_others() -> anyhow::Result<()> {
    // Smack looks for admin commands on the admin's own bare JID; the items name the domain,
    // where XEP-0133 commands run.
    let s = TestServer::start().await?;
    s.add_user("sam", "password-1").await?;
    s.add_admin("kim", "password-2").await?;
    let mut kim = s.login("kim", "password-2").await?;
    let mut sam = s.login("sam", "password-1").await?;
    let own = command_nodes(&mut kim, "kim@quark.test").await?;
    assert!(own.iter().any(|n| n.starts_with(ADMIN)), "{own:?}");
    assert_eq!(own, command_nodes(&mut kim, s.domain()).await?);
    assert!(command_nodes(&mut sam, "kim@quark.test").await?.is_empty());
    Ok(())
}
