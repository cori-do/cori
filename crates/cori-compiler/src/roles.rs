// SPDX-License-Identifier: Apache-2.0
//! An app's roles: those its cori.toml declares (`[auth] roles`) and those it uses, in its front's and endpoints' allow
//! lists, its stores' role files (`stores/<name>/roles/<role>.yaml`) and its human steps (`to: { role }`). The identity
//! provider, and the organization's IAM behind it, give users an app's roles; a role the app uses without declaring it
//! reaches the IAM with no description, or not at all (`cori check` names it, and a strict server refuses the deploy).
//!
//! Declared roles take a note (the string form: where the IdP takes them from) or a table with a description:
//! ```toml
//! [auth]
//! roles = { member = "group:members" }
//!
//! [auth.roles.manager]
//! description = "Decides team requests over 100 €"
//! note = "group:managers"
//! ```
use crate::Compiled;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What cori.toml says of a declared role.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RoleDecl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An app's roles: declared, and used (role → where).
#[derive(Debug, Clone, Default)]
pub struct Roles {
    pub declared: BTreeMap<String, RoleDecl>,
    pub used: BTreeMap<String, BTreeSet<String>>,
}

/// The read-only role Cori gives every store for the app's owner, builders and admins: no identity carries it.
const INSPECTOR: &str = "cori_inspector";

/// The platform's role names out of the box (`CORI_*_ROLES`, `CORI_APPROVER_ROLES`): `cori check` does not ask an app to
/// declare them. A server knows its own and leaves those out instead.
pub const PLATFORM_DEFAULTS: &[&str] = &["it_admin", "org_admin", "approver", "operator", "auditor", "billing", "builder", "store_admin"];

impl Roles {
    /// The roles of an app from its cori.toml, its compiled workflows and its stores' role files ((store, role) pairs).
    pub fn of(cori_toml: &str, workflows: &[&Compiled], store_roles: &[(String, String)]) -> Self {
        let t: toml::Value = toml::from_str(cori_toml).unwrap_or(toml::Value::Table(Default::default()));
        let mut out = Roles::default();
        if let Some(table) = t.get("auth").and_then(|a| a.get("roles")).and_then(|r| r.as_table()) {
            for (name, v) in table {
                let decl = match v {
                    toml::Value::String(s) => RoleDecl { description: None, note: Some(s.clone()).filter(|s| !s.is_empty()) },
                    toml::Value::Table(t) => RoleDecl {
                        description: t.get("description").and_then(|d| d.as_str()).map(String::from).filter(|s| !s.is_empty()),
                        note: t.get("note").and_then(|d| d.as_str()).map(String::from).filter(|s| !s.is_empty()),
                    },
                    _ => RoleDecl::default(),
                };
                out.declared.insert(name.clone(), decl);
            }
        }
        let strings = |v: Option<&toml::Value>| v.and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<String>>()).unwrap_or_default();
        for r in strings(t.get("front").and_then(|f| f.get("allow")).and_then(|a| a.get("roles"))) { out.use_role(&r, "the front's allow list"); }
        for w in workflows {
            if let Some(a) = w.manifest.endpoint.as_ref().and_then(|e| e.allow.as_ref()).and_then(|a| a.get("roles")).and_then(|r| r.as_array()) {
                for r in a.iter().filter_map(|x| x.as_str()) { out.use_role(r, &format!("workflow `{}`'s endpoint", w.id)); }
            }
            for n in &w.dag.nodes {
                if n.kind != "human" { continue; }
                if let Some(r) = n.literals.get("to").and_then(|t| t.get("role")).and_then(|r| r.as_str()) { out.use_role(r, &format!("workflow `{}`'s step `{}`", w.id, n.name)); }
            }
        }
        for (store, role) in store_roles { if role != INSPECTOR { out.use_role(role, &format!("store `{store}`'s role file")); } }
        out
    }

    fn use_role(&mut self, role: &str, at: &str) { if !role.is_empty() { self.used.entry(role.to_string()).or_default().insert(at.to_string()); } }

    /// The roles used but not declared, with where each is used; `ignore` names the deployment's own roles (its approvers,
    /// its platform roles), which no app declares.
    pub fn undeclared(&self, ignore: &[String]) -> Vec<(String, Vec<String>)> {
        self.used.iter().filter(|(r, _)| !self.declared.contains_key(*r) && !ignore.contains(r)).map(|(r, at)| (r.clone(), at.iter().cloned().collect())).collect()
    }

    /// Every role of the app, declared or used, with what cori.toml says of it (nothing for a role it only uses); `ignore`
    /// as for `undeclared`.
    pub fn all(&self, ignore: &[String]) -> BTreeMap<String, RoleDecl> {
        let mut out = self.declared.clone();
        for r in self.used.keys() { if !ignore.contains(r) { out.entry(r.clone()).or_default(); } }
        out
    }

    /// `cori check`'s words for the roles used but not declared.
    pub fn warnings(&self, ignore: &[String]) -> Vec<String> {
        self.undeclared(ignore).into_iter().map(|(r, at)| format!("role `{r}` is used ({}) but not declared in cori.toml's [auth] roles: declare it, with a description, so that your organization's identity provider knows it", at.join(", "))).collect()
    }
}

