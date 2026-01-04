use crate::auth::models::LocalUser;
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use oxide_auth::endpoint::{OwnerConsent, OwnerSolicitor, Solicitation, WebRequest, WebResponse};
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};

/// Oxide Auth owner solicitor backed by SQLite.
///
/// This implements a minimal login screen and checks credentials against the
/// `local_users` table (Argon2 PHC hashes).
pub struct DatabaseSolicitor {
    pool: SqlitePool,
    /// Device-code-ish flow helper: capture successful owner identity keyed by OAuth `state`.
    device_users: Arc<Mutex<std::collections::HashMap<String, String>>>,
}

impl DatabaseSolicitor {
    pub fn new(
        pool: SqlitePool,
        device_users: Arc<Mutex<std::collections::HashMap<String, String>>>,
    ) -> Self {
        Self { pool, device_users }
    }

    async fn verify_user(&self, username: &str, password: &str) -> anyhow::Result<Option<String>> {
        let user = sqlx::query_as::<_, LocalUser>("SELECT id, password_hash, roles FROM local_users WHERE id = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;

        let Some(user) = user else {
            return Ok(None);
        };

        let parsed_hash = PasswordHash::new(&user.password_hash).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        if Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok()
        {
            Ok(Some(user.id))
        } else {
            Ok(None)
        }
    }
}

impl<R> OwnerSolicitor<R> for DatabaseSolicitor
where
    R: WebRequest + Send + Sync,
    R::Response: Default,
{
    fn check_consent(&mut self, req: &mut R, solicitation: Solicitation) -> OwnerConsent<R::Response> {
        // If no credentials are present, show the login form.
        let maybe_params = req.urlbody().ok();
        let username = maybe_params
            .as_ref()
            .and_then(|p| p.unique_value("username"))
            .unwrap_or_default()
            .to_string();
        let password = maybe_params
            .as_ref()
            .and_then(|p| p.unique_value("password"))
            .unwrap_or_default()
            .to_string();

        if username.is_empty() || password.is_empty() {
            return login_form::<R>(solicitation, None);
        }

        // OwnerSolicitor is sync; we use block_in_place to safely call async DB lookup.
        // This moves the task to a blocking thread, avoiding nested runtime errors.
        let verified = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.verify_user(&username, &password))
        });

        let user_id = match verified {
            Ok(Some(uid)) => uid,
            _ => return login_form::<R>(solicitation, Some("Invalid credentials")),
        };

        // Capture owner identity keyed by OAuth state (device-code-ish flow).
        if let Some(state) = solicitation.state() {
            if let Ok(mut guard) = self.device_users.lock() {
                guard.insert(state.to_string(), user_id.clone());
            }
        }

        OwnerConsent::Authorized(user_id)
    }
}

fn login_form<R: WebRequest>(solicitation: Solicitation, error: Option<&str>) -> OwnerConsent<R::Response>
where
    R::Response: Default,
{
    let pg = solicitation.pre_grant();
    let state = solicitation.state().unwrap_or_default();
    let scope = pg.scope.to_string();
    let redirect_uri = pg.redirect_uri.as_str().to_string();
    let client_id = pg.client_id.clone();

    let action = format!(
        "/oidc/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&scope),
        urlencoding::encode(state),
    );

    let error_html = error.unwrap_or_default();
    let body = format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Cori Pocket IDP</title>
    <style>
      body {{ font-family: system-ui, -apple-system, Segoe UI, Roboto, sans-serif; padding: 2rem; max-width: 520px; margin: 0 auto; }}
      .card {{ border: 1px solid #ddd; border-radius: 12px; padding: 1.25rem; }}
      label {{ display: block; margin-top: 0.75rem; font-weight: 600; }}
      input {{ width: 100%; padding: 0.6rem; border: 1px solid #ccc; border-radius: 10px; margin-top: 0.25rem; }}
      button {{ margin-top: 1rem; width: 100%; padding: 0.7rem; border: 0; border-radius: 10px; background: #111; color: #fff; font-weight: 700; cursor: pointer; }}
      .error {{ color: #b00020; margin-top: 0.75rem; }}
      .hint {{ color: #555; margin-top: 0.75rem; font-size: 0.9rem; }}
    </style>
  </head>
  <body>
    <h1>Pocket IDP</h1>
    <div class="card">
      <form method="post" action="{action}">
        <label for="username">Username</label>
        <input id="username" name="username" autocomplete="username" />

        <label for="password">Password</label>
        <input id="password" name="password" type="password" autocomplete="current-password" />

        <button type="submit">Sign in</button>
        <div class="error">{error_html}</div>
        <div class="hint">Default user: <code>admin</code>. Password comes from env <code>CORI_AUTH_EMBEDDED_ADMIN_PASSWORD</code> or <code>config.toml</code>.</div>
      </form>
    </div>
  </body>
</html>"#
    );

    let mut resp: R::Response = Default::default();
    if let Err(e) = resp.ok() {
        return OwnerConsent::Error(e);
    }
    // Use body_text even for HTML (oxide-auth-axum will just return the body string).
    if let Err(e) = resp.body_text(&body) {
        return OwnerConsent::Error(e);
    }

    OwnerConsent::InProgress(resp)
}


