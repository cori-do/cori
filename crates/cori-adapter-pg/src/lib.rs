use async_trait::async_trait;
use cori_core::{ActionDefinition, StepKind};
use cori_runtime::adapter::{ActionOutcome, DataAdapter};
use sqlx::postgres::{PgArguments, PgPoolOptions};
use sqlx::{Arguments, Row};

pub mod introspect;

fn args_add<T>(args: &mut PgArguments, v: T) -> anyhow::Result<()>
where
    T: Send + Sync + 'static,
    for<'q> T: sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    args.add(v).map_err(|e| anyhow::anyhow!(e))
}

#[derive(Debug, Clone, Copy)]
pub struct PostgresAdapterOptions {
    pub max_affected_rows: u64,
    pub preview_row_limit: u32,
}

impl Default for PostgresAdapterOptions {
    fn default() -> Self {
        Self {
            max_affected_rows: 1000,
            preview_row_limit: 25,
        }
    }
}

pub struct PostgresAdapter {
    pool: sqlx::PgPool,
    options: PostgresAdapterOptions,
}

impl PostgresAdapter {
    pub async fn new(database_url: &str, options: PostgresAdapterOptions) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        Ok(Self { pool, options })
    }
}

#[async_trait]
impl DataAdapter for PostgresAdapter {
    async fn load_resource_attrs(
        &self,
        _tenant_id: &str,
        _resource_kind: &str,
        _resource_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::json!({}))
    }

    async fn execute_action(
        &self,
        _tenant_id: &str,
        action: &ActionDefinition,
        inputs: &serde_json::Value,
        preview: bool,
    ) -> anyhow::Result<ActionOutcome> {
        let pg = PgActionMeta::from_action(action)?;

        match action.policy_action.as_str() {
            "get" => self.exec_get_by_id(action, &pg, inputs).await,
            "list" => self.exec_list(action, &pg, inputs).await,
            "update_fields" => self.exec_update_fields(action, &pg, inputs, preview).await,
            "soft_delete" => self.exec_soft_delete(action, &pg, inputs, preview).await,
            other => Err(anyhow::anyhow!(
                "Unsupported Postgres action policy_action='{}' (action='{}')",
                other,
                action.name
            )),
        }
    }
}

#[derive(Debug, Clone)]
struct PgColumnMeta {
    name: String,
    data_type: String,
    nullable: bool,
}

#[derive(Debug, Clone)]
struct PgActionMeta {
    schema: String,
    table: String,
    primary_key: Vec<String>,
    tenant_column: Option<String>,
    version_column: Option<String>,
    deleted_at_column: Option<String>,
    deleted_by_column: Option<String>,
    delete_reason_column: Option<String>,
    columns: Vec<PgColumnMeta>,
    updatable_columns: Vec<String>,
}

impl PgActionMeta {
    fn from_action(action: &ActionDefinition) -> anyhow::Result<Self> {
        let pg = action
            .meta
            .get("pg")
            .ok_or_else(|| anyhow::anyhow!("Action '{}' missing meta.pg", action.name))?;

        let schema = pg
            .get("schema")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Action '{}' missing meta.pg.schema", action.name))?
            .to_string();
        let table = pg
            .get("table")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Action '{}' missing meta.pg.table", action.name))?
            .to_string();

        let primary_key: Vec<String> = pg
            .get("primary_key")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let tenant_column = pg
            .get("tenant_column")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let version_column = pg
            .get("version_column")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let deleted_at_column = pg
            .get("deleted_at_column")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let deleted_by_column = pg
            .get("deleted_by_column")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let delete_reason_column = pg
            .get("delete_reason_column")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let columns_val = pg
            .get("columns")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Action '{}' missing meta.pg.columns", action.name))?;
        let mut columns = Vec::new();
        for c in columns_val {
            let name = c
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("Action '{}' meta.pg.columns missing name", action.name)
                })?
                .to_string();
            let data_type = c
                .get("data_type")
                .and_then(|v| v.as_str())
                .unwrap_or("text")
                .to_string();
            let nullable = c.get("nullable").and_then(|v| v.as_bool()).unwrap_or(true);
            columns.push(PgColumnMeta {
                name,
                data_type,
                nullable,
            });
        }

        let mut updatable_columns: Vec<String> = pg
            .get("updatable_columns")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        updatable_columns.sort();

        Ok(Self {
            schema,
            table,
            primary_key,
            tenant_column,
            version_column,
            deleted_at_column,
            deleted_by_column,
            delete_reason_column,
            columns,
            updatable_columns,
        })
    }

    fn full_table_ident(&self) -> anyhow::Result<String> {
        Ok(format!(
            "{}.{}",
            quote_ident(&self.schema)?,
            quote_ident(&self.table)?
        ))
    }

    fn column(&self, name: &str) -> Option<&PgColumnMeta> {
        self.columns.iter().find(|c| c.name == name)
    }
}

