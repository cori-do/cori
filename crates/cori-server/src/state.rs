use crate::{auth::factory::TokenFactory, config::AppConfig};
use oxide_auth::primitives::{
    authorizer::AuthMap,
    generator::RandomGenerator,
    issuer::TokenMap,
    registrar::ClientMap,
};
use sqlx::SqlitePool;
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::RwLock;

/// Shared application state.
///
/// Note: This crate is currently a minimal server stub; we keep auth state here to
/// support the embedded Pocket IDP + Double-Lock sessions.
pub struct AppState {
    pub cfg: AppConfig,
    pub auth_db: SqlitePool,
    pub token_factory: TokenFactory,

    /// Device-code / login-flow state: device_code -> agent_id
    pub pending_logins: RwLock<std::collections::HashMap<String, String>>,

    /// Owner identity captured during OAuth consent: device_code(state) -> user_id
    pub device_users: Arc<Mutex<std::collections::HashMap<String, String>>>,

    /// Embedded OAuth2 server primitives (oxide-auth).
    pub oauth_clients: tokio::sync::Mutex<ClientMap>,
    pub oauth_authorizer: tokio::sync::Mutex<AuthMap<RandomGenerator>>,
    pub oauth_issuer: tokio::sync::Mutex<TokenMap<RandomGenerator>>,

    /// Owner solicitor backed by SQLite (oxide-auth requires a `&mut` solicitor).
    pub oauth_solicitor: Mutex<crate::auth::solicitor::DatabaseSolicitor>,
}

impl AppState {
    pub async fn init(cfg: &AppConfig) -> anyhow::Result<Self> {
        ensure_parent_dir(&cfg.server.auth_sqlite_path)?;
        let db_url = sqlite_url(&cfg.server.auth_sqlite_path);
        let pool = SqlitePool::connect(&db_url).await?;

        // Migrations (creates local_users + oauth_codes + device_tokens)
        sqlx::migrate!("./migrations").run(&pool).await?;

        // Bootstrap admin if no users exist
        crate::auth::bootstrap::bootstrap_admin(&pool, cfg).await?;

        let device_users = Arc::new(Mutex::new(std::collections::HashMap::new()));

        Ok(Self {
            cfg: cfg.clone(),
            auth_db: pool.clone(),
            token_factory: TokenFactory::new(),
            pending_logins: RwLock::new(std::collections::HashMap::new()),
            device_users: device_users.clone(),
            oauth_clients: tokio::sync::Mutex::new(ClientMap::new()),
            oauth_authorizer: tokio::sync::Mutex::new(AuthMap::new(RandomGenerator::new(32))),
            oauth_issuer: tokio::sync::Mutex::new(TokenMap::new(RandomGenerator::new(32))),
            oauth_solicitor: Mutex::new(crate::auth::solicitor::DatabaseSolicitor::new(
                pool,
                device_users,
            )),
        })
    }
}

fn sqlite_url(path: &str) -> String {
    // sqlx sqlite URL format: sqlite://relative/path.db (or sqlite:/abs/path.db)
    if Path::new(path).is_absolute() {
        format!("sqlite:{}", path)
    } else {
        format!("sqlite://{}", path)
    }
}

fn ensure_parent_dir(file_path: &str) -> anyhow::Result<()> {
    let p = Path::new(file_path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}


