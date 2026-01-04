use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct LocalUser {
    pub id: String,            // Primary Key (e.g., "alice")
    pub password_hash: String, // Argon2 PHC string
    #[allow(dead_code)]
    pub roles: String,         // Comma-separated (e.g., "admin,viewer") - future use
}

/// Hardcoded for MVP, but can become DB-backed.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct RegisteredClient {
    pub client_id: String,
    pub redirect_uri: String,
}


