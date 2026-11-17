// SPDX-License-Identifier: Apache-2.0
//! Authentication modes: `dev` (headers / dev token), `builtin` (any OIDC issuer, JWKS), `off` (anonymous), plus the
//! Cori-issued **agent tokens**: short-lived, app-scoped, minted by the `login` MCP tool from a human
//! principal, verified locally with a key derived from the master key. Approve and deploy need a human principal.
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
/// `subject_claim`: the claim naming a person (`CORI_AUTH_SUBJECT_CLAIM`, `sub` by default): their id everywhere in Cori.
pub enum AuthMode { Off, Dev, Builtin { issuer: String, audience: Option<String>, roles_claim: String, subject_claim: String } }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PrincipalKind { #[default] Human, Agent, Service }

/// Scopes an agent token carries; a human principal implicitly has every scope.
pub const AGENT_SCOPES: [&str; 4] = ["registry:read", "versions:publish", "runs:preview", "stores:read"];
pub const AGENT_TTL_DEFAULT_SECS: i64 = 3600;
pub const AGENT_TTL_MAX_SECS: i64 = 4 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Principal { pub sub: String, pub roles: Vec<String>, pub email: Option<String>, pub claims: Value, /// the bearer as presented (JWT in builtin mode), kept only to exchange it for connector audiences
    #[serde(skip)] pub token: Option<String>,
    #[serde(default)] pub kind: PrincipalKind,
    /// Agent tokens only: what the token may do, and the one app it is bound to.
    #[serde(default)] pub scopes: Vec<String>,
    #[serde(default)] pub app: Option<String>,
    /// Agent tokens only: the `jti`, checked against `agent_sessions` for revocation.
    #[serde(default)] pub session_id: Option<String>,
    /// Set once the principal is scoped to an app (`in_app`): the platform roles (admin, builder, approver…), while
    /// `roles` then holds the app's roles. Platform checks read these, so an app role can never grant a platform power.
    #[serde(default)] pub platform: Option<Vec<String>> }

impl Principal {
    /// A run's user (`ctx.user`): subject, app roles and email, and of the token's other claims only `claims` (the app's
    /// `[auth] principal_claims`): nothing else of the token reaches the run.
    pub fn to_user_json(&self, claims: &[String]) -> Value {
        let mut v = serde_json::json!({ "sub": self.sub, "roles": self.roles, "email": self.email });
        // a run's user acts with its app roles (`roles`); its platform roles ride along for store administration, except
        // from an agent session, which holds no platform power
        if let Some(p) = &self.platform { v["platform_roles"] = if self.is_agent() { serde_json::json!([]) } else { serde_json::json!(p) }; }
        for k in claims { if v.get(k).is_none() { if let Some(x) = self.claims.get(k) { v[k] = x.clone(); } } }
        v
    }
    pub fn has_any_role(&self, roles: &[String]) -> bool { roles.is_empty() || roles.iter().any(|r| self.roles.contains(r)) }
    /// Holds one of the platform administrator roles (`CORI_ADMIN_ROLES`). No platform power (admin, operator, auditor,
    /// billing, builder, store admin) ever goes through an agent session: its token carries its human's roles for what
    /// they may see and for its own app (`holds_role`), not for the platform.
    pub fn is_admin(&self) -> bool { self.holds(&platform_roles().admin) }
    /// Administrator, or one of `CORI_OPERATOR_ROLES` (LLM bindings, operations).
    pub fn is_operator(&self) -> bool { self.is_admin() || self.holds(&platform_roles().operator) }
    /// Administrator, or one of `CORI_AUDITOR_ROLES` (audit log, usage).
    pub fn is_auditor(&self) -> bool { self.is_admin() || self.holds(&platform_roles().auditor) }
    /// Administrator, auditor, or one of `CORI_BILLING_ROLES` (usage rollups).
    pub fn is_billing(&self) -> bool { self.is_auditor() || self.holds(&platform_roles().billing) }
    /// One of `CORI_BUILDER_ROLES`: may create apps and preview versions.
    pub fn is_builder(&self) -> bool { self.holds(&platform_roles().builder) }
    /// Administrator, or one of `CORI_STORE_ADMIN_ROLES` (direct writes on a store whose role writes through workflows only).
    pub fn is_store_admin(&self) -> bool { self.is_admin() || self.holds(&platform_roles().store_admin) }
    /// One of `CORI_GUEST_ROLES`: someone outside the organization an app was shared with, who reaches the apps shared with
    /// them and nothing else (`api::GUEST_ROUTES`), whatever else they hold.
    pub fn is_guest(&self) -> bool { self.holds_role(&platform_roles().guest) }
    /// A platform power: one of `roles` among the platform roles, never through an agent session.
    fn holds(&self, roles: &[String]) -> bool { !self.is_agent() && self.holds_role(roles) }
    /// One of `roles` among the platform roles, agent session or not: for what the principal may see (connectors restricted
    /// to roles) and for an agent session's rights on its own app, never for a platform power.
    pub fn holds_role(&self, roles: &[String]) -> bool { self.platform_roles().iter().any(|r| roles.contains(r)) }
    /// The platform roles (the issuer's `roles` claim), whether or not the principal is scoped to an app.
    pub fn platform_roles(&self) -> &[String] { self.platform.as_deref().unwrap_or(&self.roles) }
    /// The roles this principal holds in the app whose OIDC client is `client_id`: the client's roles in the token (where
    /// `app_roles_claim` says: Keycloak's `resource_access.<client_id>.roles` by default), the app roles an agent session
    /// was minted with, a portal session's for that client, or, without an issuer (dev and off auth), the token's own roles.
    pub fn app_roles(&self, app: &str, client_id: Option<&str>) -> Vec<String> { self.app_roles_in(app_roles_claim(), app, client_id) }
    /// `app_roles`, an issuer's tokens carrying them where `claim` says.
    pub fn app_roles_in(&self, claim: &AppRolesClaim, app: &str, client_id: Option<&str>) -> Vec<String> {
        if self.is_agent() { return if self.app.as_deref() == Some(app) { strings(&self.claims["app_roles"]) } else { vec![] }; }
        if !self.claims.is_object() { return self.roles.clone(); }
        let Some(client) = client_id else { return vec![] };
        if self.is_portal_session() { return strings(&self.claims["client_roles"][client]); }
        claim.roles(&self.claims, client)
    }
    /// Every app's roles the token holds, by client (`app_roles` of each), the clients holding none left out: what the
    /// people the server has seen hold, and what a portal's session keeps. None when it carries no app roles at all.
    pub fn client_roles(&self) -> Option<BTreeMap<String, Vec<String>>> {
        if self.is_agent() || !self.claims.is_object() { return None; }
        if self.is_portal_session() { return serde_json::from_value(self.claims["client_roles"].clone()).ok(); }
        app_roles_claim().by_client(&self.claims)
    }
    /// An apps portal's session (`api::sessions`): Cori's own token, holding each app's roles by client as the person's
    /// token gave them when they signed in.
    fn is_portal_session(&self) -> bool { !self.is_agent() && self.session_id.is_some() && self.claims["kind"] == "portal" }
    /// This principal inside one app: `roles` become the app roles, the platform roles move to `platform`.
    pub fn in_app(mut self, app: &str, client_id: Option<&str>) -> Principal {
        if self.platform.is_some() { return self; }
        let app_roles = self.app_roles(app, client_id);
        self.platform = Some(std::mem::replace(&mut self.roles, app_roles));
        self
    }
    pub fn is_agent(&self) -> bool { self.kind == PrincipalKind::Agent }
    /// Humans have every scope; an agent token has its list, and only for its app when one is given.
    pub fn has_scope(&self, scope: &str, app: Option<&str>) -> bool {
        if !self.is_agent() { return true; }
        self.scopes.iter().any(|s| s == scope) && app.map(|a| self.app.as_deref() == Some(a)).unwrap_or(true)
    }
}

/// The platform's own roles, mapped from the identity provider's role names (comma-separated environment variables,
/// read once): `CORI_ADMIN_ROLES` (default `it_admin,org_admin`), `CORI_OPERATOR_ROLES` (`operator`),
/// `CORI_AUDITOR_ROLES` (`auditor`), `CORI_BILLING_ROLES` (`billing`), `CORI_BUILDER_ROLES` (`builder`),
/// `CORI_STORE_ADMIN_ROLES` (`store_admin`), `CORI_GUEST_ROLES` (`guest`: the role the identity provider gives outsiders an
/// app was shared with). Approvers are `CORI_APPROVER_ROLES` (server configuration).
#[derive(Debug, Clone)]
pub struct PlatformRoles { pub admin: Vec<String>, pub operator: Vec<String>, pub auditor: Vec<String>, pub billing: Vec<String>, pub builder: Vec<String>, pub store_admin: Vec<String>, pub guest: Vec<String> }

pub fn platform_roles() -> &'static PlatformRoles {
    static ROLES: std::sync::OnceLock<PlatformRoles> = std::sync::OnceLock::new();
    ROLES.get_or_init(|| {
        let list = |var: &str, default: &str| -> Vec<String> {
            std::env::var(var).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string()).split(',').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect()
        };
        PlatformRoles { admin: list("CORI_ADMIN_ROLES", "it_admin,org_admin"), operator: list("CORI_OPERATOR_ROLES", "operator"), auditor: list("CORI_AUDITOR_ROLES", "auditor"), billing: list("CORI_BILLING_ROLES", "billing"), builder: list("CORI_BUILDER_ROLES", "builder"), store_admin: list("CORI_STORE_ADMIN_ROLES", "store_admin"), guest: list("CORI_GUEST_ROLES", "guest") }
    })
}

