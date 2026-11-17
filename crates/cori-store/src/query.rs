// SPDX-License-Identifier: Apache-2.0
//! Free SQL for the `query` tool. Columns and rows are Postgres' business: the statement runs in a read-only
//! transaction under the role's data role, whose grants cover only the columns the role reads and whose row level
//! security covers only its rows, so a query that reaches anything else fails in the database however it is written.
//! What remains here is what grants cannot express: exactly one SELECT, nothing that writes or locks, only the store's
//! own tables (with a readable error for a table the role does not read), and a function allow-list, which keeps free
//! SQL from changing its own session (`set_config`) or running SQL no one parsed (`query_to_xml`, `ts_stat`) on
//! servers where the admin credential cannot revoke those functions. Rows come back as JSON via row_to_json.
use anyhow::{Result, anyhow};
use cori_store_core::RoleDefinition;
use serde_json::Value;
use sqlparser::ast::{Expr, ObjectName, Query, SetExpr, Statement, TableFactor, Visit, Visitor};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use sqlx::{AssertSqlSafe, PgConnection, Row};
use std::ops::ControlFlow;

/// Functions free SQL may call: aggregates, window functions, and pure string, number, date and JSON functions.
const FUNCS: &[&str] = &[
    // aggregates
    "count", "sum", "avg", "min", "max", "string_agg", "array_agg", "bool_and", "bool_or", "every", "json_agg", "jsonb_agg", "json_object_agg", "jsonb_object_agg",
    "stddev", "stddev_pop", "stddev_samp", "variance", "var_pop", "var_samp", "corr", "percentile_cont", "percentile_disc", "mode",
    // window functions
    "row_number", "rank", "dense_rank", "percent_rank", "cume_dist", "ntile", "lag", "lead", "first_value", "last_value", "nth_value",
    // conditionals
    "coalesce", "nullif", "greatest", "least",
    // strings
    "lower", "upper", "length", "char_length", "octet_length", "concat", "concat_ws", "substring", "substr", "trim", "ltrim", "rtrim", "btrim", "left", "right",
    "replace", "position", "strpos", "split_part", "initcap", "lpad", "rpad", "reverse", "repeat", "starts_with", "format", "md5",
    "regexp_replace", "regexp_match", "regexp_matches", "regexp_like", "regexp_count", "regexp_substr", "string_to_array", "array_to_string", "to_hex",
    // numbers
    "abs", "round", "ceil", "ceiling", "floor", "trunc", "mod", "power", "sqrt", "ln", "log", "exp", "sign", "div", "width_bucket", "to_number",
    // dates
    "now", "current_date", "current_timestamp", "localtimestamp", "date", "date_trunc", "date_part", "extract", "to_char", "to_date", "to_timestamp", "age",
    "make_date", "make_time", "make_timestamp", "make_timestamptz", "make_interval", "justify_days", "justify_hours", "justify_interval", "date_bin",
    // JSON and arrays
    "to_json", "to_jsonb", "row_to_json", "json_build_object", "jsonb_build_object", "json_build_array", "jsonb_build_array", "jsonb_array_elements",
    "jsonb_array_elements_text", "json_array_elements", "json_array_elements_text", "jsonb_array_length", "json_array_length", "jsonb_extract_path",
    "jsonb_extract_path_text", "json_extract_path", "json_extract_path_text", "jsonb_typeof", "json_typeof", "jsonb_object_keys", "json_object_keys",
    "jsonb_each", "jsonb_each_text", "jsonb_strip_nulls", "array_length", "cardinality", "array_position", "array_remove", "unnest", "generate_series",
];

struct Guard<'a> {
    role: &'a RoleDefinition,
    /// Names defined by WITH anywhere in the statement: a relation with one of these names is not a table.
    ctes: Vec<String>,
    tables: Vec<String>,
    /// A function called in FROM (`generate_series(…) AS d`), checked as a function: the relation visit after it names it.
    function: Option<ObjectName>,
}

