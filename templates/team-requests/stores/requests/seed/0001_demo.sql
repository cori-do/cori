-- Demo rows for sandboxes and local runs: applied once, after the migrations, when a version's sandbox or the
-- `cori run` database is created. The live database (`cori promote`) never runs seed files.
-- `requested_by` is the user id that sees the row: `alice` is the bearer `dev:alice:member` under `cori up`.
INSERT INTO requests (title, details, amount_eur, category, summary, status, requested_by, requester, decided_by, decision_note, created_at) VALUES
 ('USB-C dock', 'For the desk by the window', 89, 'equipment', 'A USB-C dock for a second desk.', 'approved', 'alice', 'alice@example.com', 'rule', 'Up to 100 €: approved by the rule', now() - interval '6 days'),
 ('Design tool seat', 'Reviews with the agency until March', 180, 'software', 'One design tool seat for reviews with the agency.', 'approved', 'alice', 'alice@example.com', 'manager', 'Fine until March', now() - interval '3 days'),
 ('Conference in Lisbon', 'Two days, train and hotel', 950, 'travel', 'Two days at a conference in Lisbon, travel included.', 'declined', 'bob', 'bob@example.com', 'manager', 'Not this quarter', now() - interval '1 day');
