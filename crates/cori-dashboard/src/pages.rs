//! Page templates for dashboard views.

use crate::state::{SchemaInfo, ColumnInfo};
use crate::templates::{badge, card, empty_state, input, layout, stats_card, tabs};
use cori_core::{RoleConfig, TenancyConfig, CoriConfig};
use std::collections::HashMap;

// =============================================================================
// Home Page
// =============================================================================

pub fn home_page(config: &CoriConfig, role_count: usize, pending_approvals: usize) -> String {
    let stats = format!(
        r##"<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6 mb-8">
            {roles_stat}
            {tables_stat}
            {approvals_stat}
            {mcp_stat}
        </div>"##,
        roles_stat = stats_card("Roles", &role_count.to_string(), "user-shield", "blue"),
        tables_stat = stats_card("Tables", &config.tenancy.tables.len().to_string(), "database", "green"),
        approvals_stat = stats_card("Pending Approvals", &pending_approvals.to_string(), "clock", "yellow"),
        mcp_stat = stats_card("MCP Port", &config.mcp.http_port.to_string(), "server", "purple"),
    );

    let quick_actions = card("Quick Actions", &format!(
        r##"<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
            <a href="/roles/new" class="flex items-center gap-4 p-4 bg-gray-50 dark:bg-gray-700/50 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors">
                <div class="w-10 h-10 rounded-full bg-blue-100 dark:bg-blue-900/30 flex items-center justify-center">
                    <i class="fas fa-plus text-blue-500"></i>
                </div>
                <div>
                    <h4 class="font-medium text-gray-900 dark:text-white">Create New Role</h4>
                    <p class="text-sm text-gray-500 dark:text-gray-400">Define permissions for AI agents</p>
                </div>
            </a>
            <a href="/tokens" class="flex items-center gap-4 p-4 bg-gray-50 dark:bg-gray-700/50 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors">
                <div class="w-10 h-10 rounded-full bg-green-100 dark:bg-green-900/30 flex items-center justify-center">
                    <i class="fas fa-key text-green-500"></i>
                </div>
                <div>
                    <h4 class="font-medium text-gray-900 dark:text-white">Mint Token</h4>
                    <p class="text-sm text-gray-500 dark:text-gray-400">Generate tokens for agents</p>
                </div>
            </a>
            <a href="/schema" class="flex items-center gap-4 p-4 bg-gray-50 dark:bg-gray-700/50 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors">
                <div class="w-10 h-10 rounded-full bg-purple-100 dark:bg-purple-900/30 flex items-center justify-center">
                    <i class="fas fa-database text-purple-500"></i>
                </div>
                <div>
                    <h4 class="font-medium text-gray-900 dark:text-white">Browse Schema</h4>
                    <p class="text-sm text-gray-500 dark:text-gray-400">Explore database tables</p>
                </div>
            </a>
            <a href="/audit" class="flex items-center gap-4 p-4 bg-gray-50 dark:bg-gray-700/50 rounded-lg hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors">
                <div class="w-10 h-10 rounded-full bg-orange-100 dark:bg-orange-900/30 flex items-center justify-center">
                    <i class="fas fa-scroll text-orange-500"></i>
                </div>
                <div>
                    <h4 class="font-medium text-gray-900 dark:text-white">View Audit Logs</h4>
                    <p class="text-sm text-gray-500 dark:text-gray-400">Query execution history</p>
                </div>
            </a>
        </div>"##
    ));

    let connection_info = card("Connection Info", &format!(
        r##"<div class="space-y-4">
            <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-gray-600 dark:text-gray-400">MCP HTTP</span>
                <code class="text-sm bg-gray-200 dark:bg-gray-800 px-2 py-1 rounded">localhost:{mcp_port}</code>
            </div>
            <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-gray-600 dark:text-gray-400">Dashboard</span>
                <code class="text-sm bg-gray-200 dark:bg-gray-800 px-2 py-1 rounded">localhost:{dashboard_port}</code>
            </div>
            <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-gray-600 dark:text-gray-400">MCP Transport</span>
                <code class="text-sm bg-gray-200 dark:bg-gray-800 px-2 py-1 rounded">{mcp_transport}</code>
            </div>
            <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-gray-600 dark:text-gray-400">Upstream DB</span>
                <code class="text-sm bg-gray-200 dark:bg-gray-800 px-2 py-1 rounded">{upstream_host}:{upstream_port}</code>
            </div>
        </div>"##,
        mcp_port = config.mcp.http_port,
        dashboard_port = config.dashboard.listen_port,
        mcp_transport = format!("{:?}", config.mcp.transport).to_lowercase(),
        upstream_host = config.upstream.host,
        upstream_port = config.upstream.port,
    ));

    let content = format!(
        r##"<div class="mb-8">
            <h1 class="text-3xl font-bold text-gray-900 dark:text-white">Welcome to Cori</h1>
            <p class="mt-2 text-gray-600 dark:text-gray-400">
                The MCP server that enables safe, tenant-isolated database access for AI agents.
            </p>
        </div>
        
        {stats}
        
        <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
            {quick_actions}
            {connection_info}
        </div>"##
    );

    layout("Home", &content)
}

