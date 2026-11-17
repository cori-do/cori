// SPDX-License-Identifier: Apache-2.0
//! App-level store checks: `stores/<name>/migrations/*.sql` parsed into a schema, role files and `rules.yaml`
//! validated against it, the `cori.toml` bindings (`[connectors.<x>] kind = "store", ref = "<name>"`), and per
//! workflow: step SQL against the schema, `pii`-tagged columns a step exposes without a `redact:` entry.
//! Everything here is static: no database, no server. Errors block `cori check <app>` and deploy; warnings do not.
use serde_json::Value;
use sqlparser::ast::{AlterColumnOperation, AlterTableOperation, ColumnDef, ColumnOption, DataType, Expr, ObjectName, ObjectType, Statement, TableConstraint, visit_expressions};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;
use std::path::Path;

pub mod tools;

/// table → columns, as the migrations leave it.
#[derive(Debug, Default, Clone)]
pub struct Schema { pub tables: BTreeMap<String, BTreeSet<String>>, /// A migration this parser could not read: column checks are skipped for the store.
    pub partial: bool, /// table → typed definition (column types as Postgres' information_schema names them, nullability,
    /// defaults, primary and foreign keys): what the store's tool generation reads (`tools::generate`).
    pub defs: BTreeMap<String, cori_store_core::config::TableSchema> }

/// The `information_schema.columns.data_type` name of a DDL type, which the runtime generator reads (`int` → `integer`).
fn native_type(dt: &DataType) -> String {
    if matches!(dt, DataType::Array(_)) { return "ARRAY".into(); }
    let d = dt.to_string().to_lowercase();
    let head = d.split('(').next().unwrap_or("").trim().to_string();
    match head.as_str() {
        "int" | "integer" | "int4" | "serial" | "serial4" => "integer".into(),
        "bigint" | "int8" | "bigserial" | "serial8" => "bigint".into(),
        "smallint" | "int2" | "smallserial" | "serial2" => "smallint".into(),
        "varchar" | "character varying" => "character varying".into(),
        "char" | "character" => "character".into(),
        "bool" | "boolean" => "boolean".into(),
        "timestamptz" | "timestamp with time zone" => "timestamp with time zone".into(),
        "timestamp" | "timestamp without time zone" => "timestamp without time zone".into(),
        "timetz" | "time with time zone" => "time with time zone".into(),
        "time" | "time without time zone" => "time without time zone".into(),
        "decimal" | "numeric" => "numeric".into(),
        "float4" | "real" => "real".into(),
        "float8" | "double precision" | "float" | "double" => "double precision".into(),
        "text" | "uuid" | "json" | "jsonb" | "date" | "bytea" | "interval" | "inet" | "citext" => head,
        _ if d.ends_with("[]") => "ARRAY".into(),
        _ => "USER-DEFINED".into(),
    }
}

fn column_def(c: &ColumnDef) -> (cori_store_core::config::ColumnSchema, bool, Option<(String, Vec<String>)>) {
    let serial = matches!(c.data_type.to_string().to_lowercase().as_str(), "serial" | "bigserial" | "smallserial" | "serial2" | "serial4" | "serial8");
    let mut col = cori_store_core::config::ColumnSchema { name: c.name.value.clone(), column_type: cori_store_core::config::ColumnType::Unknown, native_type: Some(native_type(&c.data_type)), nullable: true, default: serial.then(|| Value::String(format!("nextval('{}_seq'::regclass)", c.name.value))), max_length: None, enum_name: None, sql_type: None };
    let (mut pk, mut fk) = (false, None);
    for o in &c.options {
        match &o.option {
            ColumnOption::NotNull => col.nullable = false,
            ColumnOption::Default(e) => col.default = Some(Value::String(e.to_string())),
            ColumnOption::PrimaryKey(_) => { pk = true; col.nullable = false; }
            ColumnOption::ForeignKey(f) => fk = Some((last(&f.foreign_table), f.referred_columns.iter().map(|i| i.value.clone()).collect())),
            _ => {}
        }
    }
    if serial { col.nullable = false; }
    (col, pk, fk)
}

fn foreign_key(columns: Vec<String>, table: String, referred: Vec<String>) -> cori_store_core::config::ForeignKey {
    cori_store_core::config::ForeignKey { name: None, columns, references: cori_store_core::config::ForeignKeyReference { table, schema: Some("public".into()), columns: referred }, on_delete: None, on_update: None }
}

fn apply_constraint(t: &mut cori_store_core::config::TableSchema, c: &TableConstraint) {
    match c {
        TableConstraint::PrimaryKey(pk) => {
            t.primary_key = pk.columns.iter().map(|ic| ic.column.expr.to_string().trim_matches('"').to_string()).collect();
            for col in t.columns.iter_mut().filter(|col| t.primary_key.contains(&col.name)) { col.nullable = false; }
        }
        TableConstraint::ForeignKey(f) => t.foreign_keys.push(foreign_key(f.columns.iter().map(|i| i.value.clone()).collect(), last(&f.foreign_table), f.referred_columns.iter().map(|i| i.value.clone()).collect())),
        _ => {}
    }
}

#[derive(Debug, Clone)]
pub struct StoreInfo { pub name: String, pub schema: Schema, /// table → columns tagged `pii` in rules.yaml
    pub pii: BTreeMap<String, BTreeSet<String>>, pub roles: BTreeMap<String, serde_yaml::Value>, pub rules: Option<serde_yaml::Value> }

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Report { pub errors: Vec<String>, pub warnings: Vec<String>, pub stores: Vec<String> }

fn last(name: &ObjectName) -> String { name.to_string().rsplit('.').next().unwrap_or("").trim_matches('"').to_string() }