/// Where an issuer's tokens carry each app's roles, which are the roles of the app's sign-in client (`client_id`, what the
/// client hook answered), in one of two shapes (`CORI_AUTH_APP_ROLES_CLAIM`, with `CORI_AUTH_APP_ROLES_PREFIX` for the
/// second):
/// - a claim per client, a dotted path naming the client `{client_id}`: `resource_access.{client_id}.roles`, Keycloak's
///   client roles (the default);
/// - one claim listing every app's roles, each named after its app's client: the prefix with `{client_id}` filled in, then
///   the role. `roles` with `{client_id}:` (app roles defined on the platform's own client, as Entra ID issues them),
///   `groups` with `cori-{client_id}-` (groups). An entry belongs to the client before the first separator (what follows
///   `{client_id}` in the prefix), so a client id holding the separator holds no role.
///
/// Platform roles keep their own claim (`CORI_AUTH_ROLES_CLAIM`), even when it is the same one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRolesClaim {
    /// The path to the clients (`resource_access`), and from a client to its roles (`roles`; empty, the client's own value).
    PerClient { clients: String, roles: String },
    /// The claim listing them, and the prefix around `{client_id}`: `before` (may be empty) and the separator `after`.
    Listed { claim: String, before: String, after: String },
}

const CLIENT_ID: &str = "{client_id}";

impl Default for AppRolesClaim {
    fn default() -> Self { AppRolesClaim::PerClient { clients: "resource_access".into(), roles: "roles".into() } }
}

impl std::fmt::Display for AppRolesClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppRolesClaim::PerClient { clients, roles } if roles.is_empty() => write!(f, "{clients}.<client_id>"),
            AppRolesClaim::PerClient { clients, roles } => write!(f, "{clients}.<client_id>.{roles}"),
            AppRolesClaim::Listed { claim, before, after } => write!(f, "{claim}, as {before}<client_id>{after}<role>"),
        }
    }
}

impl AppRolesClaim {
    /// The settings as written: the claim, and the prefix when that claim lists every app's roles.
    pub fn parse(claim: &str, prefix: Option<&str>) -> Result<Self, String> {
        let (claim, prefix) = (claim.trim(), prefix.map(str::trim).filter(|p| !p.is_empty()));
        match (claim.split_once(CLIENT_ID), prefix) {
            (Some((path, rest)), None) => {
                let clients = path.strip_suffix('.').filter(|p| !p.is_empty() && !p.ends_with('.'));
                let roles = if rest.is_empty() { Some("") } else { rest.strip_prefix('.').filter(|r| !r.is_empty() && !r.contains(CLIENT_ID)) };
                match (clients, roles) {
                    (Some(clients), Some(roles)) => Ok(AppRolesClaim::PerClient { clients: clients.into(), roles: roles.into() }),
                    _ => Err(format!("CORI_AUTH_APP_ROLES_CLAIM `{claim}`: `{{client_id}}` is one name of the dotted path, after the claim's own (`resource_access.{{client_id}}.roles`)")),
                }
            }
            (Some(_), Some(_)) => Err(format!("CORI_AUTH_APP_ROLES_PREFIX goes with a claim listing every app's roles, and CORI_AUTH_APP_ROLES_CLAIM `{claim}` names one per client: drop one")),
            (None, Some(p)) => match p.split_once(CLIENT_ID) {
                Some((before, after)) if !after.is_empty() && !after.contains(CLIENT_ID) && !claim.is_empty() => Ok(AppRolesClaim::Listed { claim: claim.into(), before: before.into(), after: after.into() }),
                _ => Err(format!("CORI_AUTH_APP_ROLES_PREFIX `{p}`: the app's client once, `{{client_id}}`, then a separator no client id holds (`{{client_id}}:`)")),
            },
            (None, None) => Err(format!("CORI_AUTH_APP_ROLES_CLAIM `{claim}` lists every app's roles: set CORI_AUTH_APP_ROLES_PREFIX, how an app's are named in it (`{{client_id}}:`), or name the client in the path (`resource_access.{{client_id}}.roles`)")),
        }
    }

    /// `CORI_AUTH_APP_ROLES_CLAIM` and `CORI_AUTH_APP_ROLES_PREFIX`; neither set, Keycloak's client roles.
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        match (var("CORI_AUTH_APP_ROLES_CLAIM"), var("CORI_AUTH_APP_ROLES_PREFIX")) {
            (Some(claim), prefix) => Self::parse(&claim, prefix.as_deref()),
            (None, Some(_)) => Err("CORI_AUTH_APP_ROLES_PREFIX is set without CORI_AUTH_APP_ROLES_CLAIM, the claim listing every app's roles".into()),
            (None, None) => Ok(Self::default()),
        }
    }

    /// The roles `claims` give in the app whose client is `client_id`.
    pub fn roles(&self, claims: &Value, client_id: &str) -> Vec<String> {
        match self {
            AppRolesClaim::PerClient { clients, roles } => lookup(claims, clients).and_then(|c| c.get(client_id)).and_then(|c| if roles.is_empty() { Some(c) } else { lookup(c, roles) }).map(strings).unwrap_or_default(),
            AppRolesClaim::Listed { .. } => self.listed(claims).into_iter().filter(|(c, _)| *c == client_id).map(|(_, r)| r.to_string()).collect(),
        }
    }

    /// Every client's roles in `claims` (`roles` of each), the clients holding none left out. None when the claim is not
    /// there at all (a token that says nothing of app roles).
    pub fn by_client(&self, claims: &Value) -> Option<BTreeMap<String, Vec<String>>> {
        match self {
            AppRolesClaim::PerClient { clients, roles } => Some(lookup(claims, clients)?.as_object()?.iter()
                .map(|(c, v)| (c.clone(), if roles.is_empty() { strings(v) } else { lookup(v, roles).map(strings).unwrap_or_default() }))
                .filter(|(_, r)| !r.is_empty()).collect()),
            AppRolesClaim::Listed { claim, .. } => {
                lookup(claims, claim)?.as_array()?;
                let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for (c, r) in self.listed(claims) { out.entry(c.to_string()).or_default().push(r.to_string()); }
                Some(out)
            }
        }
    }

    /// Why a client named `client_id` would hold no role here: it holds the separator of a listed claim.
    pub fn unreadable(&self, client_id: &str) -> Option<String> {
        match self {
            AppRolesClaim::Listed { claim, after, .. } if client_id.contains(after.as_str()) => Some(format!("client id `{client_id}` holds `{after}`, the separator of the app roles listed in `{claim}` (CORI_AUTH_APP_ROLES_PREFIX): nobody holds a role in its app")),
            _ => None,
        }
    }

    /// The (client, role) pairs of a listed claim: each entry with the prefix, split at the first separator after it.
    fn listed<'a>(&self, claims: &'a Value) -> Vec<(&'a str, &'a str)> {
        let AppRolesClaim::Listed { claim, before, after } = self else { return vec![] };
        lookup(claims, claim).and_then(Value::as_array).into_iter().flatten()
            .filter_map(|e| e.as_str()?.strip_prefix(before.as_str())?.split_once(after.as_str()))
            .filter(|(c, r)| !c.is_empty() && !r.is_empty()).collect()
    }
}