// =============================================================================
// Schema Browser Page
// =============================================================================

pub fn schema_browser_page(schema: Option<&SchemaInfo>, tenancy: &TenancyConfig) -> String {
    let content = match schema {
        Some(schema) => {
            let table_list: String = schema.tables.iter().map(|t| {
                let tenant_column = tenancy.tables.get(&t.name)
                    .and_then(|tc| tc.tenant_column.as_ref().or(tc.column.as_ref()))
                    .map(|s| s.as_str())
                    .or(t.detected_tenant_column.as_deref())
                    .unwrap_or("-");
                
                let is_global = tenancy.tables.get(&t.name).map(|tc| tc.global).unwrap_or(false)
                    || tenancy.global_tables.contains(&t.name);
                
                let status_badge = if is_global {
                    badge("Global", "blue")
                } else {
                    badge("Tenant-Scoped", "green")
                };
                
                format!(
                    r##"<div class="border border-gray-200 dark:border-gray-700 rounded-lg overflow-hidden mb-4" x-data="{{ open: false }}">
                        <button @click="open = !open" class="w-full flex items-center justify-between p-4 hover:bg-gray-50 dark:hover:bg-gray-700/50 transition-colors">
                            <div class="flex items-center gap-3">
                                <i class="fas fa-table text-gray-400"></i>
                                <span class="font-medium text-gray-900 dark:text-white">{schema}.{name}</span>
                                {status_badge}
                            </div>
                            <div class="flex items-center gap-4">
                                <span class="text-sm text-gray-500 dark:text-gray-400">{column_count} columns</span>
                                <i class="fas fa-chevron-down transform transition-transform" :class="{{ 'rotate-180': open }}"></i>
                            </div>
                        </button>
                        <div x-show="open" x-collapse>
                            <div class="border-t border-gray-200 dark:border-gray-700 p-4">
                                <div class="mb-4 flex items-center gap-4 text-sm">
                                    <span class="text-gray-500 dark:text-gray-400">Tenant Column:</span>
                                    <code class="bg-gray-100 dark:bg-gray-800 px-2 py-1 rounded">{tenant_column}</code>
                                    {pk_info}
                                </div>
                                <table class="w-full">
                                    <thead>
                                        <tr class="text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">
                                            <th class="pb-2">Column</th>
                                            <th class="pb-2">Type</th>
                                            <th class="pb-2">Nullable</th>
                                            <th class="pb-2">Default</th>
                                        </tr>
                                    </thead>
                                    <tbody class="text-sm">
                                        {columns}
                                    </tbody>
                                </table>
                                {fk_info}
                            </div>
                        </div>
                    </div>"##,
                    schema = t.schema,
                    name = t.name,
                    column_count = t.columns.len(),
                    tenant_column = tenant_column,
                    status_badge = status_badge,
                    pk_info = if !t.primary_key.is_empty() {
                        format!(r#"<span class="text-gray-500 dark:text-gray-400">Primary Key:</span>
                                   <code class="bg-gray-100 dark:bg-gray-800 px-2 py-1 rounded">{}</code>"#,
                            t.primary_key.join(", "))
                    } else {
                        String::new()
                    },
                    columns = t.columns.iter().map(|c| column_row(c, &t.primary_key, tenant_column)).collect::<String>(),
                    fk_info = if !t.foreign_keys.is_empty() {
                        format!(r##"<div class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700">
                            <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Foreign Keys</h4>
                            <ul class="space-y-1 text-sm text-gray-600 dark:text-gray-400">
                                {}
                            </ul>
                        </div>"##,
                        t.foreign_keys.iter().map(|fk| {
                            format!(r#"<li><code class="bg-gray-100 dark:bg-gray-800 px-1 rounded">{}</code> → <code class="bg-gray-100 dark:bg-gray-800 px-1 rounded">{}.{}</code></li>"#,
                                fk.columns.join(", "),
                                fk.references_table,
                                fk.references_columns.join(", "))
                        }).collect::<String>())
                    } else {
                        String::new()
                    }
                )
            }).collect();

            format!(
                r##"<div class="flex items-center justify-between mb-6">
                    <div>
                        <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Schema Browser</h1>
                        <p class="text-gray-600 dark:text-gray-400">Last refreshed: {refreshed_at}</p>
                    </div>
                    <button hx-post="/api/schema/refresh" hx-swap="none" 
                            class="flex items-center gap-2 bg-primary-600 hover:bg-primary-700 text-white px-4 py-2 rounded-lg font-medium transition-colors">
                        <i class="fas fa-sync-alt htmx-indicator animate-spin"></i>
                        <span>Refresh Schema</span>
                    </button>
                </div>
                
                <div class="mb-4">
                    <input type="text" placeholder="Search tables..." 
                           class="w-full md:w-64 px-4 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-gray-900 dark:text-white"
                           x-data x-on:input="filterTables($event.target.value)">
                </div>
                
                <div id="table-list">
                    {table_list}
                </div>
                
                <script>
                    function filterTables(query) {{
                        const tables = document.querySelectorAll('#table-list > div');
                        tables.forEach(t => {{
                            const name = t.querySelector('span.font-medium').textContent.toLowerCase();
                            t.style.display = name.includes(query.toLowerCase()) ? '' : 'none';
                        }});
                    }}
                </script>"##,
                refreshed_at = schema.refreshed_at.format("%Y-%m-%d %H:%M:%S UTC"),
            )
        },
        None => {
            format!(
                r##"<div class="text-center py-12">
                    <i class="fas fa-database text-6xl text-gray-300 dark:text-gray-600 mb-4"></i>
                    <h2 class="text-xl font-medium text-gray-900 dark:text-white mb-2">No Schema Loaded</h2>
                    <p class="text-gray-600 dark:text-gray-400 mb-6">Click below to introspect your database schema.</p>
                    <button hx-post="/api/schema/refresh" hx-swap="none"
                            class="inline-flex items-center gap-2 bg-primary-600 hover:bg-primary-700 text-white px-6 py-3 rounded-lg font-medium transition-colors">
                        <i class="fas fa-database"></i>
                        <span>Load Schema</span>
                    </button>
                </div>"##
            )
        }
    };

    layout("Schema Browser", &content)
}

