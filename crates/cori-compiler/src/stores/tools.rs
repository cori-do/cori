// SPDX-License-Identifier: Apache-2.0
//! The typed tools a Cori Store exposes to a role (`getItem`, `listItems`, `createItem`, `updateItem`, `deleteItem`),
//! derived offline from the migrations and the role files: `cori scaffold`, `cori check` and `cori store tools` read
//! them without a database. The runtime's own generator (`cori_store_core::tool_generator`) makes them, so what the
//! compiler types is what the store serves; the compiler adds an output schema per tool for `cori scaffold`.
use cori_store_core::RoleDefinition;
use cori_store_core::config::{DatabaseEngine, DatabaseInfo, SchemaDefinition, TableSchema};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Tool definitions (`tools/list` shape: name, description, inputSchema, annotations) for one role, tables in name
/// order, get/list/create/update/delete within a table. Role tables the migrations do not create are skipped
/// (`cori check` reports them).
pub fn generate(tables: &BTreeMap<String, TableSchema>, role: &RoleDefinition) -> Vec<Value> { generate_inner(tables, role, false) }

/// `generate` plus an `outputSchema` per tool: what the Store returns to a step, typed from the role's readable columns
/// (a Cori addition for `cori scaffold`; the runtime publishes none). get/create/update → the row, list → `{ data: [row],
/// count, limit, offset }`, delete → `{ deleted, … }`; when the role's change may park for approval, the row fields are
/// optional and `pending_approval`/`approval_id` appear.
pub fn generate_typed(tables: &BTreeMap<String, TableSchema>, role: &RoleDefinition) -> Vec<Value> { generate_inner(tables, role, true) }

fn generate_inner(tables: &BTreeMap<String, TableSchema>, role: &RoleDefinition, typed: bool) -> Vec<Value> {
    let def = SchemaDefinition { version: "1".into(), captured_at: String::new(), database: DatabaseInfo { engine: DatabaseEngine::Postgres, version: None }, extensions: vec![], enums: vec![], tables: tables.values().cloned().collect() };
    cori_store_core::tool_generator::generate_tools(&def, role).into_iter().map(|t| {
        let mut v = serde_json::to_value(&t).unwrap_or(Value::Null);
        if typed {
            let table = cori_store_core::tool_generator::tool_table(role, &t.name).map(|(_, table)| table);
            if let Some((table, ts)) = table.and_then(|table| tables.get(table).map(|ts| (table, ts))) { v["outputSchema"] = output_schema(role, table, ts, &t.name); }
        }
        v
    }).collect()
}

/// Whether a generated tool only reads (`get*`, `list*`).
pub fn is_read(tool: &Value) -> bool { tool["annotations"]["readOnly"].as_bool() == Some(true) }

/// The row a role reads: readable columns with their JSON type, `null` allowed on nullable columns. No `format`:
/// timestamps come back as RFC 3339 with an offset, which a strict `datetime()` would reject.
fn row_schema(role: &RoleDefinition, t: &str, ts: &TableSchema, optional: bool) -> Value {
    let mut props = Map::new();
    for c in &ts.columns {
        if !role.can_read_column(t, &c.name) { continue; }
        let dt = c.native_type.clone().unwrap_or_else(|| format!("{:?}", c.column_type).to_lowercase());
        let mut p = match json_type(&dt) { "object" => json!({}), ty if c.nullable => json!({ "type": [ty, "null"] }), ty => json!({ "type": ty }) };
        if ty_is_array(&p) { p["items"] = json!({}); }
        props.insert(c.name.clone(), p);
    }
    let required: Vec<&String> = if optional { vec![] } else { props.keys().collect() };
    json!({ "type": "object", "properties": props, "required": required })
}

fn ty_is_array(p: &Value) -> bool { p["type"] == "array" || p["type"].as_array().map(|a| a.iter().any(|x| x == "array")).unwrap_or(false) }

