// SPDX-License-Identifier: Apache-2.0
//! Where a command's server and token come from: one login per server (per server and organization on a server hosting
//! several).
//! - `~/.cori/config.toml`: your logins, one per server and organization (`cori login <url> [--org <slug>]`), each
//!   refreshing itself. The first is the default: the server of commands run outside an app folder, the one you signed in
//!   to last.
//! - `<app>/.cori/session`: the server the folder works with, and on a server of several organizations the one it belongs
//!   to. A folder that `cori login`, `cori init` or `cori pull` wrote holds no token: its commands use your login to that
//!   server and organization, never another organization's. A token of its own is an agent session (the `login` MCP tool,
//!   or its bootstrap form, which `init` and `pull` read from the environment) or a token given to `init` / `pull` as
//!   flags; a session written by an older command line holds one too.
//! - Every call from a folder that belongs to an organization names it (`x-cori-organization`), and the server refuses one
//!   whose token is another organization's.
//!
//! Resolution order: flags, then `CORI_SERVER` / `CORI_TOKEN`, then the folder's `.cori/session`, then your logins, then
//! the local dev defaults; a stored token only ever goes to the server it was issued for (`decide`).
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// An organization of a server hosting several (`/v1/me`'s `organization`): its id, and the slug people know it by.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Organization { pub id: String, #[serde(default, skip_serializing_if = "Option::is_none")] pub slug: Option<String> }

impl Organization {
    /// How it reads in messages and flags: its slug, else its id.
    pub fn label(&self) -> &str { self.slug.as_deref().unwrap_or(&self.id) }
    /// The organization `/v1/me` names, on a server hosting several.
    pub fn of_me(me: &serde_json::Value) -> Option<Self> {
        let o = &me["organization"];
        Some(Self { id: o["id"].as_str()?.to_string(), slug: o["slug"].as_str().map(String::from) })
    }
}

impl std::fmt::Display for Organization { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.label()) } }

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Session {
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub app: Option<String>,
    /// Empty in a folder that uses your login to `server`.
    #[serde(default)] pub token: String,
    /// `agent` (minted by the `login` MCP tool: app-scoped, short-lived, no approve/deploy) or `human` (`cori login`).
    #[serde(default = "default_kind")] pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub client_id: Option<String>,
    /// The scopes it signed in with, when the server adds its own to ours (`/v1/auth/config`'s `scopes`): a refresh asks
    /// for them again, for an issuer that gives a token the audience of the scopes asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub sign_in_scope: Option<String>,
    /// On a server hosting several organizations: the one a login's token names, or the one a folder belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub organization: Option<Organization>,
}
fn default_kind() -> String { "human".into() }

impl Session {
    /// A folder that works with `server` and uses your login to it.
    pub fn binding(server: &str, app: Option<&str>) -> Self { Self { server: server.to_string(), app: app.map(String::from), kind: default_kind(), ..Default::default() } }
    /// The id of its organization, if it names one.
    pub fn org_id(&self) -> Option<&str> { self.organization.as_ref().map(|o| o.id.as_str()) }
    pub fn expires_in(&self) -> Option<chrono::Duration> { self.expires_at.as_deref().and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok()).map(|e| e.with_timezone(&chrono::Utc) - chrono::Utc::now()) }
    pub fn expired(&self) -> bool { self.expires_in().map(|d| d <= chrono::Duration::zero()).unwrap_or(false) }
}

/// The server a command talks to when nothing names one: a local `cori up`.
pub const LOCAL_SERVER: &str = "http://127.0.0.1:8080";

/// Cori Cloud: where `cori login` signs in (and a new person signs up) when nothing names a server: no URL, no app folder
/// bound to one, no login yet.
pub const CLOUD_SERVER: &str = "https://app.cori.do";

/// The identity a command uses when nothing names one: the dev bearer of the person running it (`dev:<user>:it_admin`,
/// what `cori login` writes for a dev server). A server with real sign-in refuses it, and promote, approve, rollback,
/// protect and deletions never fall back to it (main).
pub fn dev_token() -> String {
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).ok().filter(|u| !u.is_empty() && !u.contains(':')).unwrap_or_else(|| "dev".into());
    format!("dev:{user}:it_admin")
}

pub fn path(app_root: &Path) -> PathBuf { app_root.join(".cori").join("session") }

/// The app root (the folder holding `cori.toml`) of a path: the path itself or an ancestor.
pub fn app_root_of(p: &Path) -> Option<PathBuf> {
    let mut cur = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().ok()?.join(p) };
    if cur.is_file() { cur.pop(); }
    loop { if cur.join("cori.toml").exists() { return Some(cur); } if !cur.pop() { return None; } }
}

pub fn load(app_root: &Path) -> Option<Session> { read(&path(app_root)) }

/// A session file.
pub fn read(p: &Path) -> Option<Session> { toml::from_str(&std::fs::read_to_string(p).ok()?).ok() }

