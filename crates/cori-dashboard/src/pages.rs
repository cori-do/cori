//! Page templates for dashboard views.

use crate::state::{ColumnInfo, SchemaInfo};
use crate::templates::{badge, card, empty_state, input, layout, stats_card, tabs};
use cori_core::CoriConfig;
use cori_core::config::role_definition::RoleDefinition;
use cori_core::config::rules_definition::RulesDefinition;
use std::collections::HashMap;

// =============================================================================
// Home Page
// =============================================================================

pub fn home_page(config: &CoriConfig, role_count: usize, pending_approvals: usize) -> String {
    let table_count = config.rules.as_ref().map(|r| r.tables.len()).unwrap_or(0);
    let stats = format!(
        r##"<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6 mb-8">
            {roles_stat}
            {tables_stat}
            {approvals_stat}
            {mcp_stat}
        </div>"##,
        roles_stat = stats_card("Roles", &role_count.to_string(), "user-shield", "blue"),
        tables_stat = stats_card("Tables", &table_count.to_string(), "database", "green"),
        approvals_stat = stats_card(
            "Pending Approvals",
            &pending_approvals.to_string(),
            "clock",
            "yellow"
        ),
        mcp_stat = stats_card(
            "MCP Port",
            &config.mcp.get_port().to_string(),
            "server",
            "purple"
        ),
    );

    let quick_actions = card(
        "Quick Actions",
        &format!(
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
        ),
    );

    let connection_info = card(
        "Connection Info",
        &format!(
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
            mcp_port = config.mcp.get_port(),
            dashboard_port = config.dashboard.get_port(),
            mcp_transport = format!("{:?}", config.mcp.transport).to_lowercase(),
            upstream_host = config.upstream.host,
            upstream_port = config.upstream.port,
        ),
    );

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

pub fn schema_browser_page(schema: Option<&SchemaInfo>, rules: Option<&RulesDefinition>) -> String {
    let content = match schema {
        Some(schema) => {
            let table_list: String = schema.tables.iter().map(|t| {
                // Get tenant column from RulesDefinition
                let tenant_column = rules
                    .and_then(|r| r.get_table_rules(&t.name))
                    .and_then(|tr| tr.get_direct_tenant_column())
                    .or(t.detected_tenant_column.as_deref())
                    .unwrap_or("-");

                // Check if table is global
                let is_global = rules
                    .and_then(|r| r.get_table_rules(&t.name))
                    .map(|tr| tr.global.unwrap_or(false))
                    .unwrap_or(false);

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
        }
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
        if is_pk {
            format!(
                r#"<span class="ml-2 text-xs bg-yellow-100 dark:bg-yellow-900/30 text-yellow-800 dark:text-yellow-300 px-1.5 py-0.5 rounded">PK</span>"#
            )
        } else {
            String::new()
        },
        if is_tenant {
            format!(
                r#"<span class="ml-2 text-xs bg-green-100 dark:bg-green-900/30 text-green-800 dark:text-green-300 px-1.5 py-0.5 rounded">Tenant</span>"#
            )
        } else {
            String::new()
        },
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

pub fn roles_page(roles: &HashMap<String, RoleDefinition>) -> String {
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
            let has_write = role.tables.values().any(|t| {
                !t.creatable.is_empty() || !t.updatable.is_empty()
            });
            let has_approval = role.table_requires_approval(name);

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
                        {write_badge}
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
                write_badge = if has_write {
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

pub fn role_editor_page(
    role: Option<&RoleDefinition>,
    schema: Option<&SchemaInfo>,
    is_new: bool,
) -> String {
    let name = role.map(|r| r.name.as_str()).unwrap_or("");
    let description = role.and_then(|r| r.description.as_deref()).unwrap_or("");

    let title = if is_new {
        "Create New Role"
    } else {
        &format!("Edit Role: {}", name)
    };
    let submit_url = if is_new {
        "/api/roles".to_string()
    } else {
        format!("/api/roles/{}", name)
    };
    let method = if is_new { "hx-post" } else { "hx-put" };

    let tables_section = if let Some(schema) = schema {
        let table_options: String = schema.tables.iter().map(|t| {
            let full_name = format!("{}.{}", t.schema, t.name);
            let is_selected = role.map(|r| r.tables.contains_key(&t.name)).unwrap_or(false);
            let perms = role.and_then(|r| r.tables.get(&t.name));

            // Derive checked state from permissions
            let read_checked = perms.map(|p| !p.readable.is_empty()).unwrap_or(false);
            let create_checked = perms.map(|p| !p.creatable.is_empty()).unwrap_or(false);
            let update_checked = perms.map(|p| !p.updatable.is_empty()).unwrap_or(false);
            let delete_checked = perms.map(|p| p.deletable.is_allowed()).unwrap_or(false);

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
                                    <input type="checkbox" name="tables[{name}][operations][]" value="read" {read_checked_attr}
                                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                                    <span class="text-sm">Read</span>
                                </label>
                                <label class="flex items-center gap-2">
                                    <input type="checkbox" name="tables[{name}][operations][]" value="create" {create_checked_attr}
                                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                                    <span class="text-sm">Create</span>
                                </label>
                                <label class="flex items-center gap-2">
                                    <input type="checkbox" name="tables[{name}][operations][]" value="update" {update_checked_attr}
                                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                                    <span class="text-sm">Update</span>
                                </label>
                                <label class="flex items-center gap-2">
                                    <input type="checkbox" name="tables[{name}][operations][]" value="delete" {delete_checked_attr}
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
                            <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Writable Columns</label>
                            <div class="flex flex-wrap gap-2">
                                {writable_columns}
                            </div>
                        </div>
                    </div>
                </div>"##,
                name = t.name,
                full_name = full_name,
                enabled = if is_selected { "true" } else { "false" },
                checked = if is_selected { "checked" } else { "" },
                read_checked_attr = if read_checked { "checked" } else { "" },
                create_checked_attr = if create_checked { "checked" } else { "" },
                update_checked_attr = if update_checked { "checked" } else { "" },
                delete_checked_attr = if delete_checked { "checked" } else { "" },
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
                writable_columns = t.columns.iter().map(|c| {
                    // Check if column is in creatable OR updatable
                    let is_writable = perms.map(|p| {
                        p.creatable.contains(&c.name) || p.updatable.contains(&c.name)
                    }).unwrap_or(false);
                    format!(
                        r#"<label class="flex items-center gap-1.5 text-sm">
                            <input type="checkbox" name="tables[{table}][writable][]" value="{col}" {checked}
                                   class="w-3 h-3 text-green-600 rounded border-gray-300">
                            <span>{col}</span>
                        </label>"#,
                        table = t.name,
                        col = c.name,
                        checked = if is_writable { "checked" } else { "" },
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
        description_input = input(
            "description",
            "Description",
            "text",
            description,
            "What this role is for"
        ),
        tables_section = tables_section,
        submit_text = if is_new {
            "Create Role"
        } else {
            "Save Changes"
        },
    );

    layout(title, &content)
}

// =============================================================================
// Role Detail Page with MCP Preview
// =============================================================================

pub fn role_detail_page(role: &RoleDefinition, mcp_tools: Option<&[serde_json::Value]>) -> String {
    use cori_core::config::role_definition::{CreatableColumns, ReadableConfig, UpdatableColumns};

    let tables_content: String = role.tables.iter().map(|(name, perms)| {
        let mut ops = Vec::new();
        if !perms.readable.is_empty() { ops.push("read"); }
        if !perms.creatable.is_empty() { ops.push("create"); }
        if !perms.updatable.is_empty() { ops.push("update"); }
        if perms.deletable.is_allowed() { ops.push("delete"); }
        let ops_str = ops.join(", ");

        let readable = match &perms.readable {
            ReadableConfig::All(_) => "*".to_string(),
            ReadableConfig::List(cols) => cols.join(", "),
            ReadableConfig::Config(cfg) => {
                if cfg.columns.is_all() {
                    "*".to_string()
                } else {
                    cfg.columns.as_list().map(|s| s.join(", ")).unwrap_or_default()
                }
            }
        };

        // Combine creatable and updatable for display
        let mut editable_parts: Vec<String> = Vec::new();

        if let CreatableColumns::Map(cols) = &perms.creatable {
            for (col, constraints) in cols {
                let mut badges = String::new();
                if constraints.requires_approval.is_some() {
                    badges.push_str(&badge("Approval", "yellow"));
                }
                if constraints.restrict_to.is_some() {
                    badges.push_str(&badge("Enum", "blue"));
                }
                editable_parts.push(format!(r#"<span class="mr-2">{} (create){}</span>"#, col, badges));
            }
        }

        if let UpdatableColumns::Map(cols) = &perms.updatable {
            for (col, constraints) in cols {
                let mut badges = String::new();
                if constraints.requires_approval.is_some() {
                    badges.push_str(&badge("Approval", "yellow"));
                }
                // Check if there's an only_when with restricted values
                if constraints.only_when.as_ref()
                    .and_then(|ow| ow.get_new_value_restriction(col))
                    .is_some() {
                    badges.push_str(&badge("Enum", "blue"));
                }
                editable_parts.push(format!(r#"<span class="mr-2">{} (update){}</span>"#, col, badges));
            }
        }

        let editable = editable_parts.join("");

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
                        <span class="text-gray-500 dark:text-gray-400">Writable:</span>
                        <span class="text-gray-700 dark:text-gray-300 ml-2">{editable}</span>
                    </div>
                </div>
            </div>"##,
            name = name,
            ops = if ops_str.is_empty() { "none" } else { &ops_str },
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
        tabs = tabs(
            "role-tabs",
            &[
                (
                    "permissions",
                    "Permissions",
                    &format!(
                        r##"<div class="bg-white dark:bg-gray-800 rounded-xl p-6 border border-gray-200 dark:border-gray-700">
                    <h3 class="text-lg font-semibold mb-4">Tables</h3>
                    <div class="space-y-3">{tables_content}</div>
                </div>"##,
                        tables_content = tables_content,
                    )
                ),
                ("mcp", "MCP Tools Preview", &mcp_preview),
            ]
        ),
    );

    layout(&format!("Role: {}", role.name), &content)
}

// Continued in next file...
