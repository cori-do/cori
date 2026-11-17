// SPDX-License-Identifier: Apache-2.0
//! Statements: values bound rather than written into the SQL, identifiers quoted, and rows read back as JSON.

use super::ToolExecutor;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Row};
use sqlx::postgres::{PgArguments, Postgres};
use sqlx::query::Query;

pub(super) use crate::grants::ident;

/// The values of a statement, bound rather than written into its text. Each is sent as text and cast to its column's
/// type (`$1::pg_catalog.int4`): Postgres reads it with that type's input, as it reads the quoted literal it replaces.
#[derive(Default)]
pub(super) struct Binds(Vec<String>);

impl Binds {
    /// The SQL standing for `value` in a column of type `cast`: a placeholder, or NULL. A number keeps its JSON digits;
    /// an object or an array is bound as its JSON text (json and jsonb columns).
    pub(super) fn value(&mut self, value: &Value, cast: Option<&str>) -> String {
        let text = match value {
            Value::Null => return "NULL".to_string(),
            Value::String(s) => s.clone(),
            Value::Number(n) => match n.as_i64() {
                Some(i) => i.to_string(),
                None => n.as_u64().map(|u| u.to_string()).unwrap_or_else(|| n.as_f64().unwrap_or_default().to_string()),
            },
            other => other.to_string(),
        };
        self.0.push(text);
        match cast {
            Some(t) => format!("${}::{t}", self.0.len()),
            None => format!("${}", self.0.len()),
        }
    }

    /// The statement with its values bound. Its text holds identifiers (quoted) and placeholders, never a value.
    pub(super) fn query(&self, sql: &str) -> Query<'static, Postgres, PgArguments> {
        self.0.iter().fold(sqlx::query(AssertSqlSafe(sql)), |q, v| q.bind(v.as_str()))
    }
}

impl ToolExecutor {
    /// The type a value for a column is cast to, from the schema (`ColumnSchema::cast_type`).
    pub(super) fn cast(&self, table: &str, column: &str) -> Option<String> {
        self.schema.as_ref()?.get_table(table)?.get_column(column)?.cast_type()
    }

    /// `column = value`, the value bound for the column's type (`= NULL` for null: an update clears the column).
    pub(super) fn eq(&self, binds: &mut Binds, table: &str, column: &str, value: &Value) -> String {
        format!("{} = {}", ident(column), binds.value(value, self.cast(table, column).as_deref()))
    }

    /// The conditions picking one row: its primary key values, the caller's tenant, and not soft-deleted.
    pub(super) fn row_conditions(&self, binds: &mut Binds, table: &str, keys: &[(String, Value)], tenant_id: &str) -> Vec<String> {
        let mut conditions: Vec<String> = keys.iter().map(|(c, v)| self.eq(binds, table, c, v)).collect();
        if let Some(tc) = self.tenant_column_for_table(table) {
            conditions.push(self.eq(binds, table, &tc, &Value::String(tenant_id.to_string())));
        }
        conditions.extend(self.active_condition(table));
        conditions
    }

    /// The primary key values of a call's arguments.
    pub(super) fn key_values(&self, table: &str, arguments: &Value) -> Result<Vec<(String, Value)>, String> {
        self.get_primary_key_columns(table)
            .into_iter()
            .map(|c| match arguments.get(&c) {
                Some(v) if is_key(v) => Ok((c, v.clone())),
                Some(_) => Err(format!("Primary key field '{}' must be a string or an integer", c)),
                None => Err(format!("Missing required primary key field: {}", c)),
            })
            .collect()
    }
}

/// A primary key value: an integer, or a string (text and uuid keys).
pub(super) fn is_key(v: &Value) -> bool {
    v.is_i64() || v.is_string()
}

