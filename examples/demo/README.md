# Cori Demo

This demo showcases Cori MCP server capabilities with a multi-tenant CRM database.

## Features Demonstrated

| Feature | Description |
|---------|-------------|
| **Biscuit Token Auth** | Cryptographic tokens with role + tenant claims |
| **Dynamic MCP Tools** | Auto-generated database tools for AI agents |
| **Tenant Isolation** | Cryptographically enforced data separation |
| **Role-Based Access** | Fine-grained table/column permissions |
| **Human-in-the-Loop** | Approval workflow for sensitive operations |
| **Admin Dashboard** | Web UI for token minting and management |

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
- Start the MCP server and dashboard
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

## Token Hierarchy

```
Role Token (long-lived, no tenant)
    │
    ├── Attenuate → Acme Agent Token (org_id=1, 24h)
    ├── Attenuate → Globex Agent Token (org_id=2, 24h)
    └── Attenuate → Initech Agent Token (org_id=3, 24h)
```

## How MCP Works

When an AI agent connects with a Biscuit token:

1. **Token Verified** - Signature, expiration, tenant claims checked
2. **Tools Generated** - Based on role permissions and database schema
3. **Actions Filtered** - Agent only sees tools for accessible tables/columns
4. **Tenant Enforced** - All queries automatically scoped to token's tenant

Example tools generated for `support_agent` role:
- `getCustomer(id)` - Fetch single customer
- `listCustomers(filters, limit)` - Query customers
- `getTicket(id)` - Fetch ticket
- `updateTicket(id, status)` - Update ticket status

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
├── cori.yaml             # Main configuration
├── test.sh               # Comprehensive test script
├── database/
│   ├── schema.sql        # Multi-tenant schema
│   └── seed.sql          # Sample data
├── roles/
│   ├── support_agent.yaml
│   ├── sales_agent.yaml
│   ├── analytics_agent.yaml
│   └── admin_agent.yaml
├── groups/               # Approval groups
├── keys/                 # Generated Biscuit keypair
├── tokens/               # Generated tokens
└── schema/
    ├── schema.yaml       # Auto-generated database schema
    ├── rules.yaml        # Tenancy and validation rules
    └── types.yaml        # Reusable semantic types
```

## Test Commands

```bash
# Run all tests
./test.sh

# Just setup (database + keys + tokens)
./test.sh setup

# Test MCP server
./test.sh mcp

# Test dashboard
./test.sh dashboard

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
cori token mint --role support_agent --output tokens/support_role.token

# Attenuate to tenant
cori token attenuate \
    --base tokens/support_role.token \
    --tenant 1 --expires 24h \
    --output tokens/acme_support.token
```

### Start Server
```bash
cori run --config cori.yaml
# Starts MCP server on :3000 and Dashboard on :8080
```

### MCP Server (stdio mode)
```bash
# Start MCP server for AI agent integration (Claude Desktop, etc.)
CORI_TOKEN="$(cat tokens/acme_support.token | base64)" cori run --stdio --config cori.yaml
```

### Claude Desktop Configuration

Add to `claude_desktop_config.json`:
```json
{
  "mcpServers": {
    "cori": {
      "command": "cori",
      "args": ["run", "--stdio", "--config", "/path/to/cori.yaml"],
      "env": { "CORI_TOKEN": "<base64 token>" }
    }
  }
}
```

## Ports

| Service | Port | Description |
|---------|------|-------------|
| Postgres (direct) | 5432 | Raw database access |
| MCP Server (HTTP) | 3000 | MCP protocol endpoint |
| Dashboard | 8080 | Admin UI |

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
# Inspect token claims
cori token inspect tokens/acme_support.token

# Verify token (with public key)
cori token inspect tokens/acme_support.token --key keys/public.key
```
