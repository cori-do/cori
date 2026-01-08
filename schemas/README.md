# Cori Configuration Schemas

This folder contains JSON Schema definitions for validating Cori's YAML configuration files. These schemas define the structure and constraints for all configuration types used by the Cori Secure Kernel for AI.

## Schema Overview

| Schema | File | Purpose |
|--------|------|---------|
| [SchemaDefinition](#schemadefinition) | `SchemaDefinition.schema.json` | Auto-generated database structure |
| [RulesDefinition](#rulesdefinition) | `RulesDefinition.schema.json` | Tenancy, soft-delete, and validation rules |
| [TypesDefinition](#typesdefinition) | `TypesDefinition.schema.json` | Reusable semantic types for validation |
| [RoleDefinition](#roledefinition) | `RoleDefinition.schema.json` | AI agent roles with table/column permissions |
| [GroupDefinition](#groupdefinition) | `GroupDefinition.schema.json` | Approval groups for human-in-the-loop |

---

## SchemaDefinition

**File:** `SchemaDefinition.schema.json`  
**Config file:** `schema/schema.yaml`  
**Managed by:** `cori db sync` (auto-generated)

Captures the database structure from introspection. This file is **auto-generated** and should not be edited manually.

### Contents

- **Database metadata**: Engine type (postgres, mysql, etc.) and version
- **Extensions**: Enabled database extensions (e.g., `uuid-ossp`, `pgcrypto`)
- **Enums**: Custom enum types with their values
- **Tables**: Complete table definitions including:
  - Columns with types, nullability, defaults, and constraints
  - Primary keys
  - Foreign keys with referential actions
  - Indexes

### Example

```yaml
version: "1.0.0"
captured_at: "2026-01-08T10:30:00Z"
database:
  engine: postgres
  version: "16.1"
tables:
  - name: customers
    schema: public
    columns:
      - name: id
        type: uuid
        nullable: false
      - name: organization_id
        type: uuid
        nullable: false
      - name: email
        type: string
        nullable: false
    primary_key: [id]
```

---

## RulesDefinition

**File:** `RulesDefinition.schema.json`  
**Config file:** `schema/rules.yaml`  
**Managed by:** User (initialize with `cori rules init`)

Defines tenancy configuration, soft-delete behavior, and column-level validation rules. This is where you specify **how data is segmented per tenant**.

### Key Concepts

| Concept | Description |
|---------|-------------|
| **Direct tenant** | Table has a column holding the tenant ID directly |
| **Inherited tenant** | Table inherits tenant from a parent table via foreign key |
| **Global table** | Shared data across all tenants (no filtering) |
| **Soft delete** | DELETE operations set a column instead of removing rows |

### Example

```yaml
version: "1.0.0"
tables:
  customers:
    description: "Customer accounts"
    tenant: organization_id        # Direct tenant column
    columns:
      email:
        type: email                # Reference to types.yaml
        tags: [pii]
        
  orders:
    tenant:
      via: customer_id             # FK column in this table
      references: customers        # Inherit tenant from customers
    soft_delete:
      column: deleted_at
      deleted_value: "NOW()"
      active_value: null
      
  products:
    global: true                   # No tenant scoping
```

---

## TypesDefinition

**File:** `TypesDefinition.schema.json`  
**Config file:** `schema/types.yaml`  
**Managed by:** User

Defines reusable semantic types for input validation. Types specify regex patterns for validation and can be tagged for categorization (e.g., `pii`, `sensitive`).

### Example

```yaml
version: "1.0.0"
types:
  email:
    description: "Email address"
    pattern: "^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\\.[a-zA-Z]{2,}$"
    tags: [pii]
    
  phone:
    description: "Phone number (E.164 format)"
    pattern: "^\\+[1-9]\\d{1,14}$"
    tags: [pii]
    
  sku:
    description: "Product SKU code"
    pattern: "^[A-Z]{3}-[0-9]{4}$"
```

---

## RoleDefinition

**File:** `RoleDefinition.schema.json`  
**Config files:** `roles/*.yaml` (one file per role)  
**Managed by:** User

Defines AI agent roles with granular table and column permissions. Roles control **what AI agents can do** when accessing the database via MCP.

### Permission Types

| Permission | Description |
|------------|-------------|
| `readable` | Columns the agent can SELECT |
| `creatable` | Columns the agent can set on INSERT (with constraints) |
| `updatable` | Columns the agent can modify on UPDATE (with constraints) |
| `deletable` | Whether DELETE is allowed (with optional soft-delete/approval) |

### Column Constraints

**For creatable columns:**
- `required`: Must provide value on INSERT
- `default`: Auto-set if not provided
- `restrict_to`: Whitelist of allowed values
- `requires_approval`: Human approval needed
- `guidance`: Instructions for AI agents

**For updatable columns:**
- `restrict_to`: Whitelist of allowed values
- `transitions`: State machine (valid from → to transitions)
- `only_when`: Conditional update (only if current value matches)
- `increment_only`: Numeric values can only increase
- `append_only`: Text values can only be appended
- `requires_approval`: Human approval needed
- `guidance`: Instructions for AI agents

### Example

```yaml
name: support_agent
description: "AI agent for customer support operations"

approvals:
  group: support_managers
  notify_on_pending: true

tables:
  customers:
    readable: [id, name, email, plan]
    # No creatable/updatable = read-only
    
  tickets:
    readable: [id, subject, status, priority, created_at]
    creatable:
      subject: { required: true }
      priority: { default: low, restrict_to: [low, medium, high] }
    updatable:
      status:
        restrict_to: [open, in_progress, resolved]
        transitions:
          open: [in_progress]
          in_progress: [open, resolved]
      priority:
        requires_approval: true
    deletable: false

blocked_tables: [users, billing, api_keys]
max_rows_per_query: 100
max_affected_rows: 10
```

---

## GroupDefinition

**File:** `GroupDefinition.schema.json`  
**Config files:** `groups/*.yaml` (one file per group)  
**Managed by:** User

Defines approval groups for human-in-the-loop actions. Groups contain members identified by email addresses who can approve sensitive operations.

### Example

```yaml
name: support_managers
description: "Managers who can approve support ticket priority changes"
members:
  - alice.manager@example.com
  - bob.lead@example.com
```

Groups are referenced in role definitions:

```yaml
# In roles/support_agent.yaml
approvals:
  group: support_managers
  notify_on_pending: true
```

---

## Configuration Relationships

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  schema/schema.yaml     │  schema/rules.yaml        │  schema/types.yaml    │
│  (Auto-generated)       │  (User-edited)            │  (User-defined)       │
├─────────────────────────┼───────────────────────────┼───────────────────────┤
│  • Tables & columns     │  • Tenant config          │  • email format       │
│  • Data types           │  • Soft delete            │  • phone pattern      │
│  • Foreign keys         │  • Column validation      │  • Custom types       │
│  • Indexes              │  • Tags (pii, sensitive)  │  • PII tags           │
└─────────────────────────┴───────────────────────────┴───────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  roles/*.yaml                         │  groups/*.yaml                      │
│  (Access Control)                     │  (Approval Groups)                  │
├───────────────────────────────────────┼─────────────────────────────────────┤
│  • WHO can access what?               │  • WHO can approve?                 │
│  • WHICH columns are readable?        │  • Member email list                │
│  • WHAT can be created/updated?       │  • Referenced by roles              │
│  • WHAT constraints apply?            │                                     │
└───────────────────────────────────────┴─────────────────────────────────────┘
```

---

## Validation

Use the CLI to validate all configuration files against these schemas:

```bash
cori validate --config cori.yaml
```

This validates:
- `schema/schema.yaml` against `SchemaDefinition.schema.json`
- `schema/rules.yaml` against `RulesDefinition.schema.json`
- `schema/types.yaml` against `TypesDefinition.schema.json`
- `roles/*.yaml` against `RoleDefinition.schema.json`
- `groups/*.yaml` against `GroupDefinition.schema.json`
- Cross-references (e.g., approval groups exist, type references are valid)

---

## IDE Support

Add these schemas to your IDE for YAML validation and autocompletion:

### VS Code

Add to `.vscode/settings.json`:

```json
{
  "yaml.schemas": {
    "./schemas/SchemaDefinition.schema.json": "schema/schema.yaml",
    "./schemas/RulesDefinition.schema.json": "schema/rules.yaml",
    "./schemas/TypesDefinition.schema.json": "schema/types.yaml",
    "./schemas/RoleDefinition.schema.json": "roles/*.yaml",
    "./schemas/GroupDefinition.schema.json": "groups/*.yaml"
  }
}
```
