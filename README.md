# Cori — the safest way to let software change your database

**Cori turns database mutations into a simple, reviewable workflow.**

Instead of “someone ran a SQL script in prod”, you get:

- **A plan** (what will happen)
- **A preview** (what would change)
- **An approval** (who allowed it)
- **An execution** (do it safely)

> Cori is built for teams who want to ship faster **without giving everyone direct write access to production databases**.

---

## Why Cori

Databases are the source of truth… and the source of most scary incidents.

- “We need to delete a customer for compliance.”
- “We must fix bad rows after a migration.”
- “Support needs to refund 50 orders.”
- “We need to backfill a column safely.”
- “We want AI agents to take actions, but… not raw SQL.”

**Cori is the action layer for your database.**  
You define safe actions, Cori executes them with guardrails.

---

## Install (takes 10 seconds)

```sh
curl -fsSL https://cli.cori.do/install.sh | bash
```

Verify:

```sh
cori --help
```

---

## What you can do in 3 minutes

### 1) Initialize a Cori project from your database

Cori reads your schema and creates a project folder.

```sh
cori init --from-db "<DATABASE_URL>" --project my-super-app
cd my-super-app
```

### 2) Capture a schema snapshot (optional but recommended)

```sh
export DATABASE_URL="<DATABASE_URL>"
cori schema snapshot
```

### 3) Generate safe “data actions” automatically

This is the magic: Cori generates a catalog of actions from your schema.

```sh
cori generate actions
```

See what you got:

```sh
cori actions list
```

Describe one action:

```sh
cori actions describe SoftDeleteCustomers
```

Validate the generated artifacts:

```sh
cori actions validate
```

---

## Your first Cori plan (no database expertise needed)

Create a file named `plan.yaml`:

```yaml
steps:
  - id: delete_customer
    kind: mutation
    action: SoftDeleteCustomers
    inputs:
      tenant_id: acme
      id: "11111111-1111-1111-1111-111111111111"
      deleted_by: "ops@acme.com"
      reason: "Customer request"
```

### Validate the plan

```sh
cori plan validate plan.yaml
```

### Preview the plan (dry-run)

```sh
cori plan preview plan.yaml
```

You’ll get a clear report of what would happen — without taking action.

---

## Apply → Approve → Execute (the safe mutation workflow)

### 1) Create an intent

This creates a tracked request to change data.

```sh
cori apply plan.yaml
```

Cori prints an `intent_id`.

### 2) Check status

```sh
cori status <intent_id>
```

### 3) Approve it

```sh
cori approve <intent_id> --reason "Approved by ops after preview" --as "user:alice"
```

### 4) Execute it

```sh
cori execute <intent_id>
```

That’s it. You just ran a production mutation like a grown-up.

---

## Want to try without making changes?

Use preview apply:

```sh
cori apply plan.yaml --preview
```

This creates an intent and runs a dry-run immediately.

---

## Policy (optional, but powerful)

Cori can generate starter policies so you can control who can do what.

Generate Cerbos stubs:

```sh
cori generate policy-stubs --engine cerbos
```

You’ll get editable policies under:

```
policies/cerbos/resources/
```

Start with permissive rules in dev, tighten them in prod — on your timeline.

---

## Schema drift? Cori is built for it

Schemas change. Cori expects that.

```sh
cori schema diff
cori generate actions --force
cori actions validate
```

---

## What Cori is (and isn’t)

✅ Cori is:
- A simple CLI to **plan/preview/approve/execute** database actions
- A generator that turns schemas into **safe, reusable actions**
- A foundation for agentic workflows (natural language → safe actions)

❌ Cori is not:
- A BI tool
- A replacement for your database
- A giant framework you have to rewrite your app for

---

## The bold target

**Cori becomes the standard “mutation gateway” for modern teams:**
- humans and systems propose changes as plans
- policies decide what is allowed
- execution happens safely
- integrations stay simple

---

## Get started now

If you have a Postgres database URL, you can try Cori immediately:

```sh
curl -fsSL https://cli.cori.do/install.sh | bash
cori init --from-db "<DATABASE_URL>" --project my-super-app
cd my-super-app
export DATABASE_URL="<DATABASE_URL>"
cori generate actions
cori actions list
```

---

## Community

- Star the repo ⭐️
- Open an issue with your dream workflow
- Tell us your “we changed prod data and regretted it” story 🙃

**Cori is here to make database changes boring (in a good way).**
