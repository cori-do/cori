//! Rules definition tests for Cori MCP.
//!
//! Tests the RulesDefinition schema:
//! - TenantConfig: direct column, inherited via FK, global tables
//! - SoftDeleteConfig: column, deleted_value, active_value
//! - ColumnRules: type_ref, pattern, allowed_values, tags

use super::common::*;
use cori_core::config::rules_definition::{
    ColumnRules, InheritedTenant, RulesDefinition, SoftDeleteConfig, SoftDeleteValue, TableRules,
    TenantConfig,
};
use std::collections::HashMap;

// =============================================================================
// TENANT CONFIG - DIRECT COLUMN
// =============================================================================

pub async fn test_tenant_direct_column(_ctx: &TestContext) {
    println!("  🧪 test_tenant_direct_column");

    let rules = create_default_rules();

    // customers table has direct tenant column
    let customers = rules.tables.get("customers").expect("customers should exist");
    let tenant_config = customers.tenant.as_ref().expect("Should have tenant config");

    match tenant_config {
        TenantConfig::Direct(col) => {
            assert_eq!(col, "organization_id", "Tenant column should be organization_id");
        }
        _ => panic!("Expected direct column tenant config"),
    }

    println!("     ✓ Direct tenant column configured correctly");
}

pub async fn test_tenant_column_helpers(_ctx: &TestContext) {
    println!("  🧪 test_tenant_column_helpers");

    let rules = create_default_rules();

    // Test get_tenant_config helper
    let tenant_config = rules.get_tenant_config("customers");
    assert!(tenant_config.is_some(), "Should have tenant config");

    // Check if it's a direct column
    if let Some(TenantConfig::Direct(col)) = tenant_config {
        assert_eq!(col, "organization_id");
    }

    println!("     ✓ Tenant column helpers work correctly");
}

// =============================================================================
// TENANT CONFIG - INHERITED VIA FK
// =============================================================================

pub async fn test_tenant_inherited_via_fk(_ctx: &TestContext) {
    println!("  🧪 test_tenant_inherited_via_fk");

    let mut tables = HashMap::new();
    tables.insert(
        "customers".to_string(),
        TableRules {
            description: Some("Customer accounts".to_string()),
            tenant: Some(TenantConfig::Direct("organization_id".to_string())),
            global: None,
            soft_delete: None,
            columns: HashMap::new(),
        },
    );
    tables.insert(
        "orders".to_string(),
        TableRules {
            description: Some("Customer orders".to_string()),
            tenant: Some(TenantConfig::Inherited(InheritedTenant {
                via: "customer_id".to_string(),
                references: "customers".to_string(),
            })),
            global: None,
            soft_delete: None,
            columns: HashMap::new(),
        },
    );

    let rules = RulesDefinition {
        version: "1.0.0".to_string(),
        tables,
    };

    let orders = rules.tables.get("orders").expect("orders should exist");
    let tenant_config = orders.tenant.as_ref().expect("Should have tenant config");

    match tenant_config {
        TenantConfig::Inherited(inherited) => {
            assert_eq!(inherited.via, "customer_id", "Should inherit via customer_id");
            assert_eq!(inherited.references, "customers", "Should reference customers table");
        }
        _ => panic!("Expected inherited tenant config"),
    }

    println!("     ✓ Inherited tenant via FK configured correctly");
}

pub async fn test_tenant_chain_inheritance(_ctx: &TestContext) {
    println!("  🧪 test_tenant_chain_inheritance");

    let mut tables = HashMap::new();

    // customers -> orders -> order_items chain
    tables.insert(
        "customers".to_string(),
        TableRules {
            description: None,
            tenant: Some(TenantConfig::Direct("organization_id".to_string())),
            global: None,
            soft_delete: None,
            columns: HashMap::new(),
        },
    );
    tables.insert(
        "orders".to_string(),
        TableRules {
            description: None,
            tenant: Some(TenantConfig::Inherited(InheritedTenant {
                via: "customer_id".to_string(),
                references: "customers".to_string(),
            })),
            global: None,
            soft_delete: None,
            columns: HashMap::new(),
        },
    );
    tables.insert(
        "order_items".to_string(),
        TableRules {
            description: None,
            tenant: Some(TenantConfig::Inherited(InheritedTenant {
                via: "order_id".to_string(),
                references: "orders".to_string(),
            })),
            global: None,
            soft_delete: None,
            columns: HashMap::new(),
        },
    );

    let rules = RulesDefinition {
        version: "1.0.0".to_string(),
        tables,
    };

    // order_items inherits tenant through orders -> customers chain
    let order_items = rules.tables.get("order_items").unwrap();
    assert!(
        order_items.tenant.is_some(),
        "order_items should have tenant config"
    );

    println!("     ✓ Tenant chain inheritance configured correctly");
}