fn quote_ident(ident: &str) -> anyhow::Result<String> {
    if ident.is_empty() {
        return Err(anyhow::anyhow!("empty identifier"));
    }
    // Be strict: we only expect generator-produced identifiers.
    if !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(anyhow::anyhow!("invalid identifier '{}'", ident));
    }
    Ok(format!("\"{}\"", ident.replace('"', "\"\"")))
}

fn cast_for_pg_type(data_type: &str) -> Option<&'static str> {
    match data_type {
        "numeric" | "real" | "double precision" | "decimal" => Some("numeric"),
        "date" => Some("date"),
        "timestamp with time zone" => Some("timestamptz"),
        "timestamp without time zone" => Some("timestamp"),
        _ => None,
    }
}

fn add_arg_for_col(
    args: &mut PgArguments,
    col: &PgColumnMeta,
    v: &serde_json::Value,
) -> anyhow::Result<()> {
    use serde_json::Value;

    if v.is_null() {
        if !col.nullable {
            return Err(anyhow::anyhow!(
                "Column '{}' is not nullable but received null",
                col.name
            ));
        }
        // Bind a NULL of an appropriate type.
        match col.data_type.as_str() {
            "uuid" => args_add(args, Option::<uuid::Uuid>::None)?,
            "boolean" => args_add(args, Option::<bool>::None)?,
            "integer" | "bigint" | "smallint" => args_add(args, Option::<i64>::None)?,
            "json" | "jsonb" => {
                args_add(args, Option::<sqlx::types::Json<serde_json::Value>>::None)?
            }
            _ => args_add(args, Option::<String>::None)?,
        }
        return Ok(());
    }

    match col.data_type.as_str() {
        "uuid" => {
            let s = v
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Expected uuid string for '{}'", col.name))?;
            let id = uuid::Uuid::parse_str(s)?;
            args_add(args, id)?;
        }
        "boolean" => {
            let b = v
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("Expected boolean for '{}'", col.name))?;
            args_add(args, b)?;
        }
        "integer" | "bigint" | "smallint" => {
            let n = v
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Expected integer for '{}'", col.name))?;
            args_add(args, n)?;
        }
        "json" | "jsonb" => {
            args_add(args, sqlx::types::Json(v.clone()))?;
        }
        // For these, bind as string and rely on explicit cast in SQL
        "numeric" | "real" | "double precision" | "decimal" => match v {
            Value::Number(n) => args_add(args, n.to_string())?,
            Value::String(s) => args_add(args, s.clone())?,
            _ => return Err(anyhow::anyhow!("Expected number/string for '{}'", col.name)),
        },
        "date" | "timestamp with time zone" | "timestamp without time zone" => {
            let s = v
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Expected date/time string for '{}'", col.name))?;
            args_add(args, s.to_string())?;
        }
        _ => {
            // default to string
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                other => other.to_string(),
            };
            args_add(args, s)?;
        }
    }

    Ok(())
}

fn get_obj<'a>(
    v: &'a serde_json::Value,
    ctx: &str,
) -> anyhow::Result<&'a serde_json::Map<String, serde_json::Value>> {
    v.as_object()
        .ok_or_else(|| anyhow::anyhow!("Expected object for {}", ctx))
}

