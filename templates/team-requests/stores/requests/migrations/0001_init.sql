-- The team's requests: plain Postgres DDL, applied in file order to a version's sandbox at `cori deploy`, to the live
-- database at `cori promote`, to the local database of `cori run`. Rows are written by `submit_request` only.
CREATE TABLE requests (
  id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text,   -- createRequest returns it; callers never invent ids
  title TEXT NOT NULL,
  details TEXT NOT NULL DEFAULT '',
  amount_eur NUMERIC NOT NULL DEFAULT 0,
  category TEXT NOT NULL DEFAULT 'other',
  summary TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'pending',
  requested_by TEXT NOT NULL,                             -- the requester's user id, set by createRequest (`tenant: requested_by`)
  requester TEXT NOT NULL DEFAULT '',                     -- who that is, as managers read it (their email)
  decided_by TEXT,                                        -- `rule` or `manager`
  decision_note TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
