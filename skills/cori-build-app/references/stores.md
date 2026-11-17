# Cori Store

A store is the app's own governed Postgres data: tables from migrations, column rules shared by every role, one
role file per app role saying what that role may read, create, update and delete. The Store turns the role into typed
tools (`getX`, `listX`, `createX`, `updateX`, `deleteX`) and a guarded `query`; a `store` step uses either. Every app has a default
store with no declaration; named stores are declared in `cori.toml`.

## Scaffold

```
cori store new <name> [--roles a,b] [--tenant owner]   # default roles: the app's [auth].roles; --tenant: rows per user
```
writes:
```
stores/<name>/
  rules.yaml                 # column rules: allowed_values, type, tags (pii)
  roles/<role>.yaml          # per app role: readable / creatable / updatable / deletable per table
  migrations/0001_init.sql   # DDL only, applied in file order, additive
  seed/0001_demo.sql         # demo rows: sandboxes and local runs only, never the live database
fixtures/<name>.json         # { "tools": {}, "query": { "*": [] } }
cori.toml                    # + [connectors.<name>] kind = "store", ref = "<name>"  and  [connectors.<name>.dev] mock = "fixtures/<name>.json"
```
Steps then say `connector: "<name>"` and the workflow lists `<name>` in `connectors:`. Edit the migration and the
role files, then `cori check .` (validates role columns against the migrations and step SQL against both).
The scaffolded `items` table is shared (`global: true`) unless `--tenant <column>` is given, which adds the column,
writes `tenant: <column>` and seeds rows for `alice`. Its `id` has a database default (`gen_random_uuid()`), so
`createItem` returns it and callers never generate ids; the role file includes `deletable: true`.

## Migrations and seed

- `migrations/*.sql`: plain Postgres DDL, applied in file order. To a version's **sandbox** at `cori deploy`, to the
  **live** database at `cori promote`, to the **local** database of `cori run` (under `cori up`). Keep them additive: a
  promotion applies pending migrations to live data and a rollback undoes none.
- `seed/*.sql`: applied once, after the migrations, when a sandbox or the local database is created. The live database
  never runs seed files. Put demo rows here, only DDL in `migrations/`.
- Database names: `cori_store_<app>_<store>` (live), `…_<hash12>` (a version's sandbox), `…_local` (`cori run`).
  `cori reseed <app> <hash>` drops and recreates a sandbox. `cori store list` shows every database of the app.

```sql
-- migrations/0001_init.sql
CREATE TABLE refund_requests (
  id TEXT PRIMARY KEY DEFAULT ('RR-' || floor(random() * 90000 + 10000)::text),
  ticket_id TEXT NOT NULL, invoice_id TEXT NOT NULL, amount_eur NUMERIC NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending', requested_by TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- seed/0001_demo.sql
INSERT INTO refund_requests (id, ticket_id, invoice_id, amount_eur) VALUES ('RR-1', 'T-1042', 'INV-42', 120);
```

## `rules.yaml` (shared by every role)

```yaml
version: "1.0.0"
tables:
  customers:
    global: true
    columns:
      email: { type: email, tags: [pii] }          # pii: reported by `cori check` when a step exposes it without `redact:`
      plan:  { allowed_values: [free, pro, enterprise] }
  refund_requests:
    global: true
    columns:
      status: { allowed_values: [pending, approved, declined, refunded] }
  notes:
    tenant: author                                  # rows belong to a user: see "Row ownership"
```

Every rule applies to every write path, whatever the key type: `allowed_values` and `type` on create and update,
and the role's `restrict_to` and `only_when` (below).

### Row ownership

`global: true` means the table is shared: every role that can read it sees every row. `tenant: <column>` scopes the
table to the **caller's user id** (`ctx.user.sub` of the run principal: the endpoint caller, or a schedule's service
account):
- `listX`, `getX`, `updateX`, `deleteX` and `query` only see rows where `<column>` equals it; another user's row is
  "no row with id = …";
- `createX` sets `<column>` from it (a value passed in the input is ignored), and `updateX` cannot change it.

So a per-user todo list is `tenant: owner`, with no ownership checks in code steps. A role that must see every
user's rows says so on the table, in its role file: `scope: all` (the default is `own`). Such a role lists, reads and
writes every row, and may set the tenant column on create when it is `creatable` (it defaults to the caller):

```yaml
# roles/manager.yaml: every member's time entries; roles/member.yaml keeps the default `scope: own`
tables:
  time_entries:
    scope: all
    readable: [id, owner, project_id, hours, day]
    creatable: { owner: {}, project_id: { required: true }, hours: { required: true }, day: { required: true } }
```

