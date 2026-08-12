use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use url::Url;

use crate::PURCHASE_ORDER_SERVICE_ROOT;
use crate::error::{AdapterError, Result};

const CONFIG_FILE_NAME: &str = "sap.toml";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    default_profile: String,
    profiles: BTreeMap<String, ProfileFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    /// HTTPS origin only. The service path is fixed in adapter code.
    base_url: String,
    #[serde(default)]
    sap_client: Option<String>,
}

/// Validated, machine-owned connection profile.
#[derive(Clone)]
pub struct MachineProfile {
    name: String,
    service_root: Url,
    sap_client: Option<String>,
}

impl MachineProfile {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Stable, non-secret credential binding for this SAP target. The profile
    /// name is deliberately excluded so renaming a local alias does not move
    /// the credential; the canonical HTTPS origin and SAP client are the
    /// actual tenant boundary.
    pub fn credential_scope(&self) -> String {
        let origin = self.service_root.origin().ascii_serialization();
        let sap_client = self.sap_client.as_deref().unwrap_or("default");
        format!("sap-odata-v1|origin={origin}|sap_client={sap_client}")
    }

    pub(crate) fn service_root(&self) -> &Url {
        &self.service_root
    }

    pub(crate) fn sap_client(&self) -> Option<&str> {
        self.sap_client.as_deref()
    }

    fn from_file(name: String, profile: ProfileFile) -> Result<Self> {
        let service_root = validated_service_root(&profile.base_url, false)?;
        let sap_client = validate_sap_client(profile.sap_client)?;
        Ok(Self {
            name,
            service_root,
            sap_client,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_loopback_test(origin: &str) -> Result<Self> {
        Ok(Self {
            name: "test".to_string(),
            service_root: validated_service_root(origin, true)?,
            sap_client: None,
        })
    }
}

/// Load the selected profile from the fixed Cori machine configuration.
/// `explicit_profile` is intended only for interactive invocations; the binary
/// rejects it before calling this function in workflow mode.
pub fn load_machine_profile(explicit_profile: Option<&str>) -> Result<MachineProfile> {
    let path = machine_config_path()?;
    load_profile_from_path(&path, explicit_profile)
}

/// Load the machine-configured default profile from an explicit Cori home.
/// Broker activities use this instead of ambient `CORI_HOME`, binding profile
/// resolution to the already owner-scoped credentials directory.
pub fn load_default_machine_profile_from_cori_home(cori_home: &Path) -> Result<MachineProfile> {
    if !cori_home.is_absolute() {
        return Err(AdapterError::ConfigUnsafe {
            reason: "the broker-provided Cori home must be an absolute path",
        });
    }
    load_profile_from_path(&cori_home.join(CONFIG_FILE_NAME), None)
}

fn machine_config_path() -> Result<PathBuf> {
    let cori_home = match std::env::var_os("CORI_HOME") {
        Some(value) if !value.is_empty() => {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(AdapterError::ConfigUnsafe {
                    reason: "CORI_HOME must be an absolute path",
                });
            }
            path
        }
        _ => dirs::home_dir()
            .ok_or(AdapterError::ConfigUnsafe {
                reason: "the operating-system home directory is unavailable",
            })?
            .join(".cori"),
    };
    Ok(cori_home.join(CONFIG_FILE_NAME))
}

fn load_profile_from_path(path: &Path, explicit_profile: Option<&str>) -> Result<MachineProfile> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| AdapterError::ConfigMissing {
        path: path.to_path_buf(),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AdapterError::ConfigUnsafe {
            reason: "sap.toml must be a regular file, not a symlink",
        });
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(AdapterError::ConfigUnsafe {
            reason: "sap.toml exceeds the 64 KiB limit",
        });
    }
    reject_group_or_world_writable(&metadata)?;

