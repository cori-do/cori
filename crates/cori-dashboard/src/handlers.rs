//! Request handlers for the dashboard.

use axum::{
    extract::{Path, Query, State},
    response::Html,
    Form,
};
use crate::state::AppState;
use crate::pages;
use crate::pages_extra;
use cori_audit::logger::AuditFilter;

// =============================================================================
// Page Handlers (HTML responses)
// =============================================================================

/// Handler for the dashboard home page.
pub async fn home(State(state): State<AppState>) -> Html<String> {
    let config = state.config();
    let role_count = config.roles.len();
    let pending_approvals = state.approval_manager()
        .map(|m| m.list_pending(None).len())
        .unwrap_or(0);
    
    Html(pages::home_page(&config, role_count, pending_approvals))
}

/// Handler for the schema browser page.
pub async fn schema_browser(State(state): State<AppState>) -> Html<String> {
    let schema = state.schema_cache();
    let tenancy = state.get_tenancy();
    
    Html(pages::schema_browser_page(schema.as_ref(), &tenancy))
}

/// Handler for the roles list page.
pub async fn roles_list(State(state): State<AppState>) -> Html<String> {
    let roles = state.get_roles();
    Html(pages::roles_page(&roles))
}

/// Handler for the new role page.
pub async fn role_new(State(state): State<AppState>) -> Html<String> {
    let schema = state.schema_cache();
    Html(pages::role_editor_page(None, schema.as_ref(), true))
}