/// Primary key values as messages show them: `order_id = 42 AND code = 'A1'`.
pub(super) fn describe_keys(keys: &[(String, Value)]) -> String {
    keys.iter()
        .map(|(c, v)| match v {
            Value::String(s) => format!("{c} = '{s}'"),
            v => format!("{c} = {v}"),
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Quoted column names, comma-separated.
pub(super) fn ident_list(columns: &[String]) -> String {
    columns.iter().map(|c| ident(c)).collect::<Vec<_>>().join(", ")
}

/// Convert a sqlx row to JSON, using provided columns or all columns if empty.
pub(super) fn row_to_json(row: &sqlx::postgres::PgRow, columns: &[String]) -> Value {
    use bigdecimal::BigDecimal;
    use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
    use sqlx::Column;

    let mut obj = serde_json::Map::new();

    for col in row.columns() {
        let name = col.name();

        // If columns list is provided and non-empty, filter
        if !columns.is_empty() && !columns.iter().any(|c| c == name) {
            continue;
        }

        // Try to extract the value as different types
        // Order matters: try more specific types first
        let value: Value =
            // Integer types
            if let Ok(v) = row.try_get::<i64, _>(name) {
                json!(v)
            } else if let Ok(v) = row.try_get::<i32, _>(name) {
                json!(v)
            } else if let Ok(v) = row.try_get::<i16, _>(name) {
                json!(v)
            }
            // Floating point
            else if let Ok(v) = row.try_get::<f64, _>(name) {
                json!(v)
            } else if let Ok(v) = row.try_get::<f32, _>(name) {
                json!(v)
            }
            // BigDecimal for DECIMAL/NUMERIC columns
            else if let Ok(v) = row.try_get::<BigDecimal, _>(name) {
                // Convert to f64 for JSON, preserving precision
                use bigdecimal::ToPrimitive;
                json!(v.to_f64().unwrap_or(0.0))
            }
            // Optional BigDecimal
            else if let Ok(v) = row.try_get::<Option<BigDecimal>, _>(name) {
                match v {
                    Some(d) => {
                        use bigdecimal::ToPrimitive;
                        json!(d.to_f64().unwrap_or(0.0))
                    }
                    None => Value::Null,
                }
            }
            // Boolean
            else if let Ok(v) = row.try_get::<bool, _>(name) {
                json!(v)
            }
            // Timestamp with timezone
            else if let Ok(v) = row.try_get::<DateTime<Utc>, _>(name) {
                json!(v.to_rfc3339())
            }
            // Optional timestamp with timezone
            else if let Ok(v) = row.try_get::<Option<DateTime<Utc>>, _>(name) {
                match v {
                    Some(dt) => json!(dt.to_rfc3339()),
                    None => Value::Null,
                }
            }
            // Timestamp without timezone
            else if let Ok(v) = row.try_get::<NaiveDateTime, _>(name) {
                json!(v.format("%Y-%m-%dT%H:%M:%S").to_string())
            }
            // Optional timestamp without timezone
            else if let Ok(v) = row.try_get::<Option<NaiveDateTime>, _>(name) {
                match v {
                    Some(dt) => json!(dt.format("%Y-%m-%dT%H:%M:%S").to_string()),
                    None => Value::Null,
                }
            }
            // Date
            else if let Ok(v) = row.try_get::<NaiveDate, _>(name) {
                json!(v.format("%Y-%m-%d").to_string())
            }
            // Optional date
            else if let Ok(v) = row.try_get::<Option<NaiveDate>, _>(name) {
                match v {
                    Some(d) => json!(d.format("%Y-%m-%d").to_string()),
                    None => Value::Null,
                }
            }
            // Time
            else if let Ok(v) = row.try_get::<NaiveTime, _>(name) {
                json!(v.format("%H:%M:%S").to_string())
            }
            // Optional time
            else if let Ok(v) = row.try_get::<Option<NaiveTime>, _>(name) {
                match v {
                    Some(t) => json!(t.format("%H:%M:%S").to_string()),
                    None => Value::Null,
                }
            }
            // UUID
            else if let Ok(v) = row.try_get::<uuid::Uuid, _>(name) {
                json!(v.to_string())
            }
            // Optional UUID
            else if let Ok(v) = row.try_get::<Option<uuid::Uuid>, _>(name) {
                match v {
                    Some(u) => json!(u.to_string()),
                    None => Value::Null,
                }
            }
            // String
            else if let Ok(v) = row.try_get::<String, _>(name) {
                json!(v)
            }
            // JSON/JSONB
            else if let Ok(v) = row.try_get::<serde_json::Value, _>(name) {
                v
            }
            // Optional string (fallback)
            else if let Ok(v) = row.try_get::<Option<String>, _>(name) {
                match v {
                    Some(s) => json!(s),
                    None => Value::Null,
                }
            }
            // String array
            else if let Ok(v) = row.try_get::<Vec<String>, _>(name) {
                json!(v)
            }
            // Optional string array
            else if let Ok(v) = row.try_get::<Option<Vec<String>>, _>(name) {
                match v {
                    Some(arr) => json!(arr),
                    None => Value::Null,
                }
            }
            // Enum: its label
            else if let Some(label) = enum_label(row, name) {
                label
            }
            // Final fallback
            else {
                Value::Null
            };

        obj.insert(name.to_string(), value);
    }

    Value::Object(obj)
}

/// The label of an enum column's value (an enum's binary form is its label), null for NULL; None for another type.
fn enum_label(row: &sqlx::postgres::PgRow, name: &str) -> Option<Value> {
    use sqlx::ValueRef;
    let raw = row.try_get_raw(name).ok()?;
    if !matches!(raw.type_info().kind(), sqlx::postgres::PgTypeKind::Enum(_)) {
        return None;
    }
    if raw.is_null() {
        return Some(Value::Null);
    }
    raw.as_str().ok().map(|s| json!(s))
}