pub fn save(app_root: &Path, s: &Session) -> Result<PathBuf> {
    let p = path(app_root);
    write(&p, s)?;
    Ok(p)
}

/// Writes a session file, readable by its owner only.
pub fn write(p: &Path, s: &Session) -> Result<()> { write_text(p, &toml::to_string(s)?) }

/// Writes a file readable by its owner only, whole or not at all: a command starting meanwhile reads the one before.
pub fn write_text(p: &Path, text: &str) -> Result<()> {
    let dir = p.parent().context("a session file needs a folder")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}.tmp", p.file_name().and_then(|n| n.to_str()).unwrap_or("session"), std::process::id()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }
    let mut f = o.open(&tmp).with_context(|| tmp.display().to_string())?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; f.set_permissions(std::fs::Permissions::from_mode(0o600))?; }
    std::io::Write::write_all(&mut f, text.as_bytes())?;
    drop(f);
    std::fs::rename(&tmp, p).with_context(|| p.display().to_string())?;
    Ok(())
}

/// Holds `<file>.lock` until dropped, so that one command at a time rewrites `file`. A refresh token is good once
/// (cori-auth rotates it, and a replayed one revokes every token of the client): two commands must never both spend it.
pub fn lock(file: &Path) -> Result<std::fs::File> {
    let mut p = file.as_os_str().to_owned();
    p.push(".lock");
    if let Some(d) = file.parent() { std::fs::create_dir_all(d)?; }
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(PathBuf::from(&p)).with_context(|| PathBuf::from(&p).display().to_string())?;
    f.lock()?;
    Ok(f)
}

/// `~/.cori/config.toml`: your logins.
pub fn global_path() -> Option<PathBuf> { dirs::home_dir().map(|h| h.join(".cori").join("config.toml")) }

/// Your logins, one per server and organization. The first is the default: the server of commands run outside an app
/// folder. It sits at the top of the file, as the one login of older files did (an older command line still reads it); the
/// others follow under `[[logins]]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Logins(pub Vec<Session>);

#[derive(Serialize, Deserialize, Default)]
struct Others { #[serde(default)] logins: Vec<Session> }

const LOGINS_HEADER: &str = "# Your cori logins, one per server and organization (`cori login <url> [--org <slug>]`). The one at the top is\n# the default: the server of commands run outside an app folder. An app folder names its server (and organization) in\n# .cori/session and uses your login to them.\n\n";

impl Logins {
    pub fn parse(text: &str) -> Self {
        let first = toml::from_str::<Session>(text).ok().filter(|s| !s.server.is_empty());
        let others = toml::from_str::<Others>(text).map(|o| o.logins).unwrap_or_default();
        let mut l = Self::default();
        for s in first.into_iter().chain(others) { if l.for_login(&s.server, s.org_id()).is_none() { l.0.push(s); } }
        l
    }

    pub fn render(&self) -> Result<String> {
        let mut out = LOGINS_HEADER.to_string();
        if let Some((first, rest)) = self.0.split_first() {
            out.push_str(&toml::to_string(first)?);
            if !rest.is_empty() { out.push_str("\n# your logins to other servers\n"); out.push_str(&toml::to_string(&Others { logins: rest.to_vec() })?); }
        }
        Ok(out)
    }

    pub fn read(file: &Path) -> Self { std::fs::read_to_string(file).map(|t| Self::parse(&t)).unwrap_or_default() }

    /// Your logins in `~/.cori/config.toml`.
    pub fn load() -> Self { global_path().map(|p| Self::read(&p)).unwrap_or_default() }

    pub fn write(&self, file: &Path) -> Result<()> { write_text(file, &self.render()?) }

    pub fn default_login(&self) -> Option<&Session> { self.0.first() }

    /// Your login to the server of `url` (its origin: scheme, host, port): the one used last, whatever its organization.
    pub fn for_server(&self, url: &str) -> Option<&Session> {
        let o = origin(url)?;
        self.0.iter().find(|s| origin(&s.server).as_deref() == Some(o.as_str()))
    }

