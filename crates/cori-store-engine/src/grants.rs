// SPDX-License-Identifier: Apache-2.0
//! Compiles a Cori Store's roles and rules into Postgres objects that enforce them: roles, column grants, row level
//! security policies, `CHECK` constraints and `only_when` triggers.
//!
//! [`compile`] is a pure function of the schema, the rules and the roles. Its output is applied to one database in one
//! transaction: the first statement drops every object the compiler owns (policies named `cori_*`, constraints
//! `cori_values_*` and `cori_pattern_*`, triggers and functions `cori_only_when_*`) and revokes
//! every privilege of the store's Postgres roles, the rest re-creates them. A full rebuild is simpler and safer than a
//! diff, and the output is small.
//!
//! # Postgres roles
//!
//! Every Cori role of a store gets two `NOLOGIN` roles, both global to the server:
//! - the **data role** (`<prefix>__<role>`) runs the caller's statements. It is granted what the role may read and
//!   write, minus approval-gated columns, and sees only the rows of its scope that are not soft-deleted.
//! - the **applier role** (`<prefix>__<role>__applier`) runs Cori's own reads and writes on the role's behalf: the
//!   current row for `only_when`, approval snapshots, foreign key checks, soft deletes and approved changes. It holds
//!   the gated grants, reads every column, and is bound by the same row scope.
//!
//! The session sets the caller's id with `SET LOCAL cori.tenant = '<id>'`; a policy compares a tenant column with
//! `current_setting('cori.tenant', true)`, which is NULL when unset, so a missing id matches no row.
//!
//! # Fail closed
//!
//! Row level security is enabled on every table of the schema. A table gets policies only for the roles that name it,
//! and only when `rules.yaml` names it: a table missing from `rules.yaml` stays invisible to every data role.

use cori_store_core::config::role_definition::{ColumnCondition, ComparisonCondition, NumberOrColumnRef, OnlyWhen};
use cori_store_core::config::rules_definition::TenantConfig;
use cori_store_core::config::schema_definition::TableSchema;
use cori_store_core::{RoleDefinition, RowScope, RulesDefinition, SchemaDefinition};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Tables `provision` creates for its own bookkeeping: never granted, never given policies.
pub const BOOKKEEPING_TABLES: &[&str] = &["cori_migrations", "cori_seeds", "cori_policy"];

/// The login role of Cori's data pool: member of every data and applier role, with no privilege of its own, so a
/// statement that runs without switching role reads nothing.
pub const RUNTIME_ROLE: &str = "cori_runtime";

/// Postgres identifiers are at most 63 bytes.
const MAX_IDENT: usize = 63;

/// The compiled policy of one store database.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// The Postgres roles the statements grant to, per Cori role: (Cori role, data role, applier role). Roles are
    /// global to the server, so they are created outside the database transaction (see [`role_statements`]).
    pub roles: Vec<PgRoles>,
    /// Statements for the store's database, in order, to run in one transaction.
    pub statements: Vec<String>,
    /// Hex sha256 of the roles and statements: stored in the database, compared at open.
    pub hash: String,
    /// Findings that do not stop the compilation (a tenant column without an index, a `pattern` Postgres may reject).
    pub warnings: Vec<String>,
}

/// The Postgres roles of one Cori role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgRoles {
    /// The Cori role (`roles/<name>.yaml`).
    pub role: String,
    /// Runs the caller's statements.
    pub data: String,
    /// Runs Cori's own statements on the role's behalf.
    pub applier: String,
}

/// Lower-cased `[a-z0-9_]`, every other character as `_`.
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect()
}

fn sha_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Keeps a name under Postgres' 63-byte limit: a longer one keeps a prefix and 8 hex characters of a sha256 of the
/// full name, so it stays deterministic and distinct.
fn fit(name: &str) -> String {
    if name.len() <= MAX_IDENT {
        return name.to_string();
    }
    let h = &sha_hex(name)[..8];
    let mut keep = MAX_IDENT - 9;
    while !name.is_char_boundary(keep) {
        keep -= 1;
    }
    format!("{}_{h}", &name[..keep])
}

/// The prefix of a store's Postgres role names: `cs_<app>_<store>_<h8>`, where `<h8>` hashes the exact pair so that
/// two stores whose cleaned names collide (`a-b`/`c` and `a`/`b_c`) still get distinct roles.
pub fn role_prefix(app: &str, store: &str) -> String {
    let h = &sha_hex(&format!("{app}\u{0}{store}"))[..8];
    let mut base = format!("cs_{}_{}", clean(app), clean(store));
    // leave room for `_<h8>` and a short role suffix
    base.truncate(32);
    format!("{base}_{h}")
}

/// The data and applier roles of a Cori role under a store prefix.
pub fn pg_roles(prefix: &str, role: &str) -> PgRoles {
    let r = clean(role);
    let r = if r == role { r } else { format!("{r}_{}", &sha_hex(role)[..4]) };
    PgRoles { role: role.to_string(), data: fit(&format!("{prefix}__{r}")), applier: fit(&format!("{prefix}__{r}__applier")) }
}

/// A double-quoted identifier.
pub fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// A single-quoted SQL string literal.
pub fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `body` in dollar quotes whose tag does not occur in it (`$cori$`, else `$cori_1$`, …): a value from a role or rules
/// file inside a DO block or a function body can never close the quote and run as SQL of its own.
pub fn dollar_quoted(body: &str) -> String {
    let mut tag = "$cori$".to_string();
    let mut n = 0;
    while body.contains(&tag) { n += 1; tag = format!("$cori_{n}$"); }
    format!("{tag}{body}{tag}")
}

/// A JSON value from a role or rules file as a SQL literal (strings stay untyped so they coerce to the column's type).
fn value_sql(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => literal(s),
        other => literal(&other.to_string()),
    }
}

/// `expr IN (values)`, with a JSON null in the list matching NULL.
fn in_list(expr: &str, values: &[Value]) -> String {
    let non_null: Vec<String> = values.iter().filter(|v| !v.is_null()).map(value_sql).collect();
    let has_null = values.iter().any(|v| v.is_null());
    match (non_null.is_empty(), has_null) {
        (true, true) => format!("{expr} IS NULL"),
        (true, false) => "false".into(),
        (false, false) => format!("{expr} IN ({})", non_null.join(", ")),
        (false, true) => format!("({expr} IS NULL OR {expr} IN ({}))", non_null.join(", ")),
    }
}