fn column_row(col: &ColumnInfo, primary_key: &[String], tenant_column: &str) -> String {
    let is_pk = primary_key.contains(&col.name);
    let is_tenant = col.name == tenant_column;
    
    let badges = format!(
        "{}{}",
        if is_pk { format!(r#"<span class="ml-2 text-xs bg-yellow-100 dark:bg-yellow-900/30 text-yellow-800 dark:text-yellow-300 px-1.5 py-0.5 rounded">PK</span>"#) } else { String::new() },
        if is_tenant { format!(r#"<span class="ml-2 text-xs bg-green-100 dark:bg-green-900/30 text-green-800 dark:text-green-300 px-1.5 py-0.5 rounded">Tenant</span>"#) } else { String::new() },
    );
    
    format!(
        r#"<tr class="border-t border-gray-100 dark:border-gray-800">
            <td class="py-2 font-mono text-gray-900 dark:text-white">{name}{badges}</td>
            <td class="py-2 text-gray-600 dark:text-gray-400">{data_type}</td>
            <td class="py-2 text-gray-600 dark:text-gray-400">{nullable}</td>
            <td class="py-2 text-gray-500 dark:text-gray-500 font-mono text-xs">{default}</td>
        </tr>"#,
        name = col.name,
        badges = badges,
        data_type = col.data_type,
        nullable = if col.nullable { "Yes" } else { "No" },
        default = col.default.as_deref().unwrap_or("-"),
    )
}

// =============================================================================
// Roles Page
// =============================================================================

pub fn roles_page(roles: &HashMap<String, RoleConfig>) -> String {
    let content = if roles.is_empty() {
        empty_state(
            "user-shield",
            "No Roles Defined",
            "Create your first role to define what AI agents can access.",
            Some(("Create Role", "/roles/new")),
        )
    } else {
        let role_cards: String = roles.iter().map(|(name, role)| {
            let table_count = role.tables.len();
            let has_editable = role.tables.values().any(|t| !t.editable.is_empty());
            let has_approval = role.tables.values().any(|t| {
                t.editable.values().any(|c| c.requires_approval)
            });
            
            format!(
                r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                    <div class="flex items-start justify-between mb-4">
                        <div>
                            <h3 class="text-lg font-semibold text-gray-900 dark:text-white">{name}</h3>
                            <p class="text-sm text-gray-500 dark:text-gray-400">{description}</p>
                        </div>
                        <div class="flex gap-2">
                            <a href="/roles/{name}/edit" class="p-2 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300">
                                <i class="fas fa-edit"></i>
                            </a>
                            <button hx-delete="/api/roles/{name}" hx-confirm="Delete role '{name}'? This cannot be undone."
                                    hx-swap="none" class="p-2 text-gray-400 hover:text-red-500">
                                <i class="fas fa-trash"></i>
                            </button>
                        </div>
                    </div>
                    
                    <div class="flex flex-wrap gap-2 mb-4">
                        <span class="inline-flex items-center gap-1 text-sm text-gray-600 dark:text-gray-400">
                            <i class="fas fa-table text-xs"></i> {table_count} tables
                        </span>
                        {editable_badge}
                        {approval_badge}
                    </div>
                    
                    <div class="flex flex-wrap gap-1 mb-4">
                        {table_badges}
                    </div>
                    
                    <div class="flex gap-2 pt-4 border-t border-gray-100 dark:border-gray-700">
                        <a href="/roles/{name}" class="flex-1 text-center py-2 text-sm bg-gray-100 dark:bg-gray-700 hover:bg-gray-200 dark:hover:bg-gray-600 rounded-lg transition-colors">
                            View Details
                        </a>
                        <a href="/tokens?role={name}" class="flex-1 text-center py-2 text-sm bg-primary-100 dark:bg-primary-900/30 text-primary-600 dark:text-primary-400 hover:bg-primary-200 dark:hover:bg-primary-900/50 rounded-lg transition-colors">
                            Mint Token
                        </a>
                    </div>
                </div>"##,
                name = name,
                description = role.description.as_deref().unwrap_or("No description"),
                table_count = table_count,
                editable_badge = if has_editable {
                    badge("Read/Write", "blue")
                } else {
                    badge("Read-Only", "gray")
                },
                approval_badge = if has_approval {
                    badge("Requires Approval", "yellow")
                } else {
                    String::new()
                },
                table_badges = {
                    let badges: String = role.tables.keys().take(5).map(|t| {
                        format!(r#"<span class="text-xs bg-gray-100 dark:bg-gray-700 text-gray-600 dark:text-gray-400 px-2 py-1 rounded">{}</span>"#, t)
                    }).collect();
                    let overflow = if table_count > 5 { 
                        format!(r#"<span class="text-xs text-gray-500">+{}</span>"#, table_count - 5) 
                    } else { 
                        String::new() 
                    };
                    badges + &overflow
                },
            )
        }).collect();

        format!(
            r##"<div class="flex items-center justify-between mb-6">
                <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Roles</h1>
                <a href="/roles/new" class="flex items-center gap-2 bg-primary-600 hover:bg-primary-700 text-white px-4 py-2 rounded-lg font-medium transition-colors">
                    <i class="fas fa-plus"></i>
                    <span>New Role</span>
                </a>
            </div>
            
            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
                {role_cards}
            </div>"##
        )
    };

    layout("Roles", &content)
}

// =============================================================================
// Role Editor Page
// =============================================================================

pub fn role_editor_page(role: Option<&RoleConfig>, schema: Option<&SchemaInfo>, is_new: bool) -> String {
    let name = role.map(|r| r.name.as_str()).unwrap_or("");
    let description = role.and_then(|r| r.description.as_deref()).unwrap_or("");
    let max_rows = role.and_then(|r| r.max_rows_per_query).unwrap_or(100);
    let max_affected = role.and_then(|r| r.max_affected_rows).unwrap_or(10);
    
    let title = if is_new { "Create New Role" } else { &format!("Edit Role: {}", name) };
    let submit_url = if is_new { "/api/roles".to_string() } else { format!("/api/roles/{}", name) };
    let method = if is_new { "hx-post" } else { "hx-put" };
    
    let tables_section = if let Some(schema) = schema {
        let table_options: String = schema.tables.iter().map(|t| {
            let full_name = format!("{}.{}", t.schema, t.name);
            let is_selected = role.map(|r| r.tables.contains_key(&t.name)).unwrap_or(false);
            let perms = role.and_then(|r| r.tables.get(&t.name));
            
            format!(
                r##"<div class="border border-gray-200 dark:border-gray-700 rounded-lg p-4 mb-4" x-data="{{ enabled: {enabled}, showColumns: false }}">
                    <div class="flex items-center justify-between mb-3">
                        <label class="flex items-center gap-3">
                            <input type="checkbox" x-model="enabled" name="tables[{name}][enabled]" {checked}
                                   class="w-4 h-4 text-primary-600 rounded border-gray-300 focus:ring-primary-500">
                            <span class="font-medium text-gray-900 dark:text-white">{full_name}</span>
                        </label>
                        <button type="button" @click="showColumns = !showColumns" x-show="enabled"
                                class="text-sm text-primary-600 hover:text-primary-700">
                            <span x-text="showColumns ? 'Hide columns' : 'Configure columns'"></span>
                        </button>
                    </div>
                    
                    <div x-show="enabled && showColumns" x-collapse class="mt-4 space-y-4">
                        <div>
                            <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Operations</label>
                            <div class="flex flex-wrap gap-2">
                                <label class="flex items-center gap-2">
                                    <input type="checkbox" name="tables[{name}][operations][]" value="read" {read_checked}
                                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                                    <span class="text-sm">Read</span>
                                </label>
                                <label class="flex items-center gap-2">
                                    <input type="checkbox" name="tables[{name}][operations][]" value="create" {create_checked}
                                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                                    <span class="text-sm">Create</span>
                                </label>
                                <label class="flex items-center gap-2">
                                    <input type="checkbox" name="tables[{name}][operations][]" value="update" {update_checked}
                                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                                    <span class="text-sm">Update</span>
                                </label>
                                <label class="flex items-center gap-2">
                                    <input type="checkbox" name="tables[{name}][operations][]" value="delete" {delete_checked}
                                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                                    <span class="text-sm">Delete</span>
                                </label>
                            </div>
                        </div>
                        
                        <div>
                            <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Readable Columns</label>
                            <div class="flex flex-wrap gap-2">
                                {readable_columns}
                            </div>
                        </div>
                        
                        <div>
                            <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Editable Columns</label>
                            <div class="flex flex-wrap gap-2">
                                {editable_columns}
                            </div>
                        </div>
                    </div>
                </div>"##,
                name = t.name,
                full_name = full_name,
                enabled = if is_selected { "true" } else { "false" },
                checked = if is_selected { "checked" } else { "" },
                read_checked = if perms.map(|p| p.operations.as_ref().map(|ops| ops.iter().any(|o| matches!(o, cori_core::Operation::Read))).unwrap_or(true)).unwrap_or(false) { "checked" } else { "" },
                create_checked = if perms.map(|p| p.operations.as_ref().map(|ops| ops.iter().any(|o| matches!(o, cori_core::Operation::Create))).unwrap_or(false)).unwrap_or(false) { "checked" } else { "" },
                update_checked = if perms.map(|p| p.operations.as_ref().map(|ops| ops.iter().any(|o| matches!(o, cori_core::Operation::Update))).unwrap_or(false)).unwrap_or(false) { "checked" } else { "" },
                delete_checked = if perms.map(|p| p.operations.as_ref().map(|ops| ops.iter().any(|o| matches!(o, cori_core::Operation::Delete))).unwrap_or(false)).unwrap_or(false) { "checked" } else { "" },
                readable_columns = t.columns.iter().map(|c| {
                    let is_readable = perms.map(|p| p.readable.contains(&c.name)).unwrap_or(false);
                    format!(
                        r#"<label class="flex items-center gap-1.5 text-sm">
                            <input type="checkbox" name="tables[{table}][readable][]" value="{col}" {checked}
                                   class="w-3 h-3 text-primary-600 rounded border-gray-300">
                            <span>{col}</span>
                        </label>"#,
                        table = t.name,
                        col = c.name,
                        checked = if is_readable { "checked" } else { "" },
                    )
                }).collect::<String>(),
                editable_columns = t.columns.iter().map(|c| {
                    let is_editable = perms.map(|p| p.editable.contains(&c.name)).unwrap_or(false);
                    format!(
                        r#"<label class="flex items-center gap-1.5 text-sm">
                            <input type="checkbox" name="tables[{table}][editable][]" value="{col}" {checked}
                                   class="w-3 h-3 text-green-600 rounded border-gray-300">
                            <span>{col}</span>
                        </label>"#,
                        table = t.name,
                        col = c.name,
                        checked = if is_editable { "checked" } else { "" },
                    )
                }).collect::<String>(),
            )
        }).collect();
        
        table_options
    } else {
        r#"<p class="text-gray-500 dark:text-gray-400">Load schema to configure table permissions.</p>"#.to_string()
    };

    let content = format!(
        r##"<div class="mb-6">
            <a href="/roles" class="text-primary-600 hover:text-primary-700 flex items-center gap-2 mb-4">
                <i class="fas fa-arrow-left"></i> Back to Roles
            </a>
            <h1 class="text-2xl font-bold text-gray-900 dark:text-white">{title}</h1>
        </div>
        
        <form {method}="{submit_url}" hx-swap="none" class="space-y-6">
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <h2 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Basic Information</h2>
                
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    {name_input}
                    {description_input}
                </div>
                
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mt-4">
                    {max_rows_input}
                    {max_affected_input}
                </div>
            </div>
            
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <h2 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Table Permissions</h2>
                {tables_section}
            </div>
            
            <div class="flex justify-end gap-4">
                <a href="/roles" class="px-4 py-2 text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700 rounded-lg transition-colors">
                    Cancel
                </a>
                <button type="submit" class="px-6 py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium transition-colors">
                    {submit_text}
                </button>
            </div>
        </form>"##,
        title = title,
        method = method,
        submit_url = submit_url,
        name_input = input("name", "Role Name", "text", name, "e.g., support_agent"),
        description_input = input("description", "Description", "text", description, "What this role is for"),
        max_rows_input = input("max_rows_per_query", "Max Rows per Query", "number", &max_rows.to_string(), "100"),
        max_affected_input = input("max_affected_rows", "Max Affected Rows", "number", &max_affected.to_string(), "10"),
        tables_section = tables_section,
        submit_text = if is_new { "Create Role" } else { "Save Changes" },
    );

    layout(title, &content)
}

// =============================================================================
// Role Detail Page with MCP Preview
// =============================================================================

pub fn role_detail_page(role: &RoleConfig, mcp_tools: Option<&[serde_json::Value]>) -> String {
    let tables_content: String = role.tables.iter().map(|(name, perms)| {
        let ops = perms.operations.as_ref().map(|ops| {
            ops.iter().map(|o| format!("{:?}", o).to_lowercase()).collect::<Vec<_>>().join(", ")
        }).unwrap_or_else(|| "read".to_string());
        
        let readable = match &perms.readable {
            cori_core::ReadableColumns::All(_) => "*".to_string(),
            cori_core::ReadableColumns::List(cols) => cols.join(", "),
        };
        
        let editable: String = perms.editable.iter().map(|(col, constraints)| {
            let mut badges = String::new();
            if constraints.requires_approval {
                badges.push_str(&badge("Approval", "yellow"));
            }
            if constraints.allowed_values.is_some() {
                badges.push_str(&badge("Enum", "blue"));
            }
            format!(r#"<span class="mr-2">{}{}</span>"#, col, badges)
        }).collect();

        format!(
            r##"<div class="border border-gray-200 dark:border-gray-700 rounded-lg p-4">
                <div class="flex items-center justify-between mb-2">
                    <h4 class="font-medium text-gray-900 dark:text-white">{name}</h4>
                    <span class="text-sm text-gray-500 dark:text-gray-400">{ops}</span>
                </div>
                <div class="text-sm space-y-2">
                    <div>
                        <span class="text-gray-500 dark:text-gray-400">Readable:</span>
                        <span class="text-gray-700 dark:text-gray-300 ml-2 font-mono">{readable}</span>
                    </div>
                    <div>
                        <span class="text-gray-500 dark:text-gray-400">Editable:</span>
                        <span class="text-gray-700 dark:text-gray-300 ml-2">{editable}</span>
                    </div>
                </div>
            </div>"##,
            name = name,
            ops = ops,
            readable = if readable.is_empty() { "-" } else { &readable },
            editable = if editable.is_empty() { "-".to_string() } else { editable },
        )
    }).collect();

    let mcp_preview = match mcp_tools {
        Some(tools) if !tools.is_empty() => {
            let tool_list: String = tools.iter().map(|tool| {
                let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                let desc = tool.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let annotations = tool.get("annotations").cloned().unwrap_or(serde_json::json!({}));
                let read_only = annotations.get("readOnly").and_then(|v| v.as_bool()).unwrap_or(false);
                let requires_approval = annotations.get("requiresApproval").and_then(|v| v.as_bool()).unwrap_or(false);
                
                let schema_json = tool.get("inputSchema")
                    .map(|s| serde_json::to_string_pretty(s).unwrap_or_default())
                    .unwrap_or_default();
                
                format!(
                    r##"<div class="border border-gray-200 dark:border-gray-700 rounded-lg p-4 mb-3" x-data="{{ showSchema: false }}">
                        <div class="flex items-center justify-between">
                            <div class="flex items-center gap-2">
                                <span class="font-mono text-primary-600 dark:text-primary-400">{name}</span>
                                {read_only_badge}
                                {approval_badge}
                            </div>
                            <button type="button" @click="showSchema = !showSchema" class="text-sm text-gray-500 hover:text-gray-700">
                                <i class="fas" :class="showSchema ? 'fa-chevron-up' : 'fa-chevron-down'"></i>
                            </button>
                        </div>
                        <p class="text-sm text-gray-500 dark:text-gray-400 mt-1">{desc}</p>
                        <div x-show="showSchema" x-collapse class="mt-3">
                            <pre class="bg-gray-900 text-gray-100 p-3 rounded text-xs overflow-x-auto"><code>{schema}</code></pre>
                        </div>
                    </div>"##,
                    name = name,
                    desc = desc,
                    read_only_badge = if read_only { badge("Read-Only", "gray") } else { badge("Mutation", "blue") },
                    approval_badge = if requires_approval { badge("Requires Approval", "yellow") } else { String::new() },
                    schema = schema_json.replace('<', "&lt;").replace('>', "&gt;"),
                )
            }).collect();
            
            format!(
                r##"<div class="space-y-3">
                    <p class="text-sm text-gray-600 dark:text-gray-400 mb-4">
                        These MCP tools will be available to AI agents using this role:
                    </p>
                    {tool_list}
                </div>"##
            )
        },
        _ => r#"<p class="text-gray-500 dark:text-gray-400">No MCP tools generated. Configure table permissions to generate tools.</p>"#.to_string(),
    };

    let content = format!(
        r##"<div class="mb-6">
            <a href="/roles" class="text-primary-600 hover:text-primary-700 flex items-center gap-2 mb-4">
                <i class="fas fa-arrow-left"></i> Back to Roles
            </a>
            <div class="flex items-center justify-between">
                <div>
                    <h1 class="text-2xl font-bold text-gray-900 dark:text-white">{name}</h1>
                    <p class="text-gray-600 dark:text-gray-400">{description}</p>
                </div>
                <div class="flex gap-2">
                    <a href="/roles/{name}/edit" class="px-4 py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium transition-colors">
                        <i class="fas fa-edit mr-2"></i>Edit
                    </a>
                    <a href="/tokens?role={name}" class="px-4 py-2 bg-green-600 hover:bg-green-700 text-white rounded-lg font-medium transition-colors">
                        <i class="fas fa-key mr-2"></i>Mint Token
                    </a>
                </div>
            </div>
        </div>
        
        {tabs}"##,
        name = role.name,
        description = role.description.as_deref().unwrap_or("No description"),
        tabs = tabs("role-tabs", &[
            ("permissions", "Permissions", &format!(
                r##"<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <div class="bg-white dark:bg-gray-800 rounded-xl p-6 border border-gray-200 dark:border-gray-700">
                        <h3 class="text-lg font-semibold mb-4">Constraints</h3>
                        <div class="space-y-3 text-sm">
                            <div class="flex justify-between">
                                <span class="text-gray-500">Max rows per query</span>
                                <span class="font-medium">{max_rows}</span>
                            </div>
                            <div class="flex justify-between">
                                <span class="text-gray-500">Max affected rows</span>
                                <span class="font-medium">{max_affected}</span>
                            </div>
                            <div class="flex justify-between">
                                <span class="text-gray-500">Blocked tables</span>
                                <span class="font-medium">{blocked_tables}</span>
                            </div>
                        </div>
                    </div>
                    <div class="bg-white dark:bg-gray-800 rounded-xl p-6 border border-gray-200 dark:border-gray-700">
                        <h3 class="text-lg font-semibold mb-4">Tables</h3>
                        <div class="space-y-3">{tables_content}</div>
                    </div>
                </div>"##,
                max_rows = role.max_rows_per_query.unwrap_or(100),
                max_affected = role.max_affected_rows.unwrap_or(10),
                blocked_tables = if role.blocked_tables.is_empty() { "None".to_string() } else { role.blocked_tables.join(", ") },
                tables_content = tables_content,
            )),
            ("mcp", "MCP Tools Preview", &mcp_preview),
        ]),
    );

    layout(&format!("Role: {}", role.name), &content)
}

// Continued in next file...
