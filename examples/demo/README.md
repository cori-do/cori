# Cori Demo

This demo showcases all core Cori AI Database Proxy capabilities with a multi-tenant CRM database.

## Features Demonstrated

| Feature | Description |
|---------|-------------|
| **Postgres Wire Protocol** | 100% compatible - use psql, pgAdmin, any Postgres client |
| **Biscuit Token Auth** | Cryptographic tokens with role + tenant claims |
| **RLS Injection** | Automatic tenant predicate injection on all queries |
| **Tenant Isolation** | Cryptographically enforced data separation |
| **Virtual Schema** | Hide sensitive tables from AI agents |
| **Role-Based Access** | Fine-grained table/column permissions |
| **MCP Server** | Expose database as typed tools for AI agents |

## Quick Start

### 1. Build Cori

```bash
# From repository root
cargo build --release
export PATH="$PATH:$(pwd)/target/release"
```

### 2. Start the Database

```bash
cd examples/demo
docker compose up -d
```

### 3. Run the Test Suite

```bash
./test.sh
```

This will:
- Generate Biscuit keys
- Mint tokens for different roles and tenants
- Start the Cori proxy
- Test all features

## Demo Database

The database contains a multi-tenant CRM with **3 organizations**:

| Organization | org_id | Plan | Description |
|-------------|--------|------|-------------|
| Acme Corporation | 1 | pro | Tech startup |
| Globex Inc | 2 | enterprise | Large enterprise |
| Initech | 3 | starter | Small business |

Each organization has completely isolated:
- Customers, Contacts, Orders
- Opportunities, Tickets, Tasks
- Products, Communications, Notes

**Sensitive tables** (hidden from AI agents):
- `users` - Employee accounts with password hashes
- `api_keys` - Integration secrets
- `billing` - Payment information
- `audit_logs` - System audit trail

## Connecting to Cori

Once the proxy is running:

```bash
# Read the token
TOKEN=$(cat tokens/acme_support.token)

# Connect via psql
PGPASSWORD="$TOKEN" psql -h localhost -p 5433 -U agent -d cori_demo

# Or use connection string
psql "postgresql://agent:$TOKEN@localhost:5433/cori_demo"
```

## Token Hierarchy

```
Role Token (long-lived, no tenant)
    │
    ├── Attenuate → Acme Agent Token (org_id=1, 24h)
    ├── Attenuate → Globex Agent Token (org_id=2, 24h)
    └── Attenuate → Initech Agent Token (org_id=3, 24h)
```

## How RLS Works

When an agent sends:
```sql
SELECT * FROM customers WHERE status = 'active'
```

Cori rewrites it to:
```sql
SELECT * FROM customers WHERE status = 'active' AND organization_id = 1
```

The tenant predicate is injected based on the Biscuit token's claims.

## Available Roles

| Role | Access Level |
|------|--------------|
| `support_agent` | Read customers/tickets, update ticket status |
| `sales_agent` | Full customer/opportunity access |
| `analytics_agent` | Read-only aggregation access |
| `admin_agent` | Full access (use sparingly) |

## Files

```
demo/
├── docker-compose.yml    # Database container
├── cori.yaml             # Full configuration
├── cori.yaml             # Main configuration
├── tenancy.yaml          # Tenant column mapping
├── test.sh               # Comprehensive test script
├── database/
│   ├── schema.sql        # Multi-tenant schema
│   └── seed.sql          # Sample data
├── roles/
│   ├── support_agent.yaml
│   ├── sales_agent.yaml
│   ├── analytics_agent.yaml
│   └── admin_agent.yaml
├── keys/                 # Generated Biscuit keypair
├── tokens/               # Generated tokens
└── schema/               # Schema snapshots
```

## Test Commands

```bash
# Run all tests
./test.sh

# Just setup (database + keys + tokens)
./test.sh setup

# Test proxy features only
./test.sh proxy

# Test MCP server
./test.sh mcp

# Cleanup
./test.sh cleanup
```

## Manual Testing

### Generate Keys
```bash
cori keys generate --output keys/
```

### Mint Tokens
```bash
# Role token (no tenant restriction)
cori token mint --key keys/private.key --role support_agent \
    --table "customers:customer_id,first_name,email" \
    --output tokens/support_role.token

# Attenuate to tenant
cori token attenuate --key keys/private.key \
    --base tokens/support_role.token \
    --tenant 1 --expires 24h \
    --output tokens/acme_support.token
```

### Start Proxy
```bash
cori serve --config cori.yaml
```

### Test Queries
```bash
TOKEN=$(cat tokens/acme_support.token)

# This returns only Acme customers
PGPASSWORD="$TOKEN" psql -h localhost -p 5433 -U agent -d cori_demo \
    -c "SELECT first_name, company FROM customers LIMIT 5;"

# This returns 0 rows (cross-tenant blocked)
PGPASSWORD="$TOKEN" psql -h localhost -p 5433 -U agent -d cori_demo \
    -c "SELECT * FROM customers WHERE organization_id = 2;"
```

### MCP Server
```bash
# Start MCP server for AI agent integration
cori mcp serve --config cori.yaml --token tokens/acme_support.token
```

## Ports

| Service | Port | Description |
|---------|------|-------------|
| Postgres (direct) | 5432 | Raw database access |
| Cori Proxy | 5433 | Protected access with RLS |
| Dashboard | 8080 | Admin UI (when enabled) |

## Troubleshooting

### Database won't start
```bash
docker compose down -v  # Remove volume
docker compose up -d    # Recreate
```

### Cori won't connect
```bash
# Check database is running
docker compose ps

# Check Cori logs
cat .cori.log

# Test direct connection
PGPASSWORD=postgres psql -h localhost -U postgres -d cori_demo -c "SELECT 1"
```

### Token issues
```bash
# Verify token
cori token verify --key keys/public.key tokens/acme_support.token

# Inspect claims
cori token inspect tokens/acme_support.token
```
