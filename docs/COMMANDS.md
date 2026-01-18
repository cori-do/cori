# Cori CLI Commands Reference

This document describes all available commands in the Cori CLI (`cori`).

---

## Table of Contents

- [init](#init) — Initialize a new Cori project
- [db](#db) — Database schema management
- [keys](#keys) — Biscuit keypair management
- [token](#token) — Biscuit token management
- [run](#run) — Start the MCP server and dashboard
- [tools](#tools) — Tool introspection (offline)
- [check](#check) — Validate configuration files

---

## `init`

Initialize a Cori MCP server project from an existing database.

This command introspects your database schema and creates a complete project structure with:
- Configuration file (`cori.yaml`) with tenant isolation settings
- Biscuit keypair for token authentication (`keys/`)
- Sample role definitions (`roles/`)
- Sample approval groups (`groups/`)
- Schema files (`schema/`) including auto-generated schema, rules, and types
- Proper `.gitignore` for security

### Usage

```bash
cori init --from-db <DATABASE_URL> --project <name> [--force]
```

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `--from-db <URL>` | ✅ | PostgreSQL connection URL (e.g., `postgres://user:pass@host:5432/db`) |
| `--project <name>` | ✅ | Project name (also used as output directory) |
| `--force` | ❌ | Overwrite if the project directory already exists (default: `false`) |

### Example

```bash
# Initialize a new project from an existing database
cori init --from-db "postgres://postgres:postgres@localhost:5432/myapp" --project my-cori-project

# Overwrite an existing project
cori init --from-db "postgres://postgres:postgres@localhost:5432/myapp" --project my-cori-project --force
```

### Generated Structure

```
my-cori-project/
├── cori.yaml               # Main configuration file
├── .gitignore              # Ignores keys and sensitive files
├── README.md               # Project-specific getting started guide
├── keys/
│   ├── private.key         # Ed25519 private key (keep secure!)
│   └── public.key          # Ed25519 public key
├── roles/
│   └── *.yaml              # Auto-generated role definitions
├── groups/
│   └── *.yaml              # Sample approval groups
└── schema/
    ├── schema.yaml         # Auto-generated database schema (DO NOT EDIT)
    ├── rules.yaml          # Tenancy, soft-delete, validation rules
    └── types.yaml          # Reusable semantic types
```

### What Init Detects

The `init` command automatically:
- Detects tenant columns (e.g., `tenant_id`, `organization_id`, `customer_id`)
- Identifies foreign key relationships for tenant inheritance
- Flags potentially sensitive tables (users, api_keys, billing, etc.)
- Generates appropriate role permissions based on schema analysis

---

## `db`

Database schema management commands.

### `db sync`

Sync database schema to `schema/schema.yaml`.

Introspects the configured database and generates a YAML schema definition that can be used for role-based access control configuration.

#### Usage

```bash
cori db sync [--config <path>]
```

#### Arguments

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `--config`, `-c` | ❌ | `cori.yaml` | Path to configuration file |

#### Environment Variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection URL (fallback if not in config) |

#### Example

```bash
# Sync schema using default configuration
cori db sync

# Sync with custom configuration
cori db sync --config /path/to/cori.yaml
```

#### Output

```
✔ Wrote schema definition: schema/schema.yaml
```

---

## `keys`

Biscuit keypair management commands.

### `keys generate`

Generate a new Ed25519 keypair for Biscuit token signing.

#### Usage

```bash
cori keys generate [--output <dir>]
```

#### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `--output`, `-o` | ❌ | Output directory for key files. If not specified, prints to stdout. |

#### Example

```bash
# Generate keys and save to files
cori keys generate --output keys/
# Creates:
#   keys/private.key
#   keys/public.key

# Generate keys and print to stdout (useful for env vars)
cori keys generate
```

#### Output (with --output)

```
✔ Generated Biscuit keypair:
  Private key: keys/private.key
  Public key:  keys/public.key

⚠️  Keep your private key secure! Never commit it to version control.

Set as environment variables:
  export BISCUIT_PRIVATE_KEY=$(cat keys/private.key)
  export BISCUIT_PUBLIC_KEY=$(cat keys/public.key)
```

---

## `token`

Biscuit token management commands.

All token commands use **convention over configuration** — keys are automatically loaded from:
1. Explicit `--key` argument (file path or hex string)
2. Environment variable (`BISCUIT_PRIVATE_KEY` or `BISCUIT_PUBLIC_KEY`)
3. Configuration in `cori.yaml` (`biscuit.private_key_file` / `biscuit.public_key_file`)
4. Default location: `keys/private.key` / `keys/public.key`

### `token mint`

Mint a new role token (or agent token if `--tenant` is specified).

#### Usage

```bash
cori token mint --role <role> [options]
```

#### Arguments

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `--config`, `-c` | ❌ | `cori.yaml` | Path to configuration file (for key resolution) |
| `--role` | ✅ | | Role name for the token |
| `--key` | ❌ | (from config) | Path to private key file OR hex-encoded key (overrides config) |
| `--tenant` | ❌ | | Tenant ID (if specified, creates an attenuated agent token) |
| `--expires` | ❌ | | Expiration duration (e.g., `24h`, `7d`, `30m`, `60s`) |
| `--table` | ❌ | | Tables to grant access to. Format: `table:col1,col2` or just `table`. Can be repeated. |
| `--output`, `-o` | ❌ | | Output file path. If not specified, prints to stdout. |

#### Environment Variables

| Variable | Description |
|----------|-------------|
| `BISCUIT_PRIVATE_KEY` | Hex-encoded private key (fallback if not in config) |

#### Example

```bash
# Mint a base role token (uses keys/private.key by default)
cori token mint --role support_agent --output role.token

# Mint an agent token with tenant restriction
cori token mint \
  --role support_agent \
  --tenant acme_corp \
  --expires 24h \
  --table customers:id,name,email \
  --table orders \
  --output agent.token

# With explicit key (overrides config)
cori token mint --role support_agent --key /path/to/private.key --output role.token

# Using environment variable for key
export BISCUIT_PRIVATE_KEY=$(cat keys/private.key)
cori token mint --role admin --output admin.token
```

#### Output (with --output)

```
✔ Token written to: agent.token
  Type: Agent token (tenant-restricted)
  Role: support_agent
  Tenant: acme_corp
  Expires: 24h
```

---

### `token attenuate`

Attenuate a role token with tenant restriction and expiration.

Takes an existing role token and adds restrictions (tenant scope, expiration).

#### Usage

```bash
cori token attenuate --base <token-file> --tenant <id> [options]
```

#### Arguments

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `--config`, `-c` | ❌ | `cori.yaml` | Path to configuration file (for key resolution) |
| `--base` | ✅ | | Path to base role token file |
| `--tenant` | ✅ | | Tenant ID to restrict the token to |
| `--key` | ❌ | (from config) | Path to private key file OR hex-encoded key (overrides config) |
| `--expires` | ❌ | | Expiration duration (e.g., `24h`, `7d`) |
| `--output`, `-o` | ❌ | | Output file path. If not specified, prints to stdout. |

#### Environment Variables

| Variable | Description |
|----------|-------------|
| `BISCUIT_PRIVATE_KEY` | Hex-encoded private key (fallback if not in config) |

#### Example

```bash
# Attenuate a role token for a specific tenant (uses keys/private.key by default)
cori token attenuate \
  --base role.token \
  --tenant client_a \
  --expires 24h \
  --output agent.token

# With explicit key (overrides config)
cori token attenuate \
  --base role.token \
  --tenant client_a \
  --key /path/to/private.key \
  --output agent.token
```

#### Output (with --output)

```
✔ Attenuated token written to: agent.token
  Tenant: client_a
  Expires: 24h
```

---

### `token inspect`

Inspect a token's contents, optionally verify with public key.

- **Without `--verify`**: Shows unverified token contents (block count, facts, checks) with a warning.
- **With `--verify`**: Verifies signature using key from config or `--key`.

#### Usage

```bash
cori token inspect <token> [options]
```

#### Arguments

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `<token>` | ✅ | | Token string OR path to token file |
| `--config`, `-c` | ❌ | `cori.yaml` | Path to configuration file (for key resolution) |
| `--key` | ❌ | (from config) | Path to public key file OR hex-encoded key (overrides config) |
| `--verify` | ❌ | `false` | Verify the token signature using key from config or `--key` |

#### Environment Variables

| Variable | Description |
|----------|-------------|
| `BISCUIT_PUBLIC_KEY` | Hex-encoded public key (fallback if not in config) |

#### Example

```bash
# Inspect a token from file (unverified)
cori token inspect agent.token

# Inspect and verify a token (uses keys/public.key from config)
cori token inspect agent.token --verify

# Verify with explicit key (overrides config)
cori token inspect agent.token --key /path/to/public.key

# Inspect a token string directly
cori token inspect "En0KEwoEY..."
```

#### Output (Unverified)

```
Token Information (unverified):
  Block count: 2

Biscuit {
    symbols: ...
    authority: ...
}

💡 Use --verify to verify the token signature (uses key from cori.yaml)
```

#### Output (Verified - Success)

```
✔ Token is valid

Token Details:
  Role: support_agent
  Tenant: client_a (attenuated)
  Type: Agent token
  Block count: 2
```

#### Output (Verified - Failure)

```
✖ Token verification failed: <error message>
```

---

## `run`

Start the Cori MCP server and dashboard.

By default, starts the Dashboard on `:8080` and MCP HTTP server on `:3000`.
Use `--stdio` for Claude Desktop and local agents (requires token).

### Usage

```bash
cori run [options]
```

### Arguments

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `--config`, `-c` | ❌ | `cori.yaml` | Path to configuration file |
| `--http` | ❌ | (default) | Use HTTP transport. Multi-tenant: each request carries its own token. |
| `--stdio` | ❌ | | Use stdio transport. Single-tenant: requires `--token` or `CORI_TOKEN` env. |
| `--token`, `-t` | ❌ | `$CORI_TOKEN` | Token file (required with `--stdio` unless `CORI_TOKEN` env is set) |
| `--mcp-port` | ❌ | `3000` | MCP HTTP port (only with `--http`). Overrides config file. |
| `--dashboard-port` | ❌ | `8080` | Dashboard port. Overrides config file. |
| `--no-dashboard` | ❌ | | Disable dashboard (MCP only). |

### Environment Variables

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | PostgreSQL connection URL (fallback if not in config) |
| `CORI_TOKEN` | Base64-encoded Biscuit token for MCP authentication (for stdio mode) |
| `BISCUIT_PUBLIC_KEY` | Public key for token verification (if configured via `biscuit.public_key_env`) |
| `BISCUIT_PRIVATE_KEY` | Private key for token signing (if configured via `biscuit.private_key_env`) |

### Transport Modes

| Mode | Dashboard | MCP Transport | Token Model |
|------|-----------|---------------|-------------|
| `--http` (default) | `:8080` | HTTP `:3000` | Per-request (Authorization header) |
| `--stdio` | `:8080` | stdio | Baked-in (file or `CORI_TOKEN` env) |

### Example

```bash
# Start with default configuration (HTTP mode)
cori run

# Start with custom configuration
cori run --config /path/to/cori.yaml

# Start with custom ports
cori run --mcp-port 4000 --dashboard-port 9000

# Start in stdio mode (for Claude Desktop)
cori run --stdio --token agent.token

# Start with MCP only, no dashboard
cori run --no-dashboard
```

### Claude Desktop Integration

For Claude Desktop (stdio transport), configure in `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "cori": {
      "command": "cori",
      "args": ["run", "--stdio", "--config", "/path/to/cori.yaml"],
      "env": {
        "CORI_TOKEN": "<base64-encoded-agent-token>"
      }
    }
  }
}
```

### Configuration File

The run command reads from a YAML configuration file. Key sections:

```yaml
# Upstream database connection
upstream:
  database_url_env: DATABASE_URL  # Or use individual fields
  # host: localhost
  # port: 5432
  # database: myapp
  # username: postgres
  # password: secret

# Biscuit key configuration
biscuit:
  public_key_file: keys/public.key
  private_key_file: keys/private.key
  # Or use environment variables:
  # public_key_env: BISCUIT_PUBLIC_KEY
  # private_key_env: BISCUIT_PRIVATE_KEY

# MCP server settings
mcp:
  enabled: true
  transport: http           # "stdio" or "http"
  http_port: 3000

# Dashboard settings
dashboard:
  enabled: true
  listen_port: 8080

# Role definitions directory
roles_dir: roles

# Audit logging
audit:
  enabled: true
  log_queries: true
  log_results: false
```

---

## `tools`

Tool introspection commands. These work offline without starting a server.

### `tools list`

List tools available for a role or token.

#### Usage

```bash
cori tools list [options]
```

#### Arguments

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `--config`, `-c` | ❌ | `cori.yaml` | Path to configuration file (YAML) |
| `--role` | ❌* | | Role name to generate tools for |
| `--token`, `-t` | ❌* | | Token file to extract role from |
| `--verbose` | ❌ | `false` | Show detailed tool schemas |

*Either `--role` or `--token` is required (mutually exclusive).

When using `--token`, the public key is loaded from the configuration (see convention over configuration).

#### Example

```bash
# List tools for a role
cori tools list --role support_agent

# List tools from a token (uses public key from cori.yaml)
cori tools list --token agent.token

# Show detailed schemas
cori tools list --role support_agent --verbose

# With custom config
cori tools list --config /path/to/cori.yaml --role support_agent
```

#### Output

```
🔑 Role Information:
   Role: support_agent
   Tenant: acme_corp

🔧 Available Tools (5):
   • getCustomer (read)
     Retrieve a customer by ID
   • listCustomers (read)
     List customers with optional filters
   • getTicket (read)
     Retrieve a ticket by ID
   • listTickets (read)
     List tickets with optional filters
   • updateTicket (write, approval, dry-run)
     Update a ticket's status or priority

📊 Table Access:
   • customers (read)
   • tickets (read, write)

🚫 Blocked Tables:
   • users
   • billing
   • api_keys
```

---

### `tools describe`

Show detailed schema for a specific tool.

#### Usage

```bash
cori tools describe <tool> --role <role> [--config <path>]
```

#### Arguments

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `<tool>` | ✅ | | Tool name to describe |
| `--role` | ✅ | | Role name |
| `--config`, `-c` | ❌ | `cori.yaml` | Path to configuration file (YAML) |

#### Example

```bash
cori tools describe updateTicket --role support_agent
```

#### Output

```
Tool: updateTicket

Description: Update a ticket's status or priority

Input Schema:
{
  "type": "object",
  "properties": {
    "id": { "type": "integer", "description": "Ticket ID" },
    "status": {
      "type": "string",
      "enum": ["open", "in_progress", "resolved"]
    },
    "priority": { "type": "string" }
  },
  "required": ["id"]
}

Annotations:
  • requiresApproval: true
  • dryRunSupported: true
```

---

## `check`

Validate configuration files for consistency and correctness.

This command performs comprehensive validation:
- JSON Schema validation against schemas in `schemas/`
- Cross-file consistency checks (tables, columns, groups)
- Best practice warnings (soft delete, approval groups)

The `check` command is automatically run as a pre-hook before `run` to catch configuration errors early.

### Usage

```bash
cori check [--config <path>]
```

### Arguments

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `--config`, `-c` | ❌ | `cori.yaml` | Path to configuration file |

### Example

```bash
# Check configuration in current directory
cori check

# Check with custom configuration path
cori check --config /path/to/cori.yaml
```

### Checks Performed

| Check | Description |
|-------|-------------|
| **JSON Schema** | Validates all config files against their JSON schemas |
| **Table Names** | Ensures tables in roles/rules exist in schema |
| **Column Names** | Verifies columns in roles exist in schema tables |
| **Approval Groups** | Confirms referenced approval groups are defined |
| **Soft Delete** | Validates soft delete configuration consistency |
| **Type References** | Checks that type references in rules point to defined types |
| **Non-null Columns** | Warns about non-null columns missing from creatable |

### Output (Success)

```
🔍 Checking Cori configuration...

  📋 Validating JSON schemas...
  📊 Checking table name consistency...
  📝 Checking column name consistency...
  👥 Checking approval group references...
  🗑️  Checking soft delete configuration...
  🔤 Checking type references in rules...
  ⚡ Checking non-null column constraints...

════════════════════════════════════════════════════════════
✅ All checks passed!
```

### Output (With Issues)

```
🔍 Checking Cori configuration...
  ...

❌ Errors (2):
────────────────────────────────────────────────────────────
  ✗ [table-names] [roles/support_agent.yaml]: Table 'nonexistent' not found in schema
  ✗ [approval-groups] [roles/admin.yaml]: Approval group 'managers' not defined

⚠️  Warnings (1):
────────────────────────────────────────────────────────────
  ⚠ [soft-delete] [roles/support_agent.yaml:tickets]: Role has deletable but table has soft_delete configured

════════════════════════════════════════════════════════════
Summary: 2 error(s), 1 warning(s)

❌ Configuration has errors that must be fixed.
```

---

## Quick Reference

### Token Workflow

```bash
# 1. Generate keys (one-time setup)
cori keys generate --output keys/

# 2. Initialize project from database
cori init --from-db "postgres://..." --project myproject

# 3. Validate configuration
cori check

# 4. Mint a role token (uses keys/private.key by default)
cori token mint --role support_agent --output role.token

# 5. Attenuate for a specific tenant
cori token attenuate --base role.token --tenant acme --expires 24h --output agent.token

# 6. Verify the token (uses keys/public.key from config)
cori token inspect agent.token --verify

# 7. Start the server
cori run --config cori.yaml
```

### Environment Variables Summary

| Variable | Used By | Description |
|----------|---------|-------------|
| `DATABASE_URL` | `run`, `db sync` | PostgreSQL connection URL |
| `BISCUIT_PRIVATE_KEY` | `token mint`, `token attenuate` | Hex-encoded Ed25519 private key |
| `BISCUIT_PUBLIC_KEY` | `token inspect`, `run` | Hex-encoded Ed25519 public key |
| `CORI_TOKEN` | `run --stdio` | Base64-encoded Biscuit token for MCP authentication |

---

## Duration Format

Commands that accept duration strings (`--expires`) support:

| Suffix | Unit | Example |
|--------|------|---------|
| `s` | Seconds | `60s` |
| `m` | Minutes | `30m` |
| `h` | Hours | `24h` |
| `d` | Days | `7d` |
| (none) | Hours | `24` = `24h` |