    /// Your logins to the server of `url`, the one used last first.
    pub fn all_for_server<'a>(&'a self, url: &str) -> impl Iterator<Item = &'a Session> {
        let o = origin(url);
        self.0.iter().filter(move |s| o.is_some() && origin(&s.server) == o)
    }

    /// Your login to the server of `url` in organization `org` (None: a login that names none, a server of one).
    pub fn for_login(&self, url: &str, org: Option<&str>) -> Option<&Session> { self.all_for_server(url).find(|s| s.org_id() == org) }

    /// The login a folder bound to `url` uses: the one of its organization (`org`), else one that names none (an older
    /// login, or a server of one organization: the server checks the folder's organization itself); a folder that names
    /// none takes the login when all your logins to that server are of one organization. `Err` says why there is none
    /// to use when you have logins to that server.
    pub fn pick(&self, url: &str, org: Option<&str>) -> Result<Option<&Session>, String> {
        let mine: Vec<&Session> = self.all_for_server(url).collect();
        if mine.is_empty() { return Ok(None); }
        match org {
            Some(o) => mine.iter().find(|s| s.org_id() == Some(o)).or_else(|| mine.iter().find(|s| s.organization.is_none())).copied().map(Some)
                .ok_or_else(|| format!("none of your logins to {url} is for this folder's organization ({})", mine.iter().filter_map(|s| s.organization.as_ref().map(|o| o.label().to_string())).collect::<Vec<_>>().join(", "))),
            None => {
                let mut orgs: Vec<Option<&str>> = mine.iter().map(|s| s.org_id()).collect();
                orgs.sort(); orgs.dedup();
                if orgs.len() == 1 { Ok(Some(mine[0])) } else { Err(format!("this folder names no organization, and you have logins to several on {url}")) }
            }
        }
    }

    /// Forgets your login to `url` in `org`; true when there was one.
    pub fn forget(&mut self, url: &str, org: Option<&str>) -> bool {
        let o = origin(url);
        let before = self.0.len();
        self.0.retain(|s| !(o.is_some() && origin(&s.server) == o && s.org_id() == org));
        self.0.len() != before
    }

    /// Keeps `s` as your login to its server and organization, in place of the one before: first (the default) when
    /// `default`, else where that one was.
    pub fn put(&mut self, s: Session, default: bool) {
        let o = origin(&s.server);
        let at = self.0.iter().position(|x| o.is_some() && origin(&x.server) == o && x.org_id() == s.org_id());
        match (at, default) {
            (Some(i), false) => self.0[i] = s,
            (Some(i), true) => { self.0.remove(i); self.0.insert(0, s); }
            (None, true) => self.0.insert(0, s),
            (None, false) => self.0.push(s),
        }
    }
}

/// Saves `s` in `file` as your login to its server (and organization), now the default. Returns your other logins, as
/// they read (`https://app.cori.do (acme)`).
pub fn save_login(file: &Path, s: Session) -> Result<Vec<String>> {
    let _lock = lock(file)?;
    let mut l = Logins::read(file);
    l.put(s, true);
    l.write(file)?;
    Ok(l.0.iter().skip(1).map(login_label).collect())
}

/// A login as it reads: its server, and its organization when it names one.
pub fn login_label(s: &Session) -> String { match &s.organization { Some(o) => format!("{} ({o})", s.server), None => s.server.clone() } }

/// Where the token came from, for messages, for its refresh and for `init`/`pull` (which persist an explicit one).
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// `--token` or `CORI_TOKEN`.
    Env,
    /// The token of an app folder's own session.
    Session(PathBuf),
    /// Your login to the server, in `~/.cori/config.toml`.
    Login,
    /// None stored: the dev identity against a server on this machine, nothing elsewhere.
    Default,
}

pub struct Resolved { pub source: Source, pub session: Option<Session>, pub app_root: Option<PathBuf>, pub login: Option<Session>, pub server: String }

/// The app folder a command works on: the first argument that is an existing path inside an app, else the current
/// directory. Runs before clap parses, so the session can feed the `env = "CORI_SERVER"` arguments.
pub fn discover_app_root() -> Option<PathBuf> {
    for a in std::env::args().skip(1) {
        if a.starts_with('-') { continue; }
        let p = PathBuf::from(&a);
        if p.exists() { if let Some(r) = app_root_of(&p) { return Some(r); } }
    }
    std::env::current_dir().ok().and_then(|c| app_root_of(&c))
}

/// The server and the stored token a command gets, from what `apply` read (pure, for the tests), and the organization the
/// folder belongs to (named on every call).
#[derive(Debug, PartialEq)]
pub struct Decision { pub server: String, pub server_named: bool, pub token: Option<String>, pub source: Source, pub organization: Option<Organization> }

/// - The server: `--server`, else `CORI_SERVER`, else the app folder's `.cori/session`, else your default login, else a
///   local `cori up`. A token given explicitly with no server goes to the local `cori up`, never to a stored server.
/// - The token: `--token` or `CORI_TOKEN` as given (`token` stays `None`: clap reads it). Else the folder's own token,
///   else your login to that server (`Logins::pick`: the folder's organization's), whichever login is the default: a
///   stored token never goes to another server, nor a folder's commands to another organization, whatever the command
///   line says. Else nothing stored: the dev identity against a server on this machine, none elsewhere (`default_token`).
pub fn decide(named_server: Option<&str>, explicit_token: bool, session: Option<&Session>, session_path: Option<PathBuf>, logins: &Logins) -> Decision {
    let server = match named_server {
        Some(s) => s.to_string(),
        None if explicit_token => LOCAL_SERVER.to_string(),
        None => session.map(|s| s.server.clone()).or_else(|| logins.default_login().map(|l| l.server.clone())).unwrap_or_else(|| LOCAL_SERVER.to_string()),
    };
    let issued_for = |s: &str| origin(s).is_some() && origin(s) == origin(&server);
    // the folder's organization, for its own server only
    let organization = session.filter(|s| issued_for(&s.server)).and_then(|s| s.organization.clone());
    let in_folder = session.is_some_and(|s| issued_for(&s.server));
    let (token, source) = if explicit_token { (None, Source::Env) }
        else if let (Some(s), Some(p)) = (session.filter(|s| !s.token.is_empty() && issued_for(&s.server)), session_path) { (Some(s.token.clone()), Source::Session(p)) }
        else if in_folder { match logins.pick(&server, organization.as_ref().map(|o| o.id.as_str())) { Ok(Some(l)) => (Some(l.token.clone()), Source::Login), _ => (None, Source::Default) } }
        else if let Some(l) = logins.for_server(&server) { (Some(l.token.clone()), Source::Login) }
        else { (None, Source::Default) };
    Decision { server, server_named: named_server.is_some(), token, source, organization }
}

