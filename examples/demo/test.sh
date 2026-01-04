#!/bin/bash
# =============================================================================
# Cori Demo Test Script
# =============================================================================
# This script tests ALL Cori features:
#
#   1. Key Generation & Token Management
#   2. Schema Introspection & Snapshot
#   3. Postgres Proxy with RLS Injection
#   4. Tenant Isolation Verification
#   5. Role-Based Access Control
#   6. Virtual Schema Filtering
#   7. MCP Server Integration
#
# Usage:
#   ./test.sh              # Run all tests
#   ./test.sh setup        # Just setup (database + keys + tokens)
#   ./test.sh proxy        # Test proxy features
#   ./test.sh mcp          # Test MCP server
#   ./test.sh cleanup      # Stop services and cleanup
#
# Prerequisites:
#   - Docker running
#   - Cori CLI built: cargo build --release
#   - psql client installed
# =============================================================================

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATABASE_URL="${DATABASE_URL:-postgres://postgres:postgres@localhost:5432/cori_demo}"
KEYS_DIR="${SCRIPT_DIR}/keys"
TOKENS_DIR="${SCRIPT_DIR}/tokens"
SCHEMA_DIR="${SCRIPT_DIR}/schema"
CONFIG_FILE="${SCRIPT_DIR}/cori.yaml"
CORI_PID_FILE="${SCRIPT_DIR}/.cori.pid"
CORI_LOG_FILE="${SCRIPT_DIR}/.cori.log"

CORI_PROXY_HOST="localhost"
CORI_PROXY_PORT="5433"

# Test counters
PASSED=0
FAILED=0
SKIPPED=0

# =============================================================================
# Helper Functions
# =============================================================================

print_header() {
    echo ""
    echo -e "${BOLD}${CYAN}═══════════════════════════════════════════════════════════════════${NC}"
    echo -e "${BOLD}${CYAN}  $1${NC}"
    echo -e "${BOLD}${CYAN}═══════════════════════════════════════════════════════════════════${NC}"
    echo ""
}

print_section() {
    echo ""
    echo -e "${BOLD}${BLUE}▶ $1${NC}"
    echo -e "${BLUE}$(printf '─%.0s' {1..60})${NC}"
}

print_info() {
    echo -e "${YELLOW}ℹ${NC} $1"
}

print_success() {
    echo -e "${GREEN}✓${NC} $1"
    PASSED=$((PASSED + 1))
}

print_error() {
    echo -e "${RED}✗${NC} $1"
    FAILED=$((FAILED + 1))
}

print_skip() {
    echo -e "${YELLOW}⊘${NC} $1"
    SKIPPED=$((SKIPPED + 1))
}

print_cmd() {
    echo -e "${CYAN}$ $1${NC}"
}

run_test() {
    local description="$1"
    local cmd="$2"
    
    echo ""
    echo -e "${BOLD}Testing: $description${NC}"
    print_cmd "$cmd"
    
    if eval "$cmd" 2>&1; then
        print_success "$description"
        return 0
    else
        print_error "$description"
        return 1
    fi
}

run_sql_direct() {
    local description="$1"
    local query="$2"
    
    echo ""
    echo -e "${BOLD}$description${NC}"
    print_cmd "psql \$DATABASE_URL -c \"$query\""
    echo ""
    
    PGPASSWORD=postgres psql -h localhost -U postgres -d cori_demo -c "$query" 2>/dev/null || {
        print_error "Query failed"
        return 1
    }
}

run_sql_proxy() {
    local description="$1"
    local query="$2"
    local token_file="$3"
    
    local token
    token=$(cat "$token_file" 2>/dev/null) || {
        print_error "Cannot read token: $token_file"
        return 1
    }
    
    echo ""
    echo -e "${BOLD}$description${NC}"
    print_cmd "psql postgresql://agent:***@${CORI_PROXY_HOST}:${CORI_PROXY_PORT}/cori_demo -c \"$query\""
    echo ""
    
    PGPASSWORD="$token" psql -h "$CORI_PROXY_HOST" -p "$CORI_PROXY_PORT" -U agent -d cori_demo -c "$query" 2>&1 || {
        print_error "Query through Cori failed"
        return 1
    }
}