/// Where the issuer's tokens carry each app's roles (`AppRolesClaim::from_env`, read once). `serve` refuses settings that
/// do not parse; anything else reading them gets Keycloak's client roles.
pub fn app_roles_claim() -> &'static AppRolesClaim {
    static CLAIM: std::sync::OnceLock<AppRolesClaim> = std::sync::OnceLock::new();
    CLAIM.get_or_init(|| AppRolesClaim::from_env().unwrap_or_default())
}

/// The strings of a JSON list (anything else: none).
fn strings(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() }

#[derive(Default)]
pub struct JwksCache { keys: RwLock<Option<(std::time::Instant, jsonwebtoken::jwk::JwkSet)>>, discovery: RwLock<Option<(std::time::Instant, Value)>> }

/// Verifies bearers and talks to the issuer. Cheap to clone: a realm's (`AppState::in_realm`) shares the issuer's keys and
/// signs its agent tokens with its own key.
#[derive(Clone)]
pub struct Authenticator { pub mode: AuthMode, jwks: Arc<JwksCache>, http: reqwest::Client, agent_key: [u8; 32], session_key: [u8; 32], platform_client: (String, Option<String>), mcp_audience: Option<String> }

/// Signature algorithms accepted from the issuer: asymmetric only, so a key published in the JWKS can never be used as
/// an HMAC secret.
const ISSUER_ALGS: [jsonwebtoken::Algorithm; 8] = [
    jsonwebtoken::Algorithm::RS256, jsonwebtoken::Algorithm::RS384, jsonwebtoken::Algorithm::RS512,
    jsonwebtoken::Algorithm::PS256, jsonwebtoken::Algorithm::PS384, jsonwebtoken::Algorithm::PS512,
    jsonwebtoken::Algorithm::ES256, jsonwebtoken::Algorithm::ES384,
];
/// How long the issuer's keys and discovery document are reused; an unknown key id refetches sooner (key rotation).
const JWKS_TTL: std::time::Duration = std::time::Duration::from_secs(600);
const JWKS_MIN_REFRESH: std::time::Duration = std::time::Duration::from_secs(30);

/// Claims of a Cori agent token (HS256 with the derived key; `kid` = `cori-agent`). Under a tenancy, `realm` is the realm
/// it was minted in, whose key signs it.
#[derive(Debug, Serialize, Deserialize)]
struct AgentClaims { iss: String, sub: String, jti: String, iat: i64, exp: i64, kind: String, app: String, scopes: Vec<String>, roles: Vec<String>, #[serde(default)] app_roles: Vec<String>, email: Option<String>, #[serde(default)] mcp_session: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] realm: Option<String> }

/// Whether a bearer is a Cori agent token (its key id), whatever the auth mode.
pub fn is_agent_token(token: &str) -> bool { jsonwebtoken::decode_header(token).ok().and_then(|h| h.kid).as_deref() == Some("cori-agent") }

/// The realm an agent token names, read without verifying it: it only says which realm's key verifies the token.
pub fn agent_token_realm(token: &str) -> Option<crate::db::Realm> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload.trim_end_matches('=')).ok()?;
    crate::db::Realm::parse(serde_json::from_slice::<Value>(&bytes).ok()?.get("realm")?.as_str()?)
}

pub struct MintedToken { pub token: String, pub jti: String, pub expires_at: String, pub scopes: Vec<String> }

/// A session a front's own host signs a person in with, where the identity provider is not asked (`api::sessions`): Cori's
/// own token (HS256 under a key derived for the realm, `kid` `cori-session`), heard on that host only (`aud`, its origin).
/// A release's (`kind` `preview`) holds the app, the release's hash and the person's roles in the app; it carries no
/// platform role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    pub iss: String, pub kind: String, pub sub: String, pub aud: String, pub iat: i64, pub exp: i64, pub jti: String,
    #[serde(default)] pub email: Option<String>,
    #[serde(default)] pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub realm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub version: Option<String>,
    #[serde(default)] pub app_roles: Vec<String>,
    /// An apps portal's (`kind` `portal`): the person's platform roles and each app's roles by client (as their console
    /// token had them, `Principal::client_roles`), so that it lists the apps the console would. It reaches the person and
    /// their apps only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")] pub client_roles: BTreeMap<String, Vec<String>>,
}

/// The key id of a front's session token (`SessionClaims`).
pub const SESSION_KID: &str = "cori-session";

/// Whether a bearer is a session of a front's own host (its key id), whatever the auth mode.
pub fn is_session_token(token: &str) -> bool { jsonwebtoken::decode_header(token).ok().and_then(|h| h.kid).as_deref() == Some(SESSION_KID) }

/// The claims of a session token, read without verifying it: which realm's key verifies it, and the host it is for.
pub fn session_token_claims(token: &str) -> Option<SessionClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

