<div align="center">


<img src="https://assets.cori.do/cori-logo.png" alt="Cori Logo" width="140" />

### The Secure Kernel for AI

**Give AI agents database access without giving away the keys.**

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

[Quick Start](#-quick-start) • [Why Cori](#-the-problem) • [How It Works](#-how-it-works) • [Documentation](AGENTS.md)

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
- `tenancy.yaml` — Auto-detected tenant columns and FK relationships  
- `keys/` — Biscuit keypair for token signing
- `roles/` — Sample role definitions based on your schema
- `schema/snapshot.json` — Schema snapshot for drift detection

### 2. Start Cori

```sh
cd myproject
cori serve --config cori.yaml
# MCP HTTP server on :8989, Dashboard on :8080
```

### 3. Mint a Token

```sh
# Create a role token
cori token mint --role support_agent --output role.token

# Attenuate for a specific tenant
cori token attenuate \
    --base role.token \
    --tenant acme_corp \
    --expires 24h \
    --output agent.token
```

### 4. Connect Your Agent (Claude Desktop)

Add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "cori": {
      "command": "cori",
      "args": ["mcp", "serve", "--config", "/path/to/cori.yaml"],
      "env": { "CORI_TOKEN": "<base64 agent.token>" }
    }
  }
}
```

**That's it.** The agent gets typed tools like `listCustomers`, `getOrder`, `updateTicketStatus` — all automatically scoped to `acme_corp`'s data.

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

blocked_tables: [users, billing, api_keys]
max_rows_per_query: 100
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

## 🤖 MCP Integration

Cori exposes your database as **typed MCP tools** for AI agents:

**Via stdio (Claude Desktop, etc.):**
```json
{
  "mcpServers": {
    "cori": {
      "command": "cori",
      "args": ["mcp", "serve", "--config", "cori.yaml"],
      "env": { "CORI_TOKEN": "<base64 agent.token>" }
    }
  }
}
```

**Via HTTP (custom agents):**
```sh
# Start HTTP server
cori serve --config cori.yaml
# MCP endpoint at http://localhost:8989

# Call tools via HTTP
curl -X POST http://localhost:8989/tools/listCustomers \
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
│  (stdio or http on :8989)           │      (http on :8080)      │
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

[Get Started](#-quick-start) • [Read the Docs](AGENTS.md) • [Star on GitHub ⭐](https://github.com/cori-do/cori)

</div>
