#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct SapLoginFixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
}

impl SapLoginFixture {
    fn new(origin: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("cori-home");
        fs::create_dir_all(&home).expect("Cori home");
        write_sap_config(&home, origin);
        Self { _dir: dir, home }
    }

    fn login(&self, token: &str, allow_test_file_store: bool) -> Output {
        let mut command = self.command();
        command
            .args(["login", "cori-sap", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if allow_test_file_store {
            command.env("CORI_SAP_ALLOW_INSECURE_FILE_STORE", "1");
        }
        let mut child = command.spawn().expect("spawn cori login");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(format!("{token}\n").as_bytes())
            .expect("write token");
        child.wait_with_output().expect("wait for cori login")
    }

    fn logout(&self) -> Output {
        self.command()
            .args(["logout", "cori-sap"])
            .env("CORI_SAP_ALLOW_INSECURE_FILE_STORE", "1")
            .output()
            .expect("run cori logout")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cori"));
        command
            .env("CORI_HOME", &self.home)
            .env("CORI_SECRETS_BACKEND", "file")
            // Keep successful login notification from auto-spawning Temporal.
            .env("CORI_TEMPORAL_TARGET", "http://127.0.0.1:1");
        command
    }

    fn stored_secrets(&self) -> BTreeMap<String, String> {
        let path = self.home.join("credentials/llm-secrets.json");
        let source = fs::read_to_string(path).expect("secret file");
        serde_json::from_str(&source).expect("secret map")
    }
}

fn write_sap_config(home: &Path, origin: &str) {
    fs::write(
        home.join("sap.toml"),
        format!(
            r#"default_profile = "production"

[profiles.production]
base_url = "{origin}"
sap_client = "100"
"#
        ),
    )
    .expect("SAP config");
}

#[test]
fn cori_login_stores_an_owner_and_target_bound_sap_token_without_delegation() {
    let fixture = SapLoginFixture::new("https://tenant.example.com");
    let token = "sap-secret-token";
    let output = fixture.login(token, true);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("profile: production"), "stdout: {stdout}");
    assert!(
        stdout.contains("origin=https://tenant.example.com|sap_client=100"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("secure credential store"),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains(token), "stdout leaked token: {stdout}");
    assert!(!stderr.contains(token), "stderr leaked token: {stderr}");

    let secrets = fixture.stored_secrets();
    assert_eq!(secrets.len(), 1);
    let (account, value) = secrets.first_key_value().expect("SAP account");
    assert!(account.starts_with("sap/access-token/v1/user-"));
    assert!(account.contains("/target-"));
    assert!(!account.contains("tenant.example.com"));
    assert_eq!(value, token);
}

#[test]
fn sap_login_refuses_the_general_plaintext_file_fallback() {
    let fixture = SapLoginFixture::new("https://tenant.example.com");
    let output = fixture.login("must-not-be-stored", false);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OS keychain"), "stderr: {stderr}");
    assert!(!stderr.contains("must-not-be-stored"));
    assert!(!fixture.home.join("credentials/llm-secrets.json").exists());
}

#[test]
fn changing_the_sap_target_uses_a_different_credential_account() {
    let fixture = SapLoginFixture::new("https://tenant-a.example.com");
    assert!(fixture.login("token-for-a", true).status.success());
    write_sap_config(&fixture.home, "https://tenant-b.example.com");
    assert!(fixture.login("token-for-b", true).status.success());

    let secrets = fixture.stored_secrets();
    assert_eq!(secrets.len(), 2);
    let values: Vec<_> = secrets.values().map(String::as_str).collect();
    assert!(values.contains(&"token-for-a"));
    assert!(values.contains(&"token-for-b"));
}

#[test]
fn sap_logout_removes_orphaned_target_tokens_even_if_config_is_malformed() {
    let fixture = SapLoginFixture::new("https://tenant.example.com");
    let login = fixture.login("sap-secret-token", true);
    assert!(login.status.success());
    fs::write(fixture.home.join("sap.toml"), "not valid toml = [")
        .expect("break SAP config after storing");

    let output = fixture.logout();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.stored_secrets().is_empty());
}

#[test]
fn cori_fails_closed_when_the_worker_environment_contains_a_sap_token() {
    let secret = "ambient-token-must-not-leak";
    let output = Command::new(env!("CARGO_BIN_EXE_cori"))
        .arg("status")
        .env("SAP_ACCESS_TOKEN", secret)
        .output()
        .expect("run cori with ambient SAP token");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unset it and use `cori login cori-sap`"));
    assert!(!stdout.contains(secret));
    assert!(!stderr.contains(secret));
}