impl Authenticator {
    pub fn new(mode: AuthMode) -> Self {
        let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(10)).connect_timeout(std::time::Duration::from_secs(5)).build().unwrap_or_default();
        Self { mode, jwks: Arc::new(JwksCache::default()), http, agent_key: [0; 32], session_key: [0; 32], platform_client: ("cori-platform".into(), None), mcp_audience: None }
    }
    pub fn with_agent_key(mut self, key: [u8; 32]) -> Self { self.agent_key = key; self }
    /// The key front sessions are signed with (`SessionClaims`), the realm's.
    pub fn with_session_key(mut self, key: [u8; 32]) -> Self { self.session_key = key; self }

    /// Signs a front's session (`SessionClaims`).
    pub fn mint_session(&self, claims: &SessionClaims) -> Result<String, String> {
        if self.session_key.iter().all(|b| *b == 0) { return Err("front sessions are not enabled on this server (no signing key)".into()); }
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256); header.kid = Some(SESSION_KID.into());
        jsonwebtoken::encode(&header, claims, &jsonwebtoken::EncodingKey::from_secret(&self.session_key)).map_err(|e| e.to_string())
    }

    /// A front's session as a principal: a release's, its person with their roles in its app as `roles` and no platform
    /// role (`platform` empty: the app scoping is done).
    fn verify_session(&self, token: &str) -> Result<Principal, String> {
        if self.session_key.iter().all(|b| *b == 0) { return Err("front sessions are not enabled on this server".into()); }
        let mut val = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        val.set_issuer(&["cori"]); val.validate_aud = false; val.set_required_spec_claims(&["exp", "sub", "iss"]);
        let c = jsonwebtoken::decode::<SessionClaims>(token, &jsonwebtoken::DecodingKey::from_secret(&self.session_key), &val).map_err(|e| format!("invalid session token: {e}"))?.claims;
        if c.kind == "portal" {
            // the person in their organization: their platform roles, and each app's roles by client (`app_roles`)
            let claims = json!({ "kind": c.kind, "aud": c.aud, "iat": c.iat, "email": c.email, "name": c.name, "realm": c.realm, "roles": c.roles, "client_roles": c.client_roles });
            return Ok(Principal { sub: c.sub, roles: c.roles, email: c.email, claims, token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: Some(c.jti), platform: None });
        }
        let claims = json!({ "kind": c.kind, "aud": c.aud, "iat": c.iat, "email": c.email, "name": c.name, "realm": c.realm, "app": c.app, "version": c.version });
        Ok(Principal { sub: c.sub, roles: c.app_roles, email: c.email, claims, token: None, kind: PrincipalKind::Human, scopes: vec![], app: c.app, session_id: Some(c.jti), platform: Some(vec![]) })
    }
    /// The client the platform's own token requests use (`ServerConfig::auth_client_id`, `auth_client_secret`).
    pub fn with_platform_client(mut self, id: String, secret: Option<String>) -> Self { self.platform_client = (id, secret); self }
    /// The resource the MCP endpoint's metadata names (`<public url>/mcp`): an MCP host asks the issuer for a token of that
    /// audience (RFC 8707), and `/mcp` accepts it besides the configured audience. The rest of the API does not.
    pub fn with_mcp_audience(mut self, resource: String) -> Self { self.mcp_audience = Some(resource); self }

    /// The caller of an API route: an issuer's token must carry the configured audience (when one is configured).
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, String> { self.authenticate_for(headers, false).await }

    /// The person a bearer names, read without verifying it (a dev token, or an issuer's JWT): to narrow what the bearer
    /// reaches before the handler verifies it (`api::bearer_guard`), never to grant anything. None for anything else.
    pub fn unverified(&self, token: &str) -> Option<Principal> {
        let human = |sub: String, roles: Vec<String>, email: Option<String>, claims: Value| Principal { sub, roles, email, claims, token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        match &self.mode {
            AuthMode::Off => None,
            AuthMode::Dev => {
                let mut parts = token.strip_prefix("dev:")?.splitn(2, ':');
                let sub = parts.next().unwrap_or("dev").to_string();
                Some(human(sub, parts.next().unwrap_or("").split(',').filter(|s| !s.is_empty()).map(String::from).collect(), None, Value::Null))
            }
            AuthMode::Builtin { roles_claim, subject_claim, .. } => {
                use base64::Engine;
                let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?.trim_end_matches('=')).ok()?;
                let claims: Value = serde_json::from_slice(&payload).ok()?;
                let roles = lookup(&claims, roles_claim).and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().filter_map(|r| r.as_str().map(String::from)).collect();
                Some(human(subject(&claims, subject_claim)?.to_string(), roles, None, claims))
            }
        }
    }

    /// The caller of a route, `mcp` for the MCP endpoint, which also accepts tokens issued for its own resource.
    pub async fn authenticate_for(&self, headers: &HeaderMap, mcp: bool) -> Result<Principal, String> {
        let bearer = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(|s| s.trim().to_string());
        // a Cori agent token is recognised by its key id, whatever the auth mode; so is a front's session
        if let Some(t) = bearer.as_deref() { if is_agent_token(t) { return self.verify_agent(t); } }
        if let Some(t) = bearer.as_deref() { if is_session_token(t) { return self.verify_session(t); } }
        match &self.mode {
            AuthMode::Off => Ok(Principal { sub: "anonymous".into(), roles: vec![], email: None, claims: Value::Null, token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None }),
            AuthMode::Dev => {
                // `Authorization: Bearer dev:<sub>:<role,role>` or headers X-Cori-User / X-Cori-Roles
                if let Some(t) = bearer.as_deref().and_then(|t| t.strip_prefix("dev:")) {
                    let mut parts = t.splitn(2, ':');
                    let sub = parts.next().unwrap_or("dev").to_string();
                    let roles: Vec<String> = parts.next().unwrap_or("").split(',').filter(|s| !s.is_empty()).map(String::from).collect();
                    return Ok(Principal { email: Some(format!("{sub}@local")), sub, roles, claims: Value::Null, token: bearer.clone(), kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None });
                }
                // a hosted front that sends no token at all: the identity chosen in the dev widget travels as the `cori_dev` cookie
                if headers.get("x-cori-user").is_none() {
                    if let Some(t) = dev_cookie(headers) {
                        let mut parts = t.splitn(2, ':');
                        let sub = parts.next().unwrap_or("dev").to_string();
                        let roles: Vec<String> = parts.next().unwrap_or("").split(',').filter(|s| !s.is_empty()).map(String::from).collect();
                        return Ok(Principal { email: Some(format!("{sub}@local")), sub, roles, claims: Value::Null, token: Some(format!("dev:{t}")), kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None });
                    }
                }
                let sub = headers.get("x-cori-user").and_then(|v| v.to_str().ok()).unwrap_or("dev").to_string();
                let roles: Vec<String> = headers.get("x-cori-roles").and_then(|v| v.to_str().ok()).unwrap_or("").split(',').filter(|s| !s.is_empty()).map(String::from).collect();
                Ok(Principal { email: Some(format!("{sub}@local")), sub, roles, claims: Value::Null, token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None })
            }
            AuthMode::Builtin { issuer, audience, roles_claim, subject_claim } => {
                let token = bearer.ok_or("missing bearer token")?;
                let mut audiences: Vec<&str> = audience.as_deref().into_iter().collect();
                if mcp && !audiences.is_empty() { audiences.extend(self.mcp_audience.as_deref()); }
                let claims = self.verify_issuer_token(issuer, &audiences, &token).await?;
                let sub = subject(&claims, subject_claim).ok_or_else(|| format!("token has no subject (`{subject_claim}`)"))?.to_string();
                let roles = lookup(&claims, roles_claim).and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().filter_map(|r| r.as_str().map(String::from)).collect();
                let email = claims.get("email").and_then(|s| s.as_str()).map(String::from);
                Ok(Principal { sub, roles, email, claims, token: Some(token), kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None })
            }
        }
    }

    /// Mints an agent token for `who` (a human principal) bound to `app`: the human's roles minus nothing (endpoint
    /// `allow` lists still apply to preview runs), their roles in the app (`app_roles`, as `api::app_roles_of` finds them),
    /// the fixed agent scopes, `ttl` seconds (capped), and the realm it acts in
    /// (`realm`, under a tenancy; this authenticator then holds that realm's key). The caller records the `jti` in
    /// `agent_sessions` so it can be revoked.
    pub fn mint_agent(&self, who: &Principal, app: &str, app_roles: Vec<String>, ttl_secs: i64, mcp_session: Option<String>, realm: Option<&crate::db::Realm>) -> Result<MintedToken, String> {
        if who.is_agent() { return Err("an agent session cannot mint another one; the `login` tool needs the human's OAuth session".into()); }
        if self.agent_key.iter().all(|b| *b == 0) { return Err("agent tokens are not enabled on this server (no signing key)".into()); }
        let ttl = ttl_secs.clamp(60, AGENT_TTL_MAX_SECS);
        let now = chrono::Utc::now();
        let exp = now + chrono::Duration::seconds(ttl);
        let jti = uuid::Uuid::new_v4().to_string();
        let scopes: Vec<String> = AGENT_SCOPES.iter().map(|s| s.to_string()).collect();
        let claims = AgentClaims { iss: "cori".into(), sub: who.sub.clone(), jti: jti.clone(), iat: now.timestamp(), exp: exp.timestamp(), kind: "agent".into(), app: app.to_string(), scopes: scopes.clone(), roles: who.platform_roles().to_vec(), app_roles, email: who.email.clone(), mcp_session, realm: realm.map(|r| r.as_str().to_string()) };
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256); header.kid = Some("cori-agent".into());
        let token = jsonwebtoken::encode(&header, &claims, &jsonwebtoken::EncodingKey::from_secret(&self.agent_key)).map_err(|e| e.to_string())?;
        Ok(MintedToken { token, jti, expires_at: exp.to_rfc3339(), scopes })
    }

    fn verify_agent(&self, token: &str) -> Result<Principal, String> {
        if self.agent_key.iter().all(|b| *b == 0) { return Err("agent tokens are not enabled on this server".into()); }
        let mut val = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        val.set_issuer(&["cori"]); val.validate_aud = false; val.set_required_spec_claims(&["exp", "sub", "iss"]);
        let data = jsonwebtoken::decode::<AgentClaims>(token, &jsonwebtoken::DecodingKey::from_secret(&self.agent_key), &val).map_err(|e| format!("invalid agent token: {e}"))?;
        let c = data.claims;
        Ok(Principal { sub: c.sub, roles: c.roles, email: c.email, claims: json!({ "kind": "agent", "app": c.app, "app_roles": c.app_roles, "mcp_session": c.mcp_session, "realm": c.realm }), token: None, kind: PrincipalKind::Agent, scopes: c.scopes, app: Some(c.app), session_id: Some(c.jti), platform: None })
    }

    /// Verifies a token of the issuer: a known key (the JWKS is refetched once for an unknown key id), an asymmetric
    /// algorithm matching the key, the issuer, one of `audiences` (none given: no audience is checked), and the expiry.
    /// Returns the claims.
    pub async fn verify_issuer_token(&self, issuer: &str, audiences: &[&str], token: &str) -> Result<Value, String> {
        let header = jsonwebtoken::decode_header(token).map_err(|e| format!("bad token header: {e}"))?;
        if !ISSUER_ALGS.contains(&header.alg) { return Err(format!("token algorithm {:?} is not accepted (asymmetric signatures only)", header.alg)); }
        let kid = header.kid.clone().ok_or("token has no kid")?;
        let mut jwks = self.jwks(issuer, false).await?;
        if jwks.find(&kid).is_none() { jwks = self.jwks(issuer, true).await?; }
        let jwk = jwks.find(&kid).ok_or("unknown key id")?;
        if let Some(alg) = jwk.common.key_algorithm { if format!("{alg:?}") != format!("{:?}", header.alg) { return Err(format!("token algorithm {:?} does not match its key ({alg:?})", header.alg)); } }
        let key = jsonwebtoken::DecodingKey::from_jwk(jwk).map_err(|e| format!("jwk: {e}"))?;
        let mut val = jsonwebtoken::Validation::new(header.alg);
        val.set_issuer(&[issuer]);
        val.set_required_spec_claims(&["exp", "iss", "sub"]);
        val.leeway = 30;
        if audiences.is_empty() { val.validate_aud = false; } else { val.set_audience(audiences); }
        let data = jsonwebtoken::decode::<Value>(token, &key, &val).map_err(|e| format!("invalid token: {e}"))?;
        Ok(data.claims)
    }

    /// The issuer's OIDC discovery document, cached like the keys.
    pub async fn discovery(&self) -> Result<Value, String> {
        let AuthMode::Builtin { issuer, .. } = &self.mode else { return Err("no OIDC issuer (builtin auth only)".into()) };
        if let Some((at, doc)) = self.jwks.discovery.read().await.as_ref() { if at.elapsed() < JWKS_TTL { return Ok(doc.clone()); } }
        let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
        let doc: Value = self.http.get(&url).send().await.map_err(|e| format!("issuer discovery: {e}"))?.error_for_status().map_err(|e| format!("issuer discovery: {e}"))?.json().await.map_err(|e| format!("issuer discovery: {e}"))?;
        *self.jwks.discovery.write().await = Some((std::time::Instant::now(), doc.clone()));
        Ok(doc)
    }

    /// A request to the issuer's token endpoint as the platform client (`with_platform_client`), or the given client.
    /// Returns the JSON answer or the issuer's error.
    pub async fn token_request(&self, mut form: Vec<(&'static str, String)>, client: Option<(&str, &str)>) -> Result<Value, String> {
        let te = self.discovery().await?.get("token_endpoint").and_then(|u| u.as_str()).ok_or("issuer has no token_endpoint")?.to_string();
        match client {
            Some((id, secret)) => { form.push(("client_id", id.to_string())); form.push(("client_secret", secret.to_string())); }
            None => {
                form.push(("client_id", self.platform_client.0.clone()));
                if let Some(secret) = &self.platform_client.1 { form.push(("client_secret", secret.clone())); }
            }
        }
        let res = self.http.post(&te).form(&form).send().await.map_err(|e| e.to_string())?;
        let status = res.status(); let v: Value = res.json().await.unwrap_or(Value::Null);
        if !status.is_success() { return Err(v.get("error_description").or(v.get("error")).and_then(|e| e.as_str()).unwrap_or("refused").to_string()); }
        Ok(v)
    }

    /// RFC 8693 token exchange at the issuer: the run's JWT → a token for `audience` (an MCP server, a store, an activity connector).
    pub async fn exchange(&self, subject_token: &str, audience: &str, _client_id: &str) -> Result<String, String> {
        if !matches!(self.mode, AuthMode::Builtin { .. }) { return Err("token exchange needs builtin/external auth".into()); }
        let form = vec![("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange".to_string()), ("subject_token", subject_token.to_string()), ("subject_token_type", "urn:ietf:params:oauth:token-type:access_token".to_string()), ("audience", audience.to_string())];
        let v = self.token_request(form, None).await.map_err(|e| format!("token exchange for `{audience}` refused: {e}"))?;
        v.get("access_token").and_then(|t| t.as_str()).map(String::from).ok_or("no access_token in exchange response".into())
    }

    /// Exchanges the caller's token for a refresh token of the platform client (Keycloak ≥ 26.2 standard token exchange
    /// with "Allow refresh token in Standard Token Exchange"), so a run can obtain fresh access tokens after the caller's
    /// has expired. `offline` asks for `offline_access` (runs that outlive the SSO session). Returns (access, refresh).
    pub async fn exchange_for_refresh(&self, subject_token: &str, offline: bool) -> Result<(String, String), String> {
        let mut form = vec![("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange".to_string()), ("subject_token", subject_token.to_string()), ("subject_token_type", "urn:ietf:params:oauth:token-type:access_token".to_string()), ("requested_token_type", "urn:ietf:params:oauth:token-type:refresh_token".to_string())];
        if offline { form.push(("scope", "openid offline_access".to_string())); }
        let v = self.token_request(form, None).await?;
        let access = v.get("access_token").and_then(|t| t.as_str()).ok_or("no access_token")?.to_string();
        let refresh = v.get("refresh_token").and_then(|t| t.as_str()).ok_or("the issuer returned no refresh token (enable refresh tokens for standard token exchange on the platform client)")?.to_string();
        Ok((access, refresh))
    }

    /// A refresh grant as the platform client: (new access token, the rotated refresh token if the issuer sent one).
    pub async fn refresh(&self, refresh_token: &str) -> Result<(String, Option<String>), String> {
        let v = self.token_request(vec![("grant_type", "refresh_token".to_string()), ("refresh_token", refresh_token.to_string())], None).await?;
        let access = v.get("access_token").and_then(|t| t.as_str()).ok_or("no access_token in refresh response")?.to_string();
        Ok((access, v.get("refresh_token").and_then(|t| t.as_str()).map(String::from)))
    }

    /// A client credentials grant: the service account token of `client_id` (scheduled runs).
    pub async fn client_credentials(&self, client_id: &str, secret: &str) -> Result<String, String> {
        let v = self.token_request(vec![("grant_type", "client_credentials".to_string())], Some((client_id, secret))).await?;
        v.get("access_token").and_then(|t| t.as_str()).map(String::from).ok_or("no access_token in client credentials response".into())
    }

    async fn jwks(&self, issuer: &str, force: bool) -> Result<jsonwebtoken::jwk::JwkSet, String> {
        if let Some((at, set)) = self.jwks.keys.read().await.as_ref() {
            if at.elapsed() < JWKS_TTL && (!force || at.elapsed() < JWKS_MIN_REFRESH) { return Ok(set.clone()); }
        }
        let cfg: Value = self.http.get(format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'))).send().await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
        let uri = cfg.get("jwks_uri").and_then(|u| u.as_str()).ok_or("issuer has no jwks_uri")?.to_string();
        let set: jsonwebtoken::jwk::JwkSet = self.http.get(&uri).send().await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
        *self.jwks.keys.write().await = Some((std::time::Instant::now(), set.clone()));
        Ok(set)
    }
}

/// The `exp` of a JWT, read without verification (the token came from the issuer through the server itself).
pub fn jwt_exp(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()?.get("exp")?.as_i64()
}

/// `cori_dev=<percent-encoded dev:<sub>:<roles>>` from the Cookie header (set by the dev widget); the part after `dev:`.
fn dev_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get_all("cookie").iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).map(str::trim).find_map(|c| c.strip_prefix("cori_dev="))?;
    let decoded = percent_decode(raw);
    decoded.strip_prefix("dev:").map(String::from)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes(); let mut out = Vec::with_capacity(b.len()); let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() { if let Ok(x) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) { out.push(x); i += 3; continue; } }
        out.push(b[i]); i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The person a token names: the string at `claim` (a dotted path, as `lookup`), never empty.
