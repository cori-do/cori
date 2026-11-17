// SPDX-License-Identifier: Apache-2.0
//! `cori init`: an app folder from a template or blank (cori.toml, AGENTS.md, workflows/, fixtures/, the front), a git
//! repository, the session, and the app's registration on the server.
use crate::cli::*;
use crate::{commands, session};
use std::path::PathBuf;

pub async fn run(cmd: Cmd, resolved: &session::Resolved) -> anyhow::Result<()> {
    match cmd {
        Cmd::Init { dir, app, name, template, roles, front, no_git, no_register, conn: Conn { server, token } } => {
            let app_id = app.clone().unwrap_or_else(|| dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or("my-app".into()));
            // a server connection: an explicit one (flags, env, .cori/session, `cori login`), or the local dev server when it answers
            let explicit = !matches!(resolved.source, session::Source::Default) || server != session::LOCAL_SERVER || token != session::dev_token();
            let connected = !no_register && (explicit || commands::server_answers(&server).await);
            // an app reserved ahead (create_app, `cori apps create`) seeds the roles of cori.toml; a new id is registered after the folder exists
            let registered = if connected { commands::apps_get(&server, &token, &app_id).await.ok() } else { None };
            let server_roles: Vec<String> = registered.as_ref().and_then(|v| v["roles"].as_array()).map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect()).unwrap_or_default();
            let roles = if roles.is_empty() { server_roles.clone() } else { roles };
            // the display name it was reserved with, unless one is given
            let name = name.or_else(|| registered.as_ref().and_then(|v| v["name"].as_str()).filter(|n| !n.is_empty() && *n != app_id).map(String::from));
            println!("{}", init_app(&dir, app.as_deref(), name.as_deref(), template.as_deref(), &roles, front)?);
            // a template keeps its own roles: its workflows, stores and front use them
            let kept = template.as_deref().filter(|t| *t != "blank").map(|_| cori_compiler::roles::declared_order(&std::fs::read_to_string(dir.join("cori.toml")).unwrap_or_default()));
            if !no_git { if let Some(g) = commands::git_init(&dir, "cori init") { println!("{g}"); } }
            // the organization the token acts in, on a server hosting several: the folder belongs to it
            let organization = if connected { commands::organization_of(&server, &token).await } else { None };
            let written = commands::persist_session(&dir, Some(&app_id), &server, &token, &resolved.source, connected, organization)?;
            if let Some((p, s)) = &written { println!("session written to {} · {}", p.display(), session::describe(s)); }
            if connected {
                let display = name.clone().or_else(|| std::fs::read_to_string(dir.join("cori.toml")).ok().and_then(|t| toml::from_str::<toml::Value>(&t).ok()).and_then(|t| t.get("app")?.get("name")?.as_str().map(String::from))).unwrap_or_else(|| app_id.clone());
                match match registered { Some(v) => Ok(serde_json::json!({ "existed": true, "owner": v["owner"] })), None => commands::apps_create(&server, &token, &app_id, Some(&display), &roles, &[], template.as_deref()).await } {
                    Ok(v) if v["existed"].as_bool() == Some(true) => println!("app {app_id} already registered on {server}{}", match &kept {
                        _ if server_roles.is_empty() => String::new(),
                        None => format!(" · roles {} written to cori.toml", server_roles.join(", ")),
                        Some(t) if { let (mut a, mut b) = (t.clone(), server_roles.clone()); a.sort(); b.sort(); a == b } => format!(" · roles {}", server_roles.join(", ")),
                        Some(t) => format!(" · it was reserved with roles {}, and cori.toml keeps the template's ({}), which its workflows, stores and front use: rename them everywhere, or keep them (a deploy registers the roles of cori.toml)", server_roles.join(", "), t.join(", ")),
                    }),
                    Ok(v) => println!("app {app_id} registered on {server} (owner {})", v["owner"].as_str().unwrap_or("")),
                    Err(e) => eprintln!("note: app not registered on {server}: {e}\n      (`cori apps create {app_id}` later, or --no-register)"),
                }
            } else if !no_register {
                eprintln!("note: app {app_id} not registered{}: no server connection (no CORI_SERVER, .cori/session or `cori login`) and nothing answers at {server}.\n      start one with `cori up`, then in {}: `cori login {server}` and `cori apps create {app_id}` (or pass --server/--token to init; --no-register silences this)", if written.is_none() { " and no .cori/session written" } else { "" }, dir.display());
            }
        }
        _ => unreachable!("main dispatches only this module's commands"),
    }
    Ok(())
}

