// SPDX-License-Identifier: Apache-2.0
//! Every name the platform shares with a system outside its own database, built in one place from the realm it belongs
//! to: Temporal (workflow ids, schedule ids, task queues), the store cluster (database names), the issuer and browsers
//! (URLs), Slack (button values), ciphertexts (associated data) and the process's token cache. The core has one realm
//! and builds the names it always did; a name of another realm carries it, so two realms never meet in a system they
//! share.
use crate::ServerConfig;
use crate::db::Realm;

/// `<realm><sep>` before a name of a realm; nothing for the core's.
fn scoped(realm: Option<&Realm>, sep: &str) -> String { realm.map(|r| format!("{}{sep}", r.as_str())).unwrap_or_default() }

// ---- Temporal ----

/// A run started by a request: its Temporal workflow id, and its id everywhere else.
pub fn run_id(realm: Option<&Realm>, workflow: &str) -> String { format!("{}{workflow}-{}", scoped(realm, "."), uuid::Uuid::new_v4().simple()) }

/// What the ids of a schedule's runs start with (Temporal appends the firing time to a scheduled workflow's id).
pub fn scheduled_run_prefix(realm: Option<&Realm>, app: &str, workflow: &str) -> String { format!("{}sched-{app}-{workflow}", scoped(realm, ".")) }

/// A run the server's own cron starts (`CORI_SCHEDULER=server`): one id per firing, so a firing starts once.
pub fn scheduled_run_id(realm: Option<&Realm>, app: &str, workflow: &str, fire: chrono::DateTime<chrono::Utc>) -> String {
    format!("{}-{}", scheduled_run_prefix(realm, app, workflow), fire.format("%Y%m%dT%H%M%SZ"))
}

/// A workflow's Temporal schedule.
pub fn schedule_id(realm: Option<&Realm>, app: &str, workflow: &str) -> String { format!("cori.{}{app}.{workflow}", scoped(realm, ".")) }

/// The queue a run of `app` starts on: the shared queue, or the app's shard of it (`ServerConfig::shards`).
pub fn run_queue(cfg: &ServerConfig, realm: Option<&Realm>, app: &str) -> String {
    if cfg.shards <= 1 { return cfg.task_queue.clone(); }
    let key = format!("{}{app}", scoped(realm, "/"));
    let h = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    format!("{}.{}", cfg.task_queue, h % cfg.shards as u64)
}

/// A run's key in Temporal's task queue fairness: its realm, so that the realms sharing a queue get even shares of its
/// dispatch (none for the core's single realm). The engine sets it on the run's activities and children too.
pub fn fairness_key(realm: Option<&Realm>) -> Option<String> { realm.map(|r| r.as_str().to_string()) }

/// The prefix of the worker pools' queues: a pool `p` polls `<prefix>.p`.
pub fn pool_queue_prefix(cfg: &ServerConfig, realm: Option<&Realm>) -> String {
    format!("{}.{}pool", cfg.task_queue.trim_end_matches(".shared"), scoped(realm, "."))
}

/// The queue a realm's worker pool `pool` polls (one an org runs, named by its connectors).
pub fn pool_queue(cfg: &ServerConfig, realm: Option<&Realm>, pool: &str) -> String { format!("{}.{pool}", pool_queue_prefix(cfg, realm)) }

/// The queue of the platform's sandbox pool (`cli` steps, stdio MCP servers): one for every realm of the server, each
/// run's realm its fairness key. `cori.pool.sandbox`, as the core always had it.
pub fn sandbox_queue(cfg: &ServerConfig) -> String { pool_queue(cfg, None, cori_engine::workflow::SANDBOX) }

// ---- the bundle store ----

/// The S3 key of a version's archive: `<prefix>bundles/<hash>.tar.zst`, under `realms/<realm>/` for a realm's.
pub fn bundle_key(realm: Option<&Realm>, prefix: &str, hash: &str) -> String {
    format!("{prefix}{}bundles/{}.tar.zst", realm.map(|r| format!("realms/{}/", r.as_str())).unwrap_or_default(), hash.replace(':', "_"))
}