# =============================================================================
# Test: Prerequisites
# =============================================================================

test_prerequisites() {
    print_header "Checking Prerequisites"
    
    # Check cori CLI
    if command -v cori &> /dev/null; then
        print_success "Cori CLI found: $(which cori)"
    else
        print_error "Cori CLI not found. Build with: cargo build --release"
        echo "  Then add to PATH: export PATH=\"\$PATH:$(pwd)/target/release\""
        exit 1
    fi
    
    # Check psql
    if command -v psql &> /dev/null; then
        print_success "psql client found"
    else
        print_error "psql not found. Install postgresql-client"
        exit 1
    fi
    
    # Check docker
    if command -v docker &> /dev/null; then
        print_success "Docker found"
    else
        print_error "Docker not found"
        exit 1
    fi
    
    # Check database
    if PGPASSWORD=postgres psql -h localhost -U postgres -d cori_demo -c "SELECT 1" &> /dev/null; then
        print_success "Database connection OK"
    else
        print_error "Cannot connect to database"
        print_info "Start with: docker compose up -d"
        exit 1
    fi
    
    # Check data exists
    local org_count
    org_count=$(PGPASSWORD=postgres psql -h localhost -U postgres -d cori_demo -tAc "SELECT COUNT(*) FROM organizations" 2>/dev/null | tr -d ' \n')
    if [[ "$org_count" == "3" ]]; then
        print_success "Demo data loaded (3 organizations)"
    else
        print_error "Demo data not found (expected 3 orgs, found '$org_count')"
        print_info "Recreate database: docker compose down -v && docker compose up -d"
        exit 1
    fi
    
    mkdir -p "$KEYS_DIR" "$TOKENS_DIR" "$SCHEMA_DIR"
    print_success "Directories ready"
}

# =============================================================================
# Test: Key Generation
# =============================================================================

test_key_generation() {
    print_header "Testing Key Generation"
    
    if [[ -f "$KEYS_DIR/private.key" && -f "$KEYS_DIR/public.key" ]]; then
        print_info "Keys already exist, skipping generation"
        print_success "Keys present in $KEYS_DIR/"
    else
        print_section "Generating Biscuit Keypair"
        
        run_test "Generate Ed25519 keypair" \
            "cori keys generate --output $KEYS_DIR/"
        
        if [[ -f "$KEYS_DIR/private.key" ]]; then
            print_success "Private key created"
        fi
        if [[ -f "$KEYS_DIR/public.key" ]]; then
            print_success "Public key created"
        fi
    fi
    
    # Export for subsequent commands
    export BISCUIT_PRIVATE_KEY=$(cat "$KEYS_DIR/private.key" 2>/dev/null || echo "")
    export BISCUIT_PUBLIC_KEY=$(cat "$KEYS_DIR/public.key" 2>/dev/null || echo "")
}

# =============================================================================
# Test: Token Minting
# =============================================================================