fn is_numeric(native: Option<&str>) -> bool {
    let t = native.unwrap_or("").to_ascii_lowercase();
    ["int", "numeric", "decimal", "real", "double", "float", "serial", "money"].iter().any(|k| t.contains(k))
}

fn is_text(native: Option<&str>) -> bool {
    let t = native.unwrap_or("text").to_ascii_lowercase();
    t == "text" || t.starts_with("varchar") || t.starts_with("character") || t.starts_with("char") || t == "citext" || t == "name"
}

/// `current_setting('cori.tenant', true)` in the tenant column's type, so an index on the column stays usable.
fn tenant_setting(native: Option<&str>) -> String {
    let s = "current_setting('cori.tenant', true)";
    if is_text(native) {
        s.to_string()
    } else {
        format!("NULLIF({s}, '')::{}", native.unwrap_or("text"))
    }
}

struct Ctx<'a> {
    schema: &'a SchemaDefinition,
    rules: &'a RulesDefinition,
}

impl Ctx<'_> {
    fn table(&self, name: &str) -> Option<&TableSchema> {
        self.schema.tables.iter().find(|t| t.name == name && (t.schema.is_empty() || t.schema == "public"))
    }

    fn native(&self, table: &str, column: &str) -> Option<String> {
        self.table(table).and_then(|t| t.get_column(column)).and_then(|c| c.native_type.clone())
    }

    fn all_columns(&self, table: &str) -> Vec<String> {
        self.table(table).map(|t| t.columns.iter().map(|c| c.name.clone()).collect()).unwrap_or_default()
    }

    fn has_column(&self, table: &str, column: &str) -> bool {
        self.table(table).map(|t| t.has_column(column)).unwrap_or(false)
    }

    /// The table's direct tenant column, when it is tenant-scoped through one.
    fn direct_tenant(&self, table: &str) -> Option<String> {
        let tr = self.rules.get_table_rules(table)?;
        if tr.global.unwrap_or(false) {
            return None;
        }
        match &tr.tenant {
            Some(TenantConfig::Direct(c)) => Some(c.clone()),
            _ => None,
        }
    }

    /// For inherited tenancy: (child column, parent table, parent key column, parent tenant column).
    fn inherited(&self, table: &str) -> Result<Option<(String, String, String, String)>, String> {
        let Some(tr) = self.rules.get_table_rules(table) else { return Ok(None) };
        if tr.global.unwrap_or(false) {
            return Ok(None);
        }
        let Some(TenantConfig::Inherited(inh)) = &tr.tenant else { return Ok(None) };
        if !self.has_column(table, &inh.via) {
            return Err(format!("rules.yaml: `{table}` inherits its tenant via `{}`, which is not a column of `{table}`", inh.via));
        }
        let Some(parent_tenant) = self.direct_tenant(&inh.references) else {
            return Err(format!(
                "rules.yaml: `{table}` inherits its tenant from `{}`, which has no direct `tenant: <column>` (inheritance is one level deep)",
                inh.references
            ));
        };
        let t = self.table(table).ok_or_else(|| format!("rules.yaml: `{table}` is not a table of the schema"))?;
        let key = t
            .foreign_keys
            .iter()
            .find(|f| f.columns.len() == 1 && f.columns[0] == inh.via && f.references.table == inh.references)
            .and_then(|f| f.references.columns.first().cloned())
            .or_else(|| self.table(&inh.references).and_then(|p| if p.primary_key.len() == 1 { Some(p.primary_key[0].clone()) } else { None }))
            .ok_or_else(|| format!("rules.yaml: `{table}.{}` does not reference a single-column key of `{}`", inh.via, inh.references))?;
        Ok(Some((inh.via.clone(), inh.references.clone(), key, parent_tenant)))
    }

    /// The row scope expression of a role on a table (the `USING` and `WITH CHECK` of its permissive policy). None when
    /// `rules.yaml` does not name the table: no policy, so no row (fail closed).
    fn scope_expr(&self, table: &str, scope: RowScope) -> Result<Option<String>, String> {
        let Some(tr) = self.rules.get_table_rules(table) else { return Ok(None) };
        if tr.global.unwrap_or(false) || scope == RowScope::All {
            return Ok(Some("true".into()));
        }
        match &tr.tenant {
            None => Ok(Some("true".into())),
            Some(TenantConfig::Direct(col)) => {
                if !self.has_column(table, col) {
                    return Err(format!("rules.yaml: `{table}` names `tenant: {col}`, which is not a column of `{table}`"));
                }
                Ok(Some(format!("{} = {}", ident(col), tenant_setting(self.native(table, col).as_deref()))))
            }
            Some(TenantConfig::Inherited(_)) => {
                let (via, parent, key, ptenant) = self.inherited(table)?.expect("inherited tenancy");
                Ok(Some(format!(
                    "EXISTS (SELECT 1 FROM {p} cori_parent WHERE cori_parent.{k} = {t}.{v} AND cori_parent.{pt} = {s})",
                    p = ident(&parent),
                    k = ident(&key),
                    t = ident(table),
                    v = ident(&via),
                    pt = ident(&ptenant),
                    s = tenant_setting(self.native(&parent, &ptenant).as_deref())
                )))
            }
        }
    }

    /// Predicate for rows that are not soft-deleted, if the table has `soft_delete`.
    fn active(&self, table: &str) -> Option<(String, String)> {
        let sd = self.rules.get_table_rules(table)?.soft_delete.as_ref()?;
        Some((sd.column.clone(), sd.active_predicate(&ident(&sd.column), self.native(table, &sd.column).as_deref())))
    }
}