    let file = std::fs::File::open(path).map_err(|_| AdapterError::ConfigUnsafe {
        reason: "sap.toml could not be opened",
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AdapterError::ConfigUnsafe {
            reason: "sap.toml could not be read",
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CONFIG_BYTES {
        return Err(AdapterError::ConfigUnsafe {
            reason: "sap.toml exceeds the 64 KiB limit",
        });
    }
    let source = std::str::from_utf8(&bytes).map_err(|_| AdapterError::ConfigMalformed)?;
    parse_profile(source, explicit_profile)
}

#[cfg(unix)]
fn reject_group_or_world_writable(metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    if metadata.permissions().mode() & 0o022 != 0 {
        return Err(AdapterError::ConfigUnsafe {
            reason: "sap.toml must not be group- or world-writable",
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn reject_group_or_world_writable(_metadata: &std::fs::Metadata) -> Result<()> {
    Ok(())
}

fn parse_profile(source: &str, explicit_profile: Option<&str>) -> Result<MachineProfile> {
    let config: ConfigFile = toml::from_str(source).map_err(|_| AdapterError::ConfigMalformed)?;
    for name in config.profiles.keys() {
        validate_profile_name(name)?;
    }
    validate_profile_name(&config.default_profile)?;

    let selected = explicit_profile.unwrap_or(&config.default_profile);
    validate_profile_name(selected)?;
    let profile = config
        .profiles
        .into_iter()
        .find(|(name, _)| name == selected)
        .map(|(_, profile)| profile)
        .ok_or_else(|| AdapterError::ProfileMissing {
            profile: selected.to_string(),
        })?;
    MachineProfile::from_file(selected.to_string(), profile)
}

fn validate_profile_name(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return Err(AdapterError::InvalidProfileName);
    };
    if name.len() > 64
        || !first.is_ascii_lowercase() && !first.is_ascii_digit()
        || !bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(AdapterError::InvalidProfileName);
    }
    Ok(())
}

fn validate_sap_client(value: Option<String>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() != 3 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AdapterError::InvalidSapClient);
    }
    Ok(Some(value))
}

fn validated_service_root(value: &str, allow_http_loopback: bool) -> Result<Url> {
    let mut url = Url::parse(value).map_err(|_| AdapterError::InvalidEndpoint {
        reason: "base_url must be an absolute HTTPS origin",
    })?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AdapterError::InvalidEndpoint {
            reason: "base_url must not contain credentials",
        });
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(AdapterError::InvalidEndpoint {
            reason: "base_url must not contain a query or fragment",
        });
    }
    if !matches!(url.path(), "" | "/") {
        return Err(AdapterError::InvalidEndpoint {
            reason: "base_url must be an origin without a path",
        });
    }
    let host = url.host_str().ok_or(AdapterError::InvalidEndpoint {
        reason: "base_url must include a host",
    })?;
    let loopback = matches!(host, "127.0.0.1" | "::1" | "localhost");
    if url.scheme() != "https" && !(allow_http_loopback && url.scheme() == "http" && loopback) {
        return Err(AdapterError::InvalidEndpoint {
            reason: "base_url must use HTTPS",
        });
    }

    url.set_path(PURCHASE_ORDER_SERVICE_ROOT);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONFIG: &str = r#"
default_profile = "production"

[profiles.production]
base_url = "https://tenant.example.com"
sap_client = "100"

[profiles.sandbox]
base_url = "https://sandbox.example.com:8443"
"#;

    #[test]
    fn loads_default_and_explicit_profiles() {
        let default = parse_profile(VALID_CONFIG, None).expect("default profile");
        assert_eq!(default.name(), "production");
        assert_eq!(default.sap_client(), Some("100"));
        assert_eq!(default.service_root().path(), PURCHASE_ORDER_SERVICE_ROOT);
        assert_eq!(
            default.credential_scope(),
            "sap-odata-v1|origin=https://tenant.example.com|sap_client=100"
        );

        let sandbox = parse_profile(VALID_CONFIG, Some("sandbox")).expect("sandbox profile");
        assert_eq!(sandbox.name(), "sandbox");
        assert_eq!(sandbox.service_root().port(), Some(8443));
    }

    #[test]
    fn rejects_unknown_fields_so_tokens_cannot_live_in_config() {
        let source = r#"
default_profile = "production"
[profiles.production]
base_url = "https://tenant.example.com"
access_token = "must-not-be-here"
"#;
        assert!(matches!(
            parse_profile(source, None),
            Err(AdapterError::ConfigMalformed)
        ));
    }

    #[test]
    fn production_profiles_are_https_only_and_origin_only() {
        let http = r#"
default_profile = "x"
[profiles.x]
base_url = "http://tenant.example.com"
"#;
        assert!(matches!(
            parse_profile(http, None),
            Err(AdapterError::InvalidEndpoint { .. })
        ));

        let path = r#"
default_profile = "x"
[profiles.x]
base_url = "https://tenant.example.com/arbitrary/path"
"#;
        assert!(matches!(
            parse_profile(path, None),
            Err(AdapterError::InvalidEndpoint { .. })
        ));
    }

    #[test]
    fn loopback_http_exists_only_for_unit_clients() {
        assert!(MachineProfile::for_loopback_test("http://127.0.0.1:1234").is_ok());
        assert!(matches!(
            validated_service_root("http://127.0.0.1:1234", false),
            Err(AdapterError::InvalidEndpoint { .. })
        ));
    }

    #[test]
    fn credential_scope_is_canonical_and_target_bound() {
        let profile = |origin: &str, sap_client: Option<&str>| MachineProfile {
            name: "test".to_string(),
            service_root: validated_service_root(origin, false).expect("valid test origin"),
            sap_client: sap_client.map(str::to_string),
        };

        let canonical = profile("https://tenant.example.com", Some("100"));
        let equivalent = profile("https://TENANT.example.com:443", Some("100"));
        assert_eq!(canonical.credential_scope(), equivalent.credential_scope());

        for changed in [
            profile("https://other.example.com", Some("100")),
            profile("https://tenant.example.com:8443", Some("100")),
            profile("https://tenant.example.com", Some("200")),
        ] {
            assert_ne!(canonical.credential_scope(), changed.credential_scope());
        }
    }

    #[test]
    fn broker_profile_loader_requires_an_absolute_cori_home() {
        assert!(matches!(
            load_default_machine_profile_from_cori_home(Path::new("relative")),
            Err(AdapterError::ConfigUnsafe { .. })
        ));
    }
}