/// The origin (scheme, host, port) of a server URL: what a stored token is bound to.
pub fn origin(url: &str) -> Option<String> { reqwest::Url::parse(url.trim()).ok().map(|u| u.origin().ascii_serialization()) }

/// Whether a server URL is on this machine (a local `cori up`): the only server the dev identity goes to unasked.
pub fn is_local(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url.trim()) else { return false };
    u.host_str().map(|h| h == "localhost" || h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)).unwrap_or(false)
}

static SERVER: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The token of a command that names none and has no stored one for its server: the dev identity against a server on
/// this machine, none against any other.
pub fn default_token() -> String { if SERVER.get().map(|s| is_local(s)).unwrap_or(true) { dev_token() } else { String::new() } }

/// `--name X` or `--name=X` on the command line, read before clap (like the app folder).
fn flag(args: &[String], name: &str) -> Option<String> {
    let eq = format!("{name}=");
    args.iter().enumerate().find_map(|(i, a)| if a == name { args.get(i + 1).cloned() } else { a.strip_prefix(&eq).map(String::from) })
}

/// Applies `decide` to the environment clap's `--server` / `--token` read (`env = "CORI_SERVER"`, `"CORI_TOKEN"`).
pub fn apply() -> Resolved {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let app_root = discover_app_root();
    let session = app_root.as_ref().and_then(|r| load(r));
    let logins = Logins::load();
    let named = flag(&args, "--server").or_else(|| env("CORI_SERVER"));
    let explicit_token = flag(&args, "--token").is_some() || env("CORI_TOKEN").is_some();
    let d = decide(named.as_deref(), explicit_token, session.as_ref(), app_root.as_ref().map(|r| path(r)), &logins);
    let _ = SERVER.set(d.server.clone());
    // every call names the folder's organization: the server refuses one whose token is another organization's
    if let Some(o) = &d.organization { if env("CORI_ORGANIZATION").is_none() { unsafe { std::env::set_var("CORI_ORGANIZATION", &o.id); } } }
    // the server the folder or your default login names, signed in or not (a folder never falls back to another server),
    // and a stored token for that server only; a named server with no stored token for it gets none (its 401 says so)
    unsafe {
        if !d.server_named && d.server != LOCAL_SERVER { std::env::set_var("CORI_SERVER", &d.server); }
        if let Some(t) = &d.token { std::env::set_var("CORI_TOKEN", t); }
    }
    let login = if d.source == Source::Login {
        match &session { Some(s) if origin(&s.server) == origin(&d.server) => logins.pick(&d.server, d.organization.as_ref().map(|o| o.id.as_str())).ok().flatten().cloned(), _ => logins.for_server(&d.server).cloned() }
    } else { None };
    Resolved { source: d.source, session, app_root, login, server: d.server }
}

/// What a folder bound to `server` signs in with, for messages: its own token, your login to the server, the dev
/// identity, or nothing yet.
pub fn describe(s: &Session) -> String {
    let at = match &s.organization { Some(o) => format!("{} in organization {o}", s.server), None => s.server.clone() };
    if !s.token.is_empty() { return format!("{at}, with {}", if s.kind == "agent" { "the agent session given" } else { "the token given" }); }
    if let Ok(Some(_)) = Logins::load().pick(&s.server, s.org_id()) { format!("{at}, with your login to it") }
    else if is_local(&s.server) { format!("{at}, with your dev identity") }
    else { format!("{at}: not signed in to it yet (`cori login {}{}`)", s.server, s.organization.as_ref().map(|o| format!(" --org {o}")).unwrap_or_default()) }
}

