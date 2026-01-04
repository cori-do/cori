//! Connection handler for the Postgres wire protocol proxy.
//!
//! This module implements the query processing pipeline using pgwire 0.37 API.
//!
//! ## Virtual Schema
//!
//! This handler supports virtual schema filtering, which intercepts queries to
//! `information_schema` and `pg_catalog` views and returns filtered responses
//! based on the token's permissions. This prevents AI agents from discovering
//! tables and columns they cannot access.

use crate::error::ProxyError;
use async_trait::async_trait;
use cori_audit::{AuditEvent, AuditEventType, AuditLogger};
use cori_biscuit::TokenVerifier;
use cori_biscuit::VerifiedToken;
use cori_rls::{RlsInjector, VirtualSchemaConfig, VirtualSchemaHandler};
use futures::stream;
use futures::sink::SinkExt;
use futures::Sink;
use pgwire::api::auth::{
    finish_authentication, protocol_negotiation, save_startup_parameters_to_metadata,
    DefaultServerParameterProvider, StartupHandler,
};
use pgwire::api::query::SimpleQueryHandler;
use pgwire::api::results::{
    DataRowEncoder, FieldFormat, FieldInfo, QueryResponse, Response, Tag,
};
use pgwire::api::{ClientInfo, ClientPortalStore, PgWireConnectionState, PgWireServerHandlers, Type};
use pgwire::error::{ErrorInfo, PgWireError, PgWireResult};
use pgwire::messages::startup::Authentication;
use pgwire::messages::{PgWireBackendMessage, PgWireFrontendMessage};
use sqlx::Column as SqlxColumn;
use sqlx::TypeInfo;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

/// Session state for a connected client.
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// The verified token from authentication.
    pub verified_token: Option<VerifiedToken>,
    /// Tenant ID extracted from token.
    pub tenant_id: Option<String>,
    /// Role extracted from token.
    pub role: Option<String>,
    /// Client address.
    pub client_addr: Option<String>,
    /// Connection ID.
    pub connection_id: String,
    /// Tables accessible to this session (for virtual schema filtering).
    pub accessible_tables: Vec<String>,
    /// Readable columns per table (for virtual schema filtering).
    pub readable_columns: HashMap<String, Vec<String>>,
}

impl Default for SessionContext {
    fn default() -> Self {
        Self {
            verified_token: None,
            tenant_id: None,
            role: None,
            client_addr: None,
            connection_id: uuid::Uuid::new_v4().to_string(),
            accessible_tables: Vec::new(),
            readable_columns: HashMap::new(),
        }
    }
}

/// Biscuit token authentication startup handler.
///
/// This handler requests cleartext password authentication and verifies
/// the password as a Biscuit token. On successful verification, it sets
/// the tenant and role in the session context.
pub struct BiscuitAuthStartupHandler {
    /// Token verifier for Biscuit tokens.
    verifier: Arc<TokenVerifier>,
    /// Session context to update on successful authentication.
    session: Arc<RwLock<SessionContext>>,
    /// Server parameter provider.
    parameter_provider: DefaultServerParameterProvider,
    /// Role permissions for populating virtual schema access.
    role_permissions: Option<Arc<HashMap<String, crate::proxy::RolePermissions>>>,
}

impl BiscuitAuthStartupHandler {
    /// Create a new Biscuit authentication startup handler.
    pub fn new(verifier: Arc<TokenVerifier>, session: Arc<RwLock<SessionContext>>) -> Self {
        Self {
            verifier,
            session,
            parameter_provider: DefaultServerParameterProvider::default(),
            role_permissions: None,
        }
    }

    /// Create a new Biscuit authentication startup handler with role permissions.
    pub fn new_with_role_permissions(
        verifier: Arc<TokenVerifier>,
        session: Arc<RwLock<SessionContext>>,
        role_permissions: Arc<HashMap<String, crate::proxy::RolePermissions>>,
    ) -> Self {
        Self {
            verifier,
            session,
            parameter_provider: DefaultServerParameterProvider::default(),
            role_permissions: Some(role_permissions),
        }
    }
}

