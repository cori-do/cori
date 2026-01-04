//! Integration tests for the Cori Postgres proxy.
//!
//! These tests require a running Postgres instance.
//! Set DATABASE_URL environment variable to connect.
//!
//! Run with: cargo test --package cori-proxy --test integration_tests

use cori_biscuit::{KeyPair, RoleClaims, TokenBuilder, TokenVerifier};
use cori_core::config::{ProxyConfig, TenancyConfig, UpstreamConfig};
use cori_proxy::CoriProxy;

/// Test that proxy configuration can be built correctly.
#[test]
fn test_proxy_config_defaults() {
    let config = ProxyConfig::default();
    assert_eq!(config.listen_port, 5433);
    assert_eq!(config.listen_addr, "0.0.0.0");
    assert_eq!(config.max_connections, 100);
}

/// Test that upstream connection string is built correctly.
#[test]
fn test_upstream_connection_string() {
    let upstream = UpstreamConfig {
        host: "localhost".to_string(),
        port: 5432,
        database: "mydb".to_string(),
        username: "myuser".to_string(),
        password: Some("mypass".to_string()),
        credentials_env: None,
    };

    let conn_str = upstream.connection_string();
    assert!(conn_str.contains("myuser"));
    assert!(conn_str.contains("mypass"));
    assert!(conn_str.contains("localhost"));
    assert!(conn_str.contains("5432"));
    assert!(conn_str.contains("mydb"));
}

/// Test that upstream connection string uses credentials_env when available.
#[test]
fn test_upstream_connection_string_from_env() {
    // SAFETY: We're in a test and controlling the environment
    unsafe {
        std::env::set_var("TEST_DB_URL", "postgresql://testuser@testhost:1234/testdb");
    }

    let upstream = UpstreamConfig {
        host: "ignored".to_string(),
        port: 9999,
        database: "ignored".to_string(),
        username: "ignored".to_string(),
        password: None,
        credentials_env: Some("TEST_DB_URL".to_string()),
    };

    let conn_str = upstream.connection_string();
    assert_eq!(conn_str, "postgresql://testuser@testhost:1234/testdb");

    // SAFETY: Cleanup in test
    unsafe {
        std::env::remove_var("TEST_DB_URL");
    }
}

/// Test token flow: generate keypair, mint token, verify token.
#[test]
fn test_token_flow() {
    // Generate keypair
    let keypair = KeyPair::generate().unwrap();

    // Create token builder
    let builder = TokenBuilder::new(keypair.clone());

    // Create role claims
    let claims = RoleClaims::new("support_agent")
        .add_readable_table("users".to_string(), vec!["id".to_string(), "name".to_string()])
        .add_readable_table("orders".to_string(), vec!["*".to_string()]);

    // Mint role token
    let role_token = builder.mint_role_token(&claims).unwrap();
    assert!(!role_token.is_empty());

    // Attenuate to agent token
    let agent_token = builder
        .attenuate(
            &role_token,
            "client_a",
            Some(chrono::Duration::hours(24)),
            Some("test"),
        )
        .unwrap();
    assert!(!agent_token.is_empty());
    assert_ne!(role_token, agent_token);

    // Verify token
    let verifier = TokenVerifier::new(keypair.public_key());
    let verified = verifier.verify(&agent_token).unwrap();

    assert_eq!(verified.role, "support_agent");
    assert_eq!(verified.tenant, Some("client_a".to_string()));
    assert_eq!(verified.block_count(), 2);
}

/// Test tenancy config for RLS injection.
#[test]
fn test_tenancy_config() {
    let mut config = TenancyConfig::default();
    assert_eq!(config.default_column, "tenant_id");

    // Test global tables
    config.global_tables.push("products".to_string());
    assert!(config.is_global_table("products"));
    assert!(!config.is_global_table("orders"));

    // Test tenant column lookup
    assert_eq!(config.get_tenant_column("orders"), Some("tenant_id"));
    assert_eq!(config.get_tenant_column("products"), None);
}

/// Test RLS injection.
#[test]
fn test_rls_injection() {
    use cori_rls::RlsInjector;

    let config = TenancyConfig::default();
    let injector = RlsInjector::new(config);

    // Test SELECT injection
    let result = injector
        .inject("SELECT * FROM orders WHERE status = 'pending'", "client_a")
        .unwrap();
    assert!(result.rewritten_sql.contains("tenant_id = 'client_a'"));
    assert!(result.rewritten_sql.contains("status = 'pending'"));
    assert_eq!(result.tables_scoped, vec!["orders"]);

    // Test SELECT without WHERE
    let result = injector
        .inject("SELECT id, name FROM users", "client_b")
        .unwrap();
    assert!(result.rewritten_sql.contains("WHERE"));
    assert!(result.rewritten_sql.contains("tenant_id = 'client_b'"));

    // Test UPDATE injection
    let result = injector
        .inject("UPDATE orders SET status = 'shipped' WHERE id = 1", "client_c")
        .unwrap();
    assert!(result.rewritten_sql.contains("tenant_id = 'client_c'"));
}