fn output_schema(role: &RoleDefinition, t: &str, ts: &TableSchema, tool: &str) -> Value {
    let pending = |mut o: Value| { o["properties"]["pending_approval"] = json!({ "type": "boolean" }); o["properties"]["approval_id"] = json!({ "type": "string" }); o };
    let parks = role.table_requires_approval(t);
    if tool.starts_with("list") { json!({ "type": "object", "properties": { "data": { "type": "array", "items": row_schema(role, t, ts, false) }, "count": { "type": "number" }, "limit": { "type": "number" }, "offset": { "type": "number" } }, "required": ["data", "count", "limit", "offset"] }) }
    else if tool.starts_with("get") { row_schema(role, t, ts, false) }
    else if tool.starts_with("delete") {
        let deletes_park = role.tables.get(t).map(|p| p.deletable.requires_approval()).unwrap_or(false);
        let o = json!({ "type": "object", "properties": { "deleted": {} }, "required": if deletes_park { json!([]) } else { json!(["deleted"]) }, "additionalProperties": true });
        if deletes_park { pending(o) } else { o }
    }
    else if parks { pending(row_schema(role, t, ts, true)) }
    else { row_schema(role, t, ts, false) }
}

fn json_type(dt: &str) -> &'static str {
    let dt = dt.to_lowercase();
    if dt.contains("int") || dt.contains("serial") { "integer" }
    else if dt.contains("numeric") || dt.contains("decimal") || dt.contains("float") || dt.contains("double") || dt.contains("real") { "number" }
    else if dt.contains("bool") { "boolean" }
    else if dt.contains("json") { "object" }
    else if dt.contains("array") { "array" }
    else { "string" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_follow_the_role_files() {
        let sql = "CREATE TABLE customers (id SERIAL PRIMARY KEY, email TEXT NOT NULL, name VARCHAR(80), meta JSONB, tags TEXT[]);
            CREATE TABLE tickets (ticket_number TEXT PRIMARY KEY DEFAULT gen_random_uuid()::text, customer_id INT NOT NULL REFERENCES customers(id), status TEXT NOT NULL DEFAULT 'open', priority INT, amount NUMERIC(10,2), done BOOLEAN NOT NULL, due DATE, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), owner_uuid UUID);
            CREATE TABLE audit_logs (at TIMESTAMP, message TEXT);
            CREATE TABLE order_lines (order_id BIGINT NOT NULL, line INT NOT NULL, qty INT NOT NULL, PRIMARY KEY (order_id, line));
            ALTER TABLE tickets ADD COLUMN note TEXT;";
        let mut w = Vec::new();
        let schema = super::super::schema_of(&[("0001.sql".into(), sql.into())], &mut w, "s");
        assert!(w.is_empty(), "{w:?}");
        let roles = [
            "name: agent\ntables:\n  customers:\n    readable: '*'\n  tickets:\n    readable: { columns: [ticket_number, status, customer_id, created_at, amount], max_per_page: 20 }\n    creatable:\n      customer_id: { required: true, foreign_key: { verify_with: [email] } }\n      status: { restrict_to: [open], default: open, guidance: New tickets start open }\n      priority: { requires_approval: true }\n      ghost: { required: true }\n    updatable:\n      status:\n        only_when: { new.status: [open, closed] }\n      note: { guidance: Free text }\n    deletable: { verify_with: [status], requires_approval: true }\n  audit_logs:\n    readable: [at, message]\n    creatable: '*'\n    updatable: '*'\n    deletable: true\n  order_lines:\n    readable: '*'\n    creatable: '*'\n    updatable: '*'\n    deletable: true\n",
            "name: viewer\ntables:\n  tickets:\n    readable: [ticket_number, status]\n",
        ];
        let names: Vec<String> = generate(&schema.defs, &RoleDefinition::from_yaml(roles[0]).unwrap()).iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["listAuditLogs", "createAuditLog", "getCustomer", "listCustomers", "getOrderLine", "listOrderLines", "createOrderLine", "updateOrderLine", "deleteOrderLine", "getTicket", "listTickets", "createTicket", "updateTicket", "deleteTicket"]);
        let viewer = generate_typed(&schema.defs, &RoleDefinition::from_yaml(roles[1]).unwrap());
        assert_eq!(viewer.iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), ["getTicket", "listTickets"]);
        assert!(viewer.iter().all(is_read));
        assert_eq!(viewer[0]["outputSchema"]["properties"].as_object().unwrap().keys().collect::<Vec<_>>(), ["status", "ticket_number"]);
        // a table the migrations do not create gets no tool (and no panic): `cori check` reports it
        let ghost = RoleDefinition::from_yaml("name: g\ntables:\n  nowhere:\n    readable: '*'\n  tickets:\n    readable: [status]\n").unwrap();
        assert_eq!(generate(&schema.defs, &ghost).iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(), ["getTicket", "listTickets"]);
    }
}