#[async_trait]
impl StartupHandler for BiscuitAuthStartupHandler {
    async fn on_startup<C>(
        &self,
        client: &mut C,
        message: PgWireFrontendMessage,
    ) -> PgWireResult<()>
    where
        C: ClientInfo + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        match message {
            PgWireFrontendMessage::Startup(ref startup) => {
                // Handle protocol negotiation
                protocol_negotiation(client, startup).await?;
                // Save startup parameters (user, database, etc.)
                save_startup_parameters_to_metadata(client, startup);
                // Set state to authentication in progress
                client.set_state(PgWireConnectionState::AuthenticationInProgress);
                // Request cleartext password (which contains the Biscuit token)
                client
                    .send(PgWireBackendMessage::Authentication(
                        Authentication::CleartextPassword,
                    ))
                    .await?;
            }
            PgWireFrontendMessage::PasswordMessageFamily(pwd) => {
                // Extract the password (Biscuit token)
                let password_msg = pwd.into_password()?;
                let token_str = &password_msg.password;

                // Verify the Biscuit token
                match self.verifier.verify(token_str) {
                    Ok(verified_token) => {
                        // Update session with verified token info
                        let client_addr = Some(client.socket_addr().to_string());
                        let mut session = self.session.write().await;
                        session.tenant_id = verified_token.tenant.clone();
                        session.role = Some(verified_token.role.clone());
                        session.client_addr = client_addr;
                        session.verified_token = Some(verified_token.clone());

                        // Populate accessible_tables and readable_columns from role permissions
                        if let Some(ref role_perms) = self.role_permissions {
                            if let Some(perms) = role_perms.get(&verified_token.role) {
                                session.accessible_tables = perms.accessible_tables.clone();
                                session.readable_columns = perms.readable_columns.clone();
                                
                                tracing::debug!(
                                    role = %verified_token.role,
                                    tables = ?session.accessible_tables,
                                    "Populated session with role permissions"
                                );
                            } else {
                                tracing::warn!(
                                    role = %verified_token.role,
                                    "Role not found in role permissions map"
                                );
                            }
                        }

                        tracing::info!(
                            tenant = ?session.tenant_id,
                            role = ?session.role,
                            "Biscuit token verified successfully"
                        );

                        // Complete authentication
                        finish_authentication(client, &self.parameter_provider).await?;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Biscuit token verification failed");
                        // Return authentication error
                        let user = client
                            .metadata()
                            .get("user")
                            .cloned()
                            .unwrap_or_else(|| "unknown".to_string());
                        return Err(PgWireError::InvalidPassword(user));
                    }
                }
            }
            _ => {
                // Ignore other messages during startup
            }
        }
        Ok(())
    }
}

// ============================================================================
// Query Handler Implementation
// ============================================================================

/// The main query handler that processes queries and injects RLS.
pub struct CoriQueryHandler {
    /// RLS injector for adding tenant predicates.
    injector: RlsInjector,
    /// Virtual schema handler for filtering schema introspection queries.
    virtual_schema: VirtualSchemaHandler,
    /// Token verifier for authentication.
    #[allow(dead_code)]
    verifier: Arc<TokenVerifier>,
    /// Upstream Postgres connection pool.
    pool: PgPool,
    /// Audit logger.
    audit: Option<Arc<AuditLogger>>,
    /// Session context.
    session: Arc<RwLock<SessionContext>>,
}

impl CoriQueryHandler {
    /// Create a new query handler.
    pub fn new(
        injector: RlsInjector,
        verifier: Arc<TokenVerifier>,
        pool: PgPool,
        audit: Option<Arc<AuditLogger>>,
    ) -> Self {
        Self {
            injector,
            virtual_schema: VirtualSchemaHandler::default(),
            verifier,
            pool,
            audit,
            session: Arc::new(RwLock::new(SessionContext::default())),
        }
    }

    /// Create a new query handler with a shared session context.
    /// This is used when the session is shared with the startup handler.
    pub fn new_with_session(
        injector: RlsInjector,
        verifier: Arc<TokenVerifier>,
        pool: PgPool,
        audit: Option<Arc<AuditLogger>>,
        session: Arc<RwLock<SessionContext>>,
    ) -> Self {
        Self {
            injector,
            virtual_schema: VirtualSchemaHandler::default(),
            verifier,
            pool,
            audit,
            session,
        }
    }

