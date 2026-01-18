//! Keypair management for Biscuit tokens.

use crate::error::BiscuitError;
use biscuit_auth::{Algorithm, KeyPair as BiscuitKeyPair, PrivateKey, PublicKey};
use rand::RngCore;
use std::path::Path;

/// An Ed25519 keypair for signing and verifying Biscuit tokens.
pub struct KeyPair {
    inner: BiscuitKeyPair,
}

impl Clone for KeyPair {
    fn clone(&self) -> Self {
        // Recreate from the private key
        let private_key_hex = self.private_key_hex();
        Self::from_private_key_hex(&private_key_hex).expect("key should be valid")
    }
}

impl KeyPair {
    /// Generate a new random keypair.
    pub fn generate() -> Result<Self, BiscuitError> {
        // Generate random bytes for the private key
        let mut rng = rand::rng();
        let mut bytes = [0u8; 32];
        rng.fill_bytes(&mut bytes);

        let private_key = PrivateKey::from_bytes(&bytes, Algorithm::Ed25519)
            .map_err(|e| BiscuitError::KeyGenerationFailed(e.to_string()))?;
        let inner = BiscuitKeyPair::from(&private_key);

        Ok(Self { inner })
    }

    /// Create a keypair from an existing private key.
    pub fn from_private_key(private_key: PrivateKey) -> Self {
        let inner = BiscuitKeyPair::from(&private_key);
        Self { inner }
    }

    /// Load a keypair from private key bytes.
    pub fn from_private_key_bytes(bytes: &[u8]) -> Result<Self, BiscuitError> {
        let private_key = PrivateKey::from_bytes(bytes, Algorithm::Ed25519)
            .map_err(|e| BiscuitError::InvalidPrivateKey(e.to_string()))?;
        Ok(Self::from_private_key(private_key))
    }

    /// Load a keypair from a hex-encoded private key string.
    pub fn from_private_key_hex(hex: &str) -> Result<Self, BiscuitError> {
        let private_key = PrivateKey::from_bytes_hex(hex, Algorithm::Ed25519)
            .map_err(|e| BiscuitError::InvalidPrivateKey(e.to_string()))?;
        Ok(Self::from_private_key(private_key))
    }

    /// Get the inner biscuit keypair.
    pub fn inner(&self) -> &BiscuitKeyPair {
        &self.inner
    }

    /// Get the private key.
    pub fn private_key(&self) -> PrivateKey {
        self.inner.private()
    }

    /// Get the public key.
    pub fn public_key(&self) -> PublicKey {
        self.inner.public()
    }

    /// Get the private key as hex string.
    pub fn private_key_hex(&self) -> String {
        self.inner.private().to_bytes_hex()
    }

    /// Get the public key as hex string.
    pub fn public_key_hex(&self) -> String {
        self.inner.public().to_bytes_hex()
    }

    /// Save the keypair to files.
    pub fn save_to_files(
        &self,
        private_key_path: &Path,
        public_key_path: &Path,
    ) -> Result<(), BiscuitError> {
        std::fs::write(private_key_path, self.private_key_hex())?;
        std::fs::write(public_key_path, self.public_key_hex())?;
        Ok(())
    }

    /// Load a keypair from a private key file.
    pub fn load_from_file(private_key_path: &Path) -> Result<Self, BiscuitError> {
        let hex = std::fs::read_to_string(private_key_path)?;
        Self::from_private_key_hex(hex.trim())
    }
}

/// Load a public key from hex string (for verification-only scenarios).
pub fn load_public_key_hex(hex: &str) -> Result<PublicKey, BiscuitError> {
    PublicKey::from_bytes_hex(hex, Algorithm::Ed25519)
        .map_err(|e| BiscuitError::InvalidPublicKey(e.to_string()))
}

/// Load a public key from a file.
pub fn load_public_key_file(path: &Path) -> Result<PublicKey, BiscuitError> {
    let hex = std::fs::read_to_string(path)?;
    load_public_key_hex(hex.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_keypair_generation() {
        let keypair = KeyPair::generate().unwrap();
        assert!(!keypair.private_key_hex().is_empty());
        assert!(!keypair.public_key_hex().is_empty());
    }

    #[test]
    fn test_keypair_roundtrip() {
        let keypair1 = KeyPair::generate().unwrap();
        let hex = keypair1.private_key_hex();

        let keypair2 = KeyPair::from_private_key_hex(&hex).unwrap();
        assert_eq!(keypair1.public_key_hex(), keypair2.public_key_hex());
    }

    #[test]
    fn test_keypair_file_save_load() {
        let keypair = KeyPair::generate().unwrap();

        let mut priv_file = NamedTempFile::new().unwrap();
        let mut pub_file = NamedTempFile::new().unwrap();

        writeln!(priv_file, "{}", keypair.private_key_hex()).unwrap();
        writeln!(pub_file, "{}", keypair.public_key_hex()).unwrap();

        let loaded = KeyPair::load_from_file(priv_file.path()).unwrap();
        assert_eq!(keypair.public_key_hex(), loaded.public_key_hex());
    }
}