List the tenant column in a `scope: all` role's `readable` when it needs to know whose row it is (a report per
person): such a role reads it only then. A role scoped to its own rows may always use it in a `query` (it holds the
caller's own id), listed or not.

### A person with several roles: list roles strongest first

A person may hold several of the app's roles (a manager who also logs their own time as an employee; the app's owner
holds them all). In a store they act as **one** role: the first of them that cori.toml's `[auth] roles` lists,
whatever order their token gives (an identity provider often sorts them alphabetically: `employee` before `manager`).
So **list the app's roles strongest first**:

```toml
[auth]
roles = { manager = "group:managers", employee = "group:employees" }   # manager and employee: acts as manager
```

and make the first role cover what the ones after it may do: a manager who logs their own time needs the employee's
readable, creatable and updatable columns in `roles/manager.yaml` too (on top of `scope: all`). Roles that cannot be
ranked (each may do something the other may not) leave a person holding both without one of them: merge their powers
into the stronger one, or give people one of the two. `cori check` warns when a role listed later may do everything an
earlier one may, and more: swap them. Endpoints (`allow.roles`) and the front check every role a person holds; only
the stores pick one.

A child table can take its owner from its parent instead of a column of its own:
`tenant: { via: ticket_id, references: tickets }` (the parent has `tenant: <column>`; one level deep).

A table the migrations create but `rules.yaml` does not name is **invisible to every role** (no row at all), even if a
role file lists it: name it as `global: true` or with `tenant:`. `cori check .` warns.

### Soft delete

`soft_delete: { column: deleted_at }` on a table in `rules.yaml` hides rows where the column is set from every role;
`deletable: { soft_delete: true }` in a role file makes that role's `deleteX` set the column (to `now()`, or `true` for
a boolean column) instead of removing the row.

### Enforced by Postgres

Role files and rules are compiled into Postgres grants and row level security, applied to every sandbox, local and
live database: each call runs as a Postgres role that holds only the columns the role file grants, sees only the rows
of its scope, and checks `allowed_values`, `restrict_to`, `required` and `only_when` in the database as well. A
`query` that reaches another column or another user's row fails in Postgres, whatever the SQL; `cori store query`
and the console's Data tab (Schema: "Enforced by Postgres") run through the same roles. Someone who does not build the
app sees in its Data tab the tables and columns their role reads, and nothing of the others: no schema, no other role.

## `roles/<role>.yaml`

```yaml
name: support_agent
description: Support agents
approvals: { group: support_leads }               # who decides `requires_approval` changes
tables:
  customers:
    readable: [id, name, plan, arr_eur]            # no email: pii stays out of the workflow lineage
  tickets:
    readable: [ticket_number, subject, department, status, created_at]
    updatable:
      status:
        only_when:                                 # state machine on a column
          - { old.status: open, new.status: [in_progress, pending_customer] }
          - { old.status: in_progress, new.status: [pending_customer, resolved] }
  refund_requests:
    readable: [id, ticket_id, invoice_id, amount_eur, status, requested_by]
    creatable:
      ticket_id: { required: true }
      invoice_id: { required: true }
      amount_eur: { required: true }
      status: { restrict_to: [pending] }
    updatable:
      status:
        only_when: [{ old.status: pending, new.status: [approved] }]
        requires_approval: true                    # store-level gate: the change parks until an approver decides
    deletable: true                                # or { requires_approval: true }; gives the role deleteRefundRequest
```

What a role grants decides the **effect** of a store step (`readable` ⇒ Read, `creatable`/`updatable` ⇒ MayWrite,
`requires_approval` ⇒ gate) and which tools exist for it. The role is the run principal's: an endpoint caller's role,
or the schedule's service account role. Columns must exist in the migrations; `cori check .` verifies.

### Writes through workflows only

A role that may change data only by running the app's workflows (never by calling the store directly from a front, the
console or an agent) says so at the top of its file:

```yaml
name: member
writes: workflows        # create/update/delete tools only for workflow runs acting as this role; reads stay direct
tables: { ... }
```

A person or an agent calling the store directly then gets no write tools (`tools/list`) and a refusal naming the rule
(`tools/call`); a run of one of the app's workflows, acting as the same person, still writes, after the workflow's own
checks. Store administrators keep direct access. Use it for every role whose writes carry business rules (who may
approve, how much, for whom): the store's role grants alone cannot express "only through this workflow".

## Store steps