    /// Create a new query handler with custom virtual schema configuration.
    pub fn with_virtual_schema(
        injector: RlsInjector,
        verifier: Arc<TokenVerifier>,
        pool: PgPool,
        audit: Option<Arc<AuditLogger>>,
        virtual_schema_config: VirtualSchemaConfig,
    ) -> Self {
        Self {
            injector,
            virtual_schema: VirtualSchemaHandler::new(virtual_schema_config),
            verifier,
            pool,
            audit,
            session: Arc::new(RwLock::new(SessionContext::default())),
        }
    }

    /// Set the session context after authentication.
    pub async fn set_session(&self, token: VerifiedToken, client_addr: Option<String>) {
        let mut session = self.session.write().await;
        session.tenant_id = token.tenant.clone();
        session.role = Some(token.role.clone());
        session.client_addr = client_addr;
        session.verified_token = Some(token);
    }

    /// Set the session context with schema permissions for virtual schema filtering.
    pub async fn set_session_with_permissions(
        &self,
        token: VerifiedToken,
        client_addr: Option<String>,
        accessible_tables: Vec<String>,
        readable_columns: HashMap<String, Vec<String>>,
    ) {
        let mut session = self.session.write().await;
        session.tenant_id = token.tenant.clone();
        session.role = Some(token.role.clone());
        session.client_addr = client_addr;
        session.verified_token = Some(token);
        session.accessible_tables = accessible_tables;
        session.readable_columns = readable_columns;
    }

    /// Update schema permissions for the current session.
    pub async fn set_schema_permissions(
        &self,
        accessible_tables: Vec<String>,
        readable_columns: HashMap<String, Vec<String>>,
    ) {
        let mut session = self.session.write().await;
        session.accessible_tables = accessible_tables;
        session.readable_columns = readable_columns;
    }

    /// Get the current session context.
    pub async fn get_session(&self) -> SessionContext {
        self.session.read().await.clone()
    }

    /// Set a tenant ID directly (for testing or when not using tokens).
    #[allow(dead_code)]
    pub async fn set_tenant(&self, tenant_id: &str, role: &str) {
        let mut session = self.session.write().await;
        session.tenant_id = Some(tenant_id.to_string());
        session.role = Some(role.to_string());
    }

    /// Get the virtual schema handler.
    pub fn virtual_schema_handler(&self) -> &VirtualSchemaHandler {
        &self.virtual_schema
    }

    /// Check if a query would be intercepted by the virtual schema handler.
    pub fn is_schema_query(&self, sql: &str) -> bool {
        self.virtual_schema.should_intercept(sql)
    }

