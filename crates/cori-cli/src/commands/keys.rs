//! Key management commands.
//!
//! `cori keys generate` - Generate a new Biscuit keypair.

use cori_biscuit::KeyPair;
use std::fs;
use std::path::PathBuf;

/// Generate a new Biscuit keypair.
pub fn generate(output: Option<PathBuf>) -> anyhow::Result<()> {
    let keypair = KeyPair::generate()?;

    if let Some(output_dir) = output {
        // Create output directory if it doesn't exist
        fs::create_dir_all(&output_dir)?;

        let private_path = output_dir.join("private.key");
        let public_path = output_dir.join("public.key");

        fs::write(&private_path, keypair.private_key_hex())?;
        fs::write(&public_path, keypair.public_key_hex())?;

        println!("✔ Generated Biscuit keypair:");
        println!("  Private key: {}", private_path.display());
        println!("  Public key:  {}", public_path.display());
        println!();
        println!("⚠️  Keep your private key secure! Never commit it to version control.");
        println!();
        println!("Set as environment variables:");
        println!(
            "  export BISCUIT_PRIVATE_KEY=$(cat {})",
            private_path.display()
        );
        println!(
            "  export BISCUIT_PUBLIC_KEY=$(cat {})",
            public_path.display()
        );
    } else {
        // Print to stdout
        println!("Private key (keep secure!):");
        println!("{}", keypair.private_key_hex());
        println!();
        println!("Public key:");
        println!("{}", keypair.public_key_hex());
        println!();
        println!("Use --output <dir> to save keys to files.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_generate_keys_to_files() {
        let dir = tempdir().unwrap();
        generate(Some(dir.path().to_path_buf())).unwrap();

        assert!(dir.path().join("private.key").exists());
        assert!(dir.path().join("public.key").exists());

        let private_hex = fs::read_to_string(dir.path().join("private.key")).unwrap();
        let public_hex = fs::read_to_string(dir.path().join("public.key")).unwrap();

        // Hex keys should be 64 characters (32 bytes)
        assert_eq!(private_hex.len(), 64);
        assert_eq!(public_hex.len(), 64);
    }
}