// =============================================================================
// GLOBAL TABLES
// =============================================================================

pub async fn test_global_table_config(_ctx: &TestContext) {
    println!("  🧪 test_global_table_config");

    let rules = create_default_rules();

    // currencies table should be global
    let currencies = rules.tables.get("currencies").expect("currencies should exist");
    assert!(
        currencies.global.unwrap_or(false),
        "currencies should be global"
    );
    assert!(
        currencies.tenant.is_none(),
        "Global tables should not have tenant config"
    );

    println!("     ✓ Global table configured correctly");
}

pub async fn test_global_table_helpers(_ctx: &TestContext) {
    println!("  🧪 test_global_table_helpers");

    let rules = create_default_rules();

    assert!(
        rules.is_global_table("currencies"),
        "currencies should be global"
    );
    assert!(
        !rules.is_global_table("customers"),
        "customers should not be global"
    );

    println!("     ✓ Global table helpers work correctly");
}

pub async fn test_table_rules_is_tenant_scoped(_ctx: &TestContext) {
    println!("  🧪 test_table_rules_is_tenant_scoped");

    let rules = create_default_rules();

    // customers should be tenant scoped
    let customers = rules.tables.get("customers").unwrap();
    assert!(customers.is_tenant_scoped(), "customers should be tenant scoped");

    // currencies should not be tenant scoped
    let currencies = rules.tables.get("currencies").unwrap();
    assert!(!currencies.is_tenant_scoped(), "currencies should not be tenant scoped");

    println!("     ✓ is_tenant_scoped helper works correctly");
}

// =============================================================================
// SOFT DELETE CONFIG
// =============================================================================

pub async fn test_soft_delete_boolean_column(_ctx: &TestContext) {
    println!("  🧪 test_soft_delete_boolean_column");

    let soft_delete = SoftDeleteConfig {
        column: "is_deleted".to_string(),
        deleted_value: Some(SoftDeleteValue::Boolean(true)),
        active_value: Some(SoftDeleteValue::Boolean(false)),
    };

    assert_eq!(soft_delete.column, "is_deleted");
    if let Some(SoftDeleteValue::Boolean(val)) = soft_delete.deleted_value {
        assert!(val, "deleted_value should be true");
    }

    println!("     ✓ Soft delete boolean column configured correctly");
}

pub async fn test_soft_delete_timestamp_column(_ctx: &TestContext) {
    println!("  🧪 test_soft_delete_timestamp_column");

    let soft_delete = SoftDeleteConfig {
        column: "deleted_at".to_string(),
        deleted_value: Some(SoftDeleteValue::Expression("NOW()".to_string())),
        active_value: Some(SoftDeleteValue::Null),
    };

    assert_eq!(soft_delete.column, "deleted_at");
    if let Some(SoftDeleteValue::Expression(expr)) = &soft_delete.deleted_value {
        assert_eq!(expr, "NOW()");
    }

    println!("     ✓ Soft delete timestamp column configured correctly");
}

pub async fn test_soft_delete_in_table_rules(_ctx: &TestContext) {
    println!("  🧪 test_soft_delete_in_table_rules");

    let mut tables = HashMap::new();
    tables.insert(
        "users".to_string(),
        TableRules {
            description: Some("User accounts".to_string()),
            tenant: Some(TenantConfig::Direct("org_id".to_string())),
            global: None,
            soft_delete: Some(SoftDeleteConfig {
                column: "deleted_at".to_string(),
                deleted_value: None,
                active_value: None,
            }),
            columns: HashMap::new(),
        },
    );

    let rules = RulesDefinition {
        version: "1.0.0".to_string(),
        tables,
    };

    let users = rules.tables.get("users").unwrap();
    let sd = users.soft_delete.as_ref().expect("Should have soft_delete");
    assert_eq!(sd.column, "deleted_at");

    println!("     ✓ Soft delete in table rules configured correctly");
}

// =============================================================================
// COLUMN RULES
// =============================================================================

