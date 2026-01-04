use crate::config::AppConfig;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use sqlx::SqlitePool;

/// On startup, if the DB is empty, create a default `admin` user.
///
/// Password source (highest precedence first):
/// - env `CORI_AUTH_EMBEDDED_ADMIN_PASSWORD`
/// - `config.toml` `[auth.embedded].admin_password`
pub async fn bootstrap_admin(pool: &SqlitePool, cfg: &AppConfig) -> anyhow::Result<()> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(1) FROM local_users")
        .fetch_one(pool)
        .await?;

    if count.0 > 0 {
        return Ok(());
    }

    let admin_password = std::env::var("CORI_AUTH_EMBEDDED_ADMIN_PASSWORD")
        .unwrap_or_else(|_| cfg.auth.embedded.admin_password.clone());

    if admin_password.trim().is_empty() {
        anyhow::bail!("embedded auth admin password is empty (set CORI_AUTH_EMBEDDED_ADMIN_PASSWORD or config.toml [auth.embedded].admin_password)");
    }

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(admin_password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .to_string();

    sqlx::query("INSERT INTO local_users (id, password_hash, roles) VALUES (?, ?, ?)")
        .bind("admin")
        .bind(hash)
        .bind("admin")
        .execute(pool)
        .await?;

    tracing::warn!("bootstrapped embedded auth user 'admin' (password taken from env/config)");
    Ok(())
}