```ts
// typed tool (name from the role: get<Table> / list<Table> / create<Table> / update<Table>)
export default step.store({
  name: "lookup_customer", description: "Look up the customer's plan",
  connector: "tickets", tool: "getCustomer",
  input: (ctx: Ctx<"lookup_customer">) => ({ id: ctx.steps.load_ticket.ticket.customer_id }),
  output: z.object({ plan: z.enum(["free", "pro", "enterprise"]), arr_eur: z.number() }),
});

// guarded query: one SELECT (CTEs, joins and window functions allowed), run by Postgres as the role: readable
// columns and the role's rows only, caps from the role. Bind values with the function form.
export default step.store({
  name: "stale_tickets", description: "Tickets open for more than 5 days",
  connector: "tickets",
  input: (ctx: Ctx<"stale_tickets">) => ({ status: "open" }),
  query: ({ status }) => ({ text: "select ticket_number, subject from tickets where status = $1 and created_at < now() - interval '5 days'", params: [status] }),
  output: z.object({ rows: z.array(z.object({ ticket_number: z.string(), subject: z.string() })) }),
});
// no parameters: the static form `query: { text: "select …", params: [] }` is enough. A static `params` array is
// never bound from the input; `params: ["$1"]` or `params: ["status"]` sends those strings to the database.
```
A query only reads: the store runs one SELECT in a read-only transaction and refuses anything else, so a query step is
always a Read in the change review. Change rows with the store's tools (`create…`, `update…`, `delete…`), never with an
UPDATE in a query: `cori check` refuses a query text that is not one SELECT, and when the text is built by a function
the store refuses it as the step runs. Write the text out where you can: `cori check` then checks its tables and columns.
Free SQL calls aggregates, window functions and string, number, date and JSON functions, including the set-returning
`generate_series` and `unnest` in FROM (a calendar to join on). The caller is a parameter (`params: [ctx.user.sub]`,
then `$1`): `current_user` is the store's database login, not the person, and is refused.
Tool names come from the folder, no server needed: `cori store tools <store> [--role <r>]` prints each role's tools
(name, Read/Write, parameters with `*` = required) from the migrations and `roles/*.yaml`. `cori scaffold <store> <tool>
--workflow <id>` writes the tool form with a typed input skeleton and rejects a name no role exposes; `cori check`
fails a store step whose `tool` no role exposes and warns when a role of the endpoint's `allow.roles` lacks it.
`cori store tools <store> --online` (or `--version <hash>`) asks the server for what your session's role gets;
`cori store query <store> "<sql>"` tries a query (`--version <hash>` on a sandbox).

## Tool inputs and results

| tool | input | result |
|---|---|---|
| `get<Entity>` | the primary key, e.g. `{ id }` | the row (readable columns only); a missing row fails the step |
| `list<Entities>` | optional equality filters on readable columns, `limit` (default 50, capped by the role), `offset` | `{ data: [rows], count, limit, offset }`; `count` is the number of rows in this page, not a total |
| `create<Entity>` | creatable columns | the inserted row |
| `update<Entity>` | the primary key plus the columns to change; `null` clears a nullable column | the row after the change, or `{ pending_approval: true, approval_id, … }` |
| `delete<Entity>` | the primary key | `{ deleted: <n>, <key>: … }` |

Entity names are the table name singular in PascalCase (`items` → `getItem`, `listItems`). A `list` filter on a
column the role cannot read, an unknown key (such as `where`) or a non-scalar value fails the step; for ranges,
`LIKE` or joins use `query`.

## Local runs and fixtures

- With `cori up` running, `cori run` uses the embedded Postgres: the app's `…_local` database is migrated and seeded on
  first use, and the SQL of `query` steps really runs. `--store-database-url <admin url>` targets another Postgres.
- Otherwise (or with `--no-store`) the fixture answers: `"tools"` by tool name, `"query"` by **step name** (`"*"` for
  every query step). Fixtures do not run SQL; they return the rows as they are. Before deploying, run once against a
  real store.

```json
{ "tools": { "getCustomer": { "plan": "pro", "arr_eur": 4800 }, "createRefundRequest": { "id": "RR-31" } },
  "query": { "stale_tickets": [ { "ticket_number": "T-1001", "subject": "Login loop on SSO" } ] } }
```

## Pitfalls

- A `create` on an existing key fails the step. Wrap it in `try` when a duplicate is expected, and give the endpoint
  `idempotency: parameters` or `header` so a double submit replays instead of writing twice.
- A `requires_approval` update parks in the approvals inbox as a store change (`store:<app>:<target>:<store>:<id>`);
  it complements a workflow `human` gate, it does not replace one.
- PII: tag the column in `rules.yaml`, keep it out of `readable` for roles that do not need it, and add
  `redact: [steps.<step>.<field>]` in `workflow.yaml` when a step must carry it.
- Every deployed version has its own sandbox database per store; its preview url and `cori run` write there, never
  live. Only `cori promote` migrates live (and never seeds).