/// The roles cori.toml declares, in the order it lists them (`[auth] roles`, either form), else in the order of the
/// front's allow list. A person holding several of an app's roles acts in a store as the first of them in this order,
/// whatever order their token gives: an app lists its most able role first.
pub fn declared_order(cori_toml: &str) -> Vec<String> {
    use toml::de::{DeTable, DeValue};
    let Ok(doc) = DeTable::parse(cori_toml) else { return Vec::new() };
    let doc = doc.get_ref();
    if let Some(DeValue::Table(roles)) = doc.get("auth").and_then(|a| a.get_ref().get("roles")).map(|r| r.get_ref()) {
        // a table's keys come sorted: where each is written gives the order
        let mut keys: Vec<(usize, String)> = roles.iter().map(|(k, _)| (k.span().start, k.get_ref().to_string())).collect();
        keys.sort();
        return keys.into_iter().map(|(_, k)| k).collect();
    }
    match doc.get("front").and_then(|f| f.get_ref().get("allow")).and_then(|a| a.get_ref().get("roles")).map(|r| r.get_ref()) {
        Some(DeValue::Array(a)) => a.iter().filter_map(|x| match x.get_ref() { DeValue::String(s) => Some(s.to_string()), _ => None }).collect(),
        _ => Vec::new(),
    }
}

/// The (store, role) pairs of an app folder's role files (`stores/<name>/roles/<role>.yaml`).
pub fn store_roles(app_root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(stores) = std::fs::read_dir(app_root.join("stores")) else { return out };
    for s in stores.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
        let store = s.file_name().to_string_lossy().to_string();
        let Ok(roles) = std::fs::read_dir(s.path().join("roles")) else { continue };
        for r in roles.filter_map(|e| e.ok()) {
            let p = r.path();
            if p.extension().is_some_and(|x| x == "yaml" || x == "yml") && let Some(stem) = p.file_stem() { out.push((store.clone(), stem.to_string_lossy().to_string())); }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_roles_take_a_note_or_a_description() {
        let toml = "[auth]\nroles = { member = \"group:members\" }\n\n[auth.roles.manager]\ndescription = \"Decides requests\"\nnote = \"group:managers\"\n\n[front]\nallow = { roles = [\"member\", \"viewer\"] }\n";
        // `roles = {…}` and `[auth.roles.manager]` cannot both define the table in TOML: each form on its own
        let inline = Roles::of("[auth]\nroles = { member = \"group:members\" }\n", &[], &[]);
        assert_eq!(inline.declared["member"], RoleDecl { description: None, note: Some("group:members".into()) });
        let tables = Roles::of("[auth.roles.manager]\ndescription = \"Decides requests\"\nnote = \"group:managers\"\n[auth.roles.member]\n", &[], &[]);
        assert_eq!(tables.declared["manager"], RoleDecl { description: Some("Decides requests".into()), note: Some("group:managers".into()) });
        assert_eq!(tables.declared["member"], RoleDecl::default());
        assert!(toml::from_str::<toml::Value>(toml).is_err(), "TOML refuses the mix, so a cori.toml picks one form");
    }

    #[test]
    fn roles_come_in_the_order_cori_toml_lists_them() {
        assert_eq!(declared_order("[auth]\nroles = { manager = \"group:managers\", employee = \"group:employees\" }\n"), ["manager", "employee"]);
        assert_eq!(declared_order("[auth.roles.zeta]\nnote = \"z\"\n[auth.roles.alpha]\ndescription = \"a\"\n"), ["zeta", "alpha"]);
        // no [auth] roles: the front's allow list
        assert_eq!(declared_order("[app]\nid = \"a\"\n[front]\nallow = { roles = [\"lead\", \"agent\"] }\n"), ["lead", "agent"]);
        assert!(declared_order("[app]\nid = \"a\"\n").is_empty() && declared_order("not toml [").is_empty());
    }

    #[test]
    fn roles_used_but_not_declared_are_named_with_where() {
        let r = Roles::of("[auth]\nroles = { member = \"\" }\n[front]\nallow = { roles = [\"member\", \"viewer\"] }\n", &[], &[("requests".into(), "auditor".into()), ("requests".into(), "member".into()), ("requests".into(), INSPECTOR.into())]);
        let u = r.undeclared(&[]);
        assert_eq!(u, vec![("auditor".to_string(), vec!["store `requests`'s role file".to_string()]), ("viewer".to_string(), vec!["the front's allow list".to_string()])]);
        assert!(r.undeclared(&["auditor".to_string(), "viewer".to_string()]).is_empty(), "the deployment's own roles are left out");
        assert_eq!(r.all(&[]).keys().collect::<Vec<_>>(), ["auditor", "member", "viewer"]);
        assert!(r.warnings(&[])[0].contains("role `auditor` is used (store `requests`'s role file)"));
    }
}