/// The hint printed when a server answers 401, or when an agent session is past its expiry.
pub fn sign_in_hint(app_root: Option<&Path>, kind: &str, server: &str) -> String {
    if kind == "agent" {
        let where_ = app_root.map(|r| path(r).display().to_string()).unwrap_or_else(|| ".cori/session".into());
        format!("session expired ({where_}): call the `login` tool of the Cori MCP again and write its session to {where_} (or run the bootstrap command it prints)")
    } else {
        // a folder of an organization: a login to that one; a folder naming none, with logins to several there: bind it
        let folder = app_root.and_then(load).filter(|s| origin(&s.server) == origin(server));
        if let Some(o) = folder.as_ref().and_then(|s| s.organization.as_ref()) { return format!("not signed in to {o} on {server}, or the login expired: run `cori login {server} --org {o}`"); }
        let theirs: Vec<String> = Logins::load().all_for_server(server).filter_map(|l| l.organization.as_ref().map(|o| o.label().to_string())).collect();
        if folder.is_some() && theirs.len() > 1 { return format!("this folder names no organization, and you have logins to several on {server} ({}): run `cori login {server} --org <one of them>` in it to bind it to one", theirs.join(", ")); }
        format!("not signed in to {server}, or the login expired: run `cori login {server}`")
    }
}

/// Where a stored token lives, for its refresh: an app folder's own session, or your logins.
#[derive(Debug, Clone, PartialEq)]
pub enum Stored { Folder(PathBuf), Login(PathBuf) }

impl Stored {
    pub fn file(&self) -> &Path { match self { Stored::Folder(p) | Stored::Login(p) => p } }
    /// The session as it is now in its file: the folder's, or your login to its server and organization.
    fn read(&self, s: &Session) -> Option<Session> { match self { Stored::Folder(p) => read(p), Stored::Login(p) => Logins::read(p).for_login(&s.server, s.org_id()).cloned() } }
    fn write(&self, s: &Session) -> Result<()> {
        match self {
            Stored::Folder(p) => write(p, s),
            Stored::Login(p) => { let mut l = Logins::read(p); l.put(s.clone(), false); l.write(p) }
        }
    }
}

/// A human token that expires within a minute and can be refreshed.
fn needs_refresh(s: &Session) -> bool {
    s.kind == "human" && s.refresh_token.is_some() && s.issuer.is_some() && s.client_id.is_some() && s.expires_in().map(|d| d <= chrono::Duration::seconds(60)).unwrap_or(false)
}

