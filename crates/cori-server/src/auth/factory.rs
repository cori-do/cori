use biscuit_auth::{Biscuit, KeyPair};
use uuid::Uuid;

/// SessionManager / TokenFactory for Double-Lock session tokens.
///
/// Security constraint: `mint_double_lock_token` is **internal kernel logic** and
/// should only be invoked after a successful OAuth callback.
pub struct TokenFactory {
    root_key: KeyPair,
}

impl TokenFactory {
    pub fn new() -> Self {
        // MVP: ephemeral key per process. Production: load from secure storage.
        Self {
            root_key: KeyPair::new(),
        }
    }

    pub fn public_key(&self) -> biscuit_auth::PublicKey {
        self.root_key.public()
    }

    pub fn mint_double_lock_token(&self, agent_id: &str, user_id: &str) -> anyhow::Result<String> {
        let session_id = Uuid::new_v4().to_string();
        let expiry_unix = unix_now_seconds()
            .checked_add(3600)
            .ok_or_else(|| anyhow::anyhow!("time overflow computing expiry"))?;

        // Build token with all facts using code() method
        let builder = Biscuit::builder()
            .code(format!(
                r#"
                agent("{agent_id}");
                user("{user_id}");
                session_id("{session_id}");
                expiry({expiry_unix});
                "#
            ))?;

        let token = builder.build(&self.root_key)?;
        let b64 = token.to_base64()?;
        Ok(b64)
    }
}

fn unix_now_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}