impl PostgresAdapter {
    async fn exec_get_by_id(
        &self,
        action: &ActionDefinition,
        meta: &PgActionMeta,
        inputs: &serde_json::Value,
    ) -> anyhow::Result<ActionOutcome> {
        if action.kind != StepKind::Query {
            return Err(anyhow::anyhow!("Action '{}' kind mismatch", action.name));
        }

        let inputs = get_obj(inputs, "inputs")?;
        let table = meta.full_table_ident()?;
        let mut where_parts: Vec<String> = Vec::new();
        let mut args = PgArguments::default();
        let mut idx: usize = 1;

        if let Some(tc) = &meta.tenant_column {
            let v = inputs
                .get(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", tc))?;
            let col = meta
                .column(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", tc))?;
            let cast = cast_for_pg_type(&col.data_type)
                .map(|c| format!("::{}", c))
                .unwrap_or_default();
            where_parts.push(format!("{} = ${}{}", quote_ident(tc)?, idx, cast));
            add_arg_for_col(&mut args, col, v)?;
            idx += 1;
        }

        for pk in &meta.primary_key {
            let v = inputs
                .get(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", pk))?;
            let col = meta
                .column(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", pk))?;
            let cast = cast_for_pg_type(&col.data_type)
                .map(|c| format!("::{}", c))
                .unwrap_or_default();
            where_parts.push(format!("{} = ${}{}", quote_ident(pk)?, idx, cast));
            add_arg_for_col(&mut args, col, v)?;
            idx += 1;
        }

        if where_parts.is_empty() {
            return Err(anyhow::anyhow!(
                "Action '{}' cannot run without a key filter",
                action.name
            ));
        }

        let sql = format!(
            "SELECT to_jsonb(t) AS row FROM {} AS t WHERE {} LIMIT 1",
            table,
            where_parts.join(" AND ")
        );
        let rec = sqlx::query_with(&sql, args)
            .fetch_optional(&self.pool)
            .await?;
        let row_json: Option<serde_json::Value> = rec
            .map(|r| r.try_get::<serde_json::Value, _>("row"))
            .transpose()?;

        Ok(ActionOutcome {
            affected_count: 0,
            preview_diff: None,
            output: serde_json::json!({ "row": row_json }),
        })
    }

    async fn exec_list(
        &self,
        action: &ActionDefinition,
        meta: &PgActionMeta,
        inputs: &serde_json::Value,
    ) -> anyhow::Result<ActionOutcome> {
        if action.kind != StepKind::Query {
            return Err(anyhow::anyhow!("Action '{}' kind mismatch", action.name));
        }

        let inputs = get_obj(inputs, "inputs")?;
        let table = meta.full_table_ident()?;
        let limit = inputs
            .get("limit")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("Missing/invalid required input 'limit'"))?;
        let cursor = inputs
            .get("cursor")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let mut where_parts: Vec<String> = Vec::new();
        let mut args = PgArguments::default();
        let mut idx: usize = 1;

        if let Some(tc) = &meta.tenant_column {
            let v = inputs
                .get(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", tc))?;
            let col = meta
                .column(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", tc))?;
            where_parts.push(format!("{} = ${}", quote_ident(tc)?, idx));
            add_arg_for_col(&mut args, col, v)?;
            idx += 1;
        }

        let order_by = if meta.primary_key.len() == 1 {
            Some(meta.primary_key[0].clone())
        } else {
            None
        };

        if let Some(pk) = &order_by {
            if !cursor.is_null() {
                let col = meta
                    .column(pk)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", pk))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                where_parts.push(format!("{} > ${}{}", quote_ident(pk)?, idx, cast));
                add_arg_for_col(&mut args, col, &cursor)?;
                idx += 1;
            }
        } else if !cursor.is_null() {
            return Err(anyhow::anyhow!(
                "Action '{}' cursor is not supported for tables without a single-column primary key",
                action.name
            ));
        }

        // bind limit
        args_add(&mut args, limit)?;
        let limit_idx = idx;
        // idx += 1; // not needed after this point

        let where_sql = if where_parts.is_empty() {
            "".to_string()
        } else {
            format!("WHERE {}", where_parts.join(" AND "))
        };

        let order_sql = if let Some(pk) = &order_by {
            format!("ORDER BY {} ASC", quote_ident(pk)?)
        } else {
            "".to_string()
        };

        let sql = format!(
            "SELECT to_jsonb(t) AS row FROM {} AS t {} {} LIMIT ${}",
            table, where_sql, order_sql, limit_idx
        );

        let recs = sqlx::query_with(&sql, args).fetch_all(&self.pool).await?;
        let mut rows: Vec<serde_json::Value> = Vec::new();
        for r in recs {
            rows.push(r.try_get::<serde_json::Value, _>("row")?);
        }

        let next_cursor = if let Some(pk) = &order_by {
            rows.last().and_then(|row| row.get(pk)).cloned()
        } else {
            None
        };

        Ok(ActionOutcome {
            affected_count: 0,
            preview_diff: None,
            output: serde_json::json!({
                "rows": rows,
                "next_cursor": next_cursor
            }),
        })
    }

    async fn exec_update_fields(
        &self,
        action: &ActionDefinition,
        meta: &PgActionMeta,
        inputs: &serde_json::Value,
        preview: bool,
    ) -> anyhow::Result<ActionOutcome> {
        if action.kind != StepKind::Mutation {
            return Err(anyhow::anyhow!("Action '{}' kind mismatch", action.name));
        }
        let inputs = get_obj(inputs, "inputs")?;
        let patch = inputs
            .get("patch")
            .ok_or_else(|| anyhow::anyhow!("Missing required input 'patch'"))?;
        let patch_obj = get_obj(patch, "inputs.patch")?;

        if patch_obj.is_empty() {
            return Ok(ActionOutcome {
                affected_count: 0,
                preview_diff: if preview {
                    Some(serde_json::json!({ "note": "empty patch", "sample": [] }))
                } else {
                    None
                },
                output: serde_json::json!({ "ok": true, "note": "empty patch" }),
            });
        }

        // Filter patch keys to generator-approved columns (defense in depth).
        let mut patch_pairs: Vec<(&String, &serde_json::Value)> = patch_obj
            .iter()
            .filter(|(k, _)| meta.updatable_columns.iter().any(|c| c == *k))
            .collect();
        patch_pairs.sort_by(|a, b| a.0.cmp(b.0));

        if patch_pairs.is_empty() {
            return Err(anyhow::anyhow!(
                "No allowed columns found in patch for action '{}'",
                action.name
            ));
        }

        let table = meta.full_table_ident()?;

        // Build WHERE (by tenant + PK [+ expected_version] [+ not deleted])
        let mut where_parts: Vec<String> = Vec::new();
        let mut args_base = PgArguments::default();
        let mut idx: usize = 1;

        if let Some(tc) = &meta.tenant_column {
            let v = inputs
                .get(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", tc))?;
            let col = meta
                .column(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", tc))?;
            let cast = cast_for_pg_type(&col.data_type)
                .map(|c| format!("::{}", c))
                .unwrap_or_default();
            where_parts.push(format!("{} = ${}{}", quote_ident(tc)?, idx, cast));
            add_arg_for_col(&mut args_base, col, v)?;
            idx += 1;
        }

        for pk in &meta.primary_key {
            let v = inputs
                .get(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", pk))?;
            let col = meta
                .column(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", pk))?;
            let cast = cast_for_pg_type(&col.data_type)
                .map(|c| format!("::{}", c))
                .unwrap_or_default();
            where_parts.push(format!("{} = ${}{}", quote_ident(pk)?, idx, cast));
            add_arg_for_col(&mut args_base, col, v)?;
            idx += 1;
        }

        if let (Some(vc), Some(expected)) = (&meta.version_column, inputs.get("expected_version"))
            && !expected.is_null() {
                let col = meta
                    .column(vc)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", vc))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                where_parts.push(format!("{} = ${}{}", quote_ident(vc)?, idx, cast));
                add_arg_for_col(&mut args_base, col, expected)?;
                idx += 1;
            }

        if let Some(dc) = &meta.deleted_at_column {
            where_parts.push(format!("{} IS NULL", quote_ident(dc)?));
        }

        if where_parts.is_empty() {
            return Err(anyhow::anyhow!("Refusing to run unbounded update"));
        }

        if preview {
            // Preview: no writes. Count + sample + compute "after" client-side.
            let count_sql = format!(
                "SELECT count(*)::bigint AS cnt FROM {} AS t WHERE {}",
                table,
                where_parts.join(" AND ")
            );
            let cnt: i64 = {
                let r = sqlx::query_with(&count_sql, args_base.clone())
                    .fetch_one(&self.pool)
                    .await?;
                r.try_get::<i64, _>("cnt")?
            };

            let mut args_sample = args_base.clone();
            args_add(&mut args_sample, self.options.preview_row_limit as i64)?;
            let sample_limit_idx = idx;
            let sample_sql = format!(
                "SELECT to_jsonb(t) AS row FROM {} AS t WHERE {} LIMIT ${}",
                table,
                where_parts.join(" AND "),
                sample_limit_idx
            );
            let recs = sqlx::query_with(&sample_sql, args_sample)
                .fetch_all(&self.pool)
                .await?;
            let mut sample = Vec::new();
            for r in recs {
                let before: serde_json::Value = r.try_get("row")?;
                let mut after = before.clone();
                for (k, v) in &patch_pairs {
                    if let Some(obj) = after.as_object_mut() {
                        obj.insert((*k).clone(), (*v).clone());
                    }
                }
                if let Some(vc) = &meta.version_column
                    && let Some(obj) = after.as_object_mut()
                        && let Some(vv) = obj.get(vc).cloned()
                            && let Some(n) = vv.as_i64() {
                                obj.insert(vc.clone(), serde_json::json!(n + 1));
                            }
                let pk_obj = meta
                    .primary_key
                    .iter()
                    .filter_map(|pk| before.get(pk).map(|v| (pk.clone(), v.clone())))
                    .collect::<serde_json::Map<_, _>>();
                sample.push(serde_json::json!({
                    "pk": pk_obj,
                    "before": before,
                    "after": after,
                    "changed_columns": patch_pairs.iter().map(|(k, _)| (*k).clone()).collect::<Vec<_>>()
                }));
            }

            return Ok(ActionOutcome {
                affected_count: cnt.max(0) as u64,
                preview_diff: Some(serde_json::json!({
                    "affected_count": cnt,
                    "sample": sample
                })),
                output: serde_json::json!({ "ok": true, "preview": true }),
            });
        }

        // Execute: step-level transaction
        let mut tx = self.pool.begin().await?;

        // Guardrail: count affected rows
        let count_sql = format!(
            "SELECT count(*)::bigint AS cnt FROM {} AS t WHERE {}",
            table,
            where_parts.join(" AND ")
        );
        let cnt: i64 = {
            let r = sqlx::query_with(&count_sql, args_base.clone())
                .fetch_one(&mut *tx)
                .await?;
            r.try_get::<i64, _>("cnt")?
        };
        if cnt > self.options.max_affected_rows as i64 {
            return Err(anyhow::anyhow!(
                "Refusing to execute: affected rows {} exceeds max_affected_rows {}",
                cnt,
                self.options.max_affected_rows
            ));
        }

        // Build SET parts
        let mut set_parts: Vec<String> = Vec::new();
        let mut args = PgArguments::default();
        let mut set_idx: usize = 1;

        for (k, v) in &patch_pairs {
            let col = meta
                .column(k)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", k))?;
            let cast = cast_for_pg_type(&col.data_type)
                .map(|c| format!("::{}", c))
                .unwrap_or_default();
            set_parts.push(format!("{} = ${}{}", quote_ident(k)?, set_idx, cast));
            add_arg_for_col(&mut args, col, v)?;
            set_idx += 1;
        }
        if let Some(vc) = &meta.version_column {
            set_parts.push(format!("{} = {} + 1", quote_ident(vc)?, quote_ident(vc)?));
        }

        // Append WHERE binds after SET binds
        // Re-bind in the same order as in where_parts construction (tenant, pk..., expected_version)
        if let Some(tc) = &meta.tenant_column {
            let v = inputs
                .get(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", tc))?;
            let col = meta
                .column(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", tc))?;
            add_arg_for_col(&mut args, col, v)?;
        }
        for pk in &meta.primary_key {
            let v = inputs
                .get(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", pk))?;
            let col = meta
                .column(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", pk))?;
            add_arg_for_col(&mut args, col, v)?;
        }
        if let (Some(vc), Some(expected)) = (&meta.version_column, inputs.get("expected_version"))
            && !expected.is_null() {
                let col = meta
                    .column(vc)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", vc))?;
                add_arg_for_col(&mut args, col, expected)?;
            }

        // Rewrite WHERE placeholders to reflect SET binds offset
        let where_sql = {
            let mut out = Vec::new();
            let mut current = set_idx;
            if let Some(tc) = &meta.tenant_column {
                let col = meta
                    .column(tc)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", tc))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                out.push(format!("{} = ${}{}", quote_ident(tc)?, current, cast));
                current += 1;
            }
            for pk in &meta.primary_key {
                let col = meta
                    .column(pk)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", pk))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                out.push(format!("{} = ${}{}", quote_ident(pk)?, current, cast));
                current += 1;
            }
            if let (Some(vc), Some(expected)) =
                (&meta.version_column, inputs.get("expected_version"))
                && !expected.is_null() {
                    let col = meta
                        .column(vc)
                        .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", vc))?;
                    let cast = cast_for_pg_type(&col.data_type)
                        .map(|c| format!("::{}", c))
                        .unwrap_or_default();
                    out.push(format!("{} = ${}{}", quote_ident(vc)?, current, cast));
                }
            if let Some(dc) = &meta.deleted_at_column {
                out.push(format!("{} IS NULL", quote_ident(dc)?));
            }
            out.join(" AND ")
        };

        let sql = format!(
            "UPDATE {} AS t SET {} WHERE {} RETURNING to_jsonb(t) AS row",
            table,
            set_parts.join(", "),
            where_sql
        );

        let rec = sqlx::query_with(&sql, args)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;

        let row_json: Option<serde_json::Value> = rec
            .map(|r| r.try_get::<serde_json::Value, _>("row"))
            .transpose()?;

        Ok(ActionOutcome {
            affected_count: row_json.is_some() as u64,
            preview_diff: None,
            output: serde_json::json!({ "row": row_json }),
        })
    }

    async fn exec_soft_delete(
        &self,
        action: &ActionDefinition,
        meta: &PgActionMeta,
        inputs: &serde_json::Value,
        preview: bool,
    ) -> anyhow::Result<ActionOutcome> {
        if action.kind != StepKind::Mutation {
            return Err(anyhow::anyhow!("Action '{}' kind mismatch", action.name));
        }
        let inputs = get_obj(inputs, "inputs")?;
        let table = meta.full_table_ident()?;

        let deleted_at_col = meta.deleted_at_column.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "Action '{}' requires deleted_at_column in meta",
                action.name
            )
        })?;

        // WHERE: tenant + PK [+ expected_version] + not already deleted
        let mut where_parts: Vec<String> = Vec::new();
        let mut args_base = PgArguments::default();
        let mut idx: usize = 1;

        if let Some(tc) = &meta.tenant_column {
            let v = inputs
                .get(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", tc))?;
            let col = meta
                .column(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", tc))?;
            let cast = cast_for_pg_type(&col.data_type)
                .map(|c| format!("::{}", c))
                .unwrap_or_default();
            where_parts.push(format!("{} = ${}{}", quote_ident(tc)?, idx, cast));
            add_arg_for_col(&mut args_base, col, v)?;
            idx += 1;
        }
        for pk in &meta.primary_key {
            let v = inputs
                .get(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", pk))?;
            let col = meta
                .column(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", pk))?;
            let cast = cast_for_pg_type(&col.data_type)
                .map(|c| format!("::{}", c))
                .unwrap_or_default();
            where_parts.push(format!("{} = ${}{}", quote_ident(pk)?, idx, cast));
            add_arg_for_col(&mut args_base, col, v)?;
            idx += 1;
        }
        if let (Some(vc), Some(expected)) = (&meta.version_column, inputs.get("expected_version"))
            && !expected.is_null() {
                let col = meta
                    .column(vc)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", vc))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                where_parts.push(format!("{} = ${}{}", quote_ident(vc)?, idx, cast));
                add_arg_for_col(&mut args_base, col, expected)?;
                idx += 1;
            }
        where_parts.push(format!("{} IS NULL", quote_ident(deleted_at_col)?));

        if preview {
            let count_sql = format!(
                "SELECT count(*)::bigint AS cnt FROM {} AS t WHERE {}",
                table,
                where_parts.join(" AND ")
            );
            let cnt: i64 = {
                let r = sqlx::query_with(&count_sql, args_base.clone())
                    .fetch_one(&self.pool)
                    .await?;
                r.try_get::<i64, _>("cnt")?
            };

            let mut args_sample = args_base.clone();
            args_add(&mut args_sample, self.options.preview_row_limit as i64)?;
            let sample_limit_idx = idx;
            let sample_sql = format!(
                "SELECT to_jsonb(t) AS row FROM {} AS t WHERE {} LIMIT ${}",
                table,
                where_parts.join(" AND "),
                sample_limit_idx
            );
            let recs = sqlx::query_with(&sample_sql, args_sample)
                .fetch_all(&self.pool)
                .await?;
            let now = chrono::Utc::now().to_rfc3339();

            let mut sample = Vec::new();
            for r in recs {
                let before: serde_json::Value = r.try_get("row")?;
                let mut after = before.clone();
                if let Some(obj) = after.as_object_mut() {
                    obj.insert(deleted_at_col.to_string(), serde_json::json!(now));
                    if let Some(dbc) = &meta.deleted_by_column
                        && let Some(v) = inputs.get("deleted_by") {
                            obj.insert(dbc.clone(), v.clone());
                        }
                    if let Some(drc) = &meta.delete_reason_column
                        && let Some(v) = inputs.get("reason") {
                            obj.insert(drc.clone(), v.clone());
                        }
                    if let Some(vc) = &meta.version_column
                        && let Some(vv) = obj.get(vc).cloned()
                            && let Some(n) = vv.as_i64() {
                                obj.insert(vc.clone(), serde_json::json!(n + 1));
                            }
                }
                let pk_obj = meta
                    .primary_key
                    .iter()
                    .filter_map(|pk| before.get(pk).map(|v| (pk.clone(), v.clone())))
                    .collect::<serde_json::Map<_, _>>();
                sample.push(serde_json::json!({
                    "pk": pk_obj,
                    "before": before,
                    "after": after
                }));
            }

            return Ok(ActionOutcome {
                affected_count: cnt.max(0) as u64,
                preview_diff: Some(serde_json::json!({
                    "affected_count": cnt,
                    "sample": sample
                })),
                output: serde_json::json!({ "ok": true, "preview": true }),
            });
        }

        let mut tx = self.pool.begin().await?;

        // Guardrail: count
        let count_sql = format!(
            "SELECT count(*)::bigint AS cnt FROM {} AS t WHERE {}",
            table,
            where_parts.join(" AND ")
        );
        let cnt: i64 = {
            let r = sqlx::query_with(&count_sql, args_base.clone())
                .fetch_one(&mut *tx)
                .await?;
            r.try_get::<i64, _>("cnt")?
        };
        if cnt > self.options.max_affected_rows as i64 {
            return Err(anyhow::anyhow!(
                "Refusing to execute: affected rows {} exceeds max_affected_rows {}",
                cnt,
                self.options.max_affected_rows
            ));
        }

        // SET deleted_at=NOW() [+ deleted_by/+delete_reason] [+ version+1]
        let mut set_parts = vec![format!("{} = NOW()", quote_ident(deleted_at_col)?)];
        let mut args = PgArguments::default();

        // Bind SET values first (deleted_by/reason)
        let mut set_idx: usize = 1;
        if let Some(dbc) = &meta.deleted_by_column
            && let Some(v) = inputs.get("deleted_by") {
                let col = meta
                    .column(dbc)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", dbc))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                set_parts.push(format!("{} = ${}{}", quote_ident(dbc)?, set_idx, cast));
                add_arg_for_col(&mut args, col, v)?;
                set_idx += 1;
            }
        if let Some(drc) = &meta.delete_reason_column
            && let Some(v) = inputs.get("reason") {
                let col = meta
                    .column(drc)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", drc))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                set_parts.push(format!("{} = ${}{}", quote_ident(drc)?, set_idx, cast));
                add_arg_for_col(&mut args, col, v)?;
                set_idx += 1;
            }
        if let Some(vc) = &meta.version_column {
            set_parts.push(format!("{} = {} + 1", quote_ident(vc)?, quote_ident(vc)?));
        }

        // WHERE binds after SET binds
        if let Some(tc) = &meta.tenant_column {
            let v = inputs
                .get(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", tc))?;
            let col = meta
                .column(tc)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", tc))?;
            add_arg_for_col(&mut args, col, v)?;
        }
        for pk in &meta.primary_key {
            let v = inputs
                .get(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing required input '{}'", pk))?;
            let col = meta
                .column(pk)
                .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", pk))?;
            add_arg_for_col(&mut args, col, v)?;
        }
        if let (Some(vc), Some(expected)) = (&meta.version_column, inputs.get("expected_version"))
            && !expected.is_null() {
                let col = meta
                    .column(vc)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", vc))?;
                add_arg_for_col(&mut args, col, expected)?;
            }

        let where_sql = {
            let mut out = Vec::new();
            let mut current = set_idx;
            if let Some(tc) = &meta.tenant_column {
                let col = meta
                    .column(tc)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", tc))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                out.push(format!("{} = ${}{}", quote_ident(tc)?, current, cast));
                current += 1;
            }
            for pk in &meta.primary_key {
                let col = meta
                    .column(pk)
                    .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", pk))?;
                let cast = cast_for_pg_type(&col.data_type)
                    .map(|c| format!("::{}", c))
                    .unwrap_or_default();
                out.push(format!("{} = ${}{}", quote_ident(pk)?, current, cast));
                current += 1;
            }
            if let (Some(vc), Some(expected)) =
                (&meta.version_column, inputs.get("expected_version"))
                && !expected.is_null() {
                    let col = meta
                        .column(vc)
                        .ok_or_else(|| anyhow::anyhow!("Missing column metadata for '{}'", vc))?;
                    let cast = cast_for_pg_type(&col.data_type)
                        .map(|c| format!("::{}", c))
                        .unwrap_or_default();
                    out.push(format!("{} = ${}{}", quote_ident(vc)?, current, cast));
                }
            out.push(format!("{} IS NULL", quote_ident(deleted_at_col)?));
            out.join(" AND ")
        };

        let sql = format!(
            "UPDATE {} AS t SET {} WHERE {} RETURNING to_jsonb(t) AS row",
            table,
            set_parts.join(", "),
            where_sql
        );
        let rec = sqlx::query_with(&sql, args)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;

        let row_json: Option<serde_json::Value> = rec
            .map(|r| r.try_get::<serde_json::Value, _>("row"))
            .transpose()?;

        Ok(ActionOutcome {
            affected_count: row_json.is_some() as u64,
            preview_diff: None,
            output: serde_json::json!({ "row": row_json }),
        })
    }
}
