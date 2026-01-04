# Cori Examples

This directory contains a comprehensive demo of all Cori AI Database Proxy features.

## Quick Start

```bash
# 1. Build Cori
cd ..
cargo build --release
export PATH="$PATH:$(pwd)/target/release"

# 2. Start the demo database
cd examples/demo
docker compose up -d

# 3. Run the test suite
./test.sh
```

## Demo Overview

The `demo/` directory showcases Cori's complete feature set:

| Feature | Description |
|---------|-------------|
| **Biscuit Token Auth** | Ed25519 key generation, token minting, attenuation |
| **Multi-Tenant Isolation** | Row-Level Security injection on all queries |
| **Postgres Wire Protocol** | 100% compatible proxy - use any Postgres client |
| **Virtual Schema** | Hide sensitive tables from AI agents |
| **Role-Based Access** | Fine-grained table/column permissions |
| **MCP Server** | Typed database tools for AI agent integration |

## Demo Database

The demo uses a multi-tenant CRM database with **3 organizations**:

- **Acme Corporation** (org_id=1) - Tech startup on "pro" plan
- **Globex Inc** (org_id=2) - Enterprise customer
- **Initech** (org_id=3) - Small business on "starter" plan

Each organization has isolated:
- Customers, Contacts, Addresses
- Orders, Invoices, Payments
- Opportunities, Tickets, Tasks
- Products, Communications, Notes

## Directory Structure

```
examples/
├── demo/                     # Main demo directory
│   ├── docker-compose.yml    # Database container
│   ├── cori.yaml             # Full configuration
│   ├── cori.yaml             # Main configuration
│   ├── tenancy.yaml          # Tenant column mapping
│   ├── test.sh               # Comprehensive test script
│   ├── README.md             # Detailed documentation
│   ├── database/
│   │   ├── schema.sql        # Multi-tenant CRM schema
│   │   └── seed.sql          # Sample data (3 orgs)
│   ├── roles/
│   │   ├── support_agent.yaml
│   │   ├── sales_agent.yaml
│   │   ├── analytics_agent.yaml
│   │   └── admin_agent.yaml
│   ├── keys/                 # Generated Biscuit keypair
│   ├── tokens/               # Generated tokens
│   └── schema/               # Schema snapshots
└── README.md                 # This file
```

## Running the Demo

### Full Test Suite

```bash
cd demo
./test.sh
```

This tests:
1. Prerequisites check
2. Key generation
3. Token minting & attenuation
4. Schema introspection
5. Proxy server startup
6. RLS injection verification
7. Tenant isolation proof
8. Virtual schema filtering
9. MCP server integration

### Individual Tests

```bash
./test.sh setup    # Keys + tokens only
./test.sh schema   # Schema commands
./test.sh proxy    # Proxy + RLS tests
./test.sh mcp      # MCP server test
./test.sh cleanup  # Stop services
```

## How It Works

### Token-Based Authentication

```
Admin mints role token → Attenuates with tenant → Agent uses token
```

### RLS Injection

Agent query:
```sql
SELECT * FROM customers WHERE status = 'active'
```

Cori rewrites to:
```sql
SELECT * FROM customers WHERE status = 'active' AND organization_id = 1
```

### Cross-Tenant Prevention

If an Acme agent (org_id=1) tries to access Globex data (org_id=2):
```sql
-- Original: SELECT * FROM customers WHERE organization_id = 2
-- After RLS: SELECT * FROM customers WHERE organization_id = 2 AND organization_id = 1
-- Result: 0 rows (predicates conflict!)
```

## Ports

| Service | Port |
|---------|------|
| PostgreSQL (direct) | 5432 |
| Cori Proxy | 5433 |
| Dashboard | 8080 |

## See Also

- [demo/README.md](demo/README.md) - Detailed demo documentation
- [AGENTS.md](../AGENTS.md) - Full project architecture
