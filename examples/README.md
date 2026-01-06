# Cori Examples

This directory contains a comprehensive demo of Cori MCP server features.

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
| **Multi-Tenant Isolation** | Automatic tenant filtering on all operations |
| **MCP Server** | Typed database tools for AI agent integration |
| **Virtual Schema** | Hide sensitive tables from AI agents |
| **Role-Based Access** | Fine-grained table/column permissions |
| **Admin Dashboard** | Web UI for token management and monitoring |

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
│   ├── cori.yaml             # Main configuration
│   ├── tenancy.yaml          # Tenant column mapping
│   ├── test.sh               # Comprehensive test script
│   ├── README.md             # Detailed documentation
│   ├── database/
│   │   ├── schema.sql        # Multi-tenant CRM schema
│   │   └── seed.sql          # Sample data
│   ├── roles/                # Role definitions
│   │   ├── support_agent.yaml
│   │   ├── sales_agent.yaml
│   │   ├── analytics_agent.yaml
│   │   └── admin_agent.yaml
│   ├── keys/                 # Generated Biscuit keypair
│   ├── tokens/               # Generated tokens
│   └── schema/               # Schema snapshots
```

## Running the Demo

### Full Test Suite

```bash
cd demo
./test.sh
```

This tests:
1. Key generation and token minting
2. Token attenuation to specific tenants
3. MCP server tool generation
4. Dashboard health and functionality
5. Schema introspection

### Manual Testing

```bash
# Generate keys
cori keys generate --output keys/

# Mint role token
cori token mint --key keys/private.key --role support_agent \
    --table "customers:customer_id,first_name,email" \
    --output tokens/role.token

# Attenuate to tenant
cori token attenuate --key keys/private.key \
    --base tokens/role.token \
    --tenant 1 --expires 24h \
    --output tokens/agent.token

# Start server
cori serve --config cori.yaml

# In another terminal, test MCP
cori mcp serve --config cori.yaml --token tokens/agent.token
```

## Claude Desktop Integration

Add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "cori-demo": {
      "command": "cori",
      "args": ["mcp", "serve", "--config", "/path/to/examples/demo/cori.yaml"],
      "env": { "CORI_TOKEN": "<base64 agent.token>" }
    }
  }
}
```

## Services

| Service | Port | Description |
|---------|------|-------------|
| Postgres | 5432 | Demo database |
| MCP HTTP | 8989 | MCP protocol endpoint |
| Dashboard | 8080 | Admin web UI |

## Cleanup

```bash
docker compose down -v
rm -rf keys/ tokens/ schema/
```