    /// Process a SQL query: inject RLS and execute (or intercept schema queries).
    async fn process_query(&self, sql: &str) -> Result<QueryResult, ProxyError> {
        let start = Instant::now();
        let session = self.get_session().await;

        // Get tenant ID from session (default to "default" if not set for testing)
        let tenant_id = session.tenant_id.as_deref().unwrap_or("default");

        // Check if this is a schema introspection query that should be intercepted
        if let Some(virtual_response) = self.virtual_schema.handle(
            sql,
            session.accessible_tables.clone(),
            session.readable_columns.clone(),
        ) {
            tracing::debug!(
                query = %sql,
                tenant = %tenant_id,
                tables_visible = ?session.accessible_tables,
                "Schema query intercepted by virtual schema handler"
            );

            // Convert virtual schema response to QueryResult
            let columns = virtual_response
                .columns
                .into_iter()
                .map(|c| ColumnInfo {
                    name: c.name,
                    type_oid: c.type_oid,
                })
                .collect();

            let result = QueryResult {
                columns,
                rows: virtual_response.rows,
                row_count: virtual_response.row_count,
                command_tag: format!("SELECT {}", virtual_response.row_count),
            };

            // Audit log for schema query
            let duration_ms = start.elapsed().as_millis() as u64;
            if let Some(audit) = &self.audit {
                let event = AuditEvent::builder(AuditEventType::QueryExecuted)
                    .tenant(tenant_id)
                    .role(session.role.as_deref().unwrap_or("unknown"))
                    .original_query(sql)
                    .rewritten_query("[virtual schema]")
                    .tables(vec!["information_schema".to_string()])
                    .row_count(result.row_count as u64)
                    .duration_ms(duration_ms)
                    .connection_id(&session.connection_id)
                    .client_ip(session.client_addr.as_deref().unwrap_or("unknown"))
                    .build();

                if let Err(e) = audit.log(event).await {
                    tracing::warn!(error = %e, "Failed to log audit event");
                }
            }

            return Ok(result);
        }

        // Inject RLS predicates for regular queries
        let injection_result = self
            .injector
            .inject(sql, tenant_id)
            .map_err(|e| ProxyError::SqlParseError(e.to_string()))?;

        let rewritten_sql = &injection_result.rewritten_sql;

        tracing::debug!(
            original = %sql,
            rewritten = %rewritten_sql,
            tenant = %tenant_id,
            "Query rewritten with RLS"
        );

        // Execute on upstream
        let result = self.execute_upstream(rewritten_sql).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Audit log
        if let Some(audit) = &self.audit {
            let event = match &result {
                Ok(qr) => AuditEvent::builder(AuditEventType::QueryExecuted)
                    .tenant(tenant_id)
                    .role(session.role.as_deref().unwrap_or("unknown"))
                    .original_query(sql)
                    .rewritten_query(rewritten_sql)
                    .tables(injection_result.tables_scoped.clone())
                    .row_count(qr.row_count as u64)
                    .duration_ms(duration_ms)
                    .connection_id(&session.connection_id)
                    .client_ip(session.client_addr.as_deref().unwrap_or("unknown"))
                    .build(),
                Err(e) => AuditEvent::builder(AuditEventType::QueryFailed)
                    .tenant(tenant_id)
                    .role(session.role.as_deref().unwrap_or("unknown"))
                    .original_query(sql)
                    .rewritten_query(rewritten_sql)
                    .tables(injection_result.tables_scoped.clone())
                    .duration_ms(duration_ms)
                    .error(e.to_string())
                    .connection_id(&session.connection_id)
                    .client_ip(session.client_addr.as_deref().unwrap_or("unknown"))
                    .build(),
            };

            if let Err(e) = audit.log(event).await {
                tracing::warn!(error = %e, "Failed to log audit event");
            }
        }

        result
    }

