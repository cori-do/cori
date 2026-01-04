-- Embedded Pocket IDP storage (SQLite)

CREATE TABLE IF NOT EXISTS local_users (
  id TEXT PRIMARY KEY NOT NULL,
  password_hash TEXT NOT NULL,
  roles TEXT NOT NULL
);

-- Authorization codes for OAuth2 authorization_code flow
CREATE TABLE IF NOT EXISTS oauth_codes (
  code TEXT PRIMARY KEY NOT NULL,
  user_id TEXT NOT NULL,
  client_id TEXT NOT NULL,
  redirect_uri TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

-- Device-code-ish polling: minted Biscuit tokens are stored here briefly
CREATE TABLE IF NOT EXISTS device_tokens (
  device_code TEXT PRIMARY KEY NOT NULL,
  agent_id TEXT NOT NULL,
  token TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);