pub async fn test_column_rules_type_ref(_ctx: &TestContext) {
    println!("  🧪 test_column_rules_type_ref");

    let column_rules = ColumnRules {
        description: Some("User email".to_string()),
        type_ref: Some("email".to_string()),
        pattern: None,
        allowed_values: None,
        tags: vec![],
    };

    assert_eq!(column_rules.type_ref.as_deref(), Some("email"));

    println!("     ✓ Column rules type_ref configured correctly");
}

pub async fn test_column_rules_pattern(_ctx: &TestContext) {
    println!("  🧪 test_column_rules_pattern");

    let column_rules = ColumnRules {
        description: None,
        type_ref: None,
        pattern: Some(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string()),
        allowed_values: None,
        tags: vec![],
    };

    assert!(column_rules.pattern.is_some());
    assert!(column_rules.pattern.as_ref().unwrap().contains("@"));

    println!("     ✓ Column rules pattern configured correctly");
}

pub async fn test_column_rules_allowed_values(_ctx: &TestContext) {
    println!("  🧪 test_column_rules_allowed_values");

    let column_rules = ColumnRules {
        description: Some("Status field".to_string()),
        type_ref: None,
        pattern: None,
        allowed_values: Some(vec![
            serde_json::json!("active"),
            serde_json::json!("inactive"),
            serde_json::json!("pending"),
        ]),
        tags: vec![],
    };

    let allowed = column_rules.allowed_values.as_ref().unwrap();
    assert_eq!(allowed.len(), 3);
    assert!(allowed.contains(&serde_json::json!("active")));

    println!("     ✓ Column rules allowed_values configured correctly");
}

pub async fn test_column_rules_tags(_ctx: &TestContext) {
    println!("  🧪 test_column_rules_tags");

    let column_rules = ColumnRules {
        description: Some("Sensitive data".to_string()),
        type_ref: None,
        pattern: None,
        allowed_values: None,
        tags: vec!["pii".to_string(), "encrypted".to_string()],
    };

    assert!(column_rules.tags.contains(&"pii".to_string()));
    assert!(column_rules.tags.contains(&"encrypted".to_string()));

    // Test helper methods
    assert!(column_rules.has_tag("pii"));
    assert!(column_rules.is_pii());

    println!("     ✓ Column rules tags configured correctly");
}

pub async fn test_column_rules_in_table(_ctx: &TestContext) {
    println!("  🧪 test_column_rules_in_table");

    let mut columns = HashMap::new();
    columns.insert(
        "email".to_string(),
        ColumnRules {
            description: Some("User email".to_string()),
            type_ref: Some("email".to_string()),
            pattern: Some(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string()),
            allowed_values: None,
            tags: vec!["pii".to_string()],
        },
    );
    columns.insert(
        "status".to_string(),
        ColumnRules {
            description: Some("Account status".to_string()),
            type_ref: None,
            pattern: None,
            allowed_values: Some(vec![
                serde_json::json!("active"),
                serde_json::json!("disabled"),
            ]),
            tags: vec![],
        },
    );

    let table_rules = TableRules {
        description: Some("User accounts".to_string()),
        tenant: Some(TenantConfig::Direct("org_id".to_string())),
        global: None,
        soft_delete: None,
        columns,
    };

    assert_eq!(table_rules.columns.len(), 2);
    assert!(table_rules.columns.contains_key("email"));
    assert!(table_rules.columns.contains_key("status"));

    println!("     ✓ Column rules in table configured correctly");
}

// =============================================================================
// FULL RULES DEFINITION
// =============================================================================

pub async fn test_rules_definition_structure(_ctx: &TestContext) {
    println!("  🧪 test_rules_definition_structure");

    let rules = create_default_rules();

    assert_eq!(rules.version, "1.0.0");
    assert!(!rules.tables.is_empty());

    println!("     ✓ RulesDefinition structure is valid");
}

pub async fn test_rules_get_table_rules(_ctx: &TestContext) {
    println!("  🧪 test_rules_get_table_rules");

    let rules = create_default_rules();

    let customers = rules.get_table_rules("customers");
    assert!(customers.is_some(), "Should have customers table");

    let nonexistent = rules.get_table_rules("nonexistent");
    assert!(nonexistent.is_none(), "Should not have nonexistent table");

    println!("     ✓ get_table_rules helper works correctly");
}

pub async fn test_table_rules_get_direct_tenant_column(_ctx: &TestContext) {
    println!("  🧪 test_table_rules_get_direct_tenant_column");

    let rules = create_default_rules();

    let customers = rules.tables.get("customers").unwrap();
    let tenant_col = customers.get_direct_tenant_column();
    assert_eq!(tenant_col, Some("organization_id"));

    println!("     ✓ get_direct_tenant_column helper works correctly");
}

