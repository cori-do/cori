-- =============================================================================
-- Cori Demo: Multi-Tenant CRM Seed Data
-- =============================================================================
-- This creates sample data for THREE organizations (tenants):
--   1. Acme Corp (org_id=1) - A tech startup, "pro" plan
--   2. Globex Inc (org_id=2) - An enterprise company, "enterprise" plan  
--   3. Initech (org_id=3) - A small business, "starter" plan
--
-- This demonstrates how Cori's RLS injection ensures an AI agent for Acme
-- can NEVER see Globex or Initech data, and vice versa.
-- =============================================================================

-- =============================================================================
-- ORGANIZATIONS (The Tenants)
-- =============================================================================

INSERT INTO organizations (name, slug, plan, settings) VALUES
('Acme Corporation', 'acme', 'pro', '{"timezone": "America/Chicago", "currency": "USD"}'),
('Globex Inc', 'globex', 'enterprise', '{"timezone": "America/New_York", "currency": "USD", "sso_enabled": true}'),
('Initech', 'initech', 'starter', '{"timezone": "America/Los_Angeles", "currency": "USD"}');

-- =============================================================================
-- USERS (Employees per Organization)
-- =============================================================================
-- Each organization has their own users. AI agents should NOT see other orgs' users.

-- Acme Corporation (org_id = 1)
INSERT INTO users (organization_id, username, email, password_hash, first_name, last_name, role) VALUES
(1, 'sarah.admin', 'sarah@acme.io', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Sarah', 'Chen', 'admin'),
(1, 'john.sales', 'john@acme.io', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'John', 'Martinez', 'sales'),
(1, 'emily.sales', 'emily@acme.io', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Emily', 'Johnson', 'sales'),
(1, 'mike.support', 'mike@acme.io', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Mike', 'Williams', 'support');

-- Globex Inc (org_id = 2)
INSERT INTO users (organization_id, username, email, password_hash, first_name, last_name, role) VALUES
(2, 'admin', 'admin@globex.com', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'James', 'Anderson', 'admin'),
(2, 'lisa.mgr', 'lisa@globex.com', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Lisa', 'Taylor', 'manager'),
(2, 'tom.sales', 'tom@globex.com', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Tom', 'Brown', 'sales'),
(2, 'anna.sales', 'anna@globex.com', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Anna', 'Davis', 'sales'),
(2, 'chris.support', 'chris@globex.com', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Chris', 'Wilson', 'support');

-- Initech (org_id = 3)
INSERT INTO users (organization_id, username, email, password_hash, first_name, last_name, role) VALUES
(3, 'peter', 'peter@initech.co', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Peter', 'Gibbons', 'admin'),
(3, 'michael', 'michael@initech.co', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Michael', 'Bolton', 'sales'),
(3, 'samir', 'samir@initech.co', '$2a$10$XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX', 'Samir', 'Nagheenanajar', 'support');

-- =============================================================================
-- API KEYS (SENSITIVE - Should be blocked from AI agents)
-- =============================================================================

INSERT INTO api_keys (organization_id, name, key_prefix, key_hash, scopes, created_by) VALUES
(1, 'Production API', 'acme_prod', '$2a$10$KEYKEYKEYKEYKEYKEYKEYKEY', ARRAY['read', 'write'], 1),
(1, 'Zapier Integration', 'acme_zap', '$2a$10$ZAPZAPZAPZAPZAPZAPZAPZAP', ARRAY['read'], 1),
(2, 'Main Integration Key', 'glbx_main', '$2a$10$GLOBEXGLOBEXGLOBEXGLOBEX', ARRAY['read', 'write', 'admin'], 5),
(3, 'Website Key', 'init_web', '$2a$10$INITECHINITECHINITECHINITEC', ARRAY['read'], 12);

-- =============================================================================
-- BILLING (SENSITIVE - Should be blocked from AI agents)
-- =============================================================================

INSERT INTO billing (organization_id, stripe_customer_id, current_plan, billing_email, billing_name, card_last_four, card_brand, monthly_spend) VALUES
(1, 'cus_acme123', 'pro', 'billing@acme.io', 'Acme Corporation', '4242', 'visa', 299.00),
(2, 'cus_globex456', 'enterprise', 'ap@globex.com', 'Globex Inc', '5555', 'mastercard', 999.00),
(3, 'cus_initech789', 'starter', 'peter@initech.co', 'Initech', '1234', 'visa', 49.00);

-- =============================================================================
-- CUSTOMERS (Different customers per organization)
-- =============================================================================

-- Acme Corporation's customers (org_id = 1)
INSERT INTO customers (organization_id, first_name, last_name, email, phone, company, status, lifetime_value) VALUES
(1, 'David', 'Smith', 'david@bigtech.com', '555-100-1001', 'BigTech Solutions', 'active', 15000.00),
(1, 'Jennifer', 'Lee', 'jlee@startup.io', '555-100-1002', 'Startup.io', 'active', 8500.00),
(1, 'Robert', 'Garcia', 'robert@enterprise.net', '555-100-1003', 'Enterprise Networks', 'active', 45000.00),
(1, 'Amanda', 'Wilson', 'amanda@cloudco.com', '555-100-1004', 'CloudCo', 'inactive', 3200.00),
(1, 'Michael', 'Thompson', 'mthompson@datainc.com', '555-100-1005', 'Data Inc', 'active', 22000.00);

-- Globex Inc's customers (org_id = 2)
INSERT INTO customers (organization_id, first_name, last_name, email, phone, company, status, lifetime_value) VALUES
(2, 'Elizabeth', 'Moore', 'emoore@megacorp.com', '555-200-2001', 'MegaCorp International', 'active', 125000.00),
(2, 'William', 'Taylor', 'wtaylor@fortune500.com', '555-200-2002', 'Fortune 500 Ltd', 'active', 89000.00),
(2, 'Patricia', 'Anderson', 'panderson@global.biz', '555-200-2003', 'Global Business Inc', 'active', 67000.00),
(2, 'Christopher', 'Thomas', 'cthomas@techgiant.com', '555-200-2004', 'TechGiant', 'churned', 45000.00),
(2, 'Jessica', 'Jackson', 'jjackson@innovate.co', '555-200-2005', 'Innovate Co', 'active', 156000.00),
(2, 'Daniel', 'White', 'dwhite@systems.org', '555-200-2006', 'Systems Organization', 'active', 98000.00);

-- Initech's customers (org_id = 3)
INSERT INTO customers (organization_id, first_name, last_name, email, phone, company, status, lifetime_value) VALUES
(3, 'Bill', 'Lumbergh', 'bill@tps.com', '555-300-3001', 'TPS Reports Inc', 'active', 2500.00),
(3, 'Milton', 'Waddams', 'milton@stapler.co', '555-300-3002', 'Stapler Company', 'active', 1800.00),
(3, 'Nina', 'McInerneys', 'nina@flair.biz', '555-300-3003', 'Flair Business', 'inactive', 950.00);

-- =============================================================================
-- CONTACTS
-- =============================================================================

-- Acme's contacts
INSERT INTO contacts (organization_id, customer_id, first_name, last_name, position, email, phone, is_primary) VALUES
(1, 1, 'David', 'Smith', 'CEO', 'david@bigtech.com', '555-100-1001', true),
(1, 1, 'Karen', 'Smith', 'CTO', 'karen@bigtech.com', '555-100-1011', false),
(1, 2, 'Jennifer', 'Lee', 'Founder', 'jlee@startup.io', '555-100-1002', true),
(1, 3, 'Robert', 'Garcia', 'VP Sales', 'robert@enterprise.net', '555-100-1003', true),
(1, 3, 'Maria', 'Santos', 'Procurement', 'maria@enterprise.net', '555-100-1033', false);

-- Globex's contacts
INSERT INTO contacts (organization_id, customer_id, first_name, last_name, position, email, phone, is_primary) VALUES
(2, 6, 'Elizabeth', 'Moore', 'Chief Procurement Officer', 'emoore@megacorp.com', '555-200-2001', true),
(2, 6, 'Frank', 'Johnson', 'VP Operations', 'fjohnson@megacorp.com', '555-200-2011', false),
(2, 7, 'William', 'Taylor', 'CEO', 'wtaylor@fortune500.com', '555-200-2002', true),
(2, 10, 'Jessica', 'Jackson', 'CTO', 'jjackson@innovate.co', '555-200-2005', true);

-- Initech's contacts
INSERT INTO contacts (organization_id, customer_id, first_name, last_name, position, email, phone, is_primary) VALUES
(3, 12, 'Bill', 'Lumbergh', 'VP', 'bill@tps.com', '555-300-3001', true),
(3, 13, 'Milton', 'Waddams', 'Archivist', 'milton@stapler.co', '555-300-3002', true);

-- =============================================================================
-- PRODUCTS (Per-organization catalog)
-- =============================================================================

-- Acme's products (software/SaaS)
INSERT INTO products (organization_id, name, description, sku, price, cost, category, stock_quantity) VALUES
(1, 'Basic Plan', 'Basic SaaS subscription', 'ACME-BASIC', 29.99, 5.00, 'Subscription', 9999),
(1, 'Pro Plan', 'Professional SaaS subscription', 'ACME-PRO', 99.99, 15.00, 'Subscription', 9999),
(1, 'Enterprise Plan', 'Enterprise SaaS subscription', 'ACME-ENT', 299.99, 50.00, 'Subscription', 9999),
(1, 'Onboarding Service', 'White-glove onboarding', 'ACME-ONBOARD', 499.00, 200.00, 'Services', 100),
(1, 'Custom Integration', 'Custom API integration work', 'ACME-CUSTOM', 2500.00, 1000.00, 'Services', 50);

-- Globex's products (enterprise solutions)
INSERT INTO products (organization_id, name, description, sku, price, cost, category, stock_quantity) VALUES
(2, 'Data Analytics Platform', 'Enterprise analytics suite', 'GLX-ANALYTICS', 5000.00, 1500.00, 'Software', 100),
(2, 'Security Suite', 'Complete security solution', 'GLX-SECURITY', 8000.00, 2500.00, 'Software', 100),
(2, 'Cloud Infrastructure', 'Managed cloud services', 'GLX-CLOUD', 15000.00, 5000.00, 'Infrastructure', 50),
(2, 'Premium Support', 'Annual premium support', 'GLX-SUPPORT', 3000.00, 800.00, 'Support', 200),
(2, 'Training Package', 'On-site training (5 days)', 'GLX-TRAINING', 10000.00, 4000.00, 'Services', 30);

-- Initech's products (simple offerings)
INSERT INTO products (organization_id, name, description, sku, price, cost, category, stock_quantity) VALUES
(3, 'TPS Report Template', 'Standard TPS report', 'INIT-TPS', 9.99, 1.00, 'Templates', 1000),
(3, 'Consultant Hour', 'Hourly consulting', 'INIT-CONSULT', 75.00, 25.00, 'Services', 500),
(3, 'Flair Pack', '15 pieces of flair', 'INIT-FLAIR', 14.99, 5.00, 'Merchandise', 200);

-- =============================================================================
-- OPPORTUNITIES
-- =============================================================================

-- Acme's opportunities
INSERT INTO opportunities (organization_id, customer_id, assigned_to, name, description, stage, estimated_value, probability, expected_close_date) VALUES
(1, 1, 2, 'BigTech Enterprise Upgrade', 'Upgrade from Pro to Enterprise plan', 'proposal', 5400.00, 70, '2026-02-15'),
(1, 2, 2, 'Startup.io Expansion', 'Additional seats and onboarding', 'qualified', 2500.00, 50, '2026-03-01'),
(1, 3, 3, 'Enterprise Networks Renewal', 'Annual contract renewal + expansion', 'negotiation', 12000.00, 85, '2026-01-31'),
(1, 5, 2, 'Data Inc Custom Integration', 'API integration project', 'lead', 5000.00, 25, '2026-04-15');

-- Globex's opportunities
INSERT INTO opportunities (organization_id, customer_id, assigned_to, name, description, stage, estimated_value, probability, expected_close_date) VALUES
(2, 6, 7, 'MegaCorp Full Suite', 'Complete platform deployment', 'proposal', 75000.00, 60, '2026-02-28'),
(2, 7, 8, 'Fortune 500 Security Deal', 'Security suite implementation', 'qualified', 40000.00, 45, '2026-03-15'),
(2, 10, 7, 'Innovate Co Cloud Migration', 'Full cloud infrastructure', 'negotiation', 120000.00, 80, '2026-01-20'),
(2, 11, 8, 'Systems Org Training', 'Team training program', 'closed_won', 30000.00, 100, '2025-12-15');

-- Initech's opportunities  
INSERT INTO opportunities (organization_id, customer_id, assigned_to, name, description, stage, estimated_value, probability, expected_close_date) VALUES
(3, 12, 11, 'TPS Report Bulk Order', '500 TPS templates', 'proposal', 4995.00, 70, '2026-01-30'),
(3, 13, 11, 'Stapler Co Consulting', 'Office reorganization consulting', 'lead', 1500.00, 20, '2026-03-01');

-- =============================================================================
-- ORDERS
-- =============================================================================

-- Acme's orders
INSERT INTO orders (organization_id, customer_id, opportunity_id, created_by, order_number, status, subtotal, total_amount) VALUES
(1, 1, NULL, 2, 'ACME-2025-001', 'delivered', 1199.88, 1295.87),
(1, 3, NULL, 3, 'ACME-2025-002', 'processing', 3599.88, 3887.87),
(1, 5, NULL, 2, 'ACME-2025-003', 'pending', 99.99, 107.99);

-- Globex's orders
INSERT INTO orders (organization_id, customer_id, opportunity_id, created_by, order_number, status, subtotal, total_amount) VALUES
(2, 6, NULL, 7, 'GLX-2025-001', 'delivered', 28000.00, 30240.00),
(2, 7, NULL, 8, 'GLX-2025-002', 'shipped', 8000.00, 8640.00),
(2, 10, NULL, 7, 'GLX-2025-003', 'processing', 15000.00, 16200.00),
(2, 11, 8, 8, 'GLX-2025-004', 'delivered', 30000.00, 32400.00);

-- Initech's orders
INSERT INTO orders (organization_id, customer_id, created_by, order_number, status, subtotal, total_amount) VALUES
(3, 12, 11, 'INIT-2025-001', 'delivered', 99.90, 107.89);

-- =============================================================================
-- ORDER ITEMS
-- =============================================================================

-- Acme order items
INSERT INTO order_items (organization_id, order_id, product_id, quantity, unit_price, line_total) VALUES
(1, 1, 2, 12, 99.99, 1199.88),
(1, 2, 3, 12, 299.99, 3599.88),
(1, 3, 2, 1, 99.99, 99.99);

-- Globex order items
INSERT INTO order_items (organization_id, order_id, product_id, quantity, unit_price, line_total) VALUES
(2, 4, 6, 1, 5000.00, 5000.00),
(2, 4, 7, 1, 8000.00, 8000.00),
(2, 4, 8, 1, 15000.00, 15000.00),
(2, 5, 7, 1, 8000.00, 8000.00),
(2, 6, 8, 1, 15000.00, 15000.00),
(2, 7, 10, 3, 10000.00, 30000.00);

-- Initech order items
INSERT INTO order_items (organization_id, order_id, product_id, quantity, unit_price, line_total) VALUES
(3, 8, 11, 10, 9.99, 99.90);

-- =============================================================================
-- INVOICES
-- =============================================================================

INSERT INTO invoices (organization_id, order_id, invoice_number, invoice_date, due_date, status, total_amount, paid_amount) VALUES
(1, 1, 'INV-ACME-2025-001', '2025-01-05', '2025-02-04', 'paid', 1295.87, 1295.87),
(1, 2, 'INV-ACME-2025-002', '2025-01-10', '2025-02-09', 'sent', 3887.87, 0),
(1, 3, 'INV-ACME-2025-003', '2025-01-15', '2025-02-14', 'draft', 107.99, 0),
(2, 4, 'INV-GLX-2025-001', '2025-01-03', '2025-01-18', 'paid', 30240.00, 30240.00),
(2, 5, 'INV-GLX-2025-002', '2025-01-08', '2025-02-07', 'sent', 8640.00, 0),
(2, 6, 'INV-GLX-2025-003', '2025-01-12', '2025-02-11', 'sent', 16200.00, 0),
(2, 7, 'INV-GLX-2025-004', '2025-12-20', '2026-01-19', 'paid', 32400.00, 32400.00),
(3, 8, 'INV-INIT-2025-001', '2025-01-02', '2025-02-01', 'paid', 107.89, 107.89);

-- =============================================================================
-- PAYMENTS
-- =============================================================================

INSERT INTO payments (organization_id, invoice_id, payment_date, amount, payment_method, reference_number) VALUES
(1, 1, '2025-01-15', 1295.87, 'credit_card', 'PAY-ACME-001'),
(2, 4, '2025-01-10', 30240.00, 'bank_transfer', 'WIRE-GLX-001'),
(2, 7, '2025-12-28', 32400.00, 'bank_transfer', 'WIRE-GLX-002'),
(3, 8, '2025-01-05', 107.89, 'credit_card', 'PAY-INIT-001');

-- =============================================================================
-- TICKETS (Support tickets)
-- =============================================================================

-- Acme's tickets
INSERT INTO tickets (organization_id, customer_id, assigned_to, ticket_number, subject, description, status, priority, category) VALUES
(1, 1, 4, 'ACME-TKT-001', 'Cannot access dashboard', 'User reports 403 error when accessing dashboard', 'in_progress', 'high', 'Access'),
(1, 2, 4, 'ACME-TKT-002', 'Feature request: Export to PDF', 'Would like to export reports as PDF', 'open', 'low', 'Feature Request'),
(1, 3, 4, 'ACME-TKT-003', 'Billing question', 'Question about upcoming invoice', 'resolved', 'medium', 'Billing');

-- Globex's tickets
INSERT INTO tickets (organization_id, customer_id, assigned_to, ticket_number, subject, description, status, priority, category) VALUES
(2, 6, 9, 'GLX-TKT-001', 'SSO integration failing', 'SAML assertion not validating', 'in_progress', 'urgent', 'Integration'),
(2, 7, 9, 'GLX-TKT-002', 'Performance degradation', 'Slow queries in analytics module', 'open', 'high', 'Performance'),
(2, 10, 9, 'GLX-TKT-003', 'Training schedule request', 'Need to reschedule training session', 'resolved', 'low', 'Training');

-- Initech's tickets
INSERT INTO tickets (organization_id, customer_id, assigned_to, ticket_number, subject, description, status, priority, category) VALUES
(3, 12, 12, 'INIT-TKT-001', 'TPS report formatting', 'Cover sheet not printing correctly', 'open', 'medium', 'Templates');

-- =============================================================================
-- TASKS
-- =============================================================================

-- Acme's tasks
INSERT INTO tasks (organization_id, assigned_to, customer_id, opportunity_id, ticket_id, title, priority, status, due_date) VALUES
(1, 2, 1, 1, NULL, 'Send updated proposal to BigTech', 'high', 'not_started', '2026-01-20'),
(1, 3, 3, 3, NULL, 'Prepare renewal contract', 'high', 'in_progress', '2026-01-15'),
(1, 4, 1, NULL, 1, 'Investigate dashboard access issue', 'high', 'in_progress', '2026-01-05'),
(1, 2, 2, 2, NULL, 'Schedule demo call with Startup.io', 'medium', 'not_started', '2026-01-25');

-- Globex's tasks
INSERT INTO tasks (organization_id, assigned_to, customer_id, opportunity_id, title, priority, status, due_date) VALUES
(2, 7, 6, 5, 'Finalize MegaCorp proposal pricing', 'high', 'in_progress', '2026-01-18'),
(2, 8, 10, 7, 'Technical review for cloud migration', 'urgent', 'not_started', '2026-01-10'),
(2, 9, 6, NULL, 'Debug SSO integration', 'urgent', 'in_progress', '2026-01-04');

-- Initech's tasks
INSERT INTO tasks (organization_id, assigned_to, customer_id, title, priority, status, due_date) VALUES
(3, 11, 12, 'Follow up on TPS bulk order', 'medium', 'not_started', '2026-01-22');

-- =============================================================================
-- NOTES
-- =============================================================================

INSERT INTO notes (organization_id, created_by, customer_id, opportunity_id, ticket_id, content, is_internal) VALUES
(1, 2, 1, 1, NULL, 'BigTech is very interested in the enterprise features, especially SSO.', false),
(1, 2, 1, NULL, NULL, 'David mentioned they might also be interested in the custom integration. Budget approval pending.', true),
(1, 4, 1, NULL, 1, 'Confirmed the 403 error is related to session timeout. Implementing fix.', false),
(2, 7, 6, 5, NULL, 'MegaCorp wants a multi-year deal. Discussing 3-year commitment for 15% discount.', true),
(2, 9, 6, NULL, 4, 'SSO issue traced to certificate mismatch. Waiting for customer to update cert.', false),
(3, 11, 12, 9, NULL, 'Bill seems enthusiastic about the TPS templates. Following up next week.', false);

-- =============================================================================
-- COMMUNICATIONS
-- =============================================================================

INSERT INTO communications (organization_id, user_id, customer_id, contact_id, opportunity_id, type, direction, subject, content, communication_date, duration_minutes, outcome) VALUES
(1, 2, 1, 1, 1, 'call', 'outbound', 'Enterprise Plan Discussion', 'Discussed enterprise features and pricing.', '2026-01-02 14:00:00', 30, 'Positive response, sending proposal'),
(1, 3, 3, 4, 3, 'meeting', 'outbound', 'Renewal Planning Meeting', 'Annual renewal discussion with procurement team.', '2026-01-05 10:00:00', 60, 'Agreed on renewal terms pending legal review'),
(1, 4, 1, 1, NULL, 'email', 'outbound', 'Dashboard Access - Update', 'Provided update on the access issue investigation.', '2026-01-03 16:30:00', NULL, 'Customer acknowledged'),
(2, 7, 6, 6, 5, 'meeting', 'outbound', 'MegaCorp Platform Demo', 'Full platform demonstration for executive team.', '2025-12-20 09:00:00', 120, 'Very positive, moving to proposal stage'),
(2, 8, 7, 8, 6, 'call', 'inbound', 'Security Requirements Discussion', 'Customer called to discuss security compliance needs.', '2026-01-03 11:00:00', 45, 'Need to prepare compliance documentation'),
(3, 11, 12, 10, 9, 'email', 'outbound', 'TPS Template Order Quote', 'Sent quote for bulk TPS template order.', '2026-01-02 09:00:00', NULL, 'Awaiting response');

-- =============================================================================
-- TAGS
-- =============================================================================

-- Acme's tags
INSERT INTO tags (organization_id, name, color) VALUES
(1, 'Enterprise', '#8B5CF6'),
(1, 'Startup', '#10B981'),
(1, 'High Value', '#F59E0B'),
(1, 'At Risk', '#EF4444'),
(1, 'VIP', '#EC4899');

-- Globex's tags
INSERT INTO tags (organization_id, name, color) VALUES
(2, 'Fortune 500', '#3B82F6'),
(2, 'Strategic Account', '#8B5CF6'),
(2, 'Expanding', '#10B981'),
(2, 'Churned', '#EF4444'),
(2, 'Government', '#6366F1');

-- Initech's tags
INSERT INTO tags (organization_id, name, color) VALUES
(3, 'Local', '#10B981'),
(3, 'Regular', '#3B82F6');

-- =============================================================================
-- CUSTOMER TAGS
-- =============================================================================

INSERT INTO customer_tags (customer_id, tag_id, organization_id) VALUES
(1, 1, 1),   -- BigTech = Enterprise
(1, 3, 1),   -- BigTech = High Value
(2, 2, 1),   -- Startup.io = Startup
(3, 1, 1),   -- Enterprise Networks = Enterprise
(3, 3, 1),   -- Enterprise Networks = High Value
(3, 5, 1),   -- Enterprise Networks = VIP
(6, 6, 2),   -- MegaCorp = Fortune 500
(6, 7, 2),   -- MegaCorp = Strategic Account
(7, 6, 2),   -- Fortune 500 Ltd = Fortune 500
(9, 9, 2),   -- TechGiant = Churned
(10, 7, 2),  -- Innovate Co = Strategic Account
(10, 8, 2),  -- Innovate Co = Expanding
(12, 11, 3), -- TPS Reports = Local
(12, 12, 3); -- TPS Reports = Regular

-- =============================================================================
-- AUDIT LOGS (Sample entries - typically system-generated)
-- =============================================================================

INSERT INTO audit_logs (organization_id, user_id, entity_type, entity_id, action, new_values, ip_address) VALUES
(1, 1, 'organization', 1, 'update', '{"plan": "pro"}', '192.168.1.100'),
(1, 2, 'customer', 1, 'create', '{"email": "david@bigtech.com", "company": "BigTech Solutions"}', '192.168.1.101'),
(1, 2, 'opportunity', 1, 'create', '{"name": "BigTech Enterprise Upgrade", "stage": "proposal"}', '192.168.1.101'),
(2, 5, 'organization', 2, 'update', '{"settings": {"sso_enabled": true}}', '10.0.0.50'),
(2, 7, 'opportunity', 5, 'update', '{"stage": "proposal", "estimated_value": 75000}', '10.0.0.51'),
(3, 10, 'customer', 12, 'create', '{"email": "bill@tps.com", "company": "TPS Reports Inc"}', '172.16.0.10');

-- =============================================================================
-- Summary Statistics (for verification)
-- =============================================================================
-- 
-- Organizations: 3 (Acme, Globex, Initech)
-- Users: 12 total (4 Acme, 5 Globex, 3 Initech)
-- Customers: 14 total (5 Acme, 6 Globex, 3 Initech)
-- Products: 13 total (5 Acme, 5 Globex, 3 Initech)
-- Orders: 8 total (3 Acme, 4 Globex, 1 Initech)
-- Tickets: 7 total (3 Acme, 3 Globex, 1 Initech)
--
-- This data demonstrates complete tenant isolation. An AI agent with a token
-- for Acme (organization_id=1) should NEVER be able to see Globex or Initech data.
-- =============================================================================