static TEMPLATES: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../templates");
/// The React front written by `cori init --front react` (placeholders `__APP_ID__`, `__APP_NAME__`, `__APP_NAME_JSON__`).
static FRONT_TEMPLATE: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/front-template");
/// `@cori-do/client` is not on npm: the cli carries it and vendors it into `front/vendor/cori-client/`.
const CORI_CLIENT_INDEX: &str = include_str!("../../../packages/client/index.ts");
const CORI_CLIENT_REACT: &str = include_str!("../../../packages/client/react.ts");


/// Whether an embedded template file belongs in a new app: `include_dir!` takes the working tree as it is, so a
/// checkout's generated `.cori/`, installed `node_modules/` and `.DS_Store` files would otherwise land in every app.
fn shipped(rel: &std::path::Path) -> bool {
    !rel.components().any(|c| matches!(c.as_os_str().to_str(), Some(".cori" | "node_modules" | ".DS_Store")))
}

/// Credential files (`.env`, `.env.*`) stay out of git, as they stay out of bundles (`cori_server::bundles::credential_file`).
const GITIGNORE: &str = ".cori/\nnode_modules/\n.DS_Store\n.env\n.env.*\n";
/// `dist/` is build output once the front has a build step.
const GITIGNORE_BUILT_FRONT: &str = ".cori/\nnode_modules/\n.DS_Store\n.env\n.env.*\ndist/\n";