/// Handler for role detail page.
pub async fn role_detail(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Html<String> {
    if let Some(role) = state.get_role(&name) {
        // Generate MCP tools preview
        let schema = state.schema_cache();
        let mcp_tools = if let Some(schema) = &schema {
            let db_schema = crate::schema_converter::convert_to_db_schema(schema);
            let generator = cori_mcp::ToolGenerator::new(role.clone(), db_schema);
            let tools: Vec<serde_json::Value> = generator.generate_all()
                .into_iter()
                .map(|t| serde_json::to_value(t).unwrap_or_default())
                .collect();
            Some(tools)
        } else {
            None
        };
        
        Html(pages::role_detail_page(&role, mcp_tools.as_deref()))
    } else {
        Html(crate::templates::layout("Role Not Found", &crate::templates::empty_state(
            "exclamation-triangle",
            "Role Not Found",
            &format!("The role '{}' does not exist.", name),
            Some(("Back to Roles", "/roles")),
        )))
    }
}

/// Handler for role edit page.
pub async fn role_edit(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Html<String> {
    let schema = state.schema_cache();
    if let Some(role) = state.get_role(&name) {
        Html(pages::role_editor_page(Some(&role), schema.as_ref(), false))
    } else {
        Html(crate::templates::layout("Role Not Found", &crate::templates::empty_state(
            "exclamation-triangle",
            "Role Not Found",
            &format!("The role '{}' does not exist.", name),
            Some(("Back to Roles", "/roles")),
        )))
    }
}

/// Handler for the tokens page.
pub async fn tokens(
    State(state): State<AppState>,
    Query(params): Query<TokenPageParams>,
) -> Html<String> {
    let roles = state.get_roles();
    Html(pages_extra::tokens_page(&roles, params.role.as_deref()))
}

#[derive(Debug, serde::Deserialize)]
pub struct TokenPageParams {
    pub role: Option<String>,
}

/// Handler for the audit logs page.
pub async fn audit_logs(
    State(state): State<AppState>,
    Query(params): Query<crate::api_types::AuditQueryParams>,
) -> Html<String> {
    let events = if let Some(logger) = state.audit_logger() {
        let filter = AuditFilter {
            tenant_id: params.tenant_id.clone(),
            role: params.role.clone(),
            event_type: None, // Would need to parse from string
            start_time: params.start_time,
            end_time: params.end_time,
            limit: params.limit.or(Some(100)),
            offset: params.offset,
        };
        logger.query(filter).await.unwrap_or_default()
    } else {
        vec![]
    };
    
    Html(pages_extra::audit_logs_page(&events, &params))
}

/// Handler for the approvals page.
pub async fn approvals(State(state): State<AppState>) -> Html<String> {
    let approvals = state.approval_manager()
        .map(|m| m.list_pending(None))
        .unwrap_or_default();
    
    Html(pages_extra::approvals_page(&approvals))
}

/// Handler for the settings page.
pub async fn settings(State(state): State<AppState>) -> Html<String> {
    let config = state.config();
    Html(pages_extra::settings_page(&config))
}

// =============================================================================
// API Handlers (JSON/HTMX responses)
// =============================================================================

pub mod api {
    use super::*;
    use axum::{http::StatusCode, Json};
    use crate::api_types::*;

    // -------------------------------------------------------------------------
    // Schema API
    // -------------------------------------------------------------------------

    pub async fn schema_get(State(state): State<AppState>) -> Json<Option<SchemaResponse>> {
        let schema = state.schema_cache();
        Json(schema.map(|s| SchemaResponse {
            tables: s.tables.iter().map(|t| {
                let tenancy = state.get_tenancy();
                let configured_tc = tenancy.tables.get(&t.name)
                    .and_then(|tc| tc.tenant_column.clone().or(tc.column.clone()));
                let is_global = tenancy.tables.get(&t.name).map(|tc| tc.global).unwrap_or(false)
                    || tenancy.global_tables.contains(&t.name);
                
                TableSchemaResponse {
                    schema: t.schema.clone(),
                    name: t.name.clone(),
                    columns: t.columns.iter().map(|c| ColumnSchemaResponse {
                        name: c.name.clone(),
                        data_type: c.data_type.clone(),
                        nullable: c.nullable,
                        default: c.default.clone(),
                        is_primary_key: t.primary_key.contains(&c.name),
                        is_tenant_column: configured_tc.as_ref() == Some(&c.name) 
                            || t.detected_tenant_column.as_ref() == Some(&c.name),
                    }).collect(),
                    primary_key: t.primary_key.clone(),
                    foreign_keys: t.foreign_keys.iter().map(|fk| ForeignKeyResponse {
                        name: fk.name.clone(),
                        columns: fk.columns.clone(),
                        references_table: fk.references_table.clone(),
                        references_columns: fk.references_columns.clone(),
                    }).collect(),
                    detected_tenant_column: t.detected_tenant_column.clone(),
                    configured_tenant_column: configured_tc,
                    is_global,
                }
            }).collect(),
            refreshed_at: s.refreshed_at,
        }))
    }

    pub async fn schema_refresh(State(state): State<AppState>) -> Result<Html<String>, (StatusCode, String)> {
        let database_url = state.database_url()
            .ok_or((StatusCode::BAD_REQUEST, "No database URL configured".to_string()))?;
        
        // Introspect the database schema
        let schema_json = cori_adapter_pg::introspect::introspect_schema_json(database_url)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to introspect database: {}", e)))?;
        
        // Convert to SchemaInfo and cache it
        let schema_info = crate::schema_converter::json_to_schema_info(&schema_json)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to parse schema: {}", e)))?;
        
        state.set_schema_cache(schema_info);
        
        // Return success with HX-Trigger to refresh the page
        Ok(Html(r#"<script>showToast('Schema refreshed successfully'); window.location.reload();</script>"#.to_string()))
    }

    // -------------------------------------------------------------------------
    // Roles API
    // -------------------------------------------------------------------------

    pub async fn roles_list(State(state): State<AppState>) -> Json<RoleListResponse> {
        let roles = state.get_roles();
        Json(RoleListResponse {
            roles: roles.iter().map(|(name, role)| RoleSummary {
                name: name.clone(),
                description: role.description.clone(),
                table_count: role.tables.len(),
                has_custom_actions: !role.custom_actions.is_empty(),
            }).collect(),
        })
    }

    pub async fn role_get(
        State(state): State<AppState>,
        Path(name): Path<String>,
    ) -> Result<Json<RoleResponse>, StatusCode> {
        state.get_role(&name)
            .map(|role| Json(role_to_response(&name, &role)))
            .ok_or(StatusCode::NOT_FOUND)
    }

    pub async fn role_create(
        State(state): State<AppState>,
        Form(form): Form<RoleFormData>,
    ) -> Result<Html<String>, (StatusCode, String)> {
        let role = form_to_role_config(&form);
        state.save_role(form.name.clone(), role);
        
        Ok(Html(format!(
            r#"<script>showToast('Role "{}" created successfully'); window.location.href='/roles/{}';</script>"#,
            form.name, form.name
        )))
    }

    pub async fn role_update(
        State(state): State<AppState>,
        Path(name): Path<String>,
        Form(form): Form<RoleFormData>,
    ) -> Result<Html<String>, (StatusCode, String)> {
        if state.get_role(&name).is_none() {
            return Err((StatusCode::NOT_FOUND, "Role not found".to_string()));
        }
        
        let role = form_to_role_config(&form);
        state.save_role(name.clone(), role);
        
        Ok(Html(format!(
            r#"<script>showToast('Role "{}" updated successfully'); window.location.href='/roles/{}';</script>"#,
            name, name
        )))
    }

    pub async fn role_delete(
        State(state): State<AppState>,
        Path(name): Path<String>,
    ) -> Result<Html<String>, StatusCode> {
        state.delete_role(&name).ok_or(StatusCode::NOT_FOUND)?;
        
        Ok(Html(format!(
            r#"<script>showToast('Role "{}" deleted'); window.location.href='/roles';</script>"#,
            name
        )))
    }

    pub async fn role_mcp_preview(
        State(state): State<AppState>,
        Path(name): Path<String>,
    ) -> Result<Json<McpToolPreviewResponse>, StatusCode> {
        let role = state.get_role(&name).ok_or(StatusCode::NOT_FOUND)?;
        let schema = state.schema_cache();
        
        let tools = if let Some(schema) = schema {
            let db_schema = crate::schema_converter::convert_to_db_schema(&schema);
            let generator = cori_mcp::ToolGenerator::new(role, db_schema);
            generator.generate_all()
                .into_iter()
                .map(|t| McpToolResponse {
                    name: t.name,
                    description: t.description,
                    input_schema: t.input_schema,
                    annotations: McpToolAnnotationsResponse {
                        requires_approval: t.annotations.as_ref().and_then(|a| a.requires_approval).unwrap_or(false),
                        dry_run_supported: t.annotations.as_ref().and_then(|a| a.dry_run_supported).unwrap_or(false),
                        read_only: t.annotations.as_ref().and_then(|a| a.read_only).unwrap_or(false),
                        approval_fields: t.annotations.and_then(|a| a.approval_fields).unwrap_or_default(),
                    },
                })
                .collect()
        } else {
            vec![]
        };
        
        Ok(Json(McpToolPreviewResponse {
            role: name,
            tools,
        }))
    }

    // -------------------------------------------------------------------------
    // Tokens API
    // -------------------------------------------------------------------------

    pub async fn token_mint_role(
        State(state): State<AppState>,
        Form(form): Form<MintRoleTokenForm>,
    ) -> Result<Html<String>, (StatusCode, String)> {
        let keypair = state.keypair()
            .ok_or((StatusCode::BAD_REQUEST, "No keypair configured".to_string()))?;
        
        let role = state.get_role(&form.role)
            .ok_or((StatusCode::NOT_FOUND, "Role not found".to_string()))?;
        
        let claims = crate::token_helpers::role_config_to_claims(&role);
        let builder = cori_biscuit::TokenBuilder::new(keypair.clone());
        
        let token = builder.mint_role_token(&claims)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        
        Ok(Html(pages_extra::token_result_fragment(
            &token,
            "Role",
            Some(&form.role),
            None,
            None,
        )))
    }

    pub async fn token_mint_agent(
        State(state): State<AppState>,
        Form(form): Form<MintAgentTokenForm>,
    ) -> Result<Html<String>, (StatusCode, String)> {
        let keypair = state.keypair()
            .ok_or((StatusCode::BAD_REQUEST, "No keypair configured".to_string()))?;
        
        let role = state.get_role(&form.role)
            .ok_or((StatusCode::NOT_FOUND, "Role not found".to_string()))?;
        
        let claims = crate::token_helpers::role_config_to_claims(&role);
        let builder = cori_biscuit::TokenBuilder::new(keypair.clone());
        
        // First mint role token
        let role_token = builder.mint_role_token(&claims)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        
        // Then attenuate
        let expires = form.expires_in_hours.map(|h| chrono::Duration::hours(h as i64));
        let token = builder.attenuate(&role_token, &form.tenant, expires, Some("dashboard"))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        
        let expires_at = expires.map(|d| chrono::Utc::now() + d);
        
        Ok(Html(pages_extra::token_result_fragment(
            &token,
            "Agent",
            Some(&form.role),
            Some(&form.tenant),
            expires_at.as_ref().map(|e| e.to_rfc3339()).as_deref(),
        )))
    }

    pub async fn token_attenuate(
        State(state): State<AppState>,
        Form(form): Form<AttenuateTokenForm>,
    ) -> Result<Html<String>, (StatusCode, String)> {
        let keypair = state.keypair()
            .ok_or((StatusCode::BAD_REQUEST, "No keypair configured".to_string()))?;
        
        let builder = cori_biscuit::TokenBuilder::new(keypair.clone());
        let expires = form.expires_in_hours.map(|h| chrono::Duration::hours(h as i64));
        
        let token = builder.attenuate(&form.base_token, &form.tenant, expires, Some("dashboard"))
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        
        let expires_at = expires.map(|d| chrono::Utc::now() + d);
        
        Ok(Html(pages_extra::token_result_fragment(
            &token,
            "Agent",
            None,
            Some(&form.tenant),
            expires_at.as_ref().map(|e| e.to_rfc3339()).as_deref(),
        )))
    }

    pub async fn token_inspect(
        State(state): State<AppState>,
        Form(form): Form<InspectTokenForm>,
    ) -> Result<Html<String>, (StatusCode, String)> {
        // Try to inspect without verification first (to get basic info)
        let info = cori_biscuit::inspect_token_unverified(&form.token)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        
        // Check if we can verify
        let valid = if let Some(keypair) = state.keypair() {
            let verifier = cori_biscuit::TokenVerifier::new(keypair.public_key());
            verifier.verify(&form.token).is_ok()
        } else {
            false
        };
        
        let response = TokenInspectResponse {
            token_type: "Unknown".to_string(), // Can't determine from basic info
            role: None,
            tenant: None,
            tables: None,
            block_count: info.block_count,
            valid,
            expires_at: None,
        };
        
        Ok(Html(pages_extra::token_inspect_fragment(&response)))
    }

    // -------------------------------------------------------------------------
    // Audit API
    // -------------------------------------------------------------------------

    pub async fn audit_list(
        State(state): State<AppState>,
        Query(params): Query<AuditQueryParams>,
    ) -> Json<AuditListResponse> {
        let events = if let Some(logger) = state.audit_logger() {
            let filter = cori_audit::logger::AuditFilter {
                tenant_id: params.tenant_id,
                role: params.role,
                event_type: None,
                start_time: params.start_time,
                end_time: params.end_time,
                limit: params.limit.or(Some(100)),
                offset: params.offset,
            };
            logger.query(filter).await.unwrap_or_default()
        } else {
            vec![]
        };
        
        let total = events.len();
        Json(AuditListResponse {
            events: events.into_iter().map(audit_event_to_response).collect(),
            total,
            has_more: false,
        })
    }

    pub async fn audit_get(
        State(state): State<AppState>,
        Path(id): Path<String>,
    ) -> Result<Html<String>, StatusCode> {
        let event_id = uuid::Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
        
        if let Some(logger) = state.audit_logger() {
            if let Ok(Some(event)) = logger.get(event_id).await {
                return Ok(Html(pages_extra::audit_event_detail_fragment(&event)));
            }
        }
        
        Err(StatusCode::NOT_FOUND)
    }

    // -------------------------------------------------------------------------
    // Approvals API
    // -------------------------------------------------------------------------

    pub async fn approvals_list(State(state): State<AppState>) -> Json<ApprovalListResponse> {
        let approvals = state.approval_manager()
            .map(|m| m.list_pending(None))
            .unwrap_or_default();
        
        let pending_count = approvals.iter()
            .filter(|a| a.status == cori_mcp::ApprovalStatus::Pending)
            .count();
        
        Json(ApprovalListResponse {
            approvals: approvals.into_iter().map(approval_to_response).collect(),
            pending_count,
        })
    }

    pub async fn approval_get(
        State(state): State<AppState>,
        Path(id): Path<String>,
    ) -> Result<Json<ApprovalResponse>, StatusCode> {
        state.approval_manager()
            .and_then(|m| m.get(&id))
            .map(|a| Json(approval_to_response(a)))
            .ok_or(StatusCode::NOT_FOUND)
    }

    pub async fn approval_approve(
        State(state): State<AppState>,
        Path(id): Path<String>,
        Form(form): Form<ApprovalDecisionForm>,
    ) -> Result<Html<String>, (StatusCode, String)> {
        let manager = state.approval_manager()
            .ok_or((StatusCode::BAD_REQUEST, "Approval manager not configured".to_string()))?;
        
        manager.approve(&id, form.decided_by.as_deref().unwrap_or("dashboard"), form.reason)
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
        
        Ok(Html(r#"<script>showToast('Approved successfully'); window.location.reload();</script>"#.to_string()))
    }

    pub async fn approval_reject(
        State(state): State<AppState>,
        Path(id): Path<String>,
        Form(form): Form<ApprovalDecisionForm>,
    ) -> Result<Html<String>, (StatusCode, String)> {
        let manager = state.approval_manager()
            .ok_or((StatusCode::BAD_REQUEST, "Approval manager not configured".to_string()))?;
        
        manager.reject(&id, form.decided_by.as_deref().unwrap_or("dashboard"), form.reason)
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
        
        Ok(Html(r#"<script>showToast('Rejected'); window.location.reload();</script>"#.to_string()))
    }

    // -------------------------------------------------------------------------
    // Settings API
    // -------------------------------------------------------------------------

    pub async fn settings_get(State(state): State<AppState>) -> Json<SettingsResponse> {
        let config = state.config();
        
        Json(SettingsResponse {
            upstream: UpstreamSettingsResponse {
                host: config.upstream.host.clone(),
                port: config.upstream.port,
                database: config.upstream.database.clone(),
                user: Some(config.upstream.username.clone()),
                ssl_mode: None, // Not in current config
                connected: true, // Would need actual check
            },
            proxy: ProxySettingsResponse {
                listen_port: config.proxy.listen_port,
                max_connections: config.proxy.max_connections,
            },
            mcp: McpSettingsResponse {
                enabled: config.mcp.enabled,
                transport: format!("{:?}", config.mcp.transport),
                http_port: Some(config.mcp.http_port),
            },
            dashboard: DashboardSettingsResponse {
                enabled: config.dashboard.enabled,
                listen_port: config.dashboard.listen_port,
                auth_type: format!("{:?}", config.dashboard.auth.auth_type),
            },
            audit: AuditSettingsResponse {
                enabled: config.audit.enabled,
                log_queries: config.audit.log_queries,
                log_results: config.audit.log_results,
                retention_days: config.audit.retention_days,
                storage_backend: format!("{:?}", config.audit.storage.backend),
            },
            guardrails: GuardrailsSettingsResponse {
                max_rows_per_query: config.guardrails.max_rows_per_query,
                max_affected_rows: config.guardrails.max_affected_rows,
                blocked_operations: config.guardrails.blocked_operations.clone(),
            },
            tenancy: TenancySettingsResponse {
                tenant_id_type: config.tenancy.tenant_id.id_type.clone(),
                default_column: config.tenancy.default_column.clone(),
                table_count: config.tenancy.tables.len(),
                global_table_count: config.tenancy.global_tables.len(),
            },
        })
    }

    pub async fn settings_update_proxy(
        State(state): State<AppState>,
        Form(form): Form<ProxySettingsUpdate>,
    ) -> Html<String> {
        {
            let mut config = state.config_mut();
            if let Some(port) = form.listen_port {
                config.proxy.listen_port = port;
            }
            if let Some(max) = form.max_connections {
                config.proxy.max_connections = max;
            }
        }
        
        Html(r#"<script>showToast('Proxy settings saved');</script>"#.to_string())
    }

    pub async fn settings_update_guardrails(
        State(state): State<AppState>,
        Form(form): Form<GuardrailsSettingsUpdate>,
    ) -> Html<String> {
        {
            let mut config = state.config_mut();
            if let Some(max) = form.max_rows_per_query {
                config.guardrails.max_rows_per_query = max;
            }
            if let Some(max) = form.max_affected_rows {
                config.guardrails.max_affected_rows = max;
            }
            if let Some(ops) = form.blocked_operations {
                config.guardrails.blocked_operations = ops;
            }
        }
        
        Html(r#"<script>showToast('Guardrails saved');</script>"#.to_string())
    }

    pub async fn settings_update_audit(
        State(state): State<AppState>,
        Form(form): Form<AuditSettingsUpdate>,
    ) -> Html<String> {
        {
            let mut config = state.config_mut();
            if let Some(enabled) = form.enabled {
                config.audit.enabled = enabled;
            }
            if let Some(log_queries) = form.log_queries {
                config.audit.log_queries = log_queries;
            }
            if let Some(log_results) = form.log_results {
                config.audit.log_results = log_results;
            }
            if let Some(days) = form.retention_days {
                config.audit.retention_days = days;
            }
        }
        
        Html(r#"<script>showToast('Audit settings saved');</script>"#.to_string())
    }

    // -------------------------------------------------------------------------
    // Form Types
    // -------------------------------------------------------------------------

    #[derive(Debug, serde::Deserialize)]
    pub struct RoleFormData {
        pub name: String,
        pub description: Option<String>,
        pub max_rows_per_query: Option<u64>,
        pub max_affected_rows: Option<u64>,
        // Table permissions would be nested, simplified here
    }

    #[derive(Debug, serde::Deserialize)]
    pub struct MintRoleTokenForm {
        pub role: String,
    }

    #[derive(Debug, serde::Deserialize)]
    pub struct MintAgentTokenForm {
        pub role: String,
        pub tenant: String,
        pub expires_in_hours: Option<u64>,
    }

    #[derive(Debug, serde::Deserialize)]
    pub struct AttenuateTokenForm {
        pub base_token: String,
        pub tenant: String,
        pub expires_in_hours: Option<u64>,
    }

    #[derive(Debug, serde::Deserialize)]
    pub struct InspectTokenForm {
        pub token: String,
    }

    #[derive(Debug, serde::Deserialize)]
    pub struct ApprovalDecisionForm {
        pub decided_by: Option<String>,
        pub reason: Option<String>,
    }

    // -------------------------------------------------------------------------
    // Conversion Helpers
    // -------------------------------------------------------------------------

    fn role_to_response(name: &str, role: &cori_core::RoleConfig) -> RoleResponse {
        RoleResponse {
            name: name.to_string(),
            description: role.description.clone(),
            tables: role.tables.iter().map(|(tname, perms)| {
                let ops = perms.operations.as_ref()
                    .map(|ops| ops.iter().map(|o| format!("{:?}", o).to_lowercase()).collect())
                    .unwrap_or_else(|| vec!["read".to_string()]);
                
                let (readable, readable_all) = match &perms.readable {
                    cori_core::ReadableColumns::All(_) => (vec![], true),
                    cori_core::ReadableColumns::List(cols) => (cols.clone(), false),
                };
                
                (tname.clone(), TablePermissionResponse {
                    operations: ops,
                    readable,
                    readable_all,
                    editable: perms.editable.iter().map(|(col, constraints)| {
                        (col.clone(), ColumnConstraintResponse {
                            allowed_values: constraints.allowed_values.clone(),
                            pattern: constraints.pattern.clone(),
                            min: constraints.min,
                            max: constraints.max,
                            requires_approval: constraints.requires_approval,
                        })
                    }).collect(),
                })
            }).collect(),
            blocked_tables: role.blocked_tables.clone(),
            max_rows_per_query: role.max_rows_per_query,
            max_affected_rows: role.max_affected_rows,
            blocked_operations: role.blocked_operations.clone(),
            custom_actions: role.custom_actions.iter().map(|a| CustomActionResponse {
                name: a.name.clone(),
                description: a.description.clone(),
                requires_approval: a.requires_approval,
            }).collect(),
        }
    }

    fn form_to_role_config(form: &RoleFormData) -> cori_core::RoleConfig {
        cori_core::RoleConfig {
            name: form.name.clone(),
            description: form.description.clone(),
            tables: std::collections::HashMap::new(), // Would parse from form
            blocked_tables: vec![],
            max_rows_per_query: form.max_rows_per_query,
            max_affected_rows: form.max_affected_rows,
            blocked_operations: vec![],
            custom_actions: vec![],
            include_actions: vec![],
        }
    }

    fn audit_event_to_response(event: cori_audit::AuditEvent) -> AuditEventResponse {
        AuditEventResponse {
            event_id: event.event_id.to_string(),
            occurred_at: event.occurred_at,
            event_type: format!("{:?}", event.event_type),
            tenant_id: event.tenant_id,
            role: event.role,
            original_query: event.original_query,
            rewritten_query: event.rewritten_query,
            tables: event.tables,
            row_count: event.row_count,
            duration_ms: event.duration_ms,
            error: event.error,
            tool_name: event.tool_name,
            approval_id: event.approval_id,
        }
    }

    fn approval_to_response(approval: cori_mcp::ApprovalRequest) -> ApprovalResponse {
        ApprovalResponse {
            id: approval.id,
            tool_name: approval.tool_name,
            arguments: approval.arguments,
            approval_fields: approval.approval_fields,
            status: format!("{:?}", approval.status),
            tenant_id: approval.tenant_id,
            role: approval.role,
            created_at: approval.created_at,
            expires_at: approval.expires_at,
            decided_at: approval.decided_at,
            decided_by: approval.decided_by,
            reason: approval.reason,
        }
    }
}

