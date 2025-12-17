use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub allow: bool,
    pub obligations: serde_json::Value,
    pub rule_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyCheckInput {
    pub principal: serde_json::Value,
    pub resource: serde_json::Value,
    pub action: String,
    pub context: serde_json::Value,
}

#[async_trait]
pub trait PolicyClient: Send + Sync {
    async fn check(&self, input: PolicyCheckInput) -> anyhow::Result<PolicyDecision>;
}

/// Stub client for now; replace with real Cerbos gRPC client.
pub struct AllowAllPolicyClient;

#[async_trait]
impl PolicyClient for AllowAllPolicyClient {
    async fn check(&self, _input: PolicyCheckInput) -> anyhow::Result<PolicyDecision> {
        Ok(PolicyDecision {
            allow: true,
            obligations: serde_json::json!({}),
            rule_id: None,
            reason: Some("allow_all_stub".to_string()),
        })
    }
}

// -----------------------------
// Cerbos SDK (gRPC) backend (preferred)
// -----------------------------

pub struct CerbosGrpcPolicyClient {
    client: tokio::sync::Mutex<cerbos::sdk::CerbosAsyncClient>,
}

impl CerbosGrpcPolicyClient {
    pub async fn connect_hostport(host: &str, port: u16) -> anyhow::Result<Self> {
        use cerbos::sdk::{CerbosAsyncClient, CerbosClientOptions, CerbosEndpoint};
        let opts = CerbosClientOptions::new(CerbosEndpoint::HostPort(host, port));
        let client = CerbosAsyncClient::new(opts).await?;
        Ok(Self {
            client: tokio::sync::Mutex::new(client),
        })
    }
}

#[async_trait]
impl PolicyClient for CerbosGrpcPolicyClient {
    async fn check(&self, input: PolicyCheckInput) -> anyhow::Result<PolicyDecision> {
        let principal = build_sdk_principal(&input.principal, &input.context)?;
        let resource = build_sdk_resource(&input.resource)?;

        let mut client = self.client.lock().await;
        let allowed = client
            .is_allowed(input.action.as_str(), principal, resource, None)
            .await?;

        Ok(PolicyDecision {
            allow: allowed,
            obligations: serde_json::json!({}),
            rule_id: None,
            reason: Some("cerbos_sdk_grpc".to_string()),
        })
    }
}

fn build_sdk_principal(
    principal: &serde_json::Value,
    context: &serde_json::Value,
) -> anyhow::Result<cerbos::sdk::model::Principal> {
    use cerbos::sdk::model::Principal;

    let id = principal
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_string();
    let roles: Vec<String> = principal
        .get("roles")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut p = Principal::new(id, roles);

    // Carry Cori principal attrs into Cerbos principal attributes.
    if let Some(obj) = principal.get("attrs").and_then(|x| x.as_object()) {
        for (k, val) in obj {
            p = p.add_attr(k, JsonAttrVal(val.clone()));
        }
    }

    // Cerbos SDK v0.1.0 doesn't expose a first-class `request.context`, so we attach context
    // under principal.attr.context.* and generate policy stubs accordingly.
    p = p.add_attr("context", JsonAttrVal(context.clone()));

    Ok(p)
}

fn build_sdk_resource(v: &serde_json::Value) -> anyhow::Result<cerbos::sdk::model::Resource> {
    use cerbos::sdk::model::Resource;

    let r = v.get("resource").unwrap_or(v);
    let kind = r.get("kind").and_then(|x| x.as_str()).unwrap_or("unknown");
    let id = r.get("id").and_then(|x| x.as_str()).unwrap_or("unknown");

    let mut res = Resource::new(id, kind);
    if let Some(obj) = r.get("attr").and_then(|x| x.as_object()) {
        for (k, val) in obj {
            res = res.add_attr(k, JsonAttrVal(val.clone()));
        }
    }
    Ok(res)
}

#[derive(Debug, Clone)]
struct JsonAttrVal(serde_json::Value);

impl cerbos::sdk::attr::AttrVal for JsonAttrVal {
    fn to_value(self) -> prost_types::Value {
        json_to_prost_value(&self.0)
    }
}

fn json_to_prost_value(v: &serde_json::Value) -> prost_types::Value {
    use prost_types::value::Kind;
    use prost_types::{ListValue, Struct, Value};

    match v {
        serde_json::Value::Null => Value {
            kind: Some(Kind::NullValue(0)),
        },
        serde_json::Value::Bool(b) => Value {
            kind: Some(Kind::BoolValue(*b)),
        },
        serde_json::Value::Number(n) => Value {
            kind: Some(Kind::NumberValue(n.as_f64().unwrap_or(0.0))),
        },
        serde_json::Value::String(s) => Value {
            kind: Some(Kind::StringValue(s.clone())),
        },
        serde_json::Value::Array(arr) => Value {
            kind: Some(Kind::ListValue(ListValue {
                values: arr.iter().map(json_to_prost_value).collect(),
            })),
        },
        serde_json::Value::Object(obj) => Value {
            kind: Some(Kind::StructValue(Struct {
                fields: obj
                    .iter()
                    .map(|(k, v)| (k.clone(), json_to_prost_value(v)))
                    .collect(),
            })),
        },
    }
}
