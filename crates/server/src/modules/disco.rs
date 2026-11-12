//! Service discovery (XEP-0030) for the server domain and accounts, with extended info forms
//! (XEP-0128). Every module adds its features; this one only collects them.

use async_trait::async_trait;
use quark_core::stanza::IqType;
use quark_core::{ns, Element, StanzaError};

use super::{ok, Identity, IqCtx, IqResult, IqTarget, Module};
use crate::server::Host;

pub struct Disco;

#[async_trait]
impl Module for Disco {
    fn name(&self) -> &'static str {
        "disco"
    }

    fn snapshot_names(&self, _host: &Host) -> Vec<&'static str> {
        vec!["disco"]
    }

    fn server_features(&self, _host: &Host) -> Vec<&'static str> {
        vec![ns::DISCO_INFO, ns::DISCO_ITEMS]
    }

    fn account_features(&self, _host: &Host) -> Vec<&'static str> {
        vec![ns::DISCO_INFO, ns::DISCO_ITEMS]
    }

    fn iq_namespaces(&self) -> &'static [(&'static str, IqTarget)] {
        &[
            (ns::DISCO_INFO, IqTarget::Server),
            (ns::DISCO_ITEMS, IqTarget::Server),
            (ns::DISCO_INFO, IqTarget::Account),
            (ns::DISCO_ITEMS, IqTarget::Account),
        ]
    }

    async fn handle_iq(&self, cx: &IqCtx<'_>) -> IqResult {
        if cx.kind != IqType::Get {
            return Err(StanzaError::bad_request());
        }
        let node = cx.payload.attr("node");
        let server = cx.to.node().is_none();
        if let Some(n) = node {
            for m in &cx.srv.modules {
                let found = if server { m.server_node(cx, n).await } else { m.account_node(cx, n).await };
                if let Some(children) = found {
                    return ok(Element::new("query", cx.payload.ns()).attr_set("node", n).children_from(children));
                }
            }
            // XEP-0030 §3.1: a node nobody knows.
            return Err(StanzaError::item_not_found());
        }
        if cx.payload.ns() == ns::DISCO_INFO {
            // XEP-0030 §8: only those who may see the account's presence learn about it. A
            // node has its own access model (a PEP node's), checked by whoever answers for it.
            if !server && node.is_none() && !super::presence::may_see(cx.srv, cx.host, cx.to.node().unwrap_or_default(), cx.from).await {
                return Err(StanzaError::service_unavailable());
            }
            return ok(Element::new("query", ns::DISCO_INFO).attr_opt("node", node).children_from(info(cx, server)));
        }
        let items = if server {
            cx.srv.modules.iter().flat_map(|m| m.server_items(cx.host)).collect::<Vec<_>>()
        } else {
            // Only the account and its contacts see what it publishes (enforced by the modules).
            let mut v = Vec::new();
            for m in &cx.srv.modules {
                v.extend(m.account_items(cx).await);
            }
            v
        };
        ok(Element::new("query", ns::DISCO_ITEMS).attr_opt("node", node).children_from(items))
    }
}

/// Identities, features and forms, sorted and without duplicates.
fn info(cx: &IqCtx<'_>, server: bool) -> Vec<Element> {
    let mut identities: Vec<Identity> = Vec::new();
    let mut features: Vec<&'static str> = Vec::new();
    let mut forms: Vec<Element> = Vec::new();
    if server {
        identities.push(Identity { category: "server", kind: "im", name: Some("Quark".into()) });
    } else {
        identities.push(Identity { category: "account", kind: "registered", name: None });
    }
    for m in &cx.srv.modules {
        if server {
            identities.extend(m.server_identities(cx.host));
            features.extend(m.server_features(cx.host));
            forms.extend(m.server_forms(cx.host));
        } else {
            identities.extend(m.account_identities(cx.host));
            features.extend(m.account_features(cx.host));
        }
    }
    features.sort_unstable();
    features.dedup();
    identities.dedup();
    let mut out: Vec<Element> = identities.iter().map(Identity::to_element).collect();
    out.extend(features.into_iter().map(|f| Element::new("feature", ns::DISCO_INFO).attr_set("var", f)));
    out.extend(forms);
    out
}