/// The S3 key of a file a run was given (`api::files`), in the file store's own bucket: `<id>` under the realm's prefix.
pub fn file_key(realm: Option<&Realm>, prefix: &str, id: &str) -> String {
    format!("{prefix}{}{id}", realm.map(|r| format!("realms/{}/", r.as_str())).unwrap_or_default())
}

/// A realm's folder of the data dir: `realms/<realm>`, as its S3 keys. Its bundles and screenshots are its own, so removing
/// them never touches another realm's copy of the same bundle.
pub fn realm_dir(realm: &Realm) -> String { format!("realms/{}", realm.as_str()) }

/// The data dir's folder of `kind` (`bundles`, `screenshots`): under the realm's folder for a realm, else at the top.
pub fn realm_folder(realm: Option<&Realm>, kind: &str) -> String {
    match realm { Some(r) => format!("{}/{kind}", realm_dir(r)), None => kind.to_string() }
}

// ---- the store cluster ----

/// The name a new store database gets: the app's store `store` for `target` (`live`, a version's sandbox, `local`). A
/// realm's names start with its store tag (`RealmSettings::store_tag`, no other realm's), so that two organizations'
/// databases never share one; the core's single realm has none (`cori_store::db_name`). The store's record keeps the name
/// (`store_instances.db_name`), and a name another store's database holds gives way to a variant (`cori_store::Taken`).
pub fn store_database(tag: &str, app: &str, store: &str, target: &str) -> String {
    if tag.is_empty() { cori_store::db_name(app, store, target) } else { cori_store::db_name(&format!("{tag}_{app}"), store, target) }
}

/// The prefix of a new store's Postgres roles (each of its roles' data and applier roles): `cs_<tag>_<app>_<store>_<h8>` for
/// a realm's, so that each organization's roles are its own; `cs_<app>_<store>_<h8>` for the core's single realm. Kept in
/// the store's record (`store_instances.role_prefix`).
pub fn store_role_prefix(tag: &str, app: &str, store: &str) -> String {
    if tag.is_empty() { cori_store::policy::role_prefix(app, store) } else { cori_store::policy::role_prefix(&format!("{tag}_{app}"), store) }
}

/// Why `tag` cannot be a realm's store tag: 1 to 12 lowercase letters and digits, starting with a letter (no `_`, which
/// ends it in a name).
pub fn store_tag_problem(tag: &str) -> Option<String> {
    let ok = (1..=12).contains(&tag.len()) && tag.starts_with(|c: char| c.is_ascii_lowercase()) && tag.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (!ok).then(|| format!("store tag `{tag}`: 1 to 12 lowercase letters and digits, starting with a letter"))
}

/// What a store database is labelled with (`cori_store::Claim`): the realm, app, store and target it is for.
pub fn store_label(realm: Option<&Realm>, app: &str, store: &str, target: &str) -> String {
    format!("cori:{}/{app}/{store}/{target}", realm.map(|r| r.as_str()).unwrap_or(""))
}

// ---- the issuer and browsers ----

fn base(cfg: &ServerConfig) -> &str { cfg.public_url.trim_end_matches('/') }

/// Where apps' fronts are served when each has a host of its own (`CORI_FRONTS_URL`, e.g. `https://*.cori.run`): the `*`
/// is the front's label, so that one front's code never shares an origin with another's or with the console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontsUrl { scheme: String, domain: String, port: Option<u16> }