    /// Sanitize SQL to fix known client quirks.
    ///
    /// TablePlus sends `ORDER BY ""` when no specific column is selected for ordering,
    /// which is invalid SQL. This method removes such empty ORDER BY clauses.
    fn sanitize_sql(sql: &str) -> String {
        // Pattern: ORDER BY "" or ORDER BY "","","",... 
        // This regex matches ORDER BY followed by one or more empty quoted identifiers
        let re = regex::Regex::new(r#"(?i)\s+ORDER\s+BY\s+(""\s*,?\s*)+"#).unwrap();
        
        if re.is_match(sql) {
            let sanitized = re.replace(sql, " ").to_string();
            tracing::debug!(
                original = %sql,
                sanitized = %sanitized,
                "Sanitized SQL: removed empty ORDER BY clause"
            );
            sanitized
        } else {
            sql.to_string()
        }
    }

    /// Execute a query on the upstream Postgres.
    async fn execute_upstream(&self, sql: &str) -> Result<QueryResult, ProxyError> {
        // Sanitize SQL for known client quirks (e.g., TablePlus empty ORDER BY)
        let sql = Self::sanitize_sql(sql);
        
        // Determine query type
        let sql_upper = sql.trim().to_uppercase();
        let is_select = sql_upper.starts_with("SELECT")
            || sql_upper.starts_with("WITH")
            || sql_upper.starts_with("SHOW")
            || sql_upper.starts_with("EXPLAIN");

        if is_select {
            // For SELECT queries, fetch rows
            let rows = sqlx::query(&sql)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| ProxyError::UpstreamConnectionFailed(e.to_string()))?;

            // Get column information
            let columns: Vec<ColumnInfo> = if !rows.is_empty() {
                rows[0]
                    .columns()
                    .iter()
                    .map(|c| ColumnInfo {
                        name: SqlxColumn::name(c).to_string(),
                        type_oid: pg_type_to_oid(SqlxColumn::type_info(c).name()),
                    })
                    .collect()
            } else {
                Vec::new()
            };

            // Convert rows to string values (simplified - all as text)
            let data: Vec<Vec<Option<String>>> = rows
                .iter()
                .map(|row| {
                    (0..row.len())
                        .map(|i| {
                            row.try_get::<String, _>(i)
                                .ok()
                                .or_else(|| row.try_get::<i32, _>(i).ok().map(|v| v.to_string()))
                                .or_else(|| row.try_get::<i64, _>(i).ok().map(|v| v.to_string()))
                                .or_else(|| row.try_get::<f64, _>(i).ok().map(|v| v.to_string()))
                                .or_else(|| row.try_get::<bool, _>(i).ok().map(|v| v.to_string()))
                                .or_else(|| {
                                    row.try_get::<chrono::NaiveDateTime, _>(i)
                                        .ok()
                                        .map(|v| v.to_string())
                                })
                                .or_else(|| {
                                    row.try_get::<uuid::Uuid, _>(i)
                                        .ok()
                                        .map(|v| v.to_string())
                                })
                        })
                        .collect()
                })
                .collect();

            Ok(QueryResult {
                columns,
                rows: data,
                row_count: rows.len(),
                command_tag: format!("SELECT {}", rows.len()),
            })
        } else {
            // For DML/DDL queries, execute and get affected rows
            let result = sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .map_err(|e| ProxyError::UpstreamConnectionFailed(e.to_string()))?;

            let affected = result.rows_affected() as usize;
            let tag = if sql_upper.starts_with("INSERT") {
                format!("INSERT 0 {}", affected)
            } else if sql_upper.starts_with("UPDATE") {
                format!("UPDATE {}", affected)
            } else if sql_upper.starts_with("DELETE") {
                format!("DELETE {}", affected)
            } else {
                "OK".to_string()
            };

            Ok(QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                row_count: affected,
                command_tag: tag,
            })
        }
    }
}

/// Result of a query execution.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Column information.
    pub columns: Vec<ColumnInfo>,
    /// Row data as strings.
    pub rows: Vec<Vec<Option<String>>>,
    /// Number of rows affected/returned.
    pub row_count: usize,
    /// Command completion tag.
    pub command_tag: String,
}

/// Column information.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    /// Column name.
    pub name: String,
    /// PostgreSQL type OID.
    pub type_oid: u32,
}

/// Map type name to PostgreSQL type OID.
fn pg_type_to_oid(type_name: &str) -> u32 {
    match type_name.to_uppercase().as_str() {
        "INT4" => 23,
        "INT8" => 20,
        "INT2" => 21,
        "TEXT" => 25,
        "VARCHAR" => 1043,
        "BOOL" => 16,
        "FLOAT4" => 700,
        "FLOAT8" => 701,
        "TIMESTAMP" => 1114,
        "TIMESTAMPTZ" => 1184,
        "DATE" => 1082,
        "UUID" => 2950,
        "JSON" => 114,
        "JSONB" => 3802,
        _ => 25, // Default to TEXT
    }
}

