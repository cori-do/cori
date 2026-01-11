<div align="center">


<img src="https://assets.cori.do/cori-logo.png" alt="Cori Logo" width="140" />

### The Secure Kernel for AI

**Give AI agents database access without giving away the keys.**

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

[Quick Start](#-quick-start) • [Why Cori](#-the-problem) • [How It Works](#-how-it-works) 

</div>

---

## 🎯 The Problem

You want AI agents to work with your database. But:

- **Multi-tenant data** → Agent for Client A must never see Client B's data
- **Dynamic operations** → LLMs request actions you can't predict
- **Compliance & audit** → You need to know exactly what happened
- **Zero trust** → Traditional app-level security doesn't cut it

**Raw database access for AI is a security nightmare.**

---

## 💡 The Solution

Cori is an **MCP server** that sits between AI agents and your database.

```
AI Agent → MCP → Cori → Your Postgres
                  ↓
             ✓ Verify token
             ✓ Check permissions
             ✓ Inject tenant isolation
             ✓ Audit everything
```

**Agents discover typed tools via MCP. Cori protects your data.**

---

## ✨ Key Features

| Feature | Description |
|---------|-------------|
| **🔐 Biscuit Token Auth** | Cryptographic tokens with tenant + role claims. No forgery possible. |
| **🏢 Automatic Tenant Isolation** | Every operation is scoped to the token's tenant. |
| **📋 Role-Based Access** | Define which tables, columns, and operations each role can access. |
| **🤖 MCP Server Built-In** | AI agents discover typed tools, not raw SQL. |
| **👁️ Full Audit Trail** | Every action logged with who, what, when, and outcome. |
| **🔍 Virtual Schema** | Agents only see tables/columns they're allowed to access. |
| **✅ Human-in-the-Loop** | Flag sensitive operations for approval before execution. |

---

## 🚀 Quick Start

### Install

```sh
cargo install --path crates/cori-cli
```

### 1. Initialize from Your Database

```sh
cori init --from-db postgres://user:pass@localhost/mydb --project myproject
```

This introspects your database and generates:
- `cori.yaml` — Main configuration
- `keys/` — Biscuit keypair for token signing
- `roles/` — Sample role definitions based on your schema
- `groups/` — Sample approval groups
- `schema/schema.yaml` — Auto-generated database schema
- `schema/rules.yaml` — Tenancy, soft-delete, validation rules

### 2. Start Cori

```sh
cd myproject
cori run
# Dashboard on :8080, MCP HTTP server on :3000
```

### 3. Mint a Token

```sh
# Create a role token
cori token mint --role support_agent --key keys/private.key --output role.token

# Attenuate for a specific tenant
cori token attenuate \
    --base role.token \
    --tenant acme_corp \
    --expires 24h \
    --key keys/private.key \
    --output agent.token
```

### 4. Connect Your Agent via MCP

Add Cori to your AI agent's MCP configuration:

```json
{
  "mcpServers": {
    "cori": {
      "command": "cori",
      "args": ["run", "--stdio", "--config", "cori.yaml", "--token", "agent.token"]
    }
  }
}
```

Your agent now has **typed, safe tools** instead of raw SQL:

```
🔧 Available Tools (8):
   • getCustomer (read)       → Retrieve a customer by ID
   • listCustomers (read)     → List customers with filters
   • getTicket (read)         → Retrieve a ticket by ID
   • listTickets (read)       → List tickets with filters
   • updateTicket (write)     → Update ticket status/priority
   • getOrder (read)          → Retrieve an order by ID
   • listOrders (read)        → List orders with filters
   ...
```

Each tool is:
- **Scoped to the tenant** in the token (no data leaks)
- **Type-checked** with JSON Schema inputs
- **Permission-aware** (only actions the role allows)
- **Audited** (every call logged)

Test what tools are available for a token:

```sh
cori tools list --token agent.token --key keys/public.key
```

---

## 🛡️ Audit Logs

Cori records **every tool call, SQL query, and approval decision** in both human-readable and structured formats.

- **Console output** (when `audit.stdout` is enabled) prints lines like `[2026-01-10T22:54:10Z] QUERY_EXECUTED role=support_agent tenant=acme_corp action=listCustomers sql="SELECT ..."` and flags approvals (`ApprovalRequested`, `Approved`, `Denied`).
- **JSON log file** is written to `logs/audit.log` inside your project directory. Each line is a compact JSON object with fields such as `event_type`, `role`, `tenant_id`, `action`, `sql`, `approval_id`, and `duration_ms`, making it easy to ship to log processors or parse locally.

The dashboard automatically loads `logs/audit.log` on startup, shows total counts, sortable columns, and paginated lists for fast forensic review.

Configure `audit.directory`, `audit.stdout`, and `audit.retention_days` in `cori.yaml` to change where logs live, whether dual output is enabled, and how long entries are kept.

---

## 🔧 How It Works

### Define Your Tenancy

Tell Cori how your multi-tenant data is structured:

```yaml
# tenancy.yaml
tenant_id:
  type: uuid

tables:
  customers:
    tenant_column: organization_id
  orders:
    tenant_column: customer_org_id
  products:
    global: true  # Shared across all tenants
```

### Define Roles

Specify what each role can do:

```yaml
# roles/support_agent.yaml
name: support_agent
description: "AI agent for customer support"

tables:
  customers:
    operations: [read]
    readable: [id, name, email, plan]
    
  tickets:
    operations: [read, update]
    readable: [id, subject, status, priority]
    editable:
      status:
        allowed_values: [open, in_progress, resolved]
      priority:
        requires_approval: true  # Human must approve

default_page_size: 100
```

### Automatic Tool Generation

Cori generates MCP tools from your schema and role permissions:

```
Agent Request:
  tool: listOrders
  arguments: { status: "pending" }

Cori Executes:
  SELECT * FROM orders 
  WHERE status = 'pending' 
  AND customer_org_id = 'acme_corp'  -- injected from token
```

No code changes. No ORM plugins. Just security.

---

## 🤖 MCP Tool Generation

Cori automatically generates MCP tools from your schema and role permissions:

| Role Permission | Generated Tools |
|-----------------|-----------------|
| Table readable | `get{Entity}(id)`, `list{Entities}(filters)` |
| Table has editable columns | `create{Entity}(data)`, `update{Entity}(id, data)` |
| Table deletable | `delete{Entity}(id)` |

Tools include:
- **Typed inputs** — JSON Schema with column types, enums, constraints
- **Filter parameters** — Auto-generated from readable columns
- **Approval flags** — Sensitive fields marked for human-in-the-loop
- **Pagination** — Built-in `limit`/`offset` respecting `default_page_size`

Example generated tool schema:

**Via stdio (Claude Desktop, etc.):**
```json
{
  "name": "updateTicket",
  "description": "Update an existing ticket",
  "inputSchema": {
    "type": "object",
    "properties": {
      "id": { "type": "integer" },
      "status": { 
        "type": "string",
        "enum": ["open", "in_progress", "pending_customer", "resolved"]
      },
      "priority": { "type": "string" }
    },
    "required": ["id"]
  },
  "annotations": {
    "requiresApproval": true,
    "dryRunSupported": true
  }
}
```

**Via HTTP (custom agents):**
```sh
# Start HTTP server (default mode)
cori run
# Dashboard at http://localhost:8080
# MCP endpoint at http://localhost:3000

# Call tools via HTTP
curl -X POST http://localhost:3000/tools/listCustomers \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"filters": {"status": "active"}}'
```

Agents get tools like `getCustomer`, `listTickets`, `updateTicketStatus` — automatically generated from your schema and role permissions.

**No raw SQL. Just safe, typed actions.**

---

## 🏗️ Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         cori binary                             │
├─────────────────────────────────────┬───────────────────────────┤
│         MCP Server                  │      Admin Dashboard      │
│  (stdio or http on :3000)           │      (http on :8080)      │
├─────────────────────────────────────┴───────────────────────────┤
│  Tool Generator → Permission Check → Tenant Inject → Audit      │
├─────────────────────────────────────────────────────────────────┤
│                    Upstream Postgres                            │
└─────────────────────────────────────────────────────────────────┘
```

**Single binary. No external dependencies. No policy engine to deploy.**

---

## 🆚 Why Not Just...

| Alternative | Problem |
|-------------|---------|
| **Native Postgres RLS** | Requires session variables; no standard token format; no MCP |
| **OPA / Cerbos / Cedar** | Extra service to deploy; latency; policy sprawl |
| **API Gateway** | Doesn't understand database operations; can't inject row-level predicates |
| **LangChain SQL Agent** | Generates raw SQL; no tenant isolation |

**Cori is purpose-built for the AI-agent-to-database use case.**

---

## 📊 Current Status

> **Alpha Release** — Core MCP server and token system work. Building toward production hardening.

| Component | Status |
|-----------|--------|
| Biscuit token auth | ✅ Working |
| MCP tool generation | ✅ Working |
| Tenant isolation | ✅ Working |
| Admin dashboard | 🚧 In progress |
| Connection pooling | 📋 Planned |
| Production hardening | 📋 Planned |


---

## 📖 Documentation

- **[examples/demo/](examples/demo/)** — Working demo with Docker Compose

---

## 🤝 Contributing

We'd love your help! Here's how:

- ⭐ **Star the repo** — It helps others find us
- 🐛 **Report bugs** — Open an issue
- 💡 **Suggest features** — Tell us your use case

---

## 📜 License

Apache 2.0 — Use it, fork it, build on it.

---

<div align="center">

**Cori: Because AI agents shouldn't have `sudo` on your database.**

[Get Started](#-quick-start) • [Star on GitHub ⭐](https://github.com/cori-do/cori)

</div>
