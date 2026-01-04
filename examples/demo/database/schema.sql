-- =============================================================================
-- Cori Demo: Multi-Tenant CRM Database Schema
-- =============================================================================
-- This schema demonstrates a multi-tenant CRM database where all data is
-- segmented by organization_id. This is the type of database Cori is designed
-- to protect - ensuring AI agents can only access their tenant's data.
-- =============================================================================

-- =============================================================================
-- ORGANIZATIONS (Tenants)
-- =============================================================================
-- This table represents the different tenants/companies using the CRM.
-- Each organization's data is completely isolated from others.

CREATE TABLE organizations (
    organization_id SERIAL PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    slug VARCHAR(50) UNIQUE NOT NULL,  -- URL-friendly identifier
    plan VARCHAR(20) CHECK (plan IN ('free', 'starter', 'pro', 'enterprise')) DEFAULT 'free',
    settings JSONB DEFAULT '{}',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- USERS (Employees within organizations)
-- =============================================================================
-- Users belong to organizations and have roles within their org.
-- This table contains sensitive auth data that should NEVER be exposed to AI agents.

CREATE TABLE users (
    user_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    username VARCHAR(50) NOT NULL,
    email VARCHAR(100) NOT NULL,
    password_hash VARCHAR(255) NOT NULL,  -- SENSITIVE: Never expose to AI agents
    first_name VARCHAR(50) NOT NULL,
    last_name VARCHAR(50) NOT NULL,
    role VARCHAR(20) CHECK (role IN ('admin', 'sales', 'support', 'manager')) DEFAULT 'sales',
    is_active BOOLEAN DEFAULT true,
    last_login_at TIMESTAMP,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(organization_id, username),
    UNIQUE(organization_id, email)
);

-- =============================================================================
-- API KEYS (Organization-level secrets)
-- =============================================================================
-- API keys for integrations. NEVER expose this table to AI agents.

CREATE TABLE api_keys (
    api_key_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    name VARCHAR(100) NOT NULL,
    key_prefix VARCHAR(10) NOT NULL,    -- First few chars for identification
    key_hash VARCHAR(255) NOT NULL,     -- SENSITIVE: Hashed key
    scopes TEXT[] DEFAULT ARRAY[]::TEXT[],
    expires_at TIMESTAMP,
    last_used_at TIMESTAMP,
    created_by INTEGER REFERENCES users(user_id),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    revoked_at TIMESTAMP  -- NULL means active
);

-- =============================================================================
-- CUSTOMERS (The clients of each organization)
-- =============================================================================

CREATE TABLE customers (
    customer_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    first_name VARCHAR(50) NOT NULL,
    last_name VARCHAR(50) NOT NULL,
    email VARCHAR(100) NOT NULL,
    phone VARCHAR(20),
    company VARCHAR(100),
    status VARCHAR(20) CHECK (status IN ('active', 'inactive', 'churned')) DEFAULT 'active',
    notes TEXT,  -- Internal notes, might be sensitive
    lifetime_value DECIMAL(12, 2) DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(organization_id, email)
);

-- =============================================================================
-- CONTACTS (Individual contacts at customer companies)
-- =============================================================================

CREATE TABLE contacts (
    contact_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    customer_id INTEGER NOT NULL REFERENCES customers(customer_id) ON DELETE CASCADE,
    first_name VARCHAR(50) NOT NULL,
    last_name VARCHAR(50) NOT NULL,
    position VARCHAR(100),
    email VARCHAR(100) NOT NULL,
    phone VARCHAR(20),
    is_primary BOOLEAN DEFAULT false,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- ADDRESSES
-- =============================================================================

CREATE TABLE addresses (
    address_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    customer_id INTEGER NOT NULL REFERENCES customers(customer_id) ON DELETE CASCADE,
    street VARCHAR(200) NOT NULL,
    city VARCHAR(100) NOT NULL,
    state VARCHAR(100),
    zip VARCHAR(20),
    country VARCHAR(100) NOT NULL DEFAULT 'USA',
    is_billing BOOLEAN DEFAULT false,
    is_shipping BOOLEAN DEFAULT false,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- PRODUCTS (Catalog - often shared across tenants or per-tenant)
-- =============================================================================
-- For this demo, products are per-organization (multi-tenant catalog).

CREATE TABLE products (
    product_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    name VARCHAR(100) NOT NULL,
    description TEXT,
    sku VARCHAR(50),
    price DECIMAL(10, 2) NOT NULL,
    cost DECIMAL(10, 2),  -- Internal cost, might be sensitive
    category VARCHAR(50),
    stock_quantity INTEGER DEFAULT 0,
    is_active BOOLEAN DEFAULT true,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(organization_id, sku)
);

-- =============================================================================
-- OPPORTUNITIES (Sales pipeline)
-- =============================================================================

CREATE TABLE opportunities (
    opportunity_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    customer_id INTEGER NOT NULL REFERENCES customers(customer_id),
    assigned_to INTEGER REFERENCES users(user_id),
    name VARCHAR(100) NOT NULL,
    description TEXT,
    stage VARCHAR(50) CHECK (stage IN ('lead', 'qualified', 'proposal', 'negotiation', 'closed_won', 'closed_lost')) DEFAULT 'lead',
    estimated_value DECIMAL(12, 2),
    probability INTEGER CHECK (probability >= 0 AND probability <= 100) DEFAULT 10,
    expected_close_date DATE,
    actual_close_date DATE,
    lost_reason TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- ORDERS
-- =============================================================================

CREATE TABLE orders (
    order_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    customer_id INTEGER NOT NULL REFERENCES customers(customer_id),
    opportunity_id INTEGER REFERENCES opportunities(opportunity_id),
    created_by INTEGER REFERENCES users(user_id),
    order_number VARCHAR(50) NOT NULL,
    order_date TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    status VARCHAR(20) CHECK (status IN ('pending', 'processing', 'shipped', 'delivered', 'cancelled')) DEFAULT 'pending',
    shipping_address_id INTEGER REFERENCES addresses(address_id),
    billing_address_id INTEGER REFERENCES addresses(address_id),
    shipping_cost DECIMAL(10, 2) DEFAULT 0.00,
    tax_amount DECIMAL(10, 2) DEFAULT 0.00,
    discount_amount DECIMAL(10, 2) DEFAULT 0.00,
    subtotal DECIMAL(12, 2) NOT NULL DEFAULT 0,
    total_amount DECIMAL(12, 2) NOT NULL,
    notes TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(organization_id, order_number)
);

-- =============================================================================
-- ORDER ITEMS
-- =============================================================================

CREATE TABLE order_items (
    order_item_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    order_id INTEGER NOT NULL REFERENCES orders(order_id) ON DELETE CASCADE,
    product_id INTEGER NOT NULL REFERENCES products(product_id),
    quantity INTEGER NOT NULL DEFAULT 1,
    unit_price DECIMAL(10, 2) NOT NULL,
    discount_percentage DECIMAL(5, 2) DEFAULT 0.00,
    line_total DECIMAL(10, 2) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- INVOICES
-- =============================================================================

CREATE TABLE invoices (
    invoice_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    order_id INTEGER NOT NULL REFERENCES orders(order_id),
    invoice_number VARCHAR(50) NOT NULL,
    invoice_date DATE NOT NULL,
    due_date DATE NOT NULL,
    status VARCHAR(20) CHECK (status IN ('draft', 'sent', 'paid', 'overdue', 'cancelled', 'void')) DEFAULT 'draft',
    total_amount DECIMAL(12, 2) NOT NULL,
    paid_amount DECIMAL(12, 2) DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(organization_id, invoice_number)
);

-- =============================================================================
-- PAYMENTS
-- =============================================================================

CREATE TABLE payments (
    payment_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    invoice_id INTEGER NOT NULL REFERENCES invoices(invoice_id),
    payment_date DATE NOT NULL,
    amount DECIMAL(12, 2) NOT NULL,
    payment_method VARCHAR(50) CHECK (payment_method IN ('credit_card', 'bank_transfer', 'cash', 'check', 'paypal', 'stripe')),
    reference_number VARCHAR(100),
    notes TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- SUPPORT TICKETS
-- =============================================================================

CREATE TABLE tickets (
    ticket_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    customer_id INTEGER NOT NULL REFERENCES customers(customer_id),
    assigned_to INTEGER REFERENCES users(user_id),
    ticket_number VARCHAR(50) NOT NULL,
    subject VARCHAR(200) NOT NULL,
    description TEXT,
    status VARCHAR(20) CHECK (status IN ('open', 'in_progress', 'pending_customer', 'resolved', 'closed')) DEFAULT 'open',
    priority VARCHAR(20) CHECK (priority IN ('low', 'medium', 'high', 'urgent')) DEFAULT 'medium',
    category VARCHAR(50),
    resolved_at TIMESTAMP,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(organization_id, ticket_number)
);

-- =============================================================================
-- TASKS
-- =============================================================================

CREATE TABLE tasks (
    task_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    assigned_to INTEGER REFERENCES users(user_id),
    customer_id INTEGER REFERENCES customers(customer_id),
    opportunity_id INTEGER REFERENCES opportunities(opportunity_id),
    ticket_id INTEGER REFERENCES tickets(ticket_id),
    title VARCHAR(200) NOT NULL,
    description TEXT,
    due_date TIMESTAMP,
    priority VARCHAR(20) CHECK (priority IN ('low', 'medium', 'high', 'urgent')) DEFAULT 'medium',
    status VARCHAR(20) CHECK (status IN ('not_started', 'in_progress', 'completed', 'deferred', 'cancelled')) DEFAULT 'not_started',
    completed_at TIMESTAMP,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- NOTES (Activity notes on various entities)
-- =============================================================================

CREATE TABLE notes (
    note_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    created_by INTEGER NOT NULL REFERENCES users(user_id),
    customer_id INTEGER REFERENCES customers(customer_id),
    opportunity_id INTEGER REFERENCES opportunities(opportunity_id),
    ticket_id INTEGER REFERENCES tickets(ticket_id),
    content TEXT NOT NULL,
    is_internal BOOLEAN DEFAULT false,  -- Internal notes not visible to customers
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- COMMUNICATIONS (Email, calls, meetings log)
-- =============================================================================

CREATE TABLE communications (
    communication_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    user_id INTEGER NOT NULL REFERENCES users(user_id),
    customer_id INTEGER REFERENCES customers(customer_id),
    contact_id INTEGER REFERENCES contacts(contact_id),
    opportunity_id INTEGER REFERENCES opportunities(opportunity_id),
    type VARCHAR(20) CHECK (type IN ('email', 'call', 'meeting', 'chat', 'sms', 'other')) NOT NULL,
    direction VARCHAR(10) CHECK (direction IN ('inbound', 'outbound')) NOT NULL,
    subject VARCHAR(200),
    content TEXT,
    communication_date TIMESTAMP NOT NULL,
    duration_minutes INTEGER,
    outcome TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- TAGS (For categorizing customers)
-- =============================================================================

CREATE TABLE tags (
    tag_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    name VARCHAR(50) NOT NULL,
    color VARCHAR(7) DEFAULT '#3B82F6',  -- Hex color
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(organization_id, name)
);

-- =============================================================================
-- CUSTOMER TAGS (Junction table)
-- =============================================================================

CREATE TABLE customer_tags (
    customer_id INTEGER NOT NULL REFERENCES customers(customer_id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(tag_id) ON DELETE CASCADE,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (customer_id, tag_id)
);

-- =============================================================================
-- AUDIT LOGS (System-wide audit trail)
-- =============================================================================
-- This table tracks all changes. AI agents should typically NOT have access.

CREATE TABLE audit_logs (
    log_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id),
    user_id INTEGER REFERENCES users(user_id),
    entity_type VARCHAR(50) NOT NULL,
    entity_id INTEGER NOT NULL,
    action VARCHAR(20) NOT NULL CHECK (action IN ('create', 'read', 'update', 'delete')),
    old_values JSONB,
    new_values JSONB,
    ip_address VARCHAR(45),
    user_agent TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- BILLING (Sensitive financial data)
-- =============================================================================
-- Organization billing information. NEVER expose to AI agents.

CREATE TABLE billing (
    billing_id SERIAL PRIMARY KEY,
    organization_id INTEGER NOT NULL REFERENCES organizations(organization_id) UNIQUE,
    stripe_customer_id VARCHAR(100),
    current_plan VARCHAR(20) CHECK (current_plan IN ('free', 'starter', 'pro', 'enterprise')),
    billing_email VARCHAR(100),
    billing_name VARCHAR(100),
    tax_id VARCHAR(50),
    card_last_four VARCHAR(4),  -- SENSITIVE
    card_brand VARCHAR(20),
    billing_address JSONB,
    monthly_spend DECIMAL(12, 2) DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- =============================================================================
-- INDEXES
-- =============================================================================

-- Organization-based indexes for RLS performance
CREATE INDEX idx_users_org ON users(organization_id);
CREATE INDEX idx_customers_org ON customers(organization_id);
CREATE INDEX idx_contacts_org ON contacts(organization_id);
CREATE INDEX idx_addresses_org ON addresses(organization_id);
CREATE INDEX idx_products_org ON products(organization_id);
CREATE INDEX idx_opportunities_org ON opportunities(organization_id);
CREATE INDEX idx_orders_org ON orders(organization_id);
CREATE INDEX idx_order_items_org ON order_items(organization_id);
CREATE INDEX idx_invoices_org ON invoices(organization_id);
CREATE INDEX idx_payments_org ON payments(organization_id);
CREATE INDEX idx_tickets_org ON tickets(organization_id);
CREATE INDEX idx_tasks_org ON tasks(organization_id);
CREATE INDEX idx_notes_org ON notes(organization_id);
CREATE INDEX idx_communications_org ON communications(organization_id);
CREATE INDEX idx_tags_org ON tags(organization_id);
CREATE INDEX idx_audit_logs_org ON audit_logs(organization_id);

-- Additional useful indexes
CREATE INDEX idx_customers_email ON customers(organization_id, email);
CREATE INDEX idx_customers_company ON customers(organization_id, company);
CREATE INDEX idx_opportunities_stage ON opportunities(organization_id, stage);
CREATE INDEX idx_orders_status ON orders(organization_id, status);
CREATE INDEX idx_tickets_status ON tickets(organization_id, status);
CREATE INDEX idx_tasks_status ON tasks(organization_id, status);

-- =============================================================================
-- TRIGGER FUNCTION: Update timestamp
-- =============================================================================

CREATE OR REPLACE FUNCTION update_timestamp()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply trigger to all tables with updated_at
CREATE TRIGGER tr_organizations_updated BEFORE UPDATE ON organizations FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_users_updated BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_customers_updated BEFORE UPDATE ON customers FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_contacts_updated BEFORE UPDATE ON contacts FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_addresses_updated BEFORE UPDATE ON addresses FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_products_updated BEFORE UPDATE ON products FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_opportunities_updated BEFORE UPDATE ON opportunities FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_orders_updated BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_order_items_updated BEFORE UPDATE ON order_items FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_invoices_updated BEFORE UPDATE ON invoices FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_payments_updated BEFORE UPDATE ON payments FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_tickets_updated BEFORE UPDATE ON tickets FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_tasks_updated BEFORE UPDATE ON tasks FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_notes_updated BEFORE UPDATE ON notes FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_communications_updated BEFORE UPDATE ON communications FOR EACH ROW EXECUTE FUNCTION update_timestamp();
CREATE TRIGGER tr_billing_updated BEFORE UPDATE ON billing FOR EACH ROW EXECUTE FUNCTION update_timestamp();

-- =============================================================================
-- COMMENTS (for documentation)
-- =============================================================================

COMMENT ON TABLE organizations IS 'Tenant table - each org represents a separate company using the CRM';
COMMENT ON TABLE users IS 'Employee accounts - contains sensitive auth data';
COMMENT ON TABLE api_keys IS 'API keys for integrations - SENSITIVE, never expose to AI';
COMMENT ON TABLE billing IS 'Billing/payment info - SENSITIVE, never expose to AI';
COMMENT ON TABLE audit_logs IS 'Complete audit trail - typically admin-only access';

COMMENT ON COLUMN customers.notes IS 'Internal notes - may contain sensitive info, restrict access';
COMMENT ON COLUMN products.cost IS 'Product cost - sensitive financial data';
COMMENT ON COLUMN notes.is_internal IS 'Internal notes not visible to customers in portal';

