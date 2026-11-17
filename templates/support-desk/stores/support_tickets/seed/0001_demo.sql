-- Demo rows for sandboxes and local runs: applied once, after the migrations, when a version's sandbox or the
-- `cori run` database is created. The live database, created at `cori promote`, never runs seed files.
INSERT INTO customers VALUES ('C-77', 'Globex', 'ops@globex.example', 'pro', 4800), ('C-12', 'Initech', 'it@initech.example', 'enterprise', 52000);
INSERT INTO tickets (ticket_number, subject, body, department, status, created_at) VALUES
 ('T-1001', 'Login loop on SSO', 'Users bounce between IdP and app', 'IT', 'open', now() - interval '12 days'),
 ('T-1007', 'Wrong VAT on invoice', 'Invoice shows 19% instead of 20%', 'Finance', 'in_progress', now() - interval '9 days'),
 ('T-1010', 'Laptop not delivered', 'Ordered 3 weeks ago', 'IT', 'open', now() - interval '7 days'),
 ('T-1042', 'Invoice charged twice', 'Charged twice for INV-42', 'Finance', 'open', now() - interval '1 day');