/// Column-level grants of one Postgres role, per table and privilege.
#[derive(Default)]
struct Grants(BTreeMap<(String, String, &'static str), BTreeSet<String>>);

impl Grants {
    fn add(&mut self, role: &str, table: &str, privilege: &'static str, cols: impl IntoIterator<Item = String>) {
        self.0.entry((role.to_string(), table.to_string(), privilege)).or_default().extend(cols);
    }
    fn table(&mut self, role: &str, table: &str, privilege: &'static str) {
        self.0.entry((role.to_string(), table.to_string(), privilege)).or_default();
    }
    fn statements(&self) -> Vec<String> {
        self.0
            .iter()
            .filter_map(|((role, table, privilege), cols)| {
                if *privilege == "DELETE" {
                    return Some(format!("GRANT DELETE ON {} TO {}", ident(table), ident(role)));
                }
                if cols.is_empty() {
                    return None;
                }
                Some(format!(
                    "GRANT {privilege} ({}) ON {} TO {}",
                    cols.iter().map(|c| ident(c)).collect::<Vec<_>>().join(", "),
                    ident(table),
                    ident(role)
                ))
            })
            .collect()
    }
}

/// Policy name on a table, unique per (kind, Postgres role).
fn policy_name(kind: &str, role: &str) -> String {
    fit(&format!("cori_{kind}_{role}"))
}

/// Compiles the policy of one store database. `prefix` is [`role_prefix`] of the store.
pub fn compile(schema: &SchemaDefinition, rules: &RulesDefinition, roles: &[RoleDefinition], prefix: &str) -> Result<Policy, String> {
    let ctx = Ctx { schema, rules };
    let mut warnings = Vec::new();
    let mut roles: Vec<&RoleDefinition> = roles.iter().collect();
    roles.sort_by(|a, b| a.name.cmp(&b.name));
    for r in &roles {
        if r.name.ends_with("__applier") {
            return Err(format!("role `{}`: a role name may not end with `__applier`", r.name));
        }
    }
    let pg: Vec<PgRoles> = roles.iter().map(|r| pg_roles(prefix, &r.name)).collect();

    let tables: Vec<&TableSchema> = schema
        .tables
        .iter()
        .filter(|t| (t.schema.is_empty() || t.schema == "public") && !BOOKKEEPING_TABLES.contains(&t.name.as_str()))
        .collect();

    let mut out: Vec<String> = Vec::new();

    // 1. drop what a previous compilation created and revoke every privilege of this store's roles, including roles
    //    removed from roles/ since (matched by prefix)
    out.push(format!(
        "DO {}",
        dollar_quoted(&format!(
        r#"
DECLARE r record;
BEGIN
  FOR r IN SELECT policyname, tablename FROM pg_policies WHERE schemaname = 'public' AND starts_with(policyname, 'cori_') LOOP
    EXECUTE format('DROP POLICY %I ON public.%I', r.policyname, r.tablename);
  END LOOP;
  FOR r IN SELECT t.tgname, c.relname FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace
           WHERE n.nspname = 'public' AND NOT t.tgisinternal AND starts_with(t.tgname, 'cori_only_when_') LOOP
    EXECUTE format('DROP TRIGGER %I ON public.%I', r.tgname, r.relname);
  END LOOP;
  FOR r IN SELECT k.conname, c.relname FROM pg_constraint k JOIN pg_class c ON c.oid = k.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace
           WHERE n.nspname = 'public' AND (starts_with(k.conname, 'cori_values_') OR starts_with(k.conname, 'cori_pattern_')) LOOP
    EXECUTE format('ALTER TABLE public.%I DROP CONSTRAINT %I', r.relname, r.conname);
  END LOOP;
  FOR r IN SELECT p.oid::regprocedure AS f FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
           WHERE n.nspname = 'public' AND starts_with(p.proname, 'cori_only_when_') LOOP
    EXECUTE format('DROP FUNCTION %s', r.f);
  END LOOP;
  FOR r IN SELECT rolname FROM pg_roles WHERE starts_with(rolname, {prefix}) LOOP
    EXECUTE format('REVOKE ALL ON ALL TABLES IN SCHEMA public FROM %I', r.rolname);
    EXECUTE format('REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM %I', r.rolname);
    EXECUTE format('REVOKE ALL ON SCHEMA public FROM %I', r.rolname);
  END LOOP;
  -- nothing is readable through PUBLIC: a migration's `GRANT … TO PUBLIC` would bypass the column grants below
  REVOKE ALL ON ALL TABLES IN SCHEMA public FROM PUBLIC;
  REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM PUBLIC;
END
"#,
        prefix = literal(&format!("{prefix}__"))
    ))));

    // 3. every table behind row level security
    for t in &tables {
        out.push(format!("ALTER TABLE {} ENABLE ROW LEVEL SECURITY", ident(&t.name)));
        if rules.get_table_rules(&t.name).is_none() && roles.iter().any(|r| r.tables.contains_key(&t.name)) {
            warnings.push(format!(
                "`{}` is not named in rules.yaml: its rows are invisible to every role (add it as `global: true` or with `tenant: <column>`)",
                t.name
            ));
        }
    }
    for p in &pg {
        out.push(format!("GRANT USAGE ON SCHEMA public TO {}, {}", ident(&p.data), ident(&p.applier)));
    }

    let mut grants = Grants::default();
    let mut policies: Vec<String> = Vec::new();
    // tables a role inserts into: (table, the roles that insert)
    let mut inserting: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // tables a role reaches only indirectly (a parent for inherited tenancy, a referenced table for a foreign key
    // check): (Cori role index, table) → the columns the data role must read, if any
    let mut implied: BTreeMap<(usize, String), BTreeSet<String>> = BTreeMap::new();

    for (i, r) in roles.iter().enumerate() {
        let p = &pg[i];
        let mut names: Vec<&String> = r.tables.keys().collect();
        names.sort();
        for tname in names {
            let perms = &r.tables[tname];
            if ctx.table(tname).is_none() {
                warnings.push(format!("role `{}` names `{tname}`, which is not a table of the schema", r.name));
                continue;
            }
            let all_cols = ctx.all_columns(tname);
            let known = |cols: Vec<String>| -> Vec<String> { cols.into_iter().filter(|c| all_cols.contains(c)).collect() };
            let pk = ctx.table(tname).map(|t| t.primary_key.clone()).unwrap_or_default();
            let scope = perms.scope;
            let own_tenant = if scope == RowScope::Own { ctx.direct_tenant(tname) } else { None };
            // the column a create sets to the caller (`scope: all` too, unless the role may name the owner itself)
            let owner = ctx.direct_tenant(tname);
            let active = ctx.active(tname);
            // approval-gated columns, per operation: a column gated on update may still be set freely on create
            let gated_create: BTreeSet<String> = perms.creatable.as_map().map(|m| m.iter().filter(|(_, k)| k.requires_approval.as_ref().is_some_and(|a| a.is_required())).map(|(c, _)| c.clone()).collect()).unwrap_or_default();
            let gated_update: BTreeSet<String> = perms.updatable.as_map().map(|m| m.iter().filter(|(_, k)| k.requires_approval.as_ref().is_some_and(|a| a.is_required())).map(|(c, _)| c.clone()).collect()).unwrap_or_default();

            // data role: SELECT
            let readable: Vec<String> = if perms.readable.is_all() {
                all_cols.clone()
            } else {
                perms.readable.as_list().map(|l| l.to_vec()).unwrap_or_default()
            };
            let mut select: Vec<String> = known(readable);
            if !select.is_empty() || !perms.creatable.is_empty() || !perms.updatable.is_empty() || perms.deletable.is_allowed() {
                // the key and the caller's own tenant column: the statements Cori writes filter on them
                select.extend(pk.iter().cloned());
                select.extend(own_tenant.clone());
                select.extend(active.as_ref().map(|(c, _)| c.clone()));
            }
            grants.add(&p.data, tname, "SELECT", known(select));

            // data role: INSERT / UPDATE / DELETE, approval-gated columns excluded
            let creatable: Vec<String> = if perms.creatable.is_all() { all_cols.clone() } else { perms.creatable.column_names().into_iter().map(String::from).collect() };
            let updatable: Vec<String> = if perms.updatable.is_all() { all_cols.clone() } else { perms.updatable.column_names().into_iter().map(String::from).collect() };
            if !perms.creatable.is_empty() {
                let mut ins: Vec<String> = creatable.iter().filter(|c| !gated_create.contains(*c)).cloned().collect();
                ins.extend(owner.clone());
                grants.add(&p.data, tname, "INSERT", known(ins));
            }
            if !perms.updatable.is_empty() {
                let upd: Vec<String> = updatable.iter().filter(|c| !gated_update.contains(*c) && Some(*c) != own_tenant.as_ref()).cloned().collect();
                grants.add(&p.data, tname, "UPDATE", known(upd));
            }
            let soft = perms.deletable.is_soft_delete();
            if perms.deletable.is_allowed() && !soft && !perms.deletable.requires_approval() {
                grants.table(&p.data, tname, "DELETE");
            }

            // applier role: every column readable, every creatable/updatable column (gated ones too), soft delete
            grants.add(&p.applier, tname, "SELECT", all_cols.clone());
            if !perms.creatable.is_empty() {
                let mut ins = creatable.clone();
                ins.extend(owner.clone());
                grants.add(&p.applier, tname, "INSERT", known(ins));
                inserting.entry(tname.clone()).or_default().extend([p.data.clone(), p.applier.clone()]);
            }
            let mut upd = if perms.updatable.is_empty() { Vec::new() } else { updatable.iter().filter(|c| Some(*c) != own_tenant.as_ref()).cloned().collect() };
            if soft && let Some((c, _)) = &active {
                upd.push(c.clone());
            }
            grants.add(&p.applier, tname, "UPDATE", known(upd));
            if perms.deletable.is_allowed() && !soft {
                grants.table(&p.applier, tname, "DELETE");
            }

            // policies: scope (permissive), soft delete visibility and insert checks (restrictive)
            if let Some(expr) = ctx.scope_expr(tname, scope)? {
                for role in [&p.data, &p.applier] {
                    policies.push(format!(
                        "CREATE POLICY {} ON {} AS PERMISSIVE FOR ALL TO {} USING ({expr}) WITH CHECK ({expr})",
                        ident(&policy_name("scope", role)),
                        ident(tname),
                        ident(role)
                    ));
                }
                if let Some((_, pred)) = &active {
                    policies.push(format!(
                        "CREATE POLICY {} ON {} AS RESTRICTIVE FOR SELECT TO {} USING ({pred})",
                        ident(&policy_name("live", &p.data)),
                        ident(tname),
                        ident(&p.data)
                    ));
                }
                let mut checks: Vec<String> = Vec::new();
                // a `scope: all` role that may not name the owner files rows of its own (the scope policy checks the
                // others' rows)
                if scope == RowScope::All
                    && let Some(tc) = &owner
                    && !perms.creatable.is_empty()
                    && !perms.creatable.contains(tc)
                {
                    checks.push(format!("{} = {}", ident(tc), tenant_setting(ctx.native(tname, tc).as_deref())));
                }
                if let Some(map) = perms.creatable.as_map() {
                    let mut cols: Vec<&String> = map.keys().collect();
                    cols.sort();
                    for c in cols {
                        if !all_cols.contains(c) || Some(c) == own_tenant.as_ref() {
                            continue;
                        }
                        let k = &map[c];
                        if let Some(values) = &k.restrict_to {
                            checks.push(format!("({} IS NULL OR {})", ident(c), in_list(&ident(c), values)));
                        }
                        if k.required && k.default.is_none() {
                            checks.push(format!("{} IS NOT NULL", ident(c)));
                        }
                    }
                }
                if !checks.is_empty() {
                    policies.push(format!(
                        "CREATE POLICY {} ON {} AS RESTRICTIVE FOR INSERT TO {}, {} WITH CHECK ({})",
                        ident(&policy_name("insert", &p.data)),
                        ident(tname),
                        ident(&p.data),
                        ident(&p.applier),
                        checks.join(" AND ")
                    ));
                }
            }

            // what the role reaches indirectly
            if let Some((_, parent, key, ptenant)) = ctx.inherited(tname)? {
                implied.entry((i, parent)).or_default().extend([key, ptenant]);
            }
            let fk_refs = |map_cols: Vec<(String, Vec<String>)>, implied: &mut BTreeMap<(usize, String), BTreeSet<String>>| {
                for (col, _verify) in map_cols {
                    if let Some(f) = ctx.table(tname).and_then(|t| t.foreign_keys.iter().find(|f| f.columns.len() == 1 && f.columns[0] == col)) {
                        implied.entry((i, f.references.table.clone())).or_default();
                    }
                }
            };
            let mut fk_cols: Vec<(String, Vec<String>)> = Vec::new();
            if let Some(map) = perms.creatable.as_map() {
                fk_cols.extend(map.iter().filter_map(|(c, k)| k.foreign_key.as_ref().map(|f| (c.clone(), f.verify_with.clone()))));
            }
            if let Some(map) = perms.updatable.as_map() {
                fk_cols.extend(map.iter().filter_map(|(c, k)| k.foreign_key.as_ref().map(|f| (c.clone(), f.verify_with.clone()))));
            }
            fk_refs(fk_cols, &mut implied);

            // a tenant column without an index makes every scoped read a sequential scan
            if let Some(tc) = ctx.direct_tenant(tname)
                && let Some(t) = ctx.table(tname)
                && !t.indexes.iter().any(|ix| ix.columns.first() == Some(&tc))
                && t.primary_key.first() != Some(&tc)
            {
                warnings.push(format!("`{tname}.{tc}` scopes rows by tenant but has no index starting with it"));
            }
        }
    }

    // indirect access: the parent's key and tenant for the inherited-tenancy check (data role), every column for
    // Cori's foreign key checks (applier). A table the role also names directly already has its policies.
    for ((i, table), cols) in &implied {
        let r = roles[*i];
        let p = &pg[*i];
        if ctx.table(table).is_none() {
            continue;
        }
        grants.add(&p.applier, table, "SELECT", ctx.all_columns(table));
        if !cols.is_empty() {
            grants.add(&p.data, table, "SELECT", cols.iter().cloned());
        }
        if r.tables.contains_key(table) {
            continue;
        }
        if let Some(expr) = ctx.scope_expr(table, r.row_scope(table))? {
            let targets: Vec<&String> = if cols.is_empty() { vec![&p.applier] } else { vec![&p.data, &p.applier] };
            for role in targets {
                policies.push(format!(
                    "CREATE POLICY {} ON {} AS PERMISSIVE FOR SELECT TO {} USING ({expr})",
                    ident(&policy_name("via", role)),
                    ident(table),
                    ident(role)
                ));
            }
        }
    }

    out.extend(policies);
    out.extend(grants.statements());
    // a SERIAL/BIGSERIAL key (a column default on a sequence the table owns) calls nextval() as the inserting role, which
    // needs USAGE on that sequence (step 1 revoked it; an identity column needs nothing)
    for (table, roles) in &inserting {
        let to = roles.iter().map(|r| ident(r)).collect::<Vec<_>>().join(", ");
        let grant = format!("GRANT USAGE ON SEQUENCE %s TO {}", to.replace('%', "%%"));
        out.push(format!("DO {}", dollar_quoted(&format!(
            "DECLARE s record; BEGIN FOR s IN SELECT d.objid::regclass AS seq FROM pg_depend d JOIN pg_class c ON c.oid = d.objid AND c.relkind = 'S' \
             WHERE d.classid = 'pg_class'::regclass AND d.refobjid = to_regclass({}) AND d.deptype = 'a' LOOP EXECUTE format({}, s.seq); END LOOP; END",
            literal(&format!("public.{}", ident(table))), literal(&grant)
        ))));
    }

    // 4. column rules shared by every role: CHECK constraints (NOT VALID: existing rows are not re-checked)
    let mut rule_tables: Vec<(&String, _)> = rules.tables.iter().collect();
    rule_tables.sort_by(|a, b| a.0.cmp(b.0));
    for (tname, tr) in rule_tables {
        if ctx.table(tname).is_none() {
            continue;
        }
        let mut cols: Vec<(&String, _)> = tr.columns.iter().collect();
        cols.sort_by(|a, b| a.0.cmp(b.0));
        for (col, cr) in cols {
            if !ctx.has_column(tname, col) {
                continue;
            }
            if let Some(values) = &cr.allowed_values {
                out.push(format!(
                    "ALTER TABLE {} ADD CONSTRAINT {} CHECK ({} IS NULL OR {}) NOT VALID",
                    ident(tname),
                    ident(&fit(&format!("cori_values_{tname}_{col}"))),
                    ident(col),
                    in_list(&ident(col), values)
                ));
            }
            if let Some(pattern) = &cr.pattern {
                // a Rust regex Postgres cannot compile is reported, not fatal: the validator still applies it
                out.push(format!("DO {}", dollar_quoted(&format!(
                    " BEGIN ALTER TABLE {} ADD CONSTRAINT {} CHECK ({} IS NULL OR {}::text ~ {}) NOT VALID; \
                     EXCEPTION WHEN invalid_regular_expression THEN RAISE WARNING {}; END ",
                    ident(tname),
                    ident(&fit(&format!("cori_pattern_{tname}_{col}"))),
                    ident(col),
                    ident(col),
                    literal(pattern),
                    literal(&format!("cori: pattern of {tname}.{col} is not a Postgres regular expression")).replace('%', "%%")
                ))));
            }
        }
    }

    // 5. only_when: a BEFORE UPDATE trigger per table, checking the role's conditions on the old and new rows
    for t in &tables {
        let mut branches: Vec<String> = Vec::new();
        for (i, r) in roles.iter().enumerate() {
            let Some(map) = r.tables.get(&t.name).and_then(|p| p.updatable.as_map()) else { continue };
            let mut cols: Vec<&String> = map.keys().collect();
            cols.sort();
            let mut checks: Vec<String> = Vec::new();
            for c in cols {
                let Some(ow) = &map[c].only_when else { continue };
                if !ctx.has_column(&t.name, c) {
                    continue;
                }
                let cond = only_when_sql(&ctx, &t.name, c, ow);
                checks.push(format!(
                    "    IF NEW.{col} IS DISTINCT FROM OLD.{col} AND NOT ({cond}) THEN\n      RAISE EXCEPTION USING ERRCODE = '42501', MESSAGE = {msg};\n    END IF;",
                    col = ident(c),
                    msg = literal(&format!("role `{}` may not change `{}.{c}` from this row (only_when)", r.name, t.name))
                ));
            }
            if checks.is_empty() {
                continue;
            }
            branches.push(format!(
                "  IF current_user IN ({}, {}) THEN\n{}\n  END IF;",
                literal(&pg[i].data),
                literal(&pg[i].applier),
                checks.join("\n")
            ));
        }
        if branches.is_empty() {
            continue;
        }
        let f = ident(&fit(&format!("cori_only_when_{}", t.name)));
        out.push(format!(
            "CREATE FUNCTION public.{f}() RETURNS trigger LANGUAGE plpgsql AS {}",
            dollar_quoted(&format!("\nBEGIN\n{}\n  RETURN NEW;\nEND\n", branches.join("\n")))
        ));
        out.push(format!("CREATE TRIGGER {f} BEFORE UPDATE ON {} FOR EACH ROW EXECUTE FUNCTION public.{f}()", ident(&t.name)));
    }

    let mut h = String::new();
    for p in &pg {
        h.push_str(&format!("role {} {} {}\n", p.role, p.data, p.applier));
    }
    for s in &out {
        h.push_str(s);
        h.push_str("\n;\n");
    }
    Ok(Policy { roles: pg, statements: out, hash: sha_hex(&h), warnings })
}

/// SQL for an `only_when` on `column`: condition sets OR'd, conditions within a set AND'd. Keys `old.<col>` read the
/// row before the update, `new.<col>` (or a bare `<col>`) the row after. The database check is never stricter than the
/// validator: where the validator skips a comparison (a non-numeric value for `greater_than`), so does this.
fn only_when_sql(ctx: &Ctx, table: &str, _column: &str, ow: &OnlyWhen) -> String {
    let sets: Vec<String> = ow
        .condition_sets()
        .into_iter()
        .map(|set| {
            let mut keys: Vec<&String> = set.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|key| {
                    let (row, col) = match key.strip_prefix("old.") {
                        Some(c) => ("OLD", c),
                        None => ("NEW", key.strip_prefix("new.").unwrap_or(key)),
                    };
                    let e = format!("{row}.{}", ident(col));
                    let native = ctx.native(table, col);
                    condition_sql(&e, native.as_deref(), &set[key])
                })
                .collect();
            if parts.is_empty() { "true".to_string() } else { format!("({})", parts.join(" AND ")) }
        })
        .collect();
    if sets.is_empty() { "true".into() } else { sets.join(" OR ") }
}

fn column_ref_sql(r: &str) -> Option<String> {
    if let Some(c) = r.strip_prefix("old.") {
        Some(format!("OLD.{}", ident(c)))
    } else {
        r.strip_prefix("new.").map(|c| format!("NEW.{}", ident(c)))
    }
}

fn condition_sql(e: &str, native: Option<&str>, cond: &ColumnCondition) -> String {
    match cond {
        ColumnCondition::In(values) => in_list(e, values),
        ColumnCondition::Equals(v) => format!("{e} IS NOT DISTINCT FROM {}", value_sql(v)),
        ColumnCondition::Comparison(c) => comparison_sql(e, native, c),
    }
}

fn comparison_sql(e: &str, native: Option<&str>, c: &ComparisonCondition) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = &c.equals {
        parts.push(format!("{e} IS NOT DISTINCT FROM {}", value_sql(v)));
    }
    if let Some(v) = &c.not_equals {
        parts.push(format!("{e} IS DISTINCT FROM {}", value_sql(v)));
    }
    if c.not_null == Some(true) {
        parts.push(format!("{e} IS NOT NULL"));
    }
    if c.is_null == Some(true) {
        parts.push(format!("{e} IS NULL"));
    }
    if let Some(values) = &c.in_values {
        parts.push(in_list(e, values));
    }
    if let Some(values) = &c.not_in {
        for v in values {
            parts.push(format!("{e} IS DISTINCT FROM {}", value_sql(v)));
        }
    }
    if is_numeric(native) {
        for (bound, op) in [(&c.greater_than, ">"), (&c.greater_than_or_equal, ">="), (&c.lower_than, "<"), (&c.lower_than_or_equal, "<=")] {
            match bound {
                Some(NumberOrColumnRef::Number(n)) => parts.push(format!("({e} IS NULL OR {e} {op} {n})")),
                Some(NumberOrColumnRef::ColumnRef(r)) => match column_ref_sql(r) {
                    Some(rc) => parts.push(format!("({e} IS NULL OR ({rc} IS NOT NULL AND {e} {op} {rc}))")),
                    None => parts.push("false".into()),
                },
                None => {}
            }
        }
    }
    if let Some(prefix) = &c.starts_with {
        let p = column_ref_sql(prefix).map(|rc| format!("{rc}::text")).unwrap_or_else(|| literal(prefix));
        parts.push(format!("({e} IS NULL OR ({p} IS NOT NULL AND starts_with({e}::text, {p})))"));
    }
    if parts.is_empty() { "true".into() } else { parts.join(" AND ") }
}