pub(crate) fn subject<'a>(claims: &'a Value, claim: &str) -> Option<&'a str> {
    lookup(claims, claim).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

/// The value at a dotted path of the claims (`realm_access.roles`). A claim whose name holds dots is found as it is
/// (`https://acme.example/roles`, as Auth0 namespaces custom claims).
pub(crate) fn lookup<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    if let Some(found) = v.get(path) { return Some(found); }
    path.match_indices('.').find_map(|(i, _)| lookup(v.get(&path[..i])?, &path[i + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dev_cookie_identifies_a_front_that_sends_no_token() {
        let a = Authenticator::new(AuthMode::Dev);
        let mut h = HeaderMap::new(); h.insert("cookie", "other=1; cori_dev=dev%3Ajean%3Amember%2Cadmin".parse().unwrap());
        let p = a.authenticate(&h).await.unwrap();
        assert_eq!(p.sub, "jean"); assert_eq!(p.roles, vec!["member".to_string(), "admin".to_string()]);
        // a bearer wins over the cookie; an explicit header too
        h.insert("authorization", "Bearer dev:sam:lead".parse().unwrap());
        assert_eq!(a.authenticate(&h).await.unwrap().sub, "sam");
        let mut h2 = HeaderMap::new(); h2.insert("cookie", "cori_dev=dev%3Ajean%3Amember".parse().unwrap()); h2.insert("x-cori-user", "bob".parse().unwrap());
        assert_eq!(a.authenticate(&h2).await.unwrap().sub, "bob");
        assert_eq!(percent_decode("a%2Cb%3A"), "a,b:"); assert_eq!(percent_decode("%2"), "%2");
    }
    #[tokio::test]
    async fn the_mcp_endpoint_alone_accepts_tokens_of_its_own_resource() {
        use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};
        // an issuer: its discovery document and its keys
        let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let enc = jsonwebtoken::EncodingKey::from_rsa_pem(key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap().as_bytes()).unwrap();
        let b64 = |b: &[u8]| base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, b);
        let jwks = json!({ "keys": [{ "kty": "RSA", "use": "sig", "alg": "RS256", "kid": "k1", "n": b64(&key.n().to_bytes_be()), "e": b64(&key.e().to_bytes_be()) }] });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let discovery = json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/certs") });
        let app = axum::Router::new()
            .route("/.well-known/openid-configuration", axum::routing::get(move || async move { axum::Json(discovery) }))
            .route("/certs", axum::routing::get(move || async move { axum::Json(jwks) }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let token = |aud: Value| {
            let mut h = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256); h.kid = Some("k1".into());
            let claims = json!({ "iss": issuer, "sub": "jean", "oid": "0b7e-oid", "ext": { "employee_id": "E42" }, "aud": aud, "exp": chrono::Utc::now().timestamp() + 600, "roles": ["builder"] });
            let mut headers = HeaderMap::new();
            headers.insert("authorization", format!("Bearer {}", jsonwebtoken::encode(&h, &claims, &enc).unwrap()).parse().unwrap());
            headers
        };
        let server = |audience: Option<&str>| Authenticator::new(AuthMode::Builtin { issuer: issuer.clone(), audience: audience.map(String::from), roles_claim: "roles".into(), subject_claim: "sub".into() })
            .with_mcp_audience("https://cori.example/mcp".into());
        let a = server(Some("https://cori.example"));
        // the platform's audience: every route
        let platform = token(json!("https://cori.example"));
        assert_eq!(a.authenticate(&platform).await.unwrap().sub, "jean");
        assert!(a.authenticate_for(&platform, true).await.is_ok());
        // the MCP endpoint's own resource (what an MCP host asks for), next to the issuer's userinfo audience: `/mcp` only
        let mcp = token(json!(["https://cori.example/mcp", format!("{issuer}/userinfo")]));
        assert!(a.authenticate_for(&mcp, true).await.is_ok());
        assert!(a.authenticate(&mcp).await.unwrap_err().contains("invalid token"), "the rest of the API refuses it");
        // any other audience: nowhere
        let other = token(json!("crm-mcp"));
        assert!(a.authenticate(&other).await.is_err() && a.authenticate_for(&other, true).await.is_err());
        // with no audience configured, none is checked, as before
        assert!(server(None).authenticate(&other).await.is_ok());
        // another claim names the person (Entra's `oid`, a dotted path); a token without it is refused, never read as `sub`
        let named_by = |claim: &str| Authenticator::new(AuthMode::Builtin { issuer: issuer.clone(), audience: None, roles_claim: "roles".into(), subject_claim: claim.into() });
        assert_eq!(named_by("oid").authenticate(&platform).await.unwrap().sub, "0b7e-oid");
        assert_eq!(named_by("ext.employee_id").authenticate(&platform).await.unwrap().sub, "E42");
        assert!(named_by("upn").authenticate(&platform).await.unwrap_err().contains("no subject (`upn`)"));
    }
    #[test]
    fn agent_token_round_trip() {
        let a = Authenticator::new(AuthMode::Dev).with_agent_key([7; 32]);
        let human = Principal { sub: "jean".into(), roles: vec!["builder".into()], email: None, claims: Value::Null, token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        let m = a.mint_agent(&human, "my-app", vec![], 999_999, Some("s1".into()), None).unwrap();
        let p = a.verify_agent(&m.token).unwrap();
        assert!(p.is_agent()); assert_eq!(p.app.as_deref(), Some("my-app")); assert_eq!(p.session_id.as_deref(), Some(&m.jti[..]));
        assert!(p.has_scope("versions:publish", Some("my-app"))); assert!(!p.has_scope("versions:publish", Some("other"))); assert!(!p.has_scope("approve", None));
        assert!(a.mint_agent(&p, "my-app", vec![], 60, None, None).is_err());
        assert!(Authenticator::new(AuthMode::Dev).with_agent_key([8; 32]).verify_agent(&m.token).is_err());
        let exp = chrono::DateTime::parse_from_rfc3339(&m.expires_at).unwrap(); assert!(exp.timestamp() - chrono::Utc::now().timestamp() <= AGENT_TTL_MAX_SECS + 1);
        assert!(is_agent_token(&m.token) && agent_token_realm(&m.token).is_none(), "the core's tokens name no realm");
        // a realm's token names it, and only that realm's key verifies it
        let org = crate::db::Realm::new("org-a");
        let r = a.clone().with_agent_key([9; 32]).mint_agent(&human, "my-app", vec![], 600, None, Some(&org)).unwrap();
        assert_eq!(agent_token_realm(&r.token), Some(org));
        assert!(a.verify_agent(&r.token).is_err());
        assert_eq!(a.clone().with_agent_key([9; 32]).verify_agent(&r.token).unwrap().claims["realm"], "org-a");
    }
    #[tokio::test]
    async fn a_fronts_session_round_trips_with_no_platform_power() {
        let a = Authenticator::new(AuthMode::Builtin { issuer: "https://sso.example".into(), audience: None, roles_claim: "roles".into(), subject_claim: "sub".into() }).with_session_key([5; 32]);
        let now = chrono::Utc::now().timestamp();
        let c = SessionClaims { iss: "cori".into(), kind: "preview".into(), sub: "sam".into(), aud: "https://desk--v12.cori.example".into(), iat: now - 60, exp: now + 600, jti: "j1".into(), email: Some("sam@acme.example".into()), name: None, realm: Some("org-a".into()), app: Some("desk".into()), version: Some("sha256:ab".into()), app_roles: vec!["support_agent".into()], roles: vec![], client_roles: BTreeMap::new() };
        let t = a.mint_session(&c).unwrap();
        assert!(is_session_token(&t) && !is_agent_token(&t));
        assert_eq!(session_token_claims(&t).and_then(|c| c.realm).as_deref(), Some("org-a"));
        let mut h = HeaderMap::new(); h.insert("authorization", format!("Bearer {t}").parse().unwrap());
        let p = a.authenticate(&h).await.unwrap();
        // the person, their roles in the app, and nothing of the platform: scoping to the app changes nothing
        assert_eq!((p.sub.as_str(), p.roles.clone(), p.app.as_deref()), ("sam", vec!["support_agent".to_string()], Some("desk")));
        assert!(!p.is_agent() && p.platform_roles().is_empty() && !p.is_admin() && !p.is_builder());
        assert_eq!(p.clone().in_app("desk", Some("desk")).roles, vec!["support_agent".to_string()]);
        assert_eq!((p.claims["kind"].as_str(), p.claims["aud"].as_str(), p.claims["iat"].as_i64()), (Some("preview"), Some("https://desk--v12.cori.example"), Some(now - 60)));
        // another realm's key, an expired one, an unkeyed server: refused
        let mut h2 = HeaderMap::new(); h2.insert("authorization", format!("Bearer {t}").parse().unwrap());
        assert!(Authenticator::new(AuthMode::Dev).with_session_key([6; 32]).authenticate(&h2).await.is_err());
        assert!(Authenticator::new(AuthMode::Dev).authenticate(&h2).await.is_err() && Authenticator::new(AuthMode::Dev).mint_session(&c).is_err());
        let old = a.mint_session(&SessionClaims { exp: now - 120, ..c.clone() }).unwrap();
        let mut h3 = HeaderMap::new(); h3.insert("authorization", format!("Bearer {old}").parse().unwrap());
        assert!(a.authenticate(&h3).await.is_err());
        // a portal's: the person with their roles, each app's read as an issuer's are
        let portal = SessionClaims { kind: "portal".into(), aud: "https://acme.cori.example".into(), app: None, version: None, app_roles: vec![], roles: vec!["member".into()], client_roles: BTreeMap::from([("desk--acme".to_string(), vec!["support_agent".to_string()])]), ..c.clone() };
        let mut h4 = HeaderMap::new(); h4.insert("authorization", format!("Bearer {}", a.mint_session(&portal).unwrap()).parse().unwrap());
        let p = a.authenticate(&h4).await.unwrap();
        assert_eq!((p.roles.clone(), p.app.clone(), p.platform.clone()), (vec!["member".to_string()], None, None));
        assert_eq!(p.app_roles("desk", Some("desk--acme")), vec!["support_agent".to_string()]);
        assert!(p.app_roles("crm", Some("crm--acme")).is_empty());
        // whatever claim the issuer's tokens carry them in: the session keeps them by client
        let listed = AppRolesClaim::parse("roles", Some("{client_id}:")).unwrap();
        assert_eq!(p.app_roles_in(&listed, "desk", Some("desk--acme")), vec!["support_agent".to_string()]);
        assert_eq!(p.client_roles(), Some(BTreeMap::from([("desk--acme".to_string(), vec!["support_agent".to_string()])])));
    }
    /// An issuer token: platform roles in `roles`, app roles per client in `resource_access` (Keycloak client roles).
    fn issued(roles: &[&str], access: Value) -> Principal {
        Principal { sub: "u1".into(), roles: roles.iter().map(|r| r.to_string()).collect(), email: None, claims: json!({ "sub": "u1", "roles": roles, "resource_access": access }), token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None }
    }
    #[test]
    fn app_roles_come_from_the_app_client_only() {
        let p = issued(&["builder"], json!({ "hello": { "roles": ["member"] }, "billing": { "roles": ["approver", "it_admin"] } }));
        let in_hello = p.clone().in_app("hello", Some("hello"));
        assert_eq!(in_hello.roles, vec!["member".to_string()]);
        assert!(in_hello.is_builder(), "platform roles survive scoping");
        // a grant in one app never shows in another, nor for an app without its client in the token
        assert!(p.clone().in_app("other", Some("other")).roles.is_empty());
        assert!(p.clone().in_app("hello", None).roles.is_empty());
        // client roles named like platform roles grant no platform power
        let in_billing = p.clone().in_app("billing", Some("billing"));
        assert_eq!(in_billing.roles, vec!["approver".to_string(), "it_admin".to_string()]);
        assert!(!in_billing.is_admin() && !in_billing.is_store_admin());
        assert_eq!(in_billing.platform_roles(), &["builder".to_string()]);
        // and platform roles are not app roles: an admin holds no app role it was not granted
        let admin = issued(&["it_admin"], json!({})).in_app("hello", Some("hello"));
        assert!(admin.roles.is_empty() && admin.is_admin());
        // scoping twice keeps the first scope; the run's user carries both sets
        assert_eq!(in_hello.clone().in_app("billing", Some("billing")).roles, vec!["member".to_string()]);
        assert_eq!(in_hello.to_user_json(&[])["platform_roles"], json!(["builder"]));
        // every client's roles, those with none left out; a token without the claim says nothing
        let p = issued(&["builder"], json!({ "hello": { "roles": ["member"] }, "account": { "roles": [] } }));
        assert_eq!(p.client_roles(), Some(BTreeMap::from([("hello".to_string(), vec!["member".to_string()])])));
        assert_eq!(Principal { claims: json!({ "sub": "u1" }), ..p }.client_roles(), None);
    }
    #[test]
    fn app_roles_claim_settings() {
        let parse = |claim: &str, prefix: Option<&str>| AppRolesClaim::parse(claim, prefix);
        assert_eq!(parse("resource_access.{client_id}.roles", None), Ok(AppRolesClaim::default()));
        assert_eq!(parse(" https://acme.example/apps.{client_id} ", Some(" ")), Ok(AppRolesClaim::PerClient { clients: "https://acme.example/apps".into(), roles: String::new() }));
        assert_eq!(parse("roles", Some("{client_id}:")), Ok(AppRolesClaim::Listed { claim: "roles".into(), before: String::new(), after: ":".into() }));
        assert_eq!(parse("groups", Some("cori-{client_id}-")), Ok(AppRolesClaim::Listed { claim: "groups".into(), before: "cori-".into(), after: "-".into() }));
        // the client is one name of the path, after the claim's; a listed claim's prefix names it once, then a separator
        for (claim, prefix) in [("{client_id}.roles", None), ("resource_access.app-{client_id}.roles", None), ("resource_access.{client_id}.", None), ("a.{client_id}.{client_id}", None),
                                ("roles", None), ("roles", Some("app:")), ("roles", Some("{client_id}")), ("roles", Some("{client_id}:{client_id}:")), ("resource_access.{client_id}.roles", Some("{client_id}:"))] {
            assert!(parse(claim, prefix).is_err(), "{claim} {prefix:?}");
        }
        assert_eq!(AppRolesClaim::default().to_string(), "resource_access.<client_id>.roles");
        assert_eq!(parse("groups", Some("cori-{client_id}-")).unwrap().to_string(), "groups, as cori-<client_id>-<role>");
    }
    #[test]
    fn app_roles_in_a_claim_per_client() {
        // Keycloak's client roles, the default
        let claims = json!({ "resource_access": { "desk": { "roles": ["agent", "lead"] }, "desk.v2": { "roles": ["agent"] }, "crm": { "roles": "agent" } } });
        let keycloak = AppRolesClaim::default();
        assert_eq!(keycloak.roles(&claims, "desk"), vec!["agent".to_string(), "lead".to_string()]);
        assert_eq!(keycloak.roles(&claims, "desk.v2"), vec!["agent".to_string()], "a client id with dots is one name");
        assert!(keycloak.roles(&claims, "crm").is_empty() && keycloak.roles(&claims, "other").is_empty() && keycloak.roles(&json!({}), "desk").is_empty());
        // a namespaced claim (its name holds dots), the client's value the list itself
        let claims = json!({ "https://acme.example/apps": { "desk": ["agent"], "crm": [] } });
        let namespaced = AppRolesClaim::parse("https://acme.example/apps.{client_id}", None).unwrap();
        assert_eq!(namespaced.roles(&claims, "desk"), vec!["agent".to_string()]);
        assert_eq!(namespaced.by_client(&claims), Some(BTreeMap::from([("desk".to_string(), vec!["agent".to_string()])])));
        assert_eq!(namespaced.by_client(&json!({ "roles": [] })), None);
    }
    #[test]
    fn app_roles_in_one_claim_listing_every_apps() {
        // as Entra ID issues app roles defined on the platform's own client: platform and app roles in one claim
        let entra = AppRolesClaim::parse("roles", Some("{client_id}:")).unwrap();
        let p = Principal { claims: json!({ "sub": "u1", "roles": ["builder", "desk:agent", "desk:it_admin", "crm-2:agent", "desk:", ":agent", "Cori.Admin"] }),
            ..issued(&["builder", "desk:agent", "desk:it_admin", "crm-2:agent", "desk:", ":agent", "Cori.Admin"], Value::Null) };
        assert_eq!(p.app_roles_in(&entra, "desk", Some("desk")), vec!["agent".to_string(), "it_admin".to_string()]);
        assert_eq!(p.app_roles_in(&entra, "crm", Some("crm-2")), vec!["agent".to_string()]);
        assert!(p.app_roles_in(&entra, "other", Some("other")).is_empty() && p.app_roles_in(&entra, "desk", None).is_empty());
        assert_eq!(entra.by_client(&p.claims), Some(BTreeMap::from([("crm-2".to_string(), vec!["agent".to_string()]), ("desk".to_string(), vec!["agent".to_string(), "it_admin".to_string()])])));
        // the platform roles stay what their own claim says; an app role named like a platform role grants no power
        let mut in_desk = p.clone(); in_desk.platform = Some(std::mem::replace(&mut in_desk.roles, p.app_roles_in(&entra, "desk", Some("desk"))));
        assert!(in_desk.is_builder() && !in_desk.is_admin() && in_desk.roles.contains(&"it_admin".to_string()));
        // groups with a prefix of their own: other groups are not app roles; a client id is never matched by its start
        let groups = AppRolesClaim::parse("groups", Some("cori-{client_id}-")).unwrap();
        let claims = json!({ "groups": ["Everyone", "cori-desk-agent", "cori-desk-night_shift", "cori-desk2-agent", "cori-support-desk-agent", "cori--agent"] });
        assert_eq!(groups.roles(&claims, "desk"), vec!["agent".to_string(), "night_shift".to_string()]);
        assert_eq!(groups.roles(&claims, "desk2"), vec!["agent".to_string()]);
        // a client id holding the separator holds nothing, rather than another app's roles
        assert!(groups.roles(&claims, "support-desk").is_empty() && groups.roles(&claims, "support") == vec!["desk-agent".to_string()]);
        assert!(groups.unreadable("support-desk").is_some() && groups.unreadable("desk").is_none() && entra.unreadable("8f3c1d2e-4b5a").is_none());
        // a token without the claim says nothing; one with it and no app role, that it holds none
        assert_eq!(groups.by_client(&json!({ "sub": "u1" })), None);
        assert_eq!(groups.by_client(&json!({ "groups": ["Everyone"] })), Some(BTreeMap::new()));
    }
    #[test]
    fn claims_are_found_by_dotted_path_or_by_a_name_with_dots() {
        let claims = json!({ "realm_access": { "roles": ["a"] }, "https://acme.example/roles": ["b"], "https://acme.example/org": { "roles": ["c"] } });
        assert_eq!(lookup(&claims, "realm_access.roles"), Some(&json!(["a"])));
        assert_eq!(lookup(&claims, "https://acme.example/roles"), Some(&json!(["b"])));
        assert_eq!(lookup(&claims, "https://acme.example/org.roles"), Some(&json!(["c"])));
        assert_eq!(lookup(&claims, "realm_access.other"), None);
        assert_eq!(lookup(&claims, ""), None);
    }
    #[test]
    fn dev_tokens_and_agents_keep_their_roles_in_their_app() {
        let dev = Principal { sub: "jean".into(), roles: vec!["member".into()], email: None, claims: Value::Null, token: None, kind: PrincipalKind::Human, scopes: vec![], app: None, session_id: None, platform: None };
        assert_eq!(dev.in_app("hello", None).roles, vec!["member".to_string()]);
        let a = Authenticator::new(AuthMode::Dev).with_agent_key([7; 32]);
        let human = issued(&["builder"], json!({ "hello": { "roles": ["member"] } }));
        let agent = a.verify_agent(&a.mint_agent(&human, "hello", human.app_roles("hello", Some("hello")), 600, None, None).unwrap().token).unwrap();
        assert_eq!(agent.clone().in_app("hello", Some("hello")).roles, vec!["member".to_string()]);
        assert!(agent.clone().in_app("other", Some("other")).roles.is_empty(), "an agent session is bound to its app");
        // its human's platform roles decide what it sees, never a platform power, and its runs carry none
        let scoped = agent.in_app("hello", Some("hello"));
        assert!(scoped.holds_role(&platform_roles().builder) && !scoped.is_builder());
        assert_eq!(scoped.to_user_json(&[])["platform_roles"], json!([]));
    }
    #[test]
    fn a_run_user_carries_only_the_claims_the_app_lists() {
        let mut p = issued(&["builder"], json!({ "hello": { "roles": ["member"] } }));
        p.claims["department"] = json!("support"); p.claims["phone_number"] = json!("+33 1 23 45 67 89"); p.email = Some("u1@acme.example".into());
        let p = p.in_app("hello", Some("hello"));
        let user = p.to_user_json(&["department".to_string(), "sub".to_string()]);
        assert_eq!((&user["department"], &user["sub"], &user["email"], &user["roles"]), (&json!("support"), &json!("u1"), &json!("u1@acme.example"), &json!(["member"])));
        assert!(user.get("phone_number").is_none() && user.get("resource_access").is_none(), "{user}");
        assert!(p.to_user_json(&[]).get("department").is_none());
    }
}