impl Visitor for Guard<'_> {
    type Break = String;

    fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<String> {
        if let Some(with) = &q.with {
            for cte in &with.cte_tables { self.ctes.push(cte.alias.name.value.to_lowercase()); }
        }
        if !q.locks.is_empty() { return ControlFlow::Break("`FOR UPDATE` / `FOR SHARE` are not allowed: the query tool only reads".into()); }
        check_body(&q.body)
    }

    /// A function in FROM is a function: the allow-list holds for it as anywhere else (`LATERAL set_config(…)` would
    /// otherwise rewrite the session's tenant before the row policies read it).
    fn pre_visit_table_factor(&mut self, t: &TableFactor) -> ControlFlow<String> {
        match t {
            TableFactor::Table { name, args: Some(_), .. } => { allowed_function(&name.to_string())?; self.function = Some(name.clone()); }
            TableFactor::Function { name, .. } => allowed_function(&name.to_string())?,
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, name: &ObjectName) -> ControlFlow<String> {
        if self.function.as_ref() == Some(name) { self.function = None; return ControlFlow::Continue(()); }
        let parts: Vec<String> = name.0.iter().map(|p| p.to_string().trim_matches('"').to_string()).collect();
        let (schema, table) = match parts.as_slice() {
            [t] => (None, t.clone()),
            [s, t] => (Some(s.clone()), t.clone()),
            _ => return ControlFlow::Break(format!("`{name}`: name a table of this store")),
        };
        if let Some(s) = &schema && s != "public" { return ControlFlow::Break(format!("`{name}` is not a table of this store")); }
        if schema.is_none() && self.ctes.contains(&table.to_lowercase()) { return ControlFlow::Continue(()); }
        if !self.role.can_read(&table) { return ControlFlow::Break(format!("role `{}` cannot read table `{table}`", self.role.name)); }
        self.tables.push(table);
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, e: &Expr) -> ControlFlow<String> {
        if let Expr::Function(f) = e { allowed_function(&f.name.to_string())?; }
        ControlFlow::Continue(())
    }
}

/// Whether free SQL may call function `name`: one of `FUNCS`, never one that changes its session or runs SQL no one parsed.
fn allowed_function(name: &str) -> ControlFlow<String> {
    let n = name.to_lowercase();
    if matches!(n.as_str(), "current_user" | "session_user" | "user" | "current_role") {
        return ControlFlow::Break(format!("`{n}` is the database login the store runs as, not the person calling: pass the caller as a parameter (`params: [ctx.user.sub]`, then `$1`)"));
    }
    if n == "set_config" || n == "current_setting" || n.starts_with("pg_") || n.contains("_to_xml") || n.starts_with("query_to") || n == "ts_stat" || n.starts_with("dblink") || n.starts_with("lo_") {
        return ControlFlow::Break(format!("function `{n}` is not allowed in a query"));
    }
    if !FUNCS.contains(&n.as_str()) { return ControlFlow::Break(format!("function `{n}` is not allowed (allowed: aggregates, window functions, and string, number, date and JSON functions, generate_series and unnest)")); }
    ControlFlow::Continue(())
}

/// Refuses a query body that writes (a data-modifying WITH, SELECT INTO).
fn check_body(body: &SetExpr) -> ControlFlow<String> {
    match body {
        SetExpr::Select(s) if s.into.is_some() => ControlFlow::Break("`SELECT … INTO` is not allowed: the query tool only reads".into()),
        SetExpr::SetOperation { left, right, .. } => { check_body(left)?; check_body(right) }
        SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => ControlFlow::Break("only SELECT queries are allowed (no INSERT, UPDATE, DELETE or MERGE, including inside WITH)".into()),
        _ => ControlFlow::Continue(()),
    }
}