impl FrontsUrl {
    /// `<http|https>://*.<domain>[:port]`, nothing after.
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || format!("`{s}`: expected `https://*.<domain>` (optionally with a port), each app's front on `<label>.<domain>`");
        let (scheme, rest) = s.trim().trim_end_matches('/').split_once("://").ok_or_else(bad)?;
        if scheme != "https" && scheme != "http" { return Err(bad()); }
        let rest = rest.strip_prefix("*.").ok_or_else(bad)?.to_ascii_lowercase();
        let (domain, port) = match rest.rsplit_once(':') { Some((d, p)) => (d.to_string(), Some(p.parse::<u16>().map_err(|_| bad())?)), None => (rest, None) };
        let label_ok = |l: &str| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if domain.is_empty() || !domain.split('.').all(label_ok) { return Err(bad()); }
        Ok(Self { scheme: scheme.to_string(), domain, port })
    }
    /// The origin of the front labelled `label`.
    pub fn origin(&self, label: &str) -> String { format!("{}://{label}.{}{}", self.scheme, self.domain, self.port.map(|p| format!(":{p}")).unwrap_or_default()) }
    /// Every fronts' origin as a source expression (`https://*.cori.run`), for a CSP or an MCP host's frame list.
    pub fn pattern(&self) -> String { self.origin("*") }
    pub fn is_https(&self) -> bool { self.scheme == "https" }
    /// The label a request's `Host` names, when it is one of the fronts' hosts: one DNS label before the domain, on the
    /// fronts' port (a proxy that terminates TLS sends none).
    pub fn label_of(&self, host: &str) -> Option<String> {
        let host = host.trim().to_ascii_lowercase();
        let (name, port) = match host.rsplit_once(':') { Some((n, p)) if !n.ends_with(']') => (n.to_string(), p.parse::<u16>().ok()), _ => (host, None) };
        if port.is_some() && port != self.port { return None; }
        let label = name.strip_suffix(&format!(".{}", self.domain))?;
        (!label.is_empty() && !label.contains('.')).then(|| label.to_string())
    }
}

/// Why `label` cannot be a DNS label (lowercase letters, digits and inner dashes, at most 63): an app whose front has a
/// host of its own needs an id that is one.
pub fn dns_label_problem(label: &str) -> Option<String> {
    let ok = (1..=63).contains(&label.len()) && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !label.starts_with('-') && !label.ends_with('-');
    (!ok).then(|| "its front is served on a host of its own: lowercase letters, digits and inner dashes, at most 63 characters".to_string())
}

/// `CORI_PORTAL_URL` as an origin: `http(s)://<host>[:port]`, nothing after.
pub fn portal_origin(v: &str) -> Result<String, String> {
    let bad = || format!("`{v}`: expected the portal's origin, `https://<host>` (optionally with a port), nothing after");
    let u = reqwest::Url::parse(v.trim()).map_err(|_| bad())?;
    if !matches!(u.scheme(), "https" | "http") || u.host_str().is_none() || u.path() != "/" || u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() { return Err(bad()); }
    Ok(u.origin().ascii_serialization())
}

/// A sign-in page of the deployment's own (`CORI_PREVIEW_SIGN_IN_URL`): an absolute http(s) URL, with a path, and no
/// query or fragment (the hop's own query follows it), nor credentials.
pub fn sign_in_page(v: &str) -> Result<String, String> {
    let bad = || format!("`{v}`: expected the absolute URL of a page, `https://<host>/<path>`, with no query or fragment");
    let u = reqwest::Url::parse(v.trim()).map_err(|_| bad())?;
    if !matches!(u.scheme(), "https" | "http") || u.host_str().is_none() || u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() || u.password().is_some() { return Err(bad()); }
    Ok(u.to_string())
}

/// Where a realm's people find their apps: the root of its portal's host, under the fronts' domain when the realm has a
/// label there (`RealmSettings::portal_label`), else `CORI_PORTAL_URL`; None without a portal.
pub fn portal_url(cfg: &ServerConfig, label: &str) -> Option<String> {
    match (&cfg.fronts_url, label) {
        (Some(f), l) if !l.is_empty() => Some(format!("{}/", f.origin(l))),
        _ => cfg.portal_url.as_ref().map(|o| format!("{o}/")),
    }
}