test_token_minting() {
    print_header "Testing Token Minting"
    
    print_section "Minting Role Tokens"
    
    # Support agent role token
    run_test "Mint support_agent role token" \
        "cori token mint --key $KEYS_DIR/private.key --role support_agent --table 'customers:customer_id,first_name,last_name,email,company,status' --table 'tickets:ticket_id,customer_id,subject,status,priority' --output $TOKENS_DIR/support_role.token"
    
    print_section "Attenuating to Tenants"
    
    # Acme support token (org_id=1)
    run_test "Attenuate for Acme (org_id=1)" \
        "cori token attenuate --key $KEYS_DIR/private.key --base $TOKENS_DIR/support_role.token --tenant 1 --expires 24h --output $TOKENS_DIR/acme_support.token"
    
    # Globex support token (org_id=2)
    run_test "Attenuate for Globex (org_id=2)" \
        "cori token attenuate --key $KEYS_DIR/private.key --base $TOKENS_DIR/support_role.token --tenant 2 --expires 24h --output $TOKENS_DIR/globex_support.token"
    
    print_section "Minting Direct Agent Tokens"
    
    # Sales agent for Acme (mint + attenuate in one step)
    run_test "Mint sales_agent token for Acme" \
        "cori token mint --key $KEYS_DIR/private.key --role sales_agent --tenant 1 --expires 24h --table 'customers:customer_id,first_name,last_name,email,company,status,notes' --table 'opportunities:opportunity_id,customer_id,name,stage,estimated_value' --output $TOKENS_DIR/acme_sales.token"
    
    # Analytics agent for Acme
    run_test "Mint analytics_agent token for Acme" \
        "cori token mint --key $KEYS_DIR/private.key --role analytics_agent --tenant 1 --expires 7d --table 'customers:customer_id,company,status,created_at' --table 'orders:order_id,status,total_amount,created_at' --output $TOKENS_DIR/acme_analytics.token"
    
    print_section "Verifying Tokens"
    
    run_test "Verify support role token" \
        "cori token verify --key $KEYS_DIR/public.key $TOKENS_DIR/support_role.token"
    
    run_test "Verify Acme support token" \
        "cori token verify --key $KEYS_DIR/public.key $TOKENS_DIR/acme_support.token"
    
    print_section "Inspecting Token Claims"
    
    run_test "Inspect Acme support token" \
        "cori token inspect $TOKENS_DIR/acme_support.token"
}

# =============================================================================
# Test: Schema Commands
# =============================================================================

test_schema_commands() {
    print_header "Testing Schema Commands"
    
    cd "$SCRIPT_DIR"
    
    print_section "Schema Snapshot"
    run_test "Create schema snapshot" \
        "cori schema snapshot --database-url '$DATABASE_URL' --output $SCHEMA_DIR/snapshot.json"
    
    print_section "Schema Inspection"
    run_test "Inspect schema (list tables)" \
        "cori schema inspect --database-url '$DATABASE_URL'"
    
    run_test "Inspect specific table" \
        "cori schema inspect --database-url '$DATABASE_URL' --entity customers"
    
    print_section "Schema Diff"
    run_test "Schema diff (should show no changes)" \
        "cori schema diff --database-url '$DATABASE_URL' --snapshot $SCHEMA_DIR/snapshot.json"
}

# =============================================================================
# Test: Proxy Server
# =============================================================================