/// The checks Postgres cannot make: one SELECT that writes nothing, the store's tables only, allowed functions only.
pub fn check(role: &RoleDefinition, sql: &str) -> Result<()> {
    let stmts = Parser::parse_sql(&PostgreSqlDialect {}, sql).map_err(|e| anyhow!("sql: {e}"))?;
    if stmts.len() != 1 { return Err(anyhow!("exactly one statement is allowed")); }
    let stmt = &stmts[0];
    if !matches!(stmt, Statement::Query(_)) { return Err(anyhow!("only SELECT queries are allowed: a query reads; change rows with the store's tools (create…, update…, delete…)")); }
    let mut g = Guard { role, ctes: Vec::new(), tables: Vec::new(), function: None };
    if let ControlFlow::Break(e) = stmt.visit(&mut g) { return Err(anyhow!("{e}")); }
    if g.tables.is_empty() && g.ctes.is_empty() { return Err(anyhow!("the query reads no table")); }
    Ok(())
}

/// Runs a guarded SELECT on a session connection (read-only, under the role's data role, tenant set).
pub async fn run_guarded(conn: &mut PgConnection, role: &RoleDefinition, sql: &str, params: &[Value], max_rows: u64) -> Result<Vec<Value>> {
    check(role, sql)?;
    let wrapped = format!("SELECT row_to_json(q)::text AS r FROM ({}) q LIMIT {}", sql.trim().trim_end_matches(';'), max_rows);
    // the caller's SQL, as `check` let it through: the session runs it read-only under the role's data role
    let mut q = sqlx::query(AssertSqlSafe(wrapped));
    for p in params { q = match p { Value::Number(n) if n.is_i64() => q.bind(n.as_i64().unwrap()), Value::Number(n) => q.bind(n.as_f64().unwrap_or(0.0)), Value::Bool(b) => q.bind(*b), Value::Null => q.bind(Option::<String>::None), other => q.bind(other.as_str().map(String::from).unwrap_or_else(|| other.to_string())) }; }
    let rows = tokio::time::timeout(std::time::Duration::from_secs(10), q.fetch_all(&mut *conn)).await.map_err(|_| anyhow!("query timed out (10 s)"))?
        .map_err(|e| anyhow!("{}", crate::explain_db_error(&role.name, &e.to_string())))?;
    Ok(rows.iter().map(|r| serde_json::from_str::<Value>(&r.get::<String, _>(0)).unwrap_or(Value::Null)).collect())
}

#[cfg(test)]
mod tests {
    use super::check;

    fn member() -> cori_store_core::RoleDefinition { serde_yaml::from_str("name: member\ntables:\n  notes: { readable: [id, body, author] }\n").unwrap() }

    /// The guard alone, as on a managed Postgres whose admin cannot revoke `set_config` from PUBLIC: a function called in
    /// FROM is held to the allow-list, LATERAL or not.
    #[test]
    fn a_function_in_from_is_held_to_the_allow_list() {
        let r = member();
        for sql in [
            "select n.id from lateral set_config('cori.tenant', 'bob', true) s, lateral (select id from notes where s is not null) n",
            "select n.id from set_config('cori.tenant', 'bob', true) s, lateral (select id from notes where s is not null) n",
            "select * from lateral pg_catalog.set_config('cori.tenant', 'bob', true) s, notes",
            "select * from lateral pg_ls_dir('.') f, notes",
            "select * from notes, lateral query_to_xml('select 1', true, false, '') x",
        ] {
            let e = check(&r, sql).expect_err(sql).to_string();
            assert!(e.contains("is not allowed"), "{sql}: {e}");
        }
        // a set-returning function of the allow-list is no table of the store
        check(&r, "select d::date as day, count(n.id) as n from generate_series(date '2025-01-05', date '2025-01-11', interval '1 day') d left join notes n on true group by d order by d").unwrap();
        check(&r, "select x from unnest(array[1, 2]) x, notes").unwrap();
        assert!(check(&r, "select * from generate_series(1, (select count(*) from secrets)) g, notes").is_err(), "its arguments are checked too");
    }

    #[test]
    fn the_caller_comes_as_a_parameter() {
        let e = check(&member(), "select id from notes where author = current_user").expect_err("current_user").to_string();
        assert!(e.contains("pass the caller as a parameter"), "{e}");
        let e = check(&member(), "update notes set body = 'x'").expect_err("update").to_string();
        assert!(e.contains("change rows with the store's tools"), "{e}");
    }
}