/// Why `id` cannot be the id of an app whose front has a host of its own: a DNS label, without `--` (it separates a label's
/// parts: the app, a release, the realm's suffix), short enough that a release's host (`--v999999`) stays one label.
pub fn front_app_id_problem(id: &str) -> Option<String> {
    if let Some(why) = dns_label_problem(id) { return Some(why); }
    if id.contains("--") { return Some("its front is served on a host of its own, whose label `--` divides: single dashes only".into()); }
    (id.len() > FRONT_APP_ID_MAX).then(|| format!("its front and each release's are served on hosts of their own: at most {FRONT_APP_ID_MAX} characters"))
}

/// The longest app id whose front has a host of its own: its releases' labels (`<app>--v999999`) stay within 63 characters.
pub const FRONT_APP_ID_MAX: usize = 54;

/// The label of an app's front host: the app id and the realm's suffix (`--<org>` in Cori Cloud; nothing in the core).
pub fn front_label(app: &str, suffix: &str) -> String { format!("{app}{suffix}") }

/// The label of a release's host: the release between the app and the realm's suffix, `<app>--v<seq><suffix>`
/// (`desk--v12--acme`), so that the organization ends the label of every host of its own.
pub fn version_label(app: &str, seq: i64, suffix: &str) -> String { format!("{app}--v{seq}{suffix}") }

/// A host's label without its release, and the release (`version_label`): the part right after the app (app ids hold no
/// `--`) when it is `v` and digits without a leading zero. What remains is a front's label, as for the live host.
pub fn split_version(label: &str) -> (String, Option<i64>) {
    let Some((app, rest)) = label.split_once("--") else { return (label.to_string(), None) };
    let (part, tail) = match rest.split_once("--") { Some((p, t)) => (p, Some(t)), None => (rest, None) };
    let seq = part.strip_prefix('v').filter(|d| !d.is_empty() && !d.starts_with('0') && d.bytes().all(|b| b.is_ascii_digit())).and_then(|d| d.parse::<i64>().ok());
    match seq {
        Some(seq) => (tail.map(|t| format!("{app}--{t}")).unwrap_or_else(|| app.to_string()), Some(seq)),
        None => (label.to_string(), None),
    }
}

/// The origin an app's front is served from: a host of its own with `CORI_FRONTS_URL`, else the server's.
pub fn front_origin(cfg: &ServerConfig, suffix: &str, app: &str) -> String {
    match &cfg.fronts_url { Some(f) => f.origin(&front_label(app, suffix)), None => base(cfg).to_string() }
}

/// Where the promoted version of an app's front is served: the root of its own host with `CORI_FRONTS_URL`
/// (`https://desk--acme.cori.run/`), else `/apps/<app>/` on the server's.
pub fn front_url(cfg: &ServerConfig, suffix: &str, app: &str) -> String {
    match &cfg.fronts_url { Some(f) => format!("{}/", f.origin(&front_label(app, suffix))), None => format!("{}/apps/{app}/", base(cfg)) }
}

/// The origin a release's front is served from: a host of its own with `CORI_FRONTS_URL` (`version_label`), else the
/// server's.
pub fn release_origin(cfg: &ServerConfig, suffix: &str, app: &str, seq: i64) -> String {
    match &cfg.fronts_url { Some(f) => f.origin(&version_label(app, seq, suffix)), None => base(cfg).to_string() }
}

/// Where a release's front is served, promoted or not, on its sandbox stores: the root of its own host with
/// `CORI_FRONTS_URL` (`https://desk--v12--acme.cori.run/`), else `/apps/<app>/@v<seq>/` on the server's.
pub fn release_url(cfg: &ServerConfig, suffix: &str, app: &str, seq: i64) -> String {
    match &cfg.fronts_url { Some(_) => format!("{}/", release_origin(cfg, suffix, app, seq)), None => format!("{}/apps/{app}/@v{seq}/", base(cfg)) }
}