pub async fn test_table_rules_get_inherited_tenant(_ctx: &TestContext) {
    println!("  🧪 test_table_rules_get_inherited_tenant");

    let table_rules = TableRules {
        description: None,
        tenant: Some(TenantConfig::Inherited(InheritedTenant {
            via: "customer_id".to_string(),
            references: "customers".to_string(),
        })),
        global: None,
        soft_delete: None,
        columns: HashMap::new(),
    };

    let inherited = table_rules.get_inherited_tenant();
    assert!(inherited.is_some());
    let inherited = inherited.unwrap();
    assert_eq!(inherited.via, "customer_id");
    assert_eq!(inherited.references, "customers");

    println!("     ✓ get_inherited_tenant helper works correctly");
}

// =============================================================================
// YAML PARSING
// =============================================================================

pub async fn test_rules_definition_from_yaml(_ctx: &TestContext) {
    println!("  🧪 test_rules_definition_from_yaml");

    let yaml = r#"
version: "1.0.0"
tables:
  customers:
    description: "Customer accounts"
    tenant: organization_id
  currencies:
    description: "Currency reference data"
    global: true
"#;

    let rules = RulesDefinition::from_yaml(yaml).expect("Should parse YAML");
    assert_eq!(rules.version, "1.0.0");
    assert!(rules.tables.contains_key("customers"));
    assert!(rules.tables.contains_key("currencies"));

    println!("     ✓ RulesDefinition YAML parsing works");
}

pub async fn test_rules_definition_inherited_yaml(_ctx: &TestContext) {
    println!("  🧪 test_rules_definition_inherited_yaml");

    let yaml = r#"
version: "1.0.0"
tables:
  customers:
    tenant: organization_id
  orders:
    tenant:
      via: customer_id
      references: customers
"#;

    let rules = RulesDefinition::from_yaml(yaml).expect("Should parse YAML");

    let orders = rules.tables.get("orders").unwrap();
    let inherited = orders.get_inherited_tenant().expect("Should have inherited tenant");
    assert_eq!(inherited.via, "customer_id");
    assert_eq!(inherited.references, "customers");

    println!("     ✓ Inherited tenant YAML parsing works");
}

pub async fn test_rules_definition_soft_delete_yaml(_ctx: &TestContext) {
    println!("  🧪 test_rules_definition_soft_delete_yaml");

    let yaml = r#"
version: "1.0.0"
tables:
  users:
    tenant: org_id
    soft_delete:
      column: deleted_at
"#;

    let rules = RulesDefinition::from_yaml(yaml).expect("Should parse YAML");

    let users = rules.tables.get("users").unwrap();
    let sd = users.soft_delete.as_ref().expect("Should have soft_delete");
    assert_eq!(sd.column, "deleted_at");

    println!("     ✓ Soft delete YAML parsing works");
}

// =============================================================================
// TEST RUNNER
// =============================================================================

/// Run all rules definition tests
pub async fn run_all_tests(ctx: &TestContext) {
    println!("\n📜 Running Rules Definition Tests\n");

    // Tenant config - direct
    test_tenant_direct_column(ctx).await;
    test_tenant_column_helpers(ctx).await;

    // Tenant config - inherited
    test_tenant_inherited_via_fk(ctx).await;
    test_tenant_chain_inheritance(ctx).await;

    // Global tables
    test_global_table_config(ctx).await;
    test_global_table_helpers(ctx).await;
    test_table_rules_is_tenant_scoped(ctx).await;

    // Soft delete
    test_soft_delete_boolean_column(ctx).await;
    test_soft_delete_timestamp_column(ctx).await;
    test_soft_delete_in_table_rules(ctx).await;

    // Column rules
    test_column_rules_type_ref(ctx).await;
    test_column_rules_pattern(ctx).await;
    test_column_rules_allowed_values(ctx).await;
    test_column_rules_tags(ctx).await;
    test_column_rules_in_table(ctx).await;

    // Full structure
    test_rules_definition_structure(ctx).await;
    test_rules_get_table_rules(ctx).await;
    test_table_rules_get_direct_tenant_column(ctx).await;
    test_table_rules_get_inherited_tenant(ctx).await;

    // YAML parsing
    test_rules_definition_from_yaml(ctx).await;
    test_rules_definition_inherited_yaml(ctx).await;
    test_rules_definition_soft_delete_yaml(ctx).await;

    println!("\n✅ All Rules Definition tests passed!\n");
}