/// Catalog hardening a store database needs from a privileged credential (the server's store admin, not the database's
/// owner): the public schema closed to PUBLIC, and the functions free SQL could use to rewrite its own session
/// (`set_config`) or run SQL the guard never parsed (`query_to_xml`, `ts_stat`) revoked from PUBLIC. Steps the admin
/// is not allowed to take are skipped with a notice (a managed Postgres without a superuser); `runtime` is the login
/// that sets the session before it switches to a data role.
pub fn catalog_hardening(runtime: &str) -> String {
    r#"DO $cori$
BEGIN
  BEGIN
    REVOKE CREATE ON SCHEMA public FROM PUBLIC;
    REVOKE USAGE ON SCHEMA public FROM PUBLIC;
  EXCEPTION WHEN insufficient_privilege THEN RAISE NOTICE 'cori: public schema privileges left as they are (not its owner)';
  END;
  BEGIN
    REVOKE EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) FROM PUBLIC;
    REVOKE EXECUTE ON FUNCTION pg_catalog.query_to_xml(text, boolean, boolean, text) FROM PUBLIC;
    REVOKE EXECUTE ON FUNCTION pg_catalog.query_to_xmlschema(text, boolean, boolean, text) FROM PUBLIC;
    REVOKE EXECUTE ON FUNCTION pg_catalog.query_to_xml_and_xmlschema(text, boolean, boolean, text) FROM PUBLIC;
    REVOKE EXECUTE ON FUNCTION pg_catalog.ts_stat(text) FROM PUBLIC;
    REVOKE EXECUTE ON FUNCTION pg_catalog.ts_stat(text, text) FROM PUBLIC;
    -- the runtime login sets the session (tenant, role) with set_config before it switches to a data role
    GRANT EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) TO cori_runtime;
  EXCEPTION WHEN insufficient_privilege THEN RAISE NOTICE 'cori: catalog function privileges left as they are (not a superuser)';
  END;