/// The OIDC redirect of an app's own client (the client hook registers it). It keeps its `/apps/<app>/` path on a front's
/// own host: registered so since apps first had clients, it needs no change at the identity provider.
pub fn callback_url(origin: &str, app: &str) -> String { format!("{origin}/apps/{app}/callback") }

/// Where a sign-out returns, registered with the callback (`@cori-do/client` takes the callback without its last word): a
/// front's own host sends it on to its root.
pub fn post_logout_url(origin: &str, app: &str) -> String { format!("{origin}/apps/{app}/") }

/// The console's page of an app, of a run, of an app's approvals; `console` is the console's base URL (a webhook may
/// name another).
pub fn console_app_url(console: &str, app: &str) -> String { format!("{}/#/app/{app}", console.trim_end_matches('/')) }
pub fn console_run_url(console: &str, app: &str, run: &str) -> String { format!("{}/#/run/{app}/{run}", console.trim_end_matches('/')) }
pub fn console_approvals_url(console: &str, app: &str) -> String { format!("{}/#/approvals/{app}", console.trim_end_matches('/')) }

// ---- Slack ----

/// The value of a Slack button deciding an approval: `<verb>:<app>:<id>`, `<verb>:@<realm>:<app>:<id>` for a realm's
/// (an app id never starts with `@`). `parse_slack_action` reads both.
pub fn slack_action(realm: Option<&Realm>, verb: &str, app: &str, id: &str) -> String { format!("{verb}:{}{app}:{id}", realm.map(|r| format!("@{}:", r.as_str())).unwrap_or_default()) }

/// (verb, realm, app, approval id) of a Slack button's value.
pub fn parse_slack_action(value: &str) -> (String, Option<Realm>, String, String) {
    let (verb, rest) = value.split_once(':').unwrap_or((value, ""));
    let (realm, rest) = match rest.strip_prefix('@').and_then(|r| r.split_once(':')) { Some((r, rest)) => (Some(Realm::new(r)), rest), None => (None, rest) };
    let (app, id) = rest.split_once(':').unwrap_or((rest, ""));
    (verb.to_string(), realm, app.to_string(), id.to_string())
}

// ---- ciphertexts and caches ----

/// The associated data sealing a broker secret's value to its name (and realm).
pub fn secret_aad(realm: Option<&Realm>, name: &str) -> String { format!("secret:{}{name}", scoped(realm, ":")) }

/// The associated data sealing a secret of `app` (None: the deployment's, as `secret_aad`) to its scope and name: a row
/// moved to another app or name does not open. Names hold no `@` or `/`, so the two forms never meet.
pub fn secret_aad_in(realm: Option<&Realm>, app: Option<&str>, name: &str) -> String {
    match app { None => secret_aad(realm, name), Some(app) => format!("secret:{}@{app}/{name}", scoped(realm, ":")) }
}

/// The associated data sealing a run's principal token to the run.
pub fn run_aad(realm: Option<&Realm>, run: &str) -> String { format!("run:{}{run}", scoped(realm, ":")) }

/// The token cache: a run's user exchanged for `audience`, every entry of a run (its prefix), a service account's token.
pub fn run_token_key(realm: Option<&Realm>, run: &str, audience: &str) -> String { format!("{}{audience}", run_token_prefix(realm, run)) }
pub fn run_token_prefix(realm: Option<&Realm>, run: &str) -> String { format!("run:{}{run}:", scoped(realm, ":")) }
pub fn client_token_key(realm: Option<&Realm>, client: &str) -> String { format!("client:{}{client}", scoped(realm, ":")) }