/// Refreshes a human token that expires within a minute (OIDC refresh grant) and rewrites it where it lives. Returns the
/// token to use when it changed. Runs under the file's lock and reads it again once held: a command that waited takes the
/// token another one just refreshed, and never spends the refresh token a second time.
pub async fn refresh_if_needed(at: &Stored, s: &Session) -> Result<Option<Session>> {
    if !needs_refresh(s) { return Ok(None); }
    let file = at.file().to_path_buf();
    let _lock = tokio::task::spawn_blocking(move || lock(&file)).await??;
    let Some(cur) = at.read(s) else { return Ok(None) };
    if !needs_refresh(&cur) { return Ok((cur.token != s.token).then_some(cur)); }
    let (Some(rt), Some(issuer), Some(client_id)) = (&cur.refresh_token, &cur.issuer, &cur.client_id) else { return Ok(None) };
    let http = cori_engine::http::builder().timeout(std::time::Duration::from_secs(10)).build()?;
    let cfg: serde_json::Value = http.get(format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'))).send().await?.json().await?;
    let te = cfg["token_endpoint"].as_str().context("issuer has no token_endpoint")?;
    let mut form = vec![("grant_type", "refresh_token"), ("refresh_token", rt.as_str()), ("client_id", client_id.as_str())];
    form.extend(cur.sign_in_scope.as_deref().map(|s| ("scope", s)));
    let r = http.post(te).form(&form).send().await?;
    let v: serde_json::Value = r.json().await?;
    let Some(tok) = v["access_token"].as_str() else { return Ok(None) };
    let mut n = cur.clone();
    n.token = tok.to_string();
    if let Some(r) = v["refresh_token"].as_str() { n.refresh_token = Some(r.to_string()); }
    n.expires_at = v["expires_in"].as_i64().map(|e| (chrono::Utc::now() + chrono::Duration::seconds(e)).to_rfc3339());
    at.write(&n)?;
    Ok(Some(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(server: &str, token: &str) -> Session { Session { server: server.into(), token: token.into(), kind: "human".into(), ..Default::default() } }
    fn p() -> Option<PathBuf> { Some(PathBuf::from("/app/.cori/session")) }
    fn logins(l: &[(&str, &str)]) -> Logins { Logins(l.iter().map(|(s, t)| sess(s, t)).collect()) }
    fn tempdir(n: &str) -> PathBuf { let p = std::env::temp_dir().join(format!("cori-session-test-{n}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&p); std::fs::create_dir_all(&p).unwrap(); p }

    #[test]
    fn a_stored_token_goes_to_its_own_server_only() {
        let s = sess("https://cori.acme.example", "prod-jwt");
        // in the app folder: the session's server and token
        let d = decide(None, false, Some(&s), p(), &Logins::default());
        assert_eq!((d.server.as_str(), d.token.as_deref()), ("https://cori.acme.example", Some("prod-jwt")));
        // the same server named again (trailing slash): still the session's token
        assert_eq!(decide(Some("https://cori.acme.example/"), false, Some(&s), p(), &Logins::default()).token.as_deref(), Some("prod-jwt"));
        // another server on the command line: the session's token stays home, whatever the server
        for other in ["http://127.0.0.1:8080", "https://evil.example", "http://cori.acme.example"] {
            let d = decide(Some(other), false, Some(&s), p(), &Logins::default());
            assert_eq!((d.token, d.source), (None, Source::Default), "{other}");
        }
        // outside an app folder: the default login, for its server only
        let l = logins(&[("https://cori.acme.example", "admin-jwt")]);
        let d = decide(None, false, None, None, &l);
        assert_eq!((d.server.as_str(), d.token.as_deref(), d.source), ("https://cori.acme.example", Some("admin-jwt"), Source::Login));
        assert_eq!(decide(Some("http://127.0.0.1:8091"), false, None, None, &l).token, None);
    }

    /// The case that sent `cori init` to staging after a login to prod: logins are kept per server, a folder works with
    /// its own server whichever login is the default, and never borrows another server's.
    #[test]
    fn a_folder_uses_your_login_to_its_own_server() {
        let l = logins(&[("https://app-staging.example.com", "staging-jwt"), ("https://app.cori.do", "prod-jwt")]);
        let prod = Session::binding("https://app.cori.do", Some("my-app"));
        let d = decide(None, false, Some(&prod), p(), &l);
        assert_eq!((d.server.as_str(), d.token.as_deref(), d.source), ("https://app.cori.do", Some("prod-jwt"), Source::Login));
        // not signed in to the folder's server: no token at all, and still its server
        let other = Session::binding("https://cori.acme.example", None);
        let d = decide(None, false, Some(&other), p(), &l);
        assert_eq!((d.server.as_str(), d.token, d.source), ("https://cori.acme.example", None, Source::Default));
        // a server named on the command line takes your login to it, default or not
        assert_eq!(decide(Some("https://app.cori.do"), false, None, None, &l).token.as_deref(), Some("prod-jwt"));
        // a folder's own token (an agent session) comes first, for its server
        let agent = Session { kind: "agent".into(), ..sess("https://app.cori.do", "agent-jwt") };
        assert_eq!(decide(None, false, Some(&agent), p(), &l).token.as_deref(), Some("agent-jwt"));
    }

    fn org(id: &str, slug: &str) -> Organization { Organization { id: id.into(), slug: Some(slug.into()) } }
    fn in_org(server: &str, token: &str, o: Option<Organization>) -> Session { Session { organization: o, ..sess(server, token) } }

    /// On a server hosting several organizations, a login per organization: a folder of one never uses another's, whatever
    /// login is the default (the same app id may exist in both).
    #[test]
    fn a_folder_uses_your_login_to_its_own_organization() {
        let prod = "https://app.cori.do";
        let mut l = Logins::default();
        l.put(in_org(prod, "acme-jwt", Some(org("o-acme", "acme"))), true);
        l.put(in_org(prod, "globex-jwt", Some(org("o-globex", "globex"))), true);
        assert_eq!(l.0.len(), 2, "one login per server and organization");
        // the default is globex's; a folder of acme still takes acme's
        let acme = Session { organization: Some(org("o-acme", "acme")), ..Session::binding(prod, Some("desk")) };
        let d = decide(None, false, Some(&acme), p(), &l);
        assert_eq!((d.token.as_deref(), d.source, d.organization.as_ref().map(|o| o.id.as_str())), (Some("acme-jwt"), Source::Login, Some("o-acme")));
        // a folder of an organization you have no login to: none, rather than another organization's
        let initech = Session { organization: Some(org("o-initech", "initech")), ..Session::binding(prod, Some("desk")) };
        let d = decide(None, false, Some(&initech), p(), &l);
        assert_eq!((d.token, d.source), (None, Source::Default));
        assert!(l.pick(prod, Some("o-initech")).unwrap_err().contains("acme"), "the error names the logins there are");
        // a folder naming none, with logins to two organizations there: none either
        let unnamed = Session::binding(prod, Some("desk"));
        assert_eq!(decide(None, false, Some(&unnamed), p(), &l).token, None);
        assert!(l.pick(prod, None).is_err());
        // …and with one organization's only, that one
        let mut one = Logins::default(); one.put(in_org(prod, "acme-jwt", Some(org("o-acme", "acme"))), true);
        assert_eq!(decide(None, false, Some(&unnamed), p(), &one).token.as_deref(), Some("acme-jwt"));
        // outside a folder: the login used last to that server
        assert_eq!(decide(Some(prod), false, None, None, &l).token.as_deref(), Some("globex-jwt"));
        // a login made before organizations (it names none) serves any folder of that server: the server checks the
        // folder's organization against the token's itself
        let mut old = Logins::default(); old.put(sess(prod, "old-jwt"), true);
        assert_eq!(decide(None, false, Some(&acme), p(), &old).token.as_deref(), Some("old-jwt"));
        // signing in again to an organization replaces that login only; forgetting one keeps the other
        l.put(in_org(prod, "acme-2", Some(org("o-acme", "acme"))), true);
        assert_eq!((l.0.len(), l.for_login(prod, Some("o-acme")).map(|s| s.token.as_str()), l.default_login().map(|s| s.token.as_str())), (2, Some("acme-2"), Some("acme-2")));
        assert!(l.forget(prod, Some("o-globex")) && !l.forget(prod, Some("o-globex")));
        assert_eq!(l.0.iter().map(|s| s.token.as_str()).collect::<Vec<_>>(), ["acme-2"]);
        // the file keeps each login's organization
        l.put(in_org(prod, "globex-2", Some(org("o-globex", "globex"))), false);
        let back = Logins::parse(&l.render().unwrap());
        assert_eq!(back, l);
        assert_eq!(back.for_login(prod, Some("o-globex")).and_then(|s| s.organization.as_ref()).map(|o| o.label()), Some("globex"));
    }

    #[test]
    fn an_organization_is_read_from_me() {
        let o = Organization::of_me(&serde_json::json!({ "sub": "u", "organization": { "id": "o-1", "slug": "acme" } })).unwrap();
        assert_eq!((o.id.as_str(), o.label()), ("o-1", "acme"));
        assert_eq!(Organization::of_me(&serde_json::json!({ "sub": "u" })), None, "a server of one organization names none");
        assert_eq!(Organization { id: "o-2".into(), slug: None }.label(), "o-2");
    }

    #[test]
    fn an_explicit_token_is_used_as_given() {
        let s = sess("https://cori.acme.example", "prod-jwt");
        let d = decide(Some("http://127.0.0.1:8091"), true, Some(&s), p(), &Logins::default());
        assert_eq!((d.token, d.source), (None, Source::Env), "clap reads --token / CORI_TOKEN itself");
        // with no server named, an explicit token goes to the local `cori up`, never to the stored server
        assert_eq!(decide(None, true, Some(&s), p(), &Logins::default()).server, LOCAL_SERVER);
    }

    #[test]
    fn the_dev_identity_stays_on_this_machine() {
        for local in ["http://127.0.0.1:8080", "http://localhost:8095", "http://[::1]:8080"] { assert!(is_local(local), "{local}"); }
        for remote in ["https://cori.acme.example", "http://10.0.0.4:8080", "not a url"] { assert!(!is_local(remote), "{remote}"); }
        assert_eq!(decide(None, false, None, None, &Logins::default()).server, LOCAL_SERVER);
    }

    #[test]
    fn a_login_file_written_before_whole_sessions_still_reads() {
        // its server and token, sometimes a refresh token, but no issuer: it never refreshes, as before
        let old: Session = toml::from_str("server = \"https://cori.acme.example\"\ntoken = \"t\"\nrefresh_token = \"r\"\n").unwrap();
        assert_eq!((old.server.as_str(), old.token.as_str(), old.kind.as_str()), ("https://cori.acme.example", "t", "human"));
        assert!(old.issuer.is_none() && old.client_id.is_none());
        // the one login of an older file is the default
        let l = Logins::parse("server = \"https://cori.acme.example\"\ntoken = \"t\"\n");
        assert_eq!(l.0.len(), 1);
        assert_eq!(l.default_login().map(|s| s.token.as_str()), Some("t"));
        assert_eq!(Logins::parse(""), Logins::default());
    }

    #[test]
    fn logins_are_kept_one_per_server_the_last_first() {
        let mut l = Logins::default();
        l.put(sess("https://app-staging.example.com", "s1"), true);
        l.put(sess("https://app.cori.do", "p1"), true);
        assert_eq!(l.0.iter().map(|s| s.token.as_str()).collect::<Vec<_>>(), ["p1", "s1"]);
        // a login again to the same server (trailing slash) replaces it; a refresh keeps its place
        l.put(sess("https://app-staging.example.com/", "s2"), false);
        assert_eq!(l.0.iter().map(|s| s.token.as_str()).collect::<Vec<_>>(), ["p1", "s2"]);
        l.put(sess("https://app-staging.example.com", "s3"), true);
        assert_eq!(l.0.iter().map(|s| s.token.as_str()).collect::<Vec<_>>(), ["s3", "p1"]);
        // the file: the default at the top, which an older command line reads as its one login, then the others
        let text = l.render().unwrap();
        assert_eq!(Logins::parse(&text), l);
        let older: Session = toml::from_str(&text).unwrap();
        assert_eq!((older.server.as_str(), older.token.as_str()), ("https://app-staging.example.com", "s3"));
        assert!(text.contains("[[logins]]"), "{text}");
    }

    #[test]
    fn a_folder_bound_to_your_login_holds_no_token() {
        let d = tempdir("binding");
        let p = save(&d, &Session::binding("https://app.cori.do", Some("my-app"))).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("token = \"\"") && text.contains("server = \"https://app.cori.do\""), "{text}");
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600); }
        assert_eq!(load(&d), Some(Session::binding("https://app.cori.do", Some("my-app"))));
        // written by hand, the server alone is enough
        std::fs::write(&p, "server = \"https://app.cori.do\"\n").unwrap();
        assert_eq!(load(&d).map(|s| s.token), Some(String::new()));
    }

    #[test]
    fn flags_are_read_in_both_forms() {
        let args: Vec<String> = ["deploy", ".", "--server", "https://a.example", "--token=t"].iter().map(|s| s.to_string()).collect();
        assert_eq!(flag(&args, "--server").as_deref(), Some("https://a.example"));
        assert_eq!(flag(&args, "--token").as_deref(), Some("t"));
        assert_eq!(flag(&args, "--message"), None);
    }

    /// A stand-in issuer: its discovery document, and a token endpoint that counts the refresh grants it answers and keeps
    /// their forms.
    fn issuer() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let grants = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let forms = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (b, g, f) = (base.clone(), grants.clone(), forms.clone());
        std::thread::spawn(move || for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            // the whole request (headers, then the body its content-length announces) before answering
            let (mut req, mut buf) = (Vec::new(), [0u8; 4096]);
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 { break; }
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).to_string();
                let Some(end) = text.find("\r\n\r\n") else { continue };
                let len = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                if req.len() >= end + 4 + len { break; }
            }
            let req = String::from_utf8_lossy(&req).to_string();
            let body = if req.starts_with("GET /.well-known/openid-configuration") { format!(r#"{{"token_endpoint":"{b}/token"}}"#) } else {
                let k = g.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                f.lock().unwrap().push(req.split("\r\n\r\n").nth(1).unwrap_or_default().to_string());
                std::thread::sleep(std::time::Duration::from_millis(300));
                format!(r#"{{"access_token":"fresh-{k}","refresh_token":"r{k}","expires_in":3600}}"#)
            };
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes());
        });
        (base, grants, forms)
    }

    fn stale(server: &str, issuer: &str) -> Session {
        Session { refresh_token: Some("r0".into()), issuer: Some(issuer.into()), client_id: Some("cori-cli".into()), expires_at: Some((chrono::Utc::now() + chrono::Duration::seconds(5)).to_rfc3339()), ..sess(server, "old") }
    }

    #[tokio::test]
    async fn a_login_refreshes_once_in_its_place() {
        let (iss, grants, _) = issuer();
        let file = tempdir("refresh").join("config.toml");
        let mut l = Logins::default();
        l.put(stale("https://app.cori.do", &iss), true);
        l.put(sess("https://app-staging.example.com", "staging-jwt"), true);
        l.write(&file).unwrap();
        let s = Logins::read(&file).for_server("https://app.cori.do").cloned().unwrap();
        // two commands start at once with the same stale token: one refresh grant, and both get its token
        let at = Stored::Login(file.clone());
        let (a, b) = tokio::join!(refresh_if_needed(&at, &s), refresh_if_needed(&at, &s));
        assert_eq!(grants.load(std::sync::atomic::Ordering::SeqCst), 1, "the refresh token was spent twice");
        assert_eq!((a.unwrap().unwrap().token, b.unwrap().unwrap().token), ("fresh-1".to_string(), "fresh-1".to_string()));
        // written back in its place: the default stays staging, prod has its new tokens
        let after = Logins::read(&file);
        assert_eq!(after.default_login().map(|s| s.token.as_str()), Some("staging-jwt"));
        let prod = after.for_server("https://app.cori.do").unwrap();
        assert_eq!((prod.token.as_str(), prod.refresh_token.as_deref()), ("fresh-1", Some("r1")));
        assert!(!needs_refresh(prod));
    }

    #[tokio::test]
    async fn a_refresh_asks_for_the_scopes_its_login_did() {
        let (iss, _, forms) = issuer();
        let file = tempdir("refresh-scope").join("config.toml");
        // a login whose server adds a scope of its own (`/v1/auth/config`'s `scopes`), and one whose server adds none
        let mut l = Logins::default();
        l.put(Session { sign_in_scope: Some("openid profile email offline_access api://cori/access".into()), ..stale("https://a.cori.example", &iss) }, true);
        l.put(stale("https://b.cori.example", &iss), true);
        l.write(&file).unwrap();
        let at = Stored::Login(file.clone());
        for server in ["https://a.cori.example", "https://b.cori.example"] {
            let s = Logins::read(&file).for_server(server).cloned().unwrap();
            assert!(refresh_if_needed(&at, &s).await.unwrap().is_some());
        }
        let forms = forms.lock().unwrap();
        assert!(forms[0].contains("scope=openid+profile+email+offline_access+api%3A%2F%2Fcori%2Faccess"), "{}", forms[0]);
        assert!(!forms[1].contains("scope="), "{}", forms[1]);
    }
}