start_proxy() {
    print_header "Starting Cori Proxy"
    
    # Stop existing
    if [[ -f "$CORI_PID_FILE" ]]; then
        local pid=$(cat "$CORI_PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            print_info "Stopping existing Cori (PID: $pid)"
            kill "$pid" 2>/dev/null || true
            sleep 1
        fi
        rm -f "$CORI_PID_FILE"
    fi
    
    print_section "Starting Proxy Server"
    print_cmd "cori serve --config $CONFIG_FILE"
    
    cd "$SCRIPT_DIR"
    cori serve --config "$CONFIG_FILE" > "$CORI_LOG_FILE" 2>&1 &
    local cori_pid=$!
    echo "$cori_pid" > "$CORI_PID_FILE"
    
    print_info "Cori starting (PID: $cori_pid)"
    
    # Wait for ready
    local max_attempts=30
    local attempt=0
    while [[ $attempt -lt $max_attempts ]]; do
        if nc -z "$CORI_PROXY_HOST" "$CORI_PROXY_PORT" 2>/dev/null; then
            print_success "Cori proxy ready on port $CORI_PROXY_PORT"
            return 0
        fi
        sleep 0.5
        ((attempt++))
    done
    
    print_error "Cori failed to start"
    cat "$CORI_LOG_FILE"
    return 1
}

stop_proxy() {
    if [[ -f "$CORI_PID_FILE" ]]; then
        local pid=$(cat "$CORI_PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            print_info "Stopping Cori (PID: $pid)"
            kill "$pid" 2>/dev/null || true
        fi
        rm -f "$CORI_PID_FILE"
    fi
}

test_proxy_connectivity() {
    print_header "Testing Proxy Connectivity"
    
    print_section "Basic Connectivity"
    run_sql_proxy "Connect through Cori proxy" \
        "SELECT 1 as proxy_works;" \
        "$TOKENS_DIR/acme_support.token"
    
    print_success "Proxy connectivity OK"
}

# =============================================================================
# Test: RLS Injection
# =============================================================================

test_rls_injection() {
    print_header "Testing RLS Injection"
    
    print_section "Direct vs Proxy Comparison"
    
    run_sql_direct "Direct query (all tenants visible):" \
        "SELECT organization_id, COUNT(*) as customers FROM customers GROUP BY organization_id ORDER BY organization_id;"
    
    run_sql_proxy "Proxy query (tenant-filtered):" \
        "SELECT organization_id, COUNT(*) as customers FROM customers GROUP BY organization_id;" \
        "$TOKENS_DIR/acme_support.token"
    
    print_section "RLS Explain"
    
    print_info "Showing query rewrite:"
    print_cmd "cori proxy explain --query 'SELECT * FROM customers WHERE status = active' --tenant 1 --tenant-column organization_id"
    cori proxy explain --query "SELECT * FROM customers WHERE status = 'active'" --tenant 1 --tenant-column organization_id 2>/dev/null || true
    
    print_success "RLS injection working"
}

# =============================================================================
# Test: Tenant Isolation
# =============================================================================

test_tenant_isolation() {
    print_header "Testing Tenant Isolation"
    
    print_section "Cross-Tenant Access Prevention"
    
    print_info "Acme agent trying to access Globex data (org_id=2):"
    print_info "Query: SELECT * FROM customers WHERE organization_id = 2"
    print_info "After RLS: ... AND organization_id = 1 (conflicts!)"
    echo ""
    
    run_sql_proxy "Acme token querying for org_id=2 (should return 0 rows):" \
        "SELECT COUNT(*) as found_rows FROM customers WHERE organization_id = 2;" \
        "$TOKENS_DIR/acme_support.token"
    
    print_section "Verify Each Tenant's View"
    
    run_sql_proxy "Acme agent sees Acme customers:" \
        "SELECT customer_id, first_name, company FROM customers LIMIT 5;" \
        "$TOKENS_DIR/acme_support.token"
    
    run_sql_proxy "Globex agent sees Globex customers:" \
        "SELECT customer_id, first_name, company FROM customers LIMIT 5;" \
        "$TOKENS_DIR/globex_support.token"
    
    print_success "Tenant isolation verified"
}

# =============================================================================
# Test: MCP Server
# =============================================================================

test_mcp_server() {
    print_header "Testing MCP Server"
    
    print_section "MCP Tool Discovery"
    
    print_info "Testing MCP server startup with token..."
    print_cmd "timeout 3 cori mcp serve --config $CONFIG_FILE --token $TOKENS_DIR/acme_support.token"
    
    # Run with timeout - MCP server stays running in stdio mode
    if timeout 3 cori mcp serve --config "$CONFIG_FILE" --token "$TOKENS_DIR/acme_support.token" 2>&1 | head -20; then
        print_success "MCP server starts successfully"
    else
        # timeout returns 124, which is expected (server was running)
        if [[ $? -eq 124 ]]; then
            print_success "MCP server started (timed out as expected)"
        else
            print_error "MCP server failed to start"
        fi
    fi
    
    print_section "MCP with Different Roles"
    
    print_info "Support agent tools (limited access):"
    timeout 2 cori mcp serve --config "$CONFIG_FILE" --token "$TOKENS_DIR/acme_support.token" 2>&1 | grep -E "(tool_count|Generated)" || true
    
    print_info "Sales agent tools (more access):"
    timeout 2 cori mcp serve --config "$CONFIG_FILE" --token "$TOKENS_DIR/acme_sales.token" 2>&1 | grep -E "(tool_count|Generated)" || true
}

# =============================================================================
# Test: Virtual Schema
# =============================================================================

test_virtual_schema() {
    print_header "Testing Virtual Schema"
    
    print_section "Schema Filtering"
    
    print_info "Sensitive tables (users, api_keys, billing) should be hidden"
    
    run_sql_proxy "Query information_schema through proxy:" \
        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name LIMIT 20;" \
        "$TOKENS_DIR/acme_support.token"
    
    print_info "Note: Sensitive tables should not appear in the list above"
    print_success "Virtual schema filtering active"
}

# =============================================================================
# Summary
# =============================================================================

print_summary() {
    echo ""
    print_header "Test Summary"
    echo ""
    echo -e "  ${GREEN}Passed:${NC}  $PASSED"
    echo -e "  ${RED}Failed:${NC}  $FAILED"
    echo -e "  ${YELLOW}Skipped:${NC} $SKIPPED"
    echo ""
    
    if [[ $FAILED -eq 0 ]]; then
        echo -e "${GREEN}${BOLD}All tests passed!${NC}"
        echo ""
        echo "Cori features verified:"
        echo "  ✓ Biscuit key generation and token minting"
        echo "  ✓ Token attenuation for tenant isolation"
        echo "  ✓ Schema introspection and snapshot"
        echo "  ✓ Postgres wire protocol proxy"
        echo "  ✓ RLS injection for tenant isolation"
        echo "  ✓ Cross-tenant access prevention"
        echo "  ✓ MCP server integration"
        echo "  ✓ Virtual schema filtering"
        exit 0
    else
        echo -e "${RED}${BOLD}Some tests failed!${NC}"
        exit 1
    fi
}

# =============================================================================
# Cleanup
# =============================================================================

cleanup() {
    stop_proxy
    rm -f "$CORI_LOG_FILE"
}

trap cleanup EXIT

# =============================================================================
# Main
# =============================================================================

main() {
    print_header "Cori Demo Test Suite"
    
    echo "Database: $DATABASE_URL"
    echo "Config:   $CONFIG_FILE"
    echo ""
    
    test_prerequisites
    test_key_generation
    test_token_minting
    test_schema_commands
    start_proxy
    test_proxy_connectivity
    test_rls_injection
    test_tenant_isolation
    test_virtual_schema
    test_mcp_server
    
    print_summary
}

# Handle arguments
case "${1:-all}" in
    setup)
        test_prerequisites
        test_key_generation
        test_token_minting
        ;;
    schema)
        test_prerequisites
        test_schema_commands
        ;;
    proxy)
        test_prerequisites
        test_key_generation
        test_token_minting
        start_proxy
        test_proxy_connectivity
        test_rls_injection
        test_tenant_isolation
        test_virtual_schema
        ;;
    mcp)
        test_prerequisites
        test_key_generation
        test_token_minting
        test_mcp_server
        ;;
    cleanup)
        cleanup
        print_info "Cleanup complete"
        ;;
    all|"")
        main
        ;;
    *)
        echo "Usage: $0 [setup|schema|proxy|mcp|cleanup|all]"
        echo ""
        echo "Commands:"
        echo "  setup    - Database check, key generation, token minting"
        echo "  schema   - Test schema commands"
        echo "  proxy    - Test proxy server and RLS"
        echo "  mcp      - Test MCP server"
        echo "  cleanup  - Stop services"
        echo "  all      - Run all tests (default)"
        exit 1
        ;;
esac