/// What a connection's refresh token is sealed for (`connections`): its realm, its owner (`person:<sub>`, `app:<id>`) and
/// its provider; the same names its access token in the cache.
pub fn connection_aad(realm: Option<&Realm>, owner: &str, provider: &str) -> String { format!("connection:{}{owner}/{provider}", scoped(realm, ":")) }

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(shards: u32) -> ServerConfig { let mut c = ServerConfig::local(std::env::temp_dir()); c.task_queue = "cori.shared".into(); c.shards = shards; c.public_url = "https://cori.example/".into(); c }

    #[test]
    fn the_core_keeps_the_names_it_always_had() {
        let c = cfg(1);
        let fire = chrono::DateTime::parse_from_rfc3339("2025-10-07T07:00:00Z").unwrap().with_timezone(&chrono::Utc);
        assert_eq!(schedule_id(None, "support-desk", "digest"), "cori.support-desk.digest");
        assert_eq!(scheduled_run_id(None, "support-desk", "digest", fire), "sched-support-desk-digest-20251007T070000Z");
        assert!(run_id(None, "triage").starts_with("triage-") && run_id(None, "triage").len() == "triage-".len() + 32);
        assert_eq!((run_queue(&c, None, "a"), pool_queue(&c, None, "sandbox"), pool_queue_prefix(&c, None), sandbox_queue(&c)), ("cori.shared".into(), "cori.pool.sandbox".into(), "cori.pool".into(), "cori.pool.sandbox".into()));
        assert_eq!(store_database("", "support-desk", "tickets", "live"), cori_store::db_name("support-desk", "tickets", "live"));
        assert_eq!(store_role_prefix("", "support-desk", "tickets"), cori_store::policy::role_prefix("support-desk", "tickets"));
        let o = front_origin(&c, "", "a");
        assert_eq!((front_url(&c, "", "a"), release_url(&c, "", "a", 12), callback_url(&o, "a"), post_logout_url(&o, "a")), ("https://cori.example/apps/a/".into(), "https://cori.example/apps/a/@v12/".into(), "https://cori.example/apps/a/callback".into(), "https://cori.example/apps/a/".into()));
        assert_eq!((secret_aad(None, "llm.openai"), run_aad(None, "r1")), ("secret:llm.openai".into(), "run:r1".into()));
        assert_eq!((run_token_key(None, "r1", "crm"), client_token_key(None, "c")), ("run:r1:crm".into(), "client:c".into()));
        assert_eq!(slack_action(None, "approve", "desk", "r1:gate"), "approve:desk:r1:gate");
        assert_eq!(bundle_key(None, "cori/", "sha256:ab"), "cori/bundles/sha256_ab.tar.zst");
        assert_eq!(bundle_key(Some(&Realm::new("org-a")), "", "sha256:ab"), "realms/org-a/bundles/sha256_ab.tar.zst");
        assert_eq!(parse_slack_action("approve:desk:r1:gate"), ("approve".into(), None, "desk".into(), "r1:gate".into()));
    }

    #[test]
    fn each_front_has_a_host_of_its_own() {
        let f = FrontsUrl::parse("https://*.cori.run").unwrap();
        let mut c = cfg(1); c.fronts_url = Some(f.clone());
        assert_eq!(front_origin(&c, "--acme", "desk"), "https://desk--acme.cori.run");
        // a front is served at the root of its host; its sign-in keeps the callback registered under `/apps/<app>/`
        assert_eq!(front_url(&c, "--acme", "desk"), "https://desk--acme.cori.run/");
        // each release at the root of a host of its own, the organization ending the label
        assert_eq!(release_url(&c, "--acme", "desk", 12), "https://desk--v12--acme.cori.run/");
        assert_eq!(release_url(&c, "", "desk", 3), "https://desk--v3.cori.run/");
        assert_eq!(callback_url(&front_origin(&c, "--acme", "desk"), "desk"), "https://desk--acme.cori.run/apps/desk/callback");
        assert_eq!(f.pattern(), "https://*.cori.run");
        assert_eq!(f.label_of("desk--acme.cori.run").as_deref(), Some("desk--acme"));
        assert_eq!(f.label_of("Desk--Acme.Cori.Run").as_deref(), Some("desk--acme"));
        for not_a_front in ["cori.run", "app.cori.do", "a.b.cori.run", "evilcori.run", "desk.cori.run.evil.com", "desk.cori.run:8443"] { assert_eq!(f.label_of(not_a_front), None, "{not_a_front}"); }
        let local = FrontsUrl::parse("http://*.localhost:8092").unwrap();
        assert_eq!(local.origin("desk"), "http://desk.localhost:8092");
        assert_eq!(local.label_of("desk.localhost:8092").as_deref(), Some("desk"));
        assert_eq!(local.label_of("desk.localhost").as_deref(), Some("desk"), "behind a proxy: no port");
        assert_eq!(local.label_of("desk.localhost:9999"), None);
        for bad in ["https://cori.run", "https://*.cori.run/apps", "ftp://*.cori.run", "https://*.", "https://*.cori.run:x", "*.cori.run"] { assert!(FrontsUrl::parse(bad).is_err(), "{bad}"); }
    }

    #[test]
    fn a_realms_portal_is_its_label_under_the_fronts_domain_or_the_deployments() {
        let mut c = cfg(1);
        assert_eq!(portal_url(&c, ""), None);
        c.portal_url = portal_origin("https://apps.acme.internal").ok();
        assert_eq!(portal_url(&c, "").as_deref(), Some("https://apps.acme.internal/"));
        c.fronts_url = FrontsUrl::parse("https://*.cori.run").ok();
        assert_eq!(portal_url(&c, "jean-b335").as_deref(), Some("https://jean-b335.cori.run/"));
        assert_eq!(portal_origin("http://apps.localhost:8393/").as_deref(), Ok("http://apps.localhost:8393"));
        for bad in ["apps.acme.internal", "ftp://apps.acme.internal", "https://apps.acme.internal/portal", "https://apps.acme.internal?x=1", "https://u@apps.acme.internal"] { assert!(portal_origin(bad).is_err(), "{bad}"); }
    }

    #[test]
    fn a_sign_in_page_is_an_absolute_url_with_no_query() {
        assert_eq!(sign_in_page("https://tools.acme.internal/cori/sign-in").as_deref(), Ok("https://tools.acme.internal/cori/sign-in"));
        assert_eq!(sign_in_page(" http://tools.localhost:3000 ").as_deref(), Ok("http://tools.localhost:3000/"));
        for bad in ["/cori/sign-in", "tools.acme.internal/cori/sign-in", "javascript:alert(1)", "ftp://tools.acme.internal/x", "https://tools.acme.internal/x?next=1", "https://tools.acme.internal/x#y", "https://u:p@tools.acme.internal/x"] {
            assert!(sign_in_page(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_hosts_label_names_its_release_after_the_app() {
        assert_eq!(split_version("desk--v12--acme-1f2e"), ("desk--acme-1f2e".into(), Some(12)));
        assert_eq!(split_version("desk--v3"), ("desk".into(), Some(3)));
        for live in ["desk--acme-1f2e", "desk", "v12--acme", "desk--v012--acme", "desk--v--acme", "desk--v1x--acme", "desk--acme--v12"] {
            assert_eq!(split_version(live).1, None, "{live}");
        }
        assert_eq!(split_version(&version_label("desk", 999999, "--acme")), ("desk--acme".into(), Some(999999)));
        // the longest app id and org slug with the highest release: one DNS label
        let label = version_label(&"a".repeat(30), 999999, &format!("--{}", "s".repeat(22)));
        assert!(label.len() <= 63 && dns_label_problem(&label).is_none(), "{label}");
        assert!(version_label(&"a".repeat(FRONT_APP_ID_MAX), 999999, "").len() <= 63);
        for ok in ["desk", "support-desk", &"a".repeat(54)] { assert!(front_app_id_problem(ok).is_none(), "{ok}"); }
        for bad in ["desk--v2", "a--b", "Desk", &"a".repeat(55)] { assert!(front_app_id_problem(bad).is_some(), "{bad}"); }
    }

    #[test]
    fn a_realm_name_never_meets_another_realms() {
        let (a, b) = (Realm::new("org-a"), Realm::new("org-b"));
        assert_ne!(schedule_id(Some(&a), "desk", "w"), schedule_id(Some(&b), "desk", "w"));
        assert_ne!(schedule_id(Some(&a), "desk", "w"), schedule_id(None, "desk", "w"));
        assert!(run_id(Some(&a), "w").starts_with("org-a.w-"));
        assert_ne!(pool_queue(&cfg(1), Some(&a), "onprem"), pool_queue(&cfg(1), Some(&b), "onprem"));
        assert_ne!(store_label(Some(&a), "desk", "t", "live"), store_label(Some(&b), "desk", "t", "live"));
        assert_ne!(secret_aad(Some(&a), "s"), secret_aad(Some(&b), "s"));
        assert_ne!(run_token_key(Some(&a), "r", "x"), run_token_key(Some(&b), "r", "x"));
        let v = slack_action(Some(&a), "decline", "desk", "promote:desk:sha256:1");
        assert_eq!(parse_slack_action(&v), ("decline".into(), Some(a.clone()), "desk".into(), "promote:desk:sha256:1".into()));
        // sharding keeps an app's runs on one shard, per realm
        let c = cfg(8);
        assert_eq!(run_queue(&c, Some(&a), "desk"), run_queue(&c, Some(&a), "desk"));
        assert!(run_queue(&c, None, "desk").starts_with("cori.shared."));
    }

    #[test]
    fn each_realms_store_objects_carry_its_tag_within_postgres_limits() {
        assert_eq!(store_database("t1", "support-desk", "support_tickets", "live"), "cori_store_t1_support_desk_support_tickets");
        assert_ne!(store_database("t1", "desk", "s", "live"), store_database("t2", "desk", "s", "live"));
        assert_ne!(store_database("t1", "desk", "s", "live"), store_database("", "desk", "s", "live"));
        // a tag holds no `_`: `t1` and app `a_desk` never read as `t1_a` and app `desk`
        assert_ne!(store_database("t1", "a_desk", "s", "live"), store_database("t1a", "desk", "s", "live"));
        let (p1, p2) = (store_role_prefix("t1", "desk", "s"), store_role_prefix("t2", "desk", "s"));
        assert!(p1.starts_with("cs_t1_desk_s_") && p2.starts_with("cs_t2_desk_s_"), "{p1} {p2}");
        // the longest names (a 30-character app id and store name, a sandbox) keep the tag and stay within 63 bytes
        let (app, store, hash) = ("a-very-long-application-ident", "a_store_with_a_very_long_name_", "sha256:0123456789abcdef");
        for tag in ["t1", "tzzzzzzzzzz1"] {
            let db = store_database(tag, app, store, hash);
            assert!(db.len() <= 63 && db.starts_with(&format!("cori_store_{tag}_")), "{db}");
            let prefix = store_role_prefix(tag, app, store);
            let roles = cori_store::policy::pg_roles(&prefix, "a_role_with_a_rather_long_name_too");
            assert!(prefix.starts_with(&format!("cs_{tag}_")) && roles.data.len() <= 63 && roles.applier.len() <= 63 && roles.applier.starts_with(&format!("{prefix}__")), "{roles:?}");
            assert!(cori_store::db_name_for(&db, "cori:x/a/b/live").len() <= 63);
        }
        for ok in ["t1", "tz9", "a"] { assert!(store_tag_problem(ok).is_none(), "{ok}"); }
        for bad in ["", "T1", "1t", "t_1", "t-1", "t1234567890ab"] { assert!(store_tag_problem(bad).is_some(), "{bad}"); }
        assert_eq!(store_label(Some(&Realm::new("org-a")), "desk", "s", "live"), "cori:org-a/desk/s/live");
        assert_eq!(store_label(None, "desk", "s", "sha256:ab"), "cori:/desk/s/sha256:ab");
    }
}