/// Applies the DDL of every migration (sorted by file name) to an empty schema.
pub fn schema_of(migrations: &[(String, String)], warnings: &mut Vec<String>, store: &str) -> Schema {
    let mut s = Schema::default();
    for (file, sql) in migrations {
        let stmts = match Parser::parse_sql(&PostgreSqlDialect {}, sql) {
            Ok(v) => v,
            Err(e) => { s.partial = true; warnings.push(format!("stores/{store}/migrations/{file}: not parsed ({e}); column checks for this store are skipped")); continue; }
        };
        for st in stmts {
            match st {
                Statement::CreateTable(ct) => {
                    let name = last(&ct.name);
                    let cols = ct.columns.iter().map(|c| c.name.value.clone()).collect();
                    s.tables.insert(name.clone(), cols);
                    let mut def = cori_store_core::config::TableSchema { name: name.clone(), schema: "public".into(), columns: vec![], primary_key: vec![], foreign_keys: vec![], indexes: vec![] };
                    for c in &ct.columns {
                        let (col, pk, fk) = column_def(c);
                        if pk { def.primary_key.push(col.name.clone()); }
                        if let Some((ft, referred)) = fk { def.foreign_keys.push(foreign_key(vec![col.name.clone()], ft, referred)); }
                        def.columns.push(col);
                    }
                    for c in &ct.constraints { apply_constraint(&mut def, c); }
                    s.defs.insert(name, def);
                }
                Statement::AlterTable(at) => {
                    let t = last(&at.name);
                    let cols = s.tables.entry(t.clone()).or_default();
                    let def = s.defs.entry(t.clone()).or_insert_with(|| cori_store_core::config::TableSchema { name: t.clone(), schema: "public".into(), columns: vec![], primary_key: vec![], foreign_keys: vec![], indexes: vec![] });
                    for op in at.operations {
                        match op {
                            AlterTableOperation::AddColumn { column_def, .. } => {
                                cols.insert(column_def.name.value.clone());
                                let (col, pk, fk) = self::column_def(&column_def);
                                if pk { def.primary_key.push(col.name.clone()); }
                                if let Some((ft, referred)) = fk { def.foreign_keys.push(foreign_key(vec![col.name.clone()], ft, referred)); }
                                def.columns.retain(|c| c.name != col.name); def.columns.push(col);
                            }
                            AlterTableOperation::DropColumn { column_names, .. } => { for c in column_names { cols.remove(&c.value); def.columns.retain(|x| x.name != c.value); def.primary_key.retain(|x| x != &c.value); def.foreign_keys.retain(|f| !f.columns.contains(&c.value)); } }
                            AlterTableOperation::RenameColumn { old_column_name, new_column_name } => {
                                cols.remove(&old_column_name.value); cols.insert(new_column_name.value.clone());
                                let (o, n) = (old_column_name.value, new_column_name.value);
                                for c in def.columns.iter_mut().filter(|c| c.name == o) { c.name = n.clone(); }
                                for p in def.primary_key.iter_mut().filter(|p| **p == o) { *p = n.clone(); }
                                for f in def.foreign_keys.iter_mut() { for c in f.columns.iter_mut().filter(|c| **c == o) { *c = n.clone(); } }
                            }
                            AlterTableOperation::AddConstraint { constraint, .. } => apply_constraint(def, &constraint),
                            AlterTableOperation::AlterColumn { column_name, op } => {
                                if let Some(c) = def.columns.iter_mut().find(|c| c.name == column_name.value) {
                                    match op {
                                        AlterColumnOperation::SetNotNull => c.nullable = false,
                                        AlterColumnOperation::DropNotNull => c.nullable = true,
                                        AlterColumnOperation::SetDefault { value } => c.default = Some(Value::String(value.to_string())),
                                        AlterColumnOperation::DropDefault => c.default = None,
                                        AlterColumnOperation::SetDataType { data_type, .. } => c.native_type = Some(native_type(&data_type)),
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Statement::Drop { object_type: ObjectType::Table, names, .. } => { for n in names { s.tables.remove(&last(&n)); s.defs.remove(&last(&n)); } }
                _ => {}
            }
        }
    }
    // `REFERENCES t` without a column list names the primary key of `t`
    let pks: BTreeMap<String, Vec<String>> = s.defs.iter().map(|(k, d)| (k.clone(), d.primary_key.clone())).collect();
    for d in s.defs.values_mut() { for f in d.foreign_keys.iter_mut().filter(|f| f.references.columns.is_empty()) { f.references.columns = pks.get(&f.references.table).cloned().unwrap_or_default(); } }
    s
}

/// The stores of an app folder (`stores/<name>/`), with their parsed schema, rules and role files.
pub fn app_stores(app_root: &Path, warnings: &mut Vec<String>) -> Vec<StoreInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(app_root.join("stores")) else { return out };
    let mut dirs: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let mut migs: Vec<(String, String)> = std::fs::read_dir(dir.join("migrations")).map(|mm| mm.filter_map(|m| m.ok()).filter(|m| m.path().extension().map(|x| x == "sql").unwrap_or(false)).map(|m| (m.file_name().to_string_lossy().to_string(), std::fs::read_to_string(m.path()).unwrap_or_default())).collect()).unwrap_or_default();
        migs.sort();
        if migs.is_empty() { warnings.push(format!("stores/{name}: no migrations/*.sql (the store has no tables)")); }
        let schema = schema_of(&migs, warnings, &name);
        let rules: Option<serde_yaml::Value> = std::fs::read_to_string(dir.join("rules.yaml")).ok().and_then(|t| serde_yaml::from_str(&t).ok());
        let mut pii: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        if let Some(tables) = rules.as_ref().and_then(|r| r.get("tables")).and_then(|t| t.as_mapping()) {
            for (t, def) in tables {
                let Some(t) = t.as_str() else { continue };
                if let Some(cols) = def.get("columns").and_then(|c| c.as_mapping()) {
                    for (c, cdef) in cols {
                        let tagged = cdef.get("tags").and_then(|x| x.as_sequence()).map(|s| s.iter().any(|v| v.as_str() == Some("pii"))).unwrap_or(false);
                        if let (Some(c), true) = (c.as_str(), tagged) { pii.entry(t.to_string()).or_default().insert(c.to_string()); }
                    }
                }
            }
        }
        let mut roles = BTreeMap::new();
        if let Ok(rr) = std::fs::read_dir(dir.join("roles")) {
            for r in rr.filter_map(|r| r.ok()).filter(|r| r.path().extension().map(|x| x == "yaml" || x == "yml").unwrap_or(false)) {
                let stem = r.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                match std::fs::read_to_string(r.path()).ok().and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok()) { Some(y) => { roles.insert(stem, y); } None => warnings.push(format!("stores/{name}/roles/{stem}.yaml: not valid YAML")) }
            }
        }
        out.push(StoreInfo { name, schema, pii, roles, rules });
    }
    out
}

/// `[connectors.<x>] kind = "store", ref = "<store>"` in cori.toml: connector name → store name.
pub fn bindings(app_root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(t) = std::fs::read_to_string(app_root.join("cori.toml")).ok().and_then(|s| toml::from_str::<toml::Value>(&s).ok()) else { return out };
    if let Some(cs) = t.get("connectors").and_then(|c| c.as_table()) {
        for (name, c) in cs {
            if c.get("kind").and_then(|k| k.as_str()) == Some("store") { out.insert(name.clone(), c.get("ref").and_then(|r| r.as_str()).unwrap_or(name).to_string()); }
        }
    }
    out
}

fn column_names(v: &serde_yaml::Value) -> Option<Vec<String>> {
    match v {
        serde_yaml::Value::String(s) if s == "*" => None,
        serde_yaml::Value::Sequence(s) => Some(s.iter().filter_map(|x| x.as_str().map(String::from)).collect()),
        serde_yaml::Value::Mapping(m) => match m.get("columns") { Some(c) => column_names(c), None => Some(m.keys().filter_map(|k| k.as_str().map(String::from)).collect()) },
        _ => None,
    }
}

/// Migrations, rules and role files of every store against each other, and the cori.toml bindings.
pub fn check_app(app_root: &Path) -> Report {
    let mut r = Report::default();
    let stores = app_stores(app_root, &mut r.warnings);
    let bound = bindings(app_root);
    for (conn, store) in &bound { if !stores.iter().any(|s| &s.name == store) { r.errors.push(format!("cori.toml: [connectors.{conn}] kind = \"store\" refers to `{store}` but stores/{store}/ does not exist (`cori store new {store}`)")); } }
    for s in &stores {
        r.stores.push(s.name.clone());
        if !bound.values().any(|v| v == &s.name) { r.warnings.push(format!("stores/{}: no connector binds it; add `[connectors.{0}] kind = \"store\"` and `ref = \"{0}\"` to cori.toml and `connector: \"{0}\"` in store steps", s.name)); }
        if s.schema.partial { continue; }
        let table_list = || s.schema.tables.keys().cloned().collect::<Vec<_>>().join(", ");
        let rules_tables = s.rules.as_ref().and_then(|x| x.get("tables")).and_then(|t| t.as_mapping());
        let direct_tenant = |t: &str| -> Option<String> { let d = rules_tables?.get(t)?; if d.get("global").and_then(|g| g.as_bool()).unwrap_or(false) { return None; } d.get("tenant")?.as_str().map(String::from) };
        if let Some(tables) = rules_tables {
            for (t, def) in tables {
                let Some(t) = t.as_str() else { continue };
                let file = format!("stores/{}/rules.yaml", s.name);
                match s.schema.tables.get(t) {
                    None => r.errors.push(format!("{file}: table `{t}` is not created by the migrations (tables: {})", table_list())),
                    Some(cols) => {
                        for c in def.get("columns").and_then(|c| c.as_mapping()).map(|m| m.keys().filter_map(|k| k.as_str()).collect::<Vec<_>>()).unwrap_or_default() { if !cols.contains(c) { r.errors.push(format!("{file}: `{t}.{c}` is not a column of the migrations")); } }
                        // tenancy and soft delete become Postgres policies: a column they name must exist
                        match def.get("tenant") {
                            Some(serde_yaml::Value::String(c)) if !cols.contains(c) => r.errors.push(format!("{file}: `{t}` names `tenant: {c}`, which is not a column of `{t}`")),
                            Some(serde_yaml::Value::Mapping(m)) => {
                                let via = m.get("via").and_then(|v| v.as_str()).unwrap_or("");
                                let parent = m.get("references").and_then(|v| v.as_str()).unwrap_or("");
                                if !cols.contains(via) { r.errors.push(format!("{file}: `{t}` inherits its tenant via `{via}`, which is not a column of `{t}`")); }
                                if direct_tenant(parent).is_none() { r.errors.push(format!("{file}: `{t}` inherits its tenant from `{parent}`, which has no `tenant: <column>` of its own (inheritance is one level deep)")); }
                            }
                            _ => {}
                        }
                        if let Some(c) = def.get("soft_delete").and_then(|d| d.get("column")).and_then(|c| c.as_str()) && !cols.contains(c) { r.errors.push(format!("{file}: `{t}` soft-deletes with `{c}`, which is not a column of `{t}`")); }
                    }
                }
            }
        }
        // a table no rule names is invisible to every role (the database fails closed): say so when a role names it
        let ruled = |t: &str| rules_tables.map(|m| m.contains_key(serde_yaml::Value::String(t.to_string()))).unwrap_or(false);
        let mut dark: Vec<&str> = s.roles.values().filter_map(|y| y.get("tables").and_then(|t| t.as_mapping())).flat_map(|m| m.keys().filter_map(|k| k.as_str())).filter(|t| s.schema.tables.contains_key(*t) && !ruled(t)).collect();
        dark.sort(); dark.dedup();
        for t in dark { r.warnings.push(format!("stores/{}/rules.yaml: `{t}` is not named, so its rows are invisible to every role; add it with `tenant: <column>` or `global: true`", s.name)); }
        for (role, y) in &s.roles {
            let file = format!("stores/{}/roles/{role}.yaml", s.name);
            if let Some(m) = y.as_mapping() { for k in m.keys().filter_map(|k| k.as_str()) { if !matches!(k, "name" | "description" | "approvals" | "writes" | "tables") { r.warnings.push(format!("{file}: `{k}` is not a role setting and is ignored by the store (name, description, approvals, writes, tables)")); } } }
            // `writes: workflows` (cori_store::Store::writes_through_workflows_only) has no other value: anything else would read as a rule and enforce none
            if let Some(w) = y.get("writes") { if !w.as_str().is_some_and(|w| w.trim().eq_ignore_ascii_case("workflows")) { r.errors.push(format!("{file}: `writes` takes one value, `workflows` (the role changes data through the app's workflows only); remove the line to let it write directly")); } }
            if y.get("name").and_then(|n| n.as_str()).map(|n| n != role).unwrap_or(false) { r.errors.push(format!("{file}: `name: {}` must equal the file name `{role}`", y["name"].as_str().unwrap_or(""))); }
            let Some(tables) = y.get("tables").and_then(|t| t.as_mapping()) else { continue };
            let named: Vec<String> = tables.keys().filter_map(|k| k.as_str()).filter(|t| s.schema.tables.contains_key(*t)).map(String::from).collect();
            for (a, b, tool) in cori_store_core::tool_generator::tool_name_clashes(&named) {
                r.errors.push(format!("{file}: tables `{a}` and `{b}` give the same tool names (`{tool}`…), so a call could not tell them apart: keep one of them in this role"));
            }
            for (t, perms) in tables {
                let Some(t) = t.as_str() else { continue };
                let Some(cols) = s.schema.tables.get(t) else { r.errors.push(format!("{file}: table `{t}` is not created by the migrations (tables: {})", table_list())); continue };
                let Some(pm) = perms.as_mapping() else { continue };
                for (k, v) in pm {
                    let Some(k) = k.as_str() else { continue };
                    if !matches!(k, "readable" | "creatable" | "updatable" | "deletable" | "scope") { r.warnings.push(format!("{file}: `{t}.{k}` is not a table permission and is ignored (readable, creatable, updatable, deletable, scope)")); continue; }
                    if k == "scope" {
                        match v.as_str() {
                            Some("own") | Some("all") => { if v.as_str() == Some("all") && direct_tenant(t).is_none() && !rules_tables.and_then(|m| m.get(t)).map(|d| d.get("tenant").is_some()).unwrap_or(false) { r.warnings.push(format!("{file}: `{t}.scope: all` has no effect: `{t}` is not tenant-scoped in rules.yaml")); } }
                            _ => r.errors.push(format!("{file}: `{t}.scope` is `own` (the caller's rows, the default) or `all` (every row)")),
                        }
                        continue;
                    }
                    if k == "deletable" {
                        if v.get("soft_delete").and_then(|b| b.as_bool()).unwrap_or(false) && rules_tables.and_then(|m| m.get(t)).and_then(|d| d.get("soft_delete")).is_none() { r.errors.push(format!("{file}: `{t}.deletable.soft_delete` needs `soft_delete: {{ column: <column> }}` for `{t}` in rules.yaml")); }
                        continue;
                    }
                    for c in column_names(v).unwrap_or_default() { if !cols.contains(&c) { r.errors.push(format!("{file}: `{t}.{k}` names `{c}`, which is not a column of the migrations (columns: {})", cols.iter().cloned().collect::<Vec<_>>().join(", "))); } }
                }
            }
        }
    }
    r
}

/// The store a step's `connector` names: its cori.toml binding, or the only store when the step names none.
fn store_for<'a>(stores: &'a [StoreInfo], bound: &BTreeMap<String, String>, connector: &str) -> Option<&'a StoreInfo> {
    let name = bound.get(connector).cloned().or_else(|| if stores.len() == 1 && (connector.is_empty() || connector == "default") { Some(stores[0].name.clone()) } else { None })?;
    stores.iter().find(|s| s.name == name)
}

/// Store role → the typed tools it exposes (`tools::generate`), from the migrations and `roles/<role>.yaml`. A role
/// file the store cannot load is left out (`cori check .` reports it).
pub fn role_tools(store: &StoreInfo) -> BTreeMap<String, Vec<Value>> {
    store.roles.iter().filter_map(|(stem, y)| serde_yaml::from_value::<cori_store_core::RoleDefinition>(y.clone()).ok().map(|r| (stem.clone(), tools::generate_typed(&store.schema.defs, &r)))).collect()
}

/// The tools of a store connector (`[connectors.<x>] kind = "store"`) or store name, across roles: each tool once, with
/// `roles` (the roles exposing it) and `annotations.readOnlyHint` (get/list). `None` when the app has no such store.
pub fn connector_tools(app_root: &Path, connector: &str) -> Option<Vec<Value>> {
    let mut ignored = Vec::new();
    let stores = app_stores(app_root, &mut ignored);
    let bound = bindings(app_root);
    let store = store_for(&stores, &bound, connector).or_else(|| stores.iter().find(|s| s.name == connector))?;
    let mut out: Vec<Value> = Vec::new();
    for (role, ts) in role_tools(store) {
        for t in ts {
            match out.iter_mut().find(|x| x["name"] == t["name"]) {
                Some(x) => { if let Some(r) = x["roles"].as_array_mut() { r.push(Value::String(role.clone())); } }
                None => { let mut t = t; t["annotations"]["readOnlyHint"] = Value::Bool(tools::is_read(&t)); t["roles"] = serde_json::json!([role]); out.push(t); }
            }
        }
    }
    Some(out)
}

/// `cori store tools <store>` offline: the typed tools of each role (or of `role`), one line per tool with its effect and
/// parameters (`name*` required, `?` optional). `store` is a store name or a store connector.
pub fn tools_text(app_root: &Path, store: &str, role: Option<&str>) -> Result<String, String> {
    let mut ignored = Vec::new();
    let stores = app_stores(app_root, &mut ignored);
    let bound = bindings(app_root);
    let Some(st) = stores.iter().find(|s| s.name == store).or_else(|| store_for(&stores, &bound, store)) else {
        return Err(format!("no store `{store}` in {} (stores: {})", app_root.display(), if stores.is_empty() { "none".into() } else { stores.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ") }));
    };
    let by_role = role_tools(st);
    if let Some(r) = role { if !by_role.contains_key(r) { return Err(format!("stores/{}/roles/ has no role `{r}` (roles: {})", st.name, by_role.keys().cloned().collect::<Vec<_>>().join(", "))); } }
    let mut out = Vec::new();
    for (r, ts) in by_role.iter().filter(|(r, _)| role.map(|x| x == r.as_str()).unwrap_or(true)) {
        out.push(format!("stores/{} · role {r} (from migrations + roles/{r}.yaml; `connector: \"{}\"`, `tool: \"<name>\"`)", st.name, bound.iter().find(|(_, v)| **v == st.name).map(|(k, _)| k.as_str()).unwrap_or(&st.name)));
        if ts.is_empty() { out.push("  (no tools: the role grants nothing on tables the migrations create)".into()); }
        for t in ts {
            let schema = &t["inputSchema"];
            let required: Vec<&str> = schema["required"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            let params = schema["properties"].as_object().map(|p| p.iter().map(|(k, v)| format!("{k}{}: {}{}", if required.contains(&k.as_str()) { "*" } else { "?" }, v["type"].as_str().unwrap_or("any"), v.get("enum").map(|e| format!(" {e}")).unwrap_or_default())).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            out.push(format!("  {:<22} {:<8} {params}", t["name"].as_str().unwrap_or(""), if tools::is_read(t) { "Read" } else { "Write" }));
        }
    }
    if !ignored.is_empty() { out.push(format!("notes: {}", ignored.join("; "))); }
    Ok(out.join("\n"))
}

/// Per workflow: every store step's `tool` (not `query`) against the tools the store's roles expose. No role exposing
/// it is an error (the step fails at run time with "not available to role"); a role of the endpoint's `allow.roles`
/// that the store defines but that lacks the tool is a warning. Returns (errors, warnings).
pub fn check_workflow_tools(app_root: &Path, allow_roles: &[String], nodes: &[crate::dag::Node]) -> (Vec<String>, Vec<String>) {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());
    // a query reads: the store runs one SELECT, read-only, and refuses anything else when the step runs
    for n in nodes.iter().filter(|n| n.kind == "store") {
        let Some(text) = n.literals.get("query").and_then(|q| q.get("text")).and_then(|t| t.as_str()) else { continue };
        if let Some(why) = not_one_select(text) {
            errors.push(format!("{}:{}: step `{}`'s query {why}: a store query only reads (one SELECT), and the store refuses anything else when the step runs. Change rows with the store's tools (create…, update…, delete…: `cori store tools <store>`)", n.file, n.line, n.name));
        }
    }
    let mut ignored = Vec::new();
    let stores = app_stores(app_root, &mut ignored);
    if stores.is_empty() { return (errors, warnings); }
    let bound = bindings(app_root);
    for n in nodes.iter().filter(|n| n.kind == "store" && !n.literals.contains_key("query")) {
        let Some(tool) = n.literals.get("tool").and_then(|t| t.as_str()) else { continue };
        let connector = n.literals.get("connector").and_then(|c| c.as_str()).unwrap_or("");
        let Some(store) = store_for(&stores, &bound, connector) else { continue };
        if store.schema.partial { continue; }
        let by_role = role_tools(store);
        let has = |r: &str| by_role.get(r).map(|ts| ts.iter().any(|t| t["name"].as_str() == Some(tool))).unwrap_or(false);
        let exposing: Vec<&String> = by_role.keys().filter(|r| has(r)).collect();
        if exposing.is_empty() {
            let mut valid: Vec<String> = by_role.values().flatten().filter_map(|t| t["name"].as_str().map(String::from)).collect();
            valid.sort(); valid.dedup();
            errors.push(format!("{}:{}: step `{}` calls tool `{tool}`, which no role of stores/{} exposes (tools: {}; `cori store tools {}` lists them per role)", n.file, n.line, n.name, store.name, if valid.is_empty() { "none".into() } else { valid.join(", ") }, store.name));
            continue;
        }
        for r in allow_roles.iter().filter(|r| by_role.contains_key(*r) && !has(r)) {
            warnings.push(format!("{}:{}: step `{}` calls `{tool}`, which role `{r}` (endpoint allow.roles) does not have in stores/{}/roles/{r}.yaml; callers with only that role fail at run time (exposed to: {})", n.file, n.line, n.name, store.name, exposing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        }
    }
    (errors, warnings)
}

/// A person holding several of the app's roles acts in a store as the first of them `order` lists (cori.toml's `[auth]
/// roles`, strongest first): a role listed after one whose every power it has, and more, is listed the wrong way round.
/// One warning per such pair and store. Roles that cannot be ranked (each may do something the other may not) are the
/// app's choice, and pass.
pub fn role_order_warnings(app_root: &Path, order: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for st in app_stores(app_root, &mut Vec::new()) {
        let listed: Vec<&String> = order.iter().filter(|r| st.roles.contains_key(*r)).collect();
        for (i, first) in listed.iter().enumerate() {
            let mine = powers(&st, first);
            for later in &listed[i + 1..] {
                let theirs = powers(&st, later);
                if mine == theirs || !mine.is_subset(&theirs) { continue; }
                let more: Vec<&str> = theirs.difference(&mine).map(String::as_str).collect();
                out.push(format!("cori.toml [auth] roles lists `{first}` before `{later}`, but in stores/{} `{later}` may do everything `{first}` may, and more ({}{}): a person holding both acts as the first listed and loses that. List roles strongest first", st.name, more.iter().take(3).copied().collect::<Vec<_>>().join(", "), if more.len() > 3 { ", …" } else { "" }));
            }
        }
    }
    out
}

/// What a role may do in a store, as facts two roles can be compared on: each column it reads, creates or updates, each
/// table it deletes from, and each table of which it sees every row (`scope: all`).
fn powers(st: &StoreInfo, role: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Some(tables) = st.roles.get(role).and_then(|y| y.get("tables")).and_then(|t| t.as_mapping()) else { return out };
    for (t, p) in tables {
        let Some(t) = t.as_str() else { continue };
        // a list of columns, a map keyed by column, or "*" (every column the migrations give the table)
        let columns = |v: Option<&serde_yaml::Value>| -> Vec<String> {
            match v {
                Some(serde_yaml::Value::String(s)) if s == "*" => st.schema.tables.get(t).map(|c| c.iter().cloned().collect()).unwrap_or_default(),
                Some(serde_yaml::Value::Sequence(s)) => s.iter().filter_map(|c| c.as_str().map(String::from)).collect(),
                Some(serde_yaml::Value::Mapping(m)) => m.keys().filter_map(|c| c.as_str().map(String::from)).collect(),
                _ => Vec::new(),
            }
        };
        for (verb, key) in [("reads", "readable"), ("creates", "creatable"), ("updates", "updatable")] {
            for c in columns(p.get(key)) { out.insert(format!("{verb} `{t}.{c}`")); }
        }
        if p.get("deletable").is_some_and(|d| !matches!(d, serde_yaml::Value::Bool(false) | serde_yaml::Value::Null)) { out.insert(format!("deletes from `{t}`")); }
        if p.get("scope").and_then(|s| s.as_str()) == Some("all") { out.insert(format!("sees every row of `{t}`")); }
    }
    out
}

/// What a statement reads: the store tables (its relations, but neither the names its WITH defines nor the functions it
/// calls in FROM, `generate_series(…) AS d`), whether it is plain (no WITH, no function in FROM: every bare name in it is
/// then a column of its tables or an alias of its select list), and its select list's aliases (lower-cased).
struct Reads { tables: Vec<String>, plain: bool, aliases: BTreeSet<String> }

fn tables_read(st: &Statement) -> Reads {
    use sqlparser::ast::{Query, SelectItem, SetExpr, TableFactor, Visit, Visitor};
    struct Tables { ctes: Vec<String>, function: Option<ObjectName>, functions: bool, out: Vec<String> }
    impl Visitor for Tables {
        type Break = ();
        fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<()> {
            if let Some(w) = &q.with { self.ctes.extend(w.cte_tables.iter().map(|c| c.alias.name.value.to_lowercase())); }
            ControlFlow::Continue(())
        }
        fn pre_visit_table_factor(&mut self, t: &TableFactor) -> ControlFlow<()> {
            match t {
                TableFactor::Table { name, args: Some(_), .. } => { self.function = Some(name.clone()); self.functions = true; }
                TableFactor::Function { .. } | TableFactor::TableFunction { .. } | TableFactor::UNNEST { .. } => self.functions = true,
                _ => {}
            }
            ControlFlow::Continue(())
        }
        fn pre_visit_relation(&mut self, name: &ObjectName) -> ControlFlow<()> {
            if self.function.as_ref() == Some(name) { self.function = None; return ControlFlow::Continue(()); }
            let t = last(name);
            if name.0.len() > 1 || !self.ctes.contains(&t.to_lowercase()) { self.out.push(t); }
            ControlFlow::Continue(())
        }
    }
    let mut v = Tables { ctes: Vec::new(), function: None, functions: false, out: Vec::new() };
    let _ = st.visit(&mut v);
    let aliases = match st {
        Statement::Query(q) => match q.body.as_ref() {
            SetExpr::Select(s) => s.projection.iter().filter_map(|i| match i { SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.to_lowercase()), _ => None }).collect(),
            _ => BTreeSet::new(),
        },
        _ => BTreeSet::new(),
    };
    Reads { tables: v.out, plain: v.ctes.is_empty() && !v.functions, aliases }
}

/// Why a query text is not one SELECT, the only statement the store runs for a query (an INSERT, UPDATE, DELETE or MERGE,
/// also inside WITH; several statements); None when it is one, or when it does not parse (`check_workflow` warns then).
fn not_one_select(text: &str) -> Option<String> {
    use sqlparser::ast::{SetExpr, Statement};
    fn writes(body: &SetExpr) -> bool {
        match body {
            SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => true,
            SetExpr::SetOperation { left, right, .. } => writes(left) || writes(right),
            SetExpr::Query(q) => writes(&q.body),
            _ => false,
        }
    }
    let stmts = Parser::parse_sql(&PostgreSqlDialect {}, text).ok()?;
    match stmts.as_slice() {
        [Statement::Query(q)] if writes(&q.body) || q.with.as_ref().is_some_and(|w| w.cte_tables.iter().any(|c| writes(&c.query.body))) => Some("changes rows".into()),
        [Statement::Query(_)] => None,
        [one] => Some(format!("is {}", one.to_string().split_whitespace().next().unwrap_or("not a SELECT").to_ascii_uppercase())),
        [] => Some("is empty".into()),
        many => Some(format!("holds {} statements", many.len())),
    }
}

/// Property names of a JSON Schema, with the path that reaches them, a few levels deep (`rows[].email`).
fn schema_paths(schema: &Value, prefix: &str, depth: usize, out: &mut Vec<(String, String)>) {
    if depth > 3 { return; }
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        for (k, v) in props { let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }; out.push((k.clone(), p.clone())); schema_paths(v, &p, depth + 1, out); }
    }
    if let Some(items) = schema.get("items") { schema_paths(items, prefix, depth + 1, out); }
    for key in ["anyOf", "oneOf", "allOf"] { if let Some(alts) = schema.get(key).and_then(|a| a.as_array()) { for a in alts { schema_paths(a, prefix, depth + 1, out); } } }
}

/// Per workflow, static warnings: step `query` SQL against the store schema, and `pii` columns a store step exposes
/// without a matching `redact:` path. Never errors: the runtime and the role files keep the authority.
pub fn check_workflow(app_root: &Path, manifest_redact: &[String], nodes: &[crate::dag::Node], schemas: &BTreeMap<String, Value>) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut ignored = Vec::new();
    let stores = app_stores(app_root, &mut ignored);
    if stores.is_empty() { return warnings; }
    let bound = bindings(app_root);
    let store_of = |connector: &str| store_for(&stores, &bound, connector);
    for n in nodes.iter().filter(|n| n.kind == "store") {
        let connector = n.literals.get("connector").and_then(|c| c.as_str()).unwrap_or("");
        let Some(store) = store_of(connector) else { continue };
        let mut tables: Vec<String> = Vec::new();
        if let Some(text) = n.literals.get("query").and_then(|q| q.get("text")).and_then(|t| t.as_str()) {
            match Parser::parse_sql(&PostgreSqlDialect {}, text) {
                Ok(stmts) if !store.schema.partial => {
                    for st in &stmts {
                        let reads = tables_read(st);
                        tables.extend(reads.tables);
                        for t in &tables { if !store.schema.tables.contains_key(t) { warnings.push(format!("{}:{}: step `{}` queries table `{t}`, which the migrations of stores/{} do not create (tables: {})", n.file, n.line, n.name, store.name, store.schema.tables.keys().cloned().collect::<Vec<_>>().join(", "))); } }
                        let known: BTreeSet<&String> = tables.iter().filter_map(|t| store.schema.tables.get(t)).flatten().collect();
                        // bare names are the table's columns only in a plain query of one table (no WITH, no function in
                        // FROM), its select list's own aliases aside
                        if reads.plain && tables.len() == 1 && store.schema.tables.contains_key(&tables[0]) {
                            let _ = visit_expressions(st, |e: &Expr| {
                                if let Expr::Identifier(id) = e && !known.contains(&id.value) && id.value != "*" && !reads.aliases.contains(&id.value.to_lowercase()) { warnings.push(format!("{}:{}: step `{}` reads column `{}`, which is not in `{}` (columns: {})", n.file, n.line, n.name, id.value, tables[0], known.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))); }
                                ControlFlow::<()>::Continue(())
                            });
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => warnings.push(format!("{}:{}: step `{}` query does not parse as Postgres SQL ({e})", n.file, n.line, n.name)),
            }
        }
        // pii: the columns tagged in rules.yaml (of the queried tables, else of the whole store) that the output exposes
        let pii: BTreeSet<&String> = store.pii.iter().filter(|(t, _)| tables.is_empty() || tables.contains(t)).flat_map(|(_, cols)| cols).collect();
        if pii.is_empty() { continue; }
        let Some(schema) = schemas.get(&n.name) else { continue };
        let mut paths = Vec::new(); schema_paths(schema, "", 0, &mut paths);
        for (field, path) in paths {
            if !pii.contains(&field) { continue; }
            let covered = manifest_redact.iter().any(|r| { let r = r.trim_start_matches("steps."); r == n.name || r == format!("{}.{path}", n.name) || (r.starts_with(&format!("{}.", n.name)) && r.ends_with(&format!(".{field}"))) });
            if !covered { warnings.push(format!("{}:{}: step `{}` exposes `{path}`, tagged pii in stores/{}/rules.yaml, with no `redact: [steps.{}.{path}]` in workflow.yaml (or leave the column out of the role's readable list)", n.file, n.line, n.name, store.name, n.name)); }
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_follows_ddl() {
        let migs = vec![("0001.sql".to_string(), "CREATE TABLE t (id TEXT PRIMARY KEY, a INT); ALTER TABLE t ADD COLUMN b TEXT; ALTER TABLE t DROP COLUMN a; ALTER TABLE t RENAME COLUMN b TO c; CREATE TABLE gone (x INT); DROP TABLE gone;".to_string())];
        let mut w = Vec::new();
        let s = schema_of(&migs, &mut w, "s");
        assert!(w.is_empty()); assert_eq!(s.tables.len(), 1);
        assert_eq!(s.tables["t"].iter().cloned().collect::<Vec<_>>(), ["c", "id"]);
    }
    #[test]
    fn roles_and_rules_against_migrations() {
        let dir = std::env::temp_dir().join(format!("cori-stores-{}", std::process::id()));
        let st = dir.join("stores/tickets"); std::fs::create_dir_all(st.join("migrations")).unwrap(); std::fs::create_dir_all(st.join("roles")).unwrap();
        std::fs::write(dir.join("cori.toml"), "[app]\nid = \"x\"\n[connectors.tix]\nkind = \"store\"\nref = \"tickets\"\n").unwrap();
        std::fs::write(st.join("migrations/0001.sql"), "CREATE TABLE tickets (id TEXT, subject TEXT, body TEXT);").unwrap();
        std::fs::write(st.join("rules.yaml"), "tables:\n  tickets:\n    columns:\n      body: { tags: [pii] }\n      nope: {}\n").unwrap();
        std::fs::write(st.join("roles/agent.yaml"), "name: agent\ntables:\n  tickets:\n    readable: [id, subject, passed]\n  ghosts: { readable: [x] }\ndefault_page_size: 10\n").unwrap();
        let r = check_app(&dir);
        assert!(r.errors.iter().any(|e| e.contains("`tickets.nope`")), "{r:?}");
        assert!(r.errors.iter().any(|e| e.contains("names `passed`")), "{r:?}");
        assert!(r.errors.iter().any(|e| e.contains("table `ghosts`")), "{r:?}");
        assert!(r.warnings.iter().any(|w| w.contains("`default_page_size` is not a role setting")), "{r:?}");
        // `writes: workflows` is a role setting; it has no other value
        std::fs::write(st.join("roles/agent.yaml"), "name: agent\nwrites: workflows\ntables:\n  tickets:\n    readable: [id, subject]\n").unwrap();
        let r = check_app(&dir);
        assert!(!r.warnings.iter().chain(&r.errors).any(|w| w.contains("writes")), "{r:?}");
        std::fs::write(st.join("roles/agent.yaml"), "name: agent\nwrites: direct\ntables:\n  tickets:\n    readable: [id, subject]\n").unwrap();
        assert!(check_app(&dir).errors.iter().any(|e| e.contains("`writes` takes one value")), "writes: direct");
        std::fs::write(st.join("roles/agent.yaml"), "name: agent\ntables:\n  tickets:\n    readable: [id, subject, passed]\n  ghosts: { readable: [x] }\ndefault_page_size: 10\n").unwrap();
        // a query step over a missing column, and a pii column exposed without redact
        let node = crate::dag::Node { name: "list".into(), kind: "store".into(), description: String::new(), file: "steps/list.ts".into(), line: 1, index: 0, nested_in: None, slot: None, conditional: false, depends_on: Default::default(), after: Default::default(), refs: Default::default(), effect: crate::effects::Effect { access: crate::effects::Access::Read, target: String::new(), external: true }, literals: serde_json::json!({ "connector": "tix", "query": { "text": "select id, body, missing from tickets where id = $1" } }).as_object().unwrap().clone(), goto_targets: vec![], nested: vec![] };
        let mut schemas = BTreeMap::new(); schemas.insert("list".to_string(), serde_json::json!({ "type": "object", "properties": { "rows": { "type": "array", "items": { "type": "object", "properties": { "id": {}, "body": {} } } } } }));
        let w = check_workflow(&dir, &[], &[node.clone()], &schemas);
        assert!(w.iter().any(|x| x.contains("column `missing`")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("exposes `rows.body`")), "{w:?}");
        let w = check_workflow(&dir, &["steps.list.rows.body".into()], &[node.clone()], &schemas);
        assert!(!w.iter().any(|x| x.contains("exposes")), "{w:?}");
        // the names a WITH defines and a calendar from generate_series are no tables of the store; a misspelt table is one
        let mut days = node.clone();
        days.literals.insert("query".into(), serde_json::json!({ "text": "with recent as (select id from tickets) select d, count(r.id) from generate_series(1, 7) d left join recent r on true left join tickts t on true group by d" }));
        let w = check_workflow(&dir, &[], &[days], &BTreeMap::new());
        assert_eq!(w.iter().filter(|x| x.contains("queries table")).collect::<Vec<_>>().len(), 1, "{w:?}");
        assert!(w.iter().any(|x| x.contains("queries table `tickts`")), "{w:?}");
        // bare names are checked as columns in a plain query of one table only, its select aliases aside
        for (text, flagged) in [
            ("select d, count(t.id) as n from generate_series(1, 7) d left join tickets t on true group by d order by d", None),
            ("with t as (select id, subject as title from tickets) select title from t", None),
            ("select subject, count(*) as n from tickets group by subject order by n desc", None),
            ("select priority from tickets", Some("priority")),
        ] {
            let mut q = node.clone();
            q.literals.insert("query".into(), serde_json::json!({ "text": text }));
            let cols: Vec<String> = check_workflow(&dir, &[], &[q], &BTreeMap::new()).into_iter().filter(|x| x.contains("reads column")).collect();
            match flagged { None => assert!(cols.is_empty(), "{text}: {cols:?}"), Some(c) => assert!(cols.len() == 1 && cols[0].contains(&format!("column `{c}`")), "{text}: {cols:?}") }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn scope_tenancy_and_soft_delete_against_rules() {
        let dir = std::env::temp_dir().join(format!("cori-stores-rls-{}", std::process::id()));
        let st = dir.join("stores/desk"); std::fs::create_dir_all(st.join("migrations")).unwrap(); std::fs::create_dir_all(st.join("roles")).unwrap();
        std::fs::write(dir.join("cori.toml"), "[app]\nid = \"x\"\n[connectors.desk]\nkind = \"store\"\nref = \"desk\"\n").unwrap();
        std::fs::write(st.join("migrations/0001.sql"), "CREATE TABLE tickets (id TEXT, owner TEXT); CREATE TABLE comments (id TEXT, ticket_id TEXT); CREATE TABLE tags (id TEXT); CREATE TABLE stray (id TEXT);").unwrap();
        std::fs::write(st.join("rules.yaml"), "tables:\n  tickets: { tenant: ownr, soft_delete: { column: gone } }\n  comments: { tenant: { via: ticket_id, references: tags } }\n  tags: { global: true }\n").unwrap();
        std::fs::write(st.join("roles/lead.yaml"), "name: lead\ntables:\n  tickets: { scope: everything, readable: [id], deletable: { soft_delete: true } }\n  tags: { scope: all, readable: [id] }\n  comments: { readable: [id], deletable: { soft_delete: true } }\n  stray: { readable: [id] }\n").unwrap();
        let r = check_app(&dir);
        assert!(r.errors.iter().any(|e| e.contains("`tenant: ownr`")), "{r:?}");
        assert!(r.errors.iter().any(|e| e.contains("soft-deletes with `gone`")), "{r:?}");
        assert!(r.errors.iter().any(|e| e.contains("inherits its tenant from `tags`")), "{r:?}");
        assert!(r.errors.iter().any(|e| e.contains("`tickets.scope` is `own`")), "{r:?}");
        assert!(r.errors.iter().any(|e| e.contains("`comments.deletable.soft_delete` needs")), "{r:?}");
        assert!(r.warnings.iter().any(|w| w.contains("`tags.scope: all` has no effect")), "{r:?}");
        assert!(r.warnings.iter().any(|w| w.contains("`stray` is not named")), "{r:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn a_role_cannot_name_two_tables_with_the_same_tools() {
        let dir = std::env::temp_dir().join(format!("cori-stores-clash-{}", std::process::id()));
        let st = dir.join("stores/crm"); std::fs::create_dir_all(st.join("migrations")).unwrap(); std::fs::create_dir_all(st.join("roles")).unwrap();
        std::fs::write(dir.join("cori.toml"), "[app]\nid = \"x\"\n[connectors.crm]\nkind = \"store\"\nref = \"crm\"\n").unwrap();
        std::fs::write(st.join("migrations/0001.sql"), "CREATE TABLE person (id TEXT PRIMARY KEY); CREATE TABLE people (id TEXT PRIMARY KEY); CREATE TABLE items (id TEXT PRIMARY KEY);").unwrap();
        std::fs::write(st.join("rules.yaml"), "tables:\n  person: { global: true }\n  people: { global: true }\n  items: { global: true }\n").unwrap();
        std::fs::write(st.join("roles/both.yaml"), "name: both\ntables:\n  person: { readable: [id] }\n  people: { readable: [id] }\n").unwrap();
        std::fs::write(st.join("roles/one.yaml"), "name: one\ntables:\n  people: { readable: [id] }\n  items: { readable: [id] }\n").unwrap();
        let r = check_app(&dir);
        let clashes: Vec<&String> = r.errors.iter().filter(|e| e.contains("give the same tool names")).collect();
        assert_eq!(clashes.len(), 1, "{r:?}");
        assert!(clashes[0].starts_with("stores/crm/roles/both.yaml: tables `people` and `person`") && clashes[0].contains("`getPerson`"), "{r:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn store_step_tools_against_roles() {
        let dir = std::env::temp_dir().join(format!("cori-store-tools-{}", std::process::id()));
        let st = dir.join("stores/todos"); std::fs::create_dir_all(st.join("migrations")).unwrap(); std::fs::create_dir_all(st.join("roles")).unwrap();
        std::fs::write(dir.join("cori.toml"), "[app]\nid = \"x\"\n[connectors.todos]\nkind = \"store\"\nref = \"todos\"\n").unwrap();
        std::fs::write(st.join("migrations/0001.sql"), "CREATE TABLE items (id TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text, title TEXT NOT NULL, done BOOLEAN NOT NULL DEFAULT false, created_at TIMESTAMPTZ NOT NULL DEFAULT now());").unwrap();
        std::fs::write(st.join("roles/member.yaml"), "name: member\ntables:\n  items:\n    readable: [id, title, done, created_at]\n    creatable:\n      title: { required: true }\n    updatable:\n      done: {}\n    deletable: true\n").unwrap();
        std::fs::write(st.join("roles/viewer.yaml"), "name: viewer\ntables:\n  items:\n    readable: [id, title]\n").unwrap();
        let by_role = role_tools(&app_stores(&dir, &mut Vec::new())[0]);
        let names = |r: &str| by_role[r].iter().map(|t| t["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(names("member"), ["getItem", "listItems", "createItem", "updateItem", "deleteItem"]);
        assert_eq!(names("viewer"), ["getItem", "listItems"]);
        let tools = connector_tools(&dir, "todos").unwrap();
        let get = tools.iter().find(|t| t["name"] == "getItem").unwrap();
        assert_eq!(get["annotations"]["readOnlyHint"], true); assert_eq!(get["roles"], serde_json::json!(["member", "viewer"]));
        assert_eq!(tools.iter().find(|t| t["name"] == "createItem").unwrap()["inputSchema"]["required"], serde_json::json!(["title"]));
        assert_eq!(get["description"], "Retrieve one item by primary key (id)");
        assert_eq!(get["outputSchema"]["properties"]["done"], serde_json::json!({ "type": "boolean" }));
        let list = tools.iter().find(|t| t["name"] == "listItems").unwrap();
        assert_eq!(list["outputSchema"]["required"], serde_json::json!(["data", "count", "limit", "offset"]));
        assert_eq!(list["outputSchema"]["properties"]["data"]["items"]["required"], serde_json::json!(["created_at", "done", "id", "title"]));
        assert_eq!(tools.iter().find(|t| t["name"] == "deleteItem").unwrap()["outputSchema"]["additionalProperties"], true);
        let node = |name: &str, lit: Value| crate::dag::Node { name: name.into(), kind: "store".into(), description: String::new(), file: format!("steps/{name}.ts"), line: 1, index: 0, nested_in: None, slot: None, conditional: false, depends_on: Default::default(), after: Default::default(), refs: Default::default(), effect: crate::effects::Effect { access: crate::effects::Access::Read, target: String::new(), external: true }, literals: lit.as_object().unwrap().clone(), goto_targets: vec![], nested: vec![] };
        let nodes = vec![
            node("get", serde_json::json!({ "connector": "todos", "tool": "getItem" })),
            node("bogus", serde_json::json!({ "connector": "todos", "tool": "totallyBogus" })),
            node("create", serde_json::json!({ "connector": "todos", "tool": "createItem" })),
            node("sql", serde_json::json!({ "connector": "todos", "query": { "text": "select 1" } })),
            node("cte", serde_json::json!({ "connector": "todos", "query": { "text": "with open as (select id from items where not done) select count(*) from open" } })),
            // a query only reads: the store refuses these when they run, and the check before
            node("mark", serde_json::json!({ "connector": "todos", "query": { "text": "update items set done = true where id = $1" } })),
            node("sneaky", serde_json::json!({ "connector": "todos", "query": { "text": "with gone as (delete from items returning id) select * from gone" } })),
            node("two", serde_json::json!({ "connector": "todos", "query": { "text": "select 1; select 2" } })),
        ];
        let (e, w) = check_workflow_tools(&dir, &["member".into(), "viewer".into(), "outsider".into()], &nodes);
        assert_eq!(e.len(), 4, "{e:?}");
        assert!(e.iter().any(|m| m.contains("step `mark`'s query is UPDATE: a store query only reads")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("step `sneaky`'s query changes rows")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("step `two`'s query holds 2 statements")), "{e:?}");
        let e: Vec<&String> = e.iter().filter(|m| !m.contains("'s query")).collect();
        assert!(e[0].contains("`totallyBogus`") && e[0].contains("createItem, deleteItem, getItem, listItems, updateItem"), "{e:?}");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("step `create`") && w[0].contains("role `viewer`"), "{w:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