/// Writes `front/` (Vite + React + TypeScript + Tailwind), the vendored client, a generated `src/lib/cori.ts` and a
/// placeholder `dist/index.html` until the first `npm run build`. Returns the number of files written.
pub fn write_react_front(dir: &std::path::Path, app_id: &str, name: &str) -> anyhow::Result<usize> {
    let front = dir.join("front");
    if front.join("package.json").exists() { anyhow::bail!("{} already exists", front.join("package.json").display()); }
    let html_name = name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let json_name = serde_json::to_string(name)?;
    fn walk(d: &include_dir::Dir, out: &std::path::Path, subst: &dyn Fn(&str) -> String, n: &mut usize) -> anyhow::Result<()> {
        for f in d.files().filter(|f| shipped(f.path())) {
            let target = out.join(f.path());
            if let Some(p) = target.parent() { std::fs::create_dir_all(p)?; }
            match f.contents_utf8() { Some(t) => std::fs::write(&target, subst(t))?, None => std::fs::write(&target, f.contents())? }
            *n += 1;
        }
        for sub in d.dirs() { walk(sub, out, subst, n)?; }
        Ok(())
    }
    let mut n = 0;
    walk(&FRONT_TEMPLATE, &front, &|t: &str| t.replace("__APP_NAME_JSON__", &json_name).replace("__APP_NAME__", &html_name).replace("__APP_ID__", app_id), &mut n)?;
    std::fs::create_dir_all(front.join("vendor/cori-client"))?;
    std::fs::write(front.join("vendor/cori-client/index.ts"), CORI_CLIENT_INDEX)?;
    std::fs::write(front.join("vendor/cori-client/react.ts"), CORI_CLIENT_REACT)?;
    // the generated client of an app with no workflows yet; `npm run gen` (part of `npm run build`) rewrites it
    std::fs::create_dir_all(front.join("src/lib"))?;
    std::fs::write(front.join("src/lib/cori.ts"), cori_server::clientgen::client_ts(app_id, &[]))?;
    n += 3;
    std::fs::create_dir_all(dir.join("dist"))?;
    std::fs::write(dir.join("dist/index.html"), format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{html_name}</title></head><body><h1>{html_name}</h1><p>The front is not built yet: <code>cd front &amp;&amp; npm install &amp;&amp; npm run build</code>, then <code>cori deploy</code>.</p></body></html>\n"))?;
    Ok(n)
}

/// Adds `dist/` to an existing `.gitignore` (a template that committed its plain front gets a build step).
fn ignore_dist(dir: &std::path::Path) -> anyhow::Result<()> {
    let p = dir.join(".gitignore");
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    if text.lines().any(|l| matches!(l.trim(), "dist" | "dist/" | "/dist" | "/dist/")) { return Ok(()); }
    std::fs::write(&p, format!("{}{}dist/\n", text, if text.is_empty() || text.ends_with('\n') { "" } else { "\n" }))?;
    Ok(())
}

const REACT_FRONT_NEXT: &str = "front: `cd front && npm install && npm run build` (Vite + React + Tailwind into dist/; `npm run dev` against `cori up`)";

/// Creates an app folder (layout v3: the folder is the app, `cori.toml` at its root, `workflows/`, `fixtures/`, `dist/`,
/// `AGENTS.md`) from a template or blank. Returns a summary line. The id defaults to the folder name.
pub fn init_app(dir: &std::path::Path, app: Option<&str>, name: Option<&str>, template: Option<&str>, roles: &[String], front: Option<Front>) -> anyhow::Result<String> {
    let dir = dir.to_path_buf();
    let app_id = app.map(String::from).or_else(|| dir.file_name().map(|f| f.to_string_lossy().to_string())).unwrap_or("my-app".into());
    if app_id.is_empty() || !app_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { anyhow::bail!("app id `{app_id}`: letters, digits, - and _ only"); }
    if dir.join("cori.toml").exists() { anyhow::bail!("{} is already a Cori app (cori.toml exists)", dir.display()); }
    match template.filter(|t| *t != "blank") {
        Some(t) => {
            // a template whose front is the React app: the scaffold first, then the template's own files over it (its
            // screens, its lockfile, its generated client and its build in dist/, so that a first `cori deploy` needs no npm)
            let react = TEMPLATES.get_file(format!("{t}/front/package.json")).is_some();
            let mut n = 0;
            if react && front != Some(Front::None) {
                let display = name.map(String::from).or_else(|| template_name(t)).unwrap_or_else(|| app_id.clone());
                n += write_react_front(&dir, &app_id, &display)?;
            }
            // the app's own name where the template's shows (its built front's title, its README)
            let renamed = name.zip(template_name(t)).map(|(new, old)| (old, new.to_string()));
            n += write_template(t, &dir, &app_id, renamed.as_ref().map(|(o, n)| (o.as_str(), n.as_str())))?;
            // one conventions text for every app, the one `cori check --conventions` prints (the template ships none)
            std::fs::write(dir.join("AGENTS.md"), cori_compiler::authoring::AGENTS_MD)?;
            if let Some(nm) = name { let p = dir.join("cori.toml"); let text = std::fs::read_to_string(&p)?; let mut done = false; let out = text.lines().map(|l| if !done && l.trim_start().starts_with("name") && l.contains('=') { done = true; format!("name = {}", toml_str(nm)) } else { l.to_string() }).collect::<Vec<_>>().join("\n"); std::fs::write(&p, format!("{out}\n"))?; }
            if !dir.join(".gitignore").exists() { std::fs::write(dir.join(".gitignore"), GITIGNORE)?; }
            // a template keeps the front it ships unless another one is asked for
            let front_note = match (front, react) {
                (Some(Front::None), _) => {
                    let _ = std::fs::remove_dir_all(dir.join("dist"));
                    if react { let _ = std::fs::remove_dir_all(dir.join("front")); }
                    " without its front.".to_string()
                }
                (_, true) => {
                    ignore_dist(&dir)?;
                    mark_built(&dir);
                    ". Its React front is built in dist/, which `cori deploy` ships; after changing it or a workflow: `cd front && npm install && npm run build`.".to_string()
                }
                (Some(Front::React), false) => {
                    let display = toml::from_str::<toml::Value>(&std::fs::read_to_string(dir.join("cori.toml"))?).ok().and_then(|t| t.get("app")?.get("name")?.as_str().map(String::from)).unwrap_or_else(|| app_id.clone());
                    let _ = std::fs::remove_file(dir.join("dist/index.html"));
                    let m = write_react_front(&dir, &app_id, &display)?;
                    ignore_dist(&dir)?;
                    format!(" + a React front ({m} files). {REACT_FRONT_NEXT}.")
                }
                (Some(Front::Plain) | None, false) => ".".to_string(),
            };
            Ok(format!("created {} from template `{t}` ({n} files){front_note} Read AGENTS.md; then `cori check .`, `cori run workflows/<id> k=v`, `cori deploy .`", dir.display()))
        }
        None => {
            // strongest first, as cori.toml lists them (`--roles` takes them in that order)
            let roles: Vec<String> = if roles.is_empty() { vec!["admin".to_string(), "app_user".to_string()] } else { roles.to_vec() };
            std::fs::create_dir_all(dir.join("workflows"))?;
            std::fs::create_dir_all(dir.join("fixtures"))?;
            let front = front.unwrap_or(Front::React);
            let role_map = roles.iter().map(|r| format!("{r} = \"group:{r}\"")).collect::<Vec<_>>().join(", ");
            let front_toml = if front == Front::None { String::new() } else { format!("[front]\nroot = \"dist\"\nallow = {{ roles = [{}] }}\n\n", roles.iter().map(|r| format!("{r:?}")).collect::<Vec<_>>().join(", ")) };
            // one dev persona per role: who the dev widget offers under `cori up` (the first is the default identity: the
            // weakest role, the one most people hold)
            let personas_toml = if front == Front::None { String::new() } else {
                let subs = ["alice", "bob", "carol", "dan", "erin", "frank"];
                format!("\n# `cori up`: the users the dev widget lets you be; keep them in line with the stores' seed\n{}", roles.iter().rev().enumerate().map(|(i, r)| format!("[[dev.personas]]\nsub = {:?}\nroles = [{r:?}]\n", subs.get(i).map(|s| s.to_string()).unwrap_or_else(|| format!("user{i}")))).collect::<Vec<_>>().join("\n"))
            };
            std::fs::write(dir.join("cori.toml"), format!("[app]\nid = \"{app_id}\"\nname = {}\n\n{front_toml}[auth]\n# the app's roles, strongest first: a person holding several acts in the stores as the first listed, so give that\n# role's files everything the roles after it may do\nroles = {{ {role_map} }}\n\n[llm]\nprovider = \"mock\"\nmock = \"fixtures/llm.json\"\n{personas_toml}", toml_str(name.unwrap_or(&app_id))))?;
            std::fs::write(dir.join("AGENTS.md"), cori_compiler::authoring::AGENTS_MD)?;
            std::fs::write(dir.join("README.md"), format!("# {}\n\nA Cori app: workflows under `workflows/`, dev fixtures under `fixtures/`, the front under `front/` (built into `dist/`).\nSee `AGENTS.md` for the layout and the commands.\n", name.unwrap_or(&app_id)))?;
            std::fs::write(dir.join(".gitignore"), if front == Front::React { GITIGNORE_BUILT_FRONT } else { GITIGNORE })?;
            std::fs::write(dir.join("fixtures/llm.json"), "{}\n")?;
            let front_note = match front {
                Front::React => { write_react_front(&dir, &app_id, name.unwrap_or(&app_id))?; format!(" {REACT_FRONT_NEXT}.") }
                Front::Plain => {
                    std::fs::create_dir_all(dir.join("dist"))?;
                    let html_name = name.unwrap_or(&app_id).replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                    std::fs::write(dir.join("dist/index.html"), format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{0}</title></head><body><h1>{0}</h1><p>Front hosted by Cori. Replace this file with your app's build (dist/).</p></body></html>\n", html_name))?;
                    String::new()
                }
                Front::None => " No front.".to_string(),
            };
            Ok(format!("created {} (roles: {}).{front_note} Read AGENTS.md; then `cori workflow new <id> …`, `cori scaffold …`, `cori check workflows/<id>`, `cori run …`, `cori deploy .`", dir.display(), roles.join(", ")))
        }
    }
}


/// Writes template `name` into `dest`, its id replaced by `app_id` wherever it appears (cori.toml, package names, the
/// generated client, the built front), and its display name by the app's (`display`: the template's, the app's) in the
/// text a person reads (the built front's title and strings, the README); cori.toml's `name` is set by the caller.
fn write_template(name: &str, dest: &PathBuf, app_id: &str, display: Option<(&str, &str)>) -> anyhow::Result<usize> {
    let dir = TEMPLATES.get_dir(name).ok_or_else(|| anyhow::anyhow!("unknown template `{name}` (available: {})", TEMPLATES.dirs().map(|d| d.path().display().to_string()).collect::<Vec<_>>().join(", ")))?;
    fn walk(d: &include_dir::Dir, name: &str, dest: &PathBuf, app_id: &str, display: Option<(&str, &str)>, n: &mut usize) -> anyhow::Result<()> {
        for f in d.files().filter(|f| shipped(f.path())) {
            let rel = f.path().strip_prefix(name)?.to_path_buf();
            let out = dest.join(&rel);
            if let Some(p) = out.parent() { std::fs::create_dir_all(p)?; }
            match f.contents_utf8() {
                Some(t) => {
                    let mut t = t.replace(name, app_id);
                    if let Some((old, new)) = display && let Some(new) = display_in(&rel, new) { t = t.replace(old, &new); }
                    std::fs::write(&out, t)?
                }
                None => std::fs::write(&out, f.contents())?,
            }
            *n += 1;
        }
        for sub in d.dirs() { walk(sub, name, dest, app_id, display, n)?; }
        Ok(())
    }
    let mut n = 0; walk(dir, name, dest, app_id, display, &mut n)?; Ok(n)
}

/// The app's display name as it goes into a template file `rel`: HTML-escaped in a page, as is in the README and the
/// built scripts (inside their string literals, so a name with quotes, backslashes or `$` stays out of those), and in no
/// other file (cori.toml's `name` is set apart).
fn display_in(rel: &std::path::Path, name: &str) -> Option<String> {
    match rel.extension().and_then(|e| e.to_str()) {
        Some("html") => Some(name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")),
        Some("md") => Some(name.to_string()),
        Some("js") if !name.contains(['"', '\'', '`', '\\', '$', '<']) => Some(name.to_string()),
        _ => None,
    }
}

/// `s` as a TOML string, escaped as TOML escapes (a display name from the server may hold any character).
fn toml_str(s: &str) -> String { toml::Value::String(s.to_string()).to_string() }

/// A template's display name (`[app] name` of its cori.toml).
fn template_name(t: &str) -> Option<String> {
    let text = TEMPLATES.get_file(format!("{t}/cori.toml"))?.contents_utf8()?;
    toml::from_str::<toml::Value>(text).ok()?.get("app")?.get("name")?.as_str().map(String::from)
}

/// Dates `dist/` after the files it was built from, which a template writes after it: `cori deploy` warns when a source
/// is newer than the build.
fn mark_built(dir: &std::path::Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(dir.join("dist/index.html")) { let _ = f.set_modified(std::time::SystemTime::now()); }
}

#[cfg(test)]
mod init_tests {
    use super::*;
    /// One conventions text: `cori init` writes the one `cori check --conventions` prints, from a template too, which
    /// ships no copy of its own to drift from it.
    #[test]
    fn every_agents_md_is_the_conventions_text() {
        for t in TEMPLATES.dirs() {
            let t = t.path().display().to_string();
            assert!(TEMPLATES.get_file(format!("{t}/AGENTS.md")).is_none(), "templates/{t}/AGENTS.md: `cori init` writes cori_compiler::authoring::AGENTS_MD");
            let d = tempdir(&format!("agents-{t}"));
            init_app(&d, Some("desk"), None, Some(&t), &[], Some(Front::None)).unwrap();
            assert_eq!(std::fs::read_to_string(d.join("AGENTS.md")).unwrap(), cori_compiler::authoring::AGENTS_MD);
        }
    }

    /// The starter template's front is the React scaffold with its own screens over it, already built: a first deploy
    /// needs no npm. scripts/template-front.sh keeps the build in line with the sources (CI checks it).
    #[test]
    fn a_react_template_ships_its_screens_and_its_build() {
        let d = tempdir("react-tpl");
        init_app(&d, Some("desk"), None, Some("team-requests"), &[], None).unwrap();
        for f in ["front/vite.config.ts", "front/tsconfig.json", "front/src/App.tsx", "front/src/components/ui.tsx", "front/vendor/cori-client/index.ts", "front/src/views/NewRequest.tsx", "front/package-lock.json", "dist/index.html", "dist/THIRD-PARTY-NOTICES.md"] { assert!(d.join(f).is_file(), "{f} missing"); }
        let client = std::fs::read_to_string(d.join("front/src/lib/cori.ts")).unwrap();
        assert!(client.contains("export const submitRequest") && client.contains(r#"createCoriClient({ app: "desk" })"#), "the template's generated client, for this app: {client}");
        // the template's id becomes the app's everywhere, in the built front too (its client names the app); AGENTS.md is
        // the conventions text, which names the templates
        let all = walkdir::WalkDir::new(&d).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file() && e.file_name() != "AGENTS.md").map(|e| std::fs::read_to_string(e.path()).unwrap_or_default()).collect::<String>();
        assert!(!all.contains("team-requests") && !all.contains("__APP_"), "the template's id or a placeholder was left");
        assert!(std::fs::read_to_string(d.join(".gitignore")).unwrap().lines().any(|l| l == "dist/"));
        assert!(commands::front_build_warning(&d).is_none(), "the build is dated after the sources it was made from");
        let d = tempdir("react-tpl-none");
        init_app(&d, Some("desk"), None, Some("team-requests"), &[], Some(Front::None)).unwrap();
        assert!(!d.join("front").exists() && !d.join("dist").exists());
        // an app of its own name shows it, not the template's, in its front and its built front
        let d = tempdir("react-tpl-named");
        init_app(&d, Some("timesheets"), Some("Timesheets & Co"), Some("team-requests"), &[], None).unwrap();
        let template = template_name("team-requests").unwrap();
        for f in ["front/index.html", "dist/index.html"] { assert!(std::fs::read_to_string(d.join(f)).unwrap().contains("<title>Timesheets &amp; Co</title>"), "{f}"); }
        let all = walkdir::WalkDir::new(&d).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file() && e.file_name() != "AGENTS.md").map(|e| std::fs::read_to_string(e.path()).unwrap_or_default()).collect::<String>();
        assert!(!all.contains(&template), "`{template}` was left");
        assert!(std::fs::read_to_string(d.join("cori.toml")).unwrap().contains("name = \"Timesheets & Co\""));
        // a name with any character stays a TOML string
        let d = tempdir("odd-name");
        init_app(&d, Some("odd"), Some("Tab\there \"quoted\" \u{1b} \\ end"), None, &[], Some(Front::None)).unwrap();
        let t: toml::Value = toml::from_str(&std::fs::read_to_string(d.join("cori.toml")).unwrap()).expect("valid TOML");
        assert_eq!(t["app"]["name"].as_str(), Some("Tab\there \"quoted\" \u{1b} \\ end"));
    }

    #[test]
    fn generated_and_os_files_are_not_shipped() {
        for p in ["support-desk/workflows/x/.cori/deno.json", "node_modules/zod/index.js", "support-desk/.DS_Store"] { assert!(!shipped(std::path::Path::new(p)), "{p}"); }
        for p in ["support-desk/cori.toml", "support-desk/workflows/x/steps/a.ts", "src/lib/cori.ts"] { assert!(shipped(std::path::Path::new(p)), "{p}"); }
    }
    fn tempdir(n: &str) -> PathBuf { let p = std::env::temp_dir().join(format!("cori-init-test-{n}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&p); std::fs::create_dir_all(&p).unwrap(); p.join("app") }

    #[test]
    fn blank_app_gets_a_react_front() {
        let d = tempdir("react");
        init_app(&d, Some("my-app"), Some("My <App>"), None, &["agent".into()], None).unwrap();
        for f in ["front/package.json", "front/vite.config.ts", "front/tsconfig.json", "front/src/App.tsx", "front/src/lib/cori.ts", "front/vendor/cori-client/index.ts", "front/vendor/cori-client/react.ts", "dist/index.html"] { assert!(d.join(f).is_file(), "{f} missing"); }
        let all = walkdir::WalkDir::new(d.join("front")).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).map(|e| std::fs::read_to_string(e.path()).unwrap_or_default()).collect::<String>();
        assert!(!all.contains("__APP_"), "a placeholder was left in front/");
        assert!(std::fs::read_to_string(d.join("front/vite.config.ts")).unwrap().contains(r#"const APP = "my-app";"#));
        assert!(std::fs::read_to_string(d.join("front/index.html")).unwrap().contains("<title>My &lt;App&gt;</title>"));
        assert!(std::fs::read_to_string(d.join(".gitignore")).unwrap().lines().any(|l| l == "dist/"));
        assert!(std::fs::read_to_string(d.join("cori.toml")).unwrap().contains("[front]"));
        assert!(std::fs::read_to_string(d.join("cori.toml")).unwrap().contains("[[dev.personas]]"));
    }

    #[test]
    fn plain_and_none_fronts() {
        let d = tempdir("plain");
        init_app(&d, Some("p"), None, None, &[], Some(Front::Plain)).unwrap();
        assert!(d.join("dist/index.html").is_file() && !d.join("front").exists());
        assert!(!std::fs::read_to_string(d.join(".gitignore")).unwrap().contains("dist/"));
        let d = tempdir("none");
        init_app(&d, Some("n"), None, None, &[], Some(Front::None)).unwrap();
        assert!(!d.join("dist").exists() && !d.join("front").exists());
        assert!(!std::fs::read_to_string(d.join("cori.toml")).unwrap().contains("[front]"));
    }

    #[test]
    fn template_keeps_its_front_unless_asked() {
        let d = tempdir("tpl");
        init_app(&d, Some("desk"), None, Some("support-desk"), &[], None).unwrap();
        assert!(d.join("dist/index.html").is_file() && !d.join("front/package.json").exists());
        assert!(!d.join("workflows/support_ticket_triage/.cori").exists(), "a checkout's generated .cori/ reached the new app");
        let d = tempdir("tpl-react");
        init_app(&d, Some("desk"), None, Some("support-desk"), &[], Some(Front::React)).unwrap();
        assert!(d.join("front/package.json").is_file());
        assert!(std::fs::read_to_string(d.join(".gitignore")).unwrap().lines().any(|l| l == "dist/"));
    }
}