#[async_trait]
impl SimpleQueryHandler for CoriQueryHandler {
    async fn do_query<C>(&self, _client: &mut C, query: &str) -> PgWireResult<Vec<Response>>
    where
        C: ClientInfo + ClientPortalStore + Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::Error: Debug,
        PgWireError: From<<C as Sink<PgWireBackendMessage>>::Error>,
    {
        // Session is set by BiscuitAuthStartupHandler during authentication.
        // At this point, the tenant_id and role should already be set if a valid token was provided.
        match self.process_query(query).await {
            Ok(result) => {
                if result.columns.is_empty() {
                    // Command completion (INSERT, UPDATE, DELETE, etc.)
                    Ok(vec![Response::Execution(Tag::new(&result.command_tag))])
                } else {
                    // Query with results
                    let fields: Vec<FieldInfo> = result
                        .columns
                        .iter()
                        .map(|c| {
                            FieldInfo::new(
                                c.name.clone().into(),
                                None,
                                None,
                                Type::TEXT,
                                FieldFormat::Text,
                            )
                        })
                        .collect();

                    let schema = Arc::new(fields);

                    // Build data rows
                    let rows: Vec<PgWireResult<pgwire::messages::data::DataRow>> = result
                        .rows
                        .into_iter()
                        .map(|row| {
                            let mut encoder = DataRowEncoder::new(schema.clone());
                            for value in row {
                                if let Err(e) = encoder.encode_field(&value) {
                                    return Err(e);
                                }
                            }
                            Ok(encoder.take_row())
                        })
                        .collect();

                    let data_row_stream = stream::iter(rows);

                    Ok(vec![Response::Query(QueryResponse::new(
                        schema,
                        data_row_stream,
                    ))])
                }
            }
            Err(e) => Err(PgWireError::UserError(Box::new(ErrorInfo::new(
                "ERROR".to_owned(),
                "XX000".to_owned(),
                e.to_string(),
            )))),
        }
    }
}

/// Server handlers implementation for pgwire 0.37.
///
/// This struct implements `PgWireServerHandlers` and provides both
/// the startup handler (for Biscuit authentication) and query handler.
pub struct CoriServerHandlers {
    query_handler: Arc<CoriQueryHandler>,
    startup_handler: Arc<BiscuitAuthStartupHandler>,
}

impl CoriServerHandlers {
    /// Create new server handlers with the given query handler and startup handler.
    pub fn new(
        query_handler: Arc<CoriQueryHandler>,
        startup_handler: Arc<BiscuitAuthStartupHandler>,
    ) -> Self {
        Self {
            query_handler,
            startup_handler,
        }
    }
}

impl PgWireServerHandlers for CoriServerHandlers {
    fn simple_query_handler(&self) -> Arc<impl SimpleQueryHandler> {
        self.query_handler.clone()
    }

    fn startup_handler(&self) -> Arc<impl StartupHandler> {
        self.startup_handler.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pg_type_to_oid() {
        assert_eq!(pg_type_to_oid("INT4"), 23);
        assert_eq!(pg_type_to_oid("TEXT"), 25);
        assert_eq!(pg_type_to_oid("UUID"), 2950);
    }

    #[test]
    fn test_session_context_default() {
        let session = SessionContext::default();
        assert!(session.verified_token.is_none());
        assert!(session.tenant_id.is_none());
        assert!(session.role.is_none());
        assert!(!session.connection_id.is_empty());
    }

    #[test]
    fn test_sanitize_sql_empty_order_by() {
        // Single empty identifier - TablePlus pattern
        let sql = r#"SELECT * FROM "public"."notes" ORDER BY "" LIMIT 300 OFFSET 0;"#;
        let result = CoriQueryHandler::sanitize_sql(sql);
        assert_eq!(result, r#"SELECT * FROM "public"."notes" LIMIT 300 OFFSET 0;"#);
        
        // Multiple empty identifiers
        let sql = r#"SELECT * FROM "public"."communications" ORDER BY "","","" LIMIT 300;"#;
        let result = CoriQueryHandler::sanitize_sql(sql);
        assert_eq!(result, r#"SELECT * FROM "public"."communications" LIMIT 300;"#);
    }

    #[test]
    fn test_sanitize_sql_valid_order_by() {
        // Valid ORDER BY should not be modified
        let sql = r#"SELECT * FROM users ORDER BY "name" LIMIT 100;"#;
        let result = CoriQueryHandler::sanitize_sql(sql);
        assert_eq!(result, sql);
        
        // Unquoted column name
        let sql = "SELECT * FROM users ORDER BY created_at DESC;";
        let result = CoriQueryHandler::sanitize_sql(sql);
        assert_eq!(result, sql);
    }

    #[test]
    fn test_sanitize_sql_no_order_by() {
        // Query without ORDER BY should not be modified
        let sql = "SELECT * FROM users WHERE id = 1;";
        let result = CoriQueryHandler::sanitize_sql(sql);
        assert_eq!(result, sql);
    }
}