/// Test proxy creation with all components.
#[test]
fn test_proxy_creation_full() {
    let keypair = KeyPair::generate().unwrap();
    let verifier = TokenVerifier::new(keypair.public_key());

    let mut tenancy_config = TenancyConfig::default();
    tenancy_config.default_column = "organization_id".to_string();
    tenancy_config.global_tables.push("products".to_string());

    let config = ProxyConfig {
        listen_addr: "127.0.0.1".to_string(),
        listen_port: 15433, // Use a non-standard port to avoid conflicts
        max_connections: 5,
        connection_timeout: 30,
        tls: Default::default(),
    };
    
    let upstream = UpstreamConfig {
        host: "localhost".to_string(),
        port: 5432,
        database: "test".to_string(),
        username: "test".to_string(),
        password: None,
        credentials_env: None,
    };

    // Just verify proxy can be created
    let _proxy = CoriProxy::new(config, upstream, verifier, tenancy_config, None);
}

/// Test audit event creation.
#[test]
fn test_audit_events() {
    use cori_audit::{AuditEvent, AuditEventType};

    let event = AuditEvent::builder(AuditEventType::QueryExecuted)
        .tenant("client_a")
        .role("support_agent")
        .original_query("SELECT * FROM users")
        .rewritten_query("SELECT * FROM users WHERE tenant_id = 'client_a'")
        .tables(vec!["users".to_string()])
        .row_count(10)
        .duration_ms(5)
        .connection_id("conn-123")
        .client_ip("192.168.1.1")
        .build();

    assert!(matches!(event.event_type, AuditEventType::QueryExecuted));
    assert_eq!(event.tenant_id, Some("client_a".to_string()));
    assert_eq!(event.role, Some("support_agent".to_string()));
    assert_eq!(event.row_count, Some(10));
}

/// Test DDL rejection.
#[test]
fn test_ddl_rejection() {
    use cori_rls::{RlsError, RlsInjector};

    let config = TenancyConfig::default();
    let injector = RlsInjector::new(config);

    // DDL should be rejected
    let result = injector.inject("DROP TABLE users", "client_a");
    assert!(matches!(result, Err(RlsError::DdlNotAllowed { .. })));

    let result = injector.inject("CREATE TABLE test (id INT)", "client_a");
    assert!(matches!(result, Err(RlsError::DdlNotAllowed { .. })));

    let result = injector.inject("ALTER TABLE users ADD COLUMN foo TEXT", "client_a");
    assert!(matches!(result, Err(RlsError::DdlNotAllowed { .. })));
}

/// Test JOIN query RLS injection.
#[test]
fn test_join_rls_injection() {
    use cori_rls::RlsInjector;

    let config = TenancyConfig::default();
    let injector = RlsInjector::new(config);

    let result = injector
        .inject(
            "SELECT o.*, u.name FROM orders o JOIN users u ON o.user_id = u.id",
            "client_a",
        )
        .unwrap();

    // Both tables should have tenant predicates
    assert!(result.rewritten_sql.contains("o.tenant_id = 'client_a'"));
    assert!(result.rewritten_sql.contains("u.tenant_id = 'client_a'"));
    assert_eq!(result.tables_scoped.len(), 2);
}

/// Test global table exclusion from RLS.
#[test]
fn test_global_table_no_rls() {
    use cori_rls::RlsInjector;

    let mut config = TenancyConfig::default();
    config.global_tables.push("products".to_string());
    config.global_tables.push("categories".to_string());

    let injector = RlsInjector::new(config);

    // Global table should have no injection
    let result = injector
        .inject("SELECT * FROM products WHERE price > 100", "client_a")
        .unwrap();
    assert!(!result.rewritten_sql.contains("tenant_id"));
    assert!(result.tables_scoped.is_empty());

    // Non-global table should have injection
    let result = injector
        .inject("SELECT * FROM orders", "client_a")
        .unwrap();
    assert!(result.rewritten_sql.contains("tenant_id"));
    assert!(!result.tables_scoped.is_empty());
}

/// Test per-table tenant column configuration.
#[test]
fn test_per_table_tenant_column() {
    use cori_core::config::TableTenancyConfig;
    use cori_rls::RlsInjector;
    use std::collections::HashMap;

    let mut tables = HashMap::new();
    tables.insert(
        "legacy_orders".to_string(),
        TableTenancyConfig {
            tenant_column: Some("customer_id".to_string()),
            column: None,
            tenant_type: None,
            global: false,
            tenant_via: None,
            fk_column: None,
        },
    );
    tables.insert(
        "external_users".to_string(),
        TableTenancyConfig {
            tenant_column: Some("org_id".to_string()),
            column: None,
            tenant_type: None,
            global: false,
            tenant_via: None,
            fk_column: None,
        },
    );

    let config = TenancyConfig {
        default_column: "tenant_id".to_string(),
        tables,
        global_tables: vec![],
        ..Default::default()
    };

    let injector = RlsInjector::new(config);

    // Legacy table uses customer_id
    let result = injector
        .inject("SELECT * FROM legacy_orders", "client_a")
        .unwrap();
    assert!(result.rewritten_sql.contains("customer_id = 'client_a'"));
    assert!(!result.rewritten_sql.contains("tenant_id"));

    // External users table uses org_id
    let result = injector
        .inject("SELECT * FROM external_users", "client_a")
        .unwrap();
    assert!(result.rewritten_sql.contains("org_id = 'client_a'"));

    // Regular table uses default tenant_id
    let result = injector
        .inject("SELECT * FROM orders", "client_a")
        .unwrap();
    assert!(result.rewritten_sql.contains("tenant_id = 'client_a'"));
}