END
$cori$"#.replace("cori_runtime", &ident(runtime))
}

/// Server-wide statements that create the Postgres roles of a policy and let `runtime` (the login Cori's data pool
/// uses) switch to them. Each is idempotent on its own; run them outside the database transaction, before it.
pub fn role_statements(policy: &Policy, runtime: &str) -> Vec<String> {
    let mut out = Vec::new();
    for p in &policy.roles {
        for role in [&p.data, &p.applier] {
            // a role of that name that already exists with powers of its own (created by hand, or by an attacker with
            // CREATEROLE) is refused rather than trusted; the attributes a CREATEROLE admin may set are reset
            out.push(format!(
                "DO {}",
                dollar_quoted(&format!(
                    " BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {lit}) THEN CREATE ROLE {id} NOLOGIN; END IF; \
                     EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END "
                    , lit = literal(role), id = ident(role)
                ))
            ));
            out.push(format!(
                "DO {}",
                dollar_quoted(&format!(
                    " BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {lit} AND (rolsuper OR rolbypassrls OR rolreplication)) THEN \
                     RAISE EXCEPTION 'cori: role % has superuser, bypassrls or replication: drop it or remove those attributes', {lit}; END IF; \
                     ALTER ROLE {id} NOLOGIN NOCREATEDB NOCREATEROLE; END ",
                    lit = literal(role), id = ident(role)
                ))
            ));
            out.push(format!("GRANT {} TO {}", ident(role), ident(runtime)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"
version: "1"
captured_at: "2025-08-21T00:00:00Z"
database: { engine: postgres }
tables:
  - name: tickets
    columns:
      - { name: id, type: text, native_type: text, nullable: false }
      - { name: title, type: text, native_type: text, nullable: false }
      - { name: status, type: text, native_type: text, nullable: false }
      - { name: owner_id, type: uuid, native_type: uuid, nullable: false }
      - { name: secret, type: text, native_type: text, nullable: true }
      - { name: amount, type: integer, native_type: integer, nullable: true }
      - { name: deleted_at, type: timestamp, native_type: timestamptz, nullable: true }
    primary_key: [id]
  - name: comments
    columns:
      - { name: id, type: text, native_type: text, nullable: false }
      - { name: ticket_id, type: text, native_type: text, nullable: false }
      - { name: body, type: text, native_type: text, nullable: false }
    primary_key: [id]
    foreign_keys:
      - { columns: [ticket_id], references: { table: tickets, columns: [id] } }
  - name: stray
    columns:
      - { name: id, type: text, native_type: text, nullable: false }
    primary_key: [id]
"#;

    const RULES: &str = r#"
version: "1.0.0"
tables:
  tickets:
    tenant: owner_id
    soft_delete: { column: deleted_at }
    columns:
      status: { allowed_values: [open, pending, closed] }
      title: { pattern: "^[A-Z]" }
  comments:
    tenant: { via: ticket_id, references: tickets }
"#;

    const AGENT: &str = r#"
name: agent
tables:
  tickets:
    readable: [id, title, status, owner_id]
    creatable:
      title: { required: true }
      status: { restrict_to: [open, pending] }
    updatable:
      status:
        only_when:
          - { old.status: open, new.status: [pending, closed] }
          - { old.status: pending, new.status: [open, closed] }
      secret: { requires_approval: true }
      amount: { only_when: { new.amount: { greater_than: old.amount } } }
    deletable: { soft_delete: true }
  comments:
    readable: [id, ticket_id, body]
    creatable: { ticket_id: {}, body: { required: true } }
"#;

    const LEAD: &str = r#"
name: lead
tables:
  tickets:
    scope: all
    readable: "*"
    deletable: true
"#;

    fn compiled() -> Policy {
        let schema: SchemaDefinition = serde_yaml_from(SCHEMA);
        let rules = RulesDefinition::from_yaml(RULES).unwrap();
        let roles = vec![RoleDefinition::from_yaml(AGENT).unwrap(), RoleDefinition::from_yaml(LEAD).unwrap()];
        compile(&schema, &rules, &roles, &role_prefix("desk", "tickets")).unwrap()
    }

    fn serde_yaml_from(s: &str) -> SchemaDefinition {
        SchemaDefinition::from_yaml(s).expect("schema")
    }

    fn has(p: &Policy, needle: &str) -> bool {
        p.statements.iter().any(|s| s.contains(needle))
    }

    #[test]
    fn values_cannot_close_a_dollar_quote() {
        assert_eq!(dollar_quoted("x"), "$cori$x$cori$");
        assert_eq!(dollar_quoted("a $cori$ b"), "$cori_1$a $cori$ b$cori_1$");
        assert_eq!(dollar_quoted("$cori$ $cori_1$"), "$cori_2$$cori$ $cori_1$$cori_2$");
        // a rules pattern and an only_when value written to break out of the quoted body
        let rules = RULES.replace(r#"title: { pattern: "^[A-Z]" }"#, r#"title: { pattern: "x$cori$; DROP TABLE tickets; --" }"#);
        let agent = AGENT.replace("- { old.status: open, new.status: [pending, closed] }", "- { old.status: \"o$cori$; DROP TABLE tickets; --\", new.status: [pending, closed] }");
        let schema = serde_yaml_from(SCHEMA);
        let rules = RulesDefinition::from_yaml(&rules).unwrap();
        let p = compile(&schema, &rules, &[RoleDefinition::from_yaml(&agent).unwrap()], "cs_t").unwrap();
        for st in p.statements.iter().filter(|s| s.contains("DROP TABLE tickets")) {
            // the injected text sits inside a quote whose tag it does not contain
            let tag_end = st.find("$cori").map(|i| i + st[i + 1..].find('$').unwrap() + 2).unwrap();
            let tag = &st[st.find("$cori").unwrap()..tag_end];
            assert_ne!(tag, "$cori$", "{st}");
            assert_eq!(st.matches(tag).count(), 2, "{st}");
        }
        assert!(p.statements.iter().any(|s| s.contains("DROP TABLE tickets")), "the test values reach the SQL");
    }

    #[test]
    fn names_are_distinct_and_short() {
        assert_ne!(role_prefix("a-b", "c"), role_prefix("a", "b_c"));
        let long = pg_roles(&role_prefix(&"x".repeat(80), &"y".repeat(80)), &"z".repeat(80));
        assert!(long.data.len() <= 63 && long.applier.len() <= 63, "{long:?}");
        assert_ne!(long.data, long.applier);
        assert_ne!(pg_roles("p", "a-b").data, pg_roles("p", "a_b").data);
    }

    #[test]
    fn own_scope_compares_the_typed_tenant_and_all_scope_sees_everything() {
        let p = compiled();
        let agent = &p.roles[0];
        let lead = &p.roles[1];
        assert!(has(&p, &format!("TO \"{}\" USING (\"owner_id\" = NULLIF(current_setting('cori.tenant', true), '')::uuid)", agent.data)), "{:#?}", p.statements);
        assert!(has(&p, &format!("TO \"{}\" USING (true)", lead.data)));
        // inherited tenancy goes through the parent
        assert!(has(&p, "EXISTS (SELECT 1 FROM \"tickets\" cori_parent WHERE cori_parent.\"id\" = \"comments\".\"ticket_id\""));
    }

    #[test]
    fn an_all_scope_create_sets_the_owner_unless_the_role_names_it() {
        let schema = serde_yaml_from(SCHEMA);
        let rules = RulesDefinition::from_yaml(RULES).unwrap();
        let files = RoleDefinition::from_yaml("name: files\ntables:\n  tickets: { scope: all, readable: [id, title], creatable: { title: {} } }\n").unwrap();
        let names = RoleDefinition::from_yaml("name: names\ntables:\n  tickets: { scope: all, readable: [id, title], creatable: { title: {}, owner_id: {} } }\n").unwrap();
        let p = compile(&schema, &rules, &[files, names], &role_prefix("desk", "tickets")).unwrap();
        let (files, names) = (&p.roles[0], &p.roles[1]);
        // the owner column is granted to the inserting roles, the data role and the applier alike
        for role in [&files.data, &files.applier, &names.data] {
            assert!(has(&p, &format!("GRANT INSERT (\"owner_id\", \"title\") ON \"tickets\" TO \"{role}\"")), "{role}: {:#?}", p.statements);
        }
        // a role that may not name it inserts only rows of its own
        assert!(has(&p, &format!("FOR INSERT TO \"{}\", \"{}\" WITH CHECK (\"owner_id\" = NULLIF(current_setting('cori.tenant', true), '')::uuid)", files.data, files.applier)), "{:#?}", p.statements);
        assert!(!p.statements.iter().any(|s| s.contains("FOR INSERT") && s.contains(&names.data)), "{:#?}", p.statements);
    }

    #[test]
    fn grants_follow_the_role_and_leave_gated_columns_to_the_applier() {
        let p = compiled();
        let agent = &p.roles[0];
        assert!(has(&p, &format!("GRANT SELECT (\"deleted_at\", \"id\", \"owner_id\", \"status\", \"title\") ON \"tickets\" TO \"{}\"", agent.data)), "{:#?}", p.statements);
        assert!(has(&p, &format!("GRANT UPDATE (\"amount\", \"status\") ON \"tickets\" TO \"{}\"", agent.data)));

        assert!(has(&p, &format!("GRANT UPDATE (\"amount\", \"deleted_at\", \"secret\", \"status\") ON \"tickets\" TO \"{}\"", agent.applier)));
        // soft delete: no DELETE for the agent, DELETE for the lead
        assert!(!has(&p, &format!("GRANT DELETE ON \"tickets\" TO \"{}\"", agent.data)));
        assert!(has(&p, &format!("GRANT DELETE ON \"tickets\" TO \"{}\"", p.roles[1].data)));
        // soft-deleted rows hidden from the data role only
        assert!(has(&p, &format!("AS RESTRICTIVE FOR SELECT TO \"{}\" USING (\"deleted_at\" IS NULL)", agent.data)));
    }

    #[test]
    fn a_column_gated_on_update_stays_insertable() {
        let schema = serde_yaml_from(SCHEMA);
        let rules = RulesDefinition::from_yaml(RULES).unwrap();
        let role = RoleDefinition::from_yaml("name: agent\ntables:\n  tickets:\n    readable: [id, status]\n    creatable: { title: {}, status: { restrict_to: [open] } }\n    updatable: { status: { requires_approval: true }, title: {} }\n").unwrap();
        let p = compile(&schema, &rules, &[role], "p").unwrap();
        let data = &p.roles[0].data;
        assert!(has(&p, &format!("GRANT INSERT (\"owner_id\", \"status\", \"title\") ON \"tickets\" TO \"{data}\"")), "{:#?}", p.statements);
        assert!(has(&p, &format!("GRANT UPDATE (\"title\") ON \"tickets\" TO \"{data}\"")), "{:#?}", p.statements);
    }

    #[test]
    fn insert_checks_constraints_and_triggers() {
        let p = compiled();
        assert!(has(&p, "WITH CHECK ((\"status\" IS NULL OR \"status\" IN ('open', 'pending')) AND \"title\" IS NOT NULL)"), "{:#?}", p.statements);
        assert!(has(&p, "CHECK (\"status\" IS NULL OR \"status\" IN ('open', 'pending', 'closed')) NOT VALID"));
        assert!(has(&p, "~ '^[A-Z]'"));
        assert!(has(&p, "(NEW.\"status\" IN ('pending', 'closed') AND OLD.\"status\" IS NOT DISTINCT FROM 'open') OR (NEW.\"status\" IN ('open', 'closed') AND OLD.\"status\" IS NOT DISTINCT FROM 'pending')"), "{}", p.statements.iter().filter(|s| s.contains("only_when")).cloned().collect::<Vec<_>>().join("\n"));
        assert!(has(&p, "(NEW.\"amount\" IS NULL OR (OLD.\"amount\" IS NOT NULL AND NEW.\"amount\" > OLD.\"amount\"))"));
        assert!(has(&p, "CREATE TRIGGER \"cori_only_when_tickets\" BEFORE UPDATE ON \"tickets\""));
    }

    #[test]
    fn unruled_tables_stay_dark_and_the_hash_is_stable() {
        let p = compiled();
        assert!(has(&p, "ALTER TABLE \"stray\" ENABLE ROW LEVEL SECURITY"));
        assert!(!p.statements.iter().any(|s| s.starts_with("CREATE POLICY") && s.contains("ON \"stray\"")));
        assert_eq!(p.hash, compiled().hash);
        assert!(p.warnings.iter().any(|w| w.contains("owner_id") && w.contains("index")), "{:?}", p.warnings);
    }

    #[test]
    fn inherited_tenancy_needs_a_direct_parent() {
        let schema = serde_yaml_from(SCHEMA);
        let rules = RulesDefinition::from_yaml("version: \"1.0.0\"\ntables:\n  tickets: { global: true }\n  comments: { tenant: { via: ticket_id, references: tickets } }\n").unwrap();
        let roles = vec![RoleDefinition::from_yaml(AGENT).unwrap()];
        let e = compile(&schema, &rules, &roles, "p").unwrap_err();
        assert!(e.contains("no direct"), "{e}");
    }
}
