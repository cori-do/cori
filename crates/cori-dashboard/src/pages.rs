//! Page templates for dashboard views.

use crate::state::{ColumnInfo, SchemaInfo};
use crate::templates::{badge, card, empty_state, input, layout, stats_card};
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
        </div>"##,
    );

    let connection_info = card(
        "Connection Info",
        &format!(
            r##"<div class="space-y-4">
            <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-gray-600 dark:text-gray-400">MCP HTTP</span>
                <code class="text-sm bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-300 px-2 py-1 rounded">localhost:{mcp_port}</code>
            </div>
            <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-gray-600 dark:text-gray-400">Dashboard</span>
                <code class="text-sm bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-300 px-2 py-1 rounded">localhost:{dashboard_port}</code>
            </div>
            <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-gray-600 dark:text-gray-400">MCP Transport</span>
                <code class="text-sm bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-300 px-2 py-1 rounded">{mcp_transport}</code>
            </div>
            <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-gray-600 dark:text-gray-400">Upstream DB</span>
                <code class="text-sm bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-300 px-2 py-1 rounded">{upstream_host}:{upstream_port}</code>
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
                                <i class="fas fa-chevron-down text-gray-500 dark:text-gray-400 transform transition-transform" :class="{{ 'rotate-180': open }}"></i>
                            </div>
                        </button>
                        <div x-show="open" x-collapse>
                            <div class="border-t border-gray-200 dark:border-gray-700 p-4">
                                <div class="mb-4 flex items-center gap-4 text-sm">
                                    <span class="text-gray-500 dark:text-gray-400">Tenant Column:</span>
                                    <code class="bg-gray-100 dark:bg-gray-800 text-gray-900 dark:text-gray-300 px-2 py-1 rounded">{tenant_column}</code>
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
                                   <code class="bg-gray-100 dark:bg-gray-800 text-gray-900 dark:text-gray-300 px-2 py-1 rounded">{}</code>"#,
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
                            format!(r#"<li><code class="bg-gray-100 dark:bg-gray-800 text-gray-900 dark:text-gray-300 px-1 rounded">{}</code> → <code class="bg-gray-100 dark:bg-gray-800 text-gray-900 dark:text-gray-300 px-1 rounded">{}.{}</code></li>"#,
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
            r##"<div class="text-center py-12">
                    <i class="fas fa-database text-6xl text-gray-300 dark:text-gray-600 mb-4"></i>
                    <h2 class="text-xl font-medium text-gray-900 dark:text-white mb-2">No Schema Loaded</h2>
                    <p class="text-gray-600 dark:text-gray-400 mb-6">Click below to introspect your database schema.</p>
                    <button hx-post="/api/schema/refresh" hx-swap="none"
                            class="inline-flex items-center gap-2 bg-primary-600 hover:bg-primary-700 text-white px-6 py-3 rounded-lg font-medium transition-colors">
                        <i class="fas fa-database"></i>
                        <span>Load Schema</span>
                    </button>
                </div>"##.to_string()
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
            r#"<span class="ml-2 text-xs bg-yellow-100 dark:bg-yellow-900/30 text-yellow-800 dark:text-yellow-300 px-1.5 py-0.5 rounded">PK</span>"#.to_string()
        } else {
            String::new()
        },
        if is_tenant {
            r#"<span class="ml-2 text-xs bg-green-100 dark:bg-green-900/30 text-green-800 dark:text-green-300 px-1.5 py-0.5 rounded">Tenant</span>"#.to_string()
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
                        <a href="/roles/{name}" class="flex-1 text-center py-2 text-sm bg-gray-100 dark:bg-gray-700 text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-600 rounded-lg transition-colors">
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
    rules: Option<&RulesDefinition>,
    is_new: bool,
) -> String {
    let name = role.map(|r| r.name.as_str()).unwrap_or("");
    let description = role.and_then(|r| r.description.as_deref()).unwrap_or("");
    let approval_group = role
        .and_then(|r| r.approvals.as_ref())
        .map(|a| a.group.as_str())
        .unwrap_or("");

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

    // Serialize existing role tables to JSON for the editor
    let tables_json = role
        .map(|r| role_tables_to_json(&r.tables))
        .unwrap_or_else(|| "{}".to_string());

    // Build available columns info from schema with tenant info from rules
    let schema_info_json = schema
        .map(|s| schema_to_json(s, rules))
        .unwrap_or_else(|| "{}".to_string());

    // Escape JSON for HTML attributes
    let tables_json_escaped = html_escape::encode_double_quoted_attribute(&tables_json);
    let schema_info_json_escaped = html_escape::encode_double_quoted_attribute(&schema_info_json);

    let content = format!(
        r##"<div class="mb-6">
            <a href="/roles" class="text-primary-600 hover:text-primary-700 flex items-center gap-2 mb-4">
                <i class="fas fa-arrow-left"></i> Back to Roles
            </a>
            <h1 class="text-2xl font-bold text-gray-900 dark:text-white">{title}</h1>
            <p class="text-gray-500 dark:text-gray-400 mt-1">Define what tables and columns this role can access</p>
        </div>

        <form {method}="{submit_url}" hx-swap="none" class="space-y-6"
              x-data="roleEditor()"
              data-tables="{tables_json_escaped}"
              data-schema="{schema_info_json_escaped}">
            
            <!-- Basic Information -->
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <h2 class="text-lg font-semibold text-gray-900 dark:text-white mb-4 flex items-center gap-2">
                    <i class="fas fa-id-card text-primary-500"></i> Basic Information
                </h2>

                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    {name_input}
                    {description_input}
                </div>
                
                <div class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700">
                    <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">
                        Default Approval Group <span class="text-gray-400 font-normal">(optional)</span>
                    </label>
                    <div class="flex items-center gap-2">
                        <i class="fas fa-users text-gray-400"></i>
                        <input type="text" name="approval_group" value="{approval_group}"
                               class="flex-1 px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700 text-gray-900 dark:text-white"
                               placeholder="e.g., support_managers">
                    </div>
                    <p class="text-xs text-gray-500 mt-1">Group that must approve sensitive operations</p>
                </div>
            </div>

            <!-- Table Permissions -->
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <div class="flex items-center justify-between mb-6">
                    <div>
                        <h2 class="text-lg font-semibold text-gray-900 dark:text-white flex items-center gap-2">
                            <i class="fas fa-database text-primary-500"></i> Table Permissions
                        </h2>
                        <p class="text-sm text-gray-500 mt-1">Select tables and configure access permissions</p>
                    </div>
                    <button type="button" @click="showTableSelector = true" 
                            class="inline-flex items-center gap-2 bg-primary-600 hover:bg-primary-700 text-white px-4 py-2 rounded-lg font-medium transition-colors">
                        <i class="fas fa-plus"></i> Add Table
                    </button>
                </div>

                <!-- Table selector modal -->
                <div x-show="showTableSelector" x-cloak x-transition:enter="transition ease-out duration-200"
                     x-transition:enter-start="opacity-0" x-transition:enter-end="opacity-100"
                     class="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4"
                     @click.self="showTableSelector = false" @keydown.escape.window="showTableSelector = false">
                    <div class="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-lg w-full max-h-[80vh] flex flex-col"
                         @click.stop>
                        <div class="p-4 border-b border-gray-200 dark:border-gray-700">
                            <h3 class="text-lg font-semibold text-gray-900 dark:text-white">Add Table</h3>
                            <p class="text-sm text-gray-500">Select a table to configure permissions</p>
                        </div>
                        <div class="flex-1 overflow-y-auto p-4">
                            <template x-if="availableTables.length === 0">
                                <div class="text-center py-8 text-gray-500">
                                    <i class="fas fa-check-circle text-3xl text-green-500 mb-2"></i>
                                    <p>All tables have been added</p>
                                </div>
                            </template>
                            <div class="space-y-2">
                                <template x-for="table in availableTables" :key="table.name">
                                    <button type="button" @click="selectTable(table.name)"
                                            class="w-full text-left p-4 rounded-lg hover:bg-primary-50 dark:hover:bg-primary-900/20 border border-gray-200 dark:border-gray-700 hover:border-primary-300 dark:hover:border-primary-600 transition-colors group">
                                        <div class="flex items-center justify-between">
                                            <div class="flex items-center gap-3">
                                                <i class="fas fa-table text-gray-400 group-hover:text-primary-500"></i>
                                                <span class="font-medium text-gray-900 dark:text-white" x-text="table.name"></span>
                                            </div>
                                            <span class="text-sm text-gray-400" x-text="table.columns.length + ' columns'"></span>
                                        </div>
                                    </button>
                                </template>
                            </div>
                        </div>
                        <div class="p-4 border-t border-gray-200 dark:border-gray-700 flex justify-end">
                            <button type="button" @click="showTableSelector = false"
                                    class="px-4 py-2 text-gray-600 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 font-medium">
                                Cancel
                            </button>
                        </div>
                    </div>
                </div>

                <!-- Column constraints modal -->
                <div x-show="editingColumn" x-cloak x-transition:enter="transition ease-out duration-200"
                     x-transition:enter-start="opacity-0" x-transition:enter-end="opacity-100"
                     class="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4"
                     @click.self="editingColumn = null" @keydown.escape.window="editingColumn = null">
                    <div class="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-2xl w-full max-h-[90vh] flex flex-col" @click.stop>
                        <div class="p-4 border-b border-gray-200 dark:border-gray-700 flex-shrink-0">
                            <h3 class="text-lg font-semibold text-gray-900 dark:text-white">
                                Column Constraints: <span class="text-primary-600" x-text="editingColumn?.col"></span>
                            </h3>
                            <p class="text-sm text-gray-500" x-text="editingColumn?.type === 'creatable' ? 'Configure INSERT constraints' : 'Configure UPDATE constraints'"></p>
                        </div>
                        <div class="p-4 space-y-4 overflow-y-auto flex-1">
                            <!-- Common constraints -->
                            <label class="flex items-center gap-3 p-3 rounded-lg bg-gray-50 dark:bg-gray-700/50 cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700">
                                <input type="checkbox" x-model="editingColumn.constraints.requires_approval"
                                       class="w-4 h-4 text-amber-600 rounded">
                                <div>
                                    <span class="font-medium text-gray-900 dark:text-white">Requires Approval</span>
                                    <p class="text-xs text-gray-500">Human must approve before execution</p>
                                </div>
                            </label>
                            
                            <!-- Creatable-specific -->
                            <template x-if="editingColumn?.type === 'creatable'">
                                <div class="space-y-4">
                                    <label class="flex items-center gap-3 p-3 rounded-lg bg-gray-50 dark:bg-gray-700/50 cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700">
                                        <input type="checkbox" x-model="editingColumn.constraints.required"
                                               class="w-4 h-4 text-green-600 rounded">
                                        <div>
                                            <span class="font-medium text-gray-900 dark:text-white">Required</span>
                                            <p class="text-xs text-gray-500">Must be provided when creating</p>
                                        </div>
                                    </label>
                                    
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">Default Value</label>
                                        <input type="text" x-model="editingColumn.constraints.default"
                                               class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700"
                                               placeholder="Auto-set if not provided">
                                    </div>
                                    
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">Restrict to Values</label>
                                        <input type="text" x-model="editingColumn.constraints.restrict_to_str"
                                               class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700"
                                               placeholder="value1, value2, value3">
                                        <p class="text-xs text-gray-500 mt-1">Comma-separated list of allowed values</p>
                                    </div>
                                    
                                    <!-- FK Verification (only for FK columns) -->
                                    <template x-if="isForeignKey(editingColumn.table, editingColumn.col)">
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">
                                                <i class="fas fa-link text-purple-500 mr-1"></i> Foreign Key Verification
                                                <span class="text-xs font-normal text-gray-500 ml-1" x-text="'(references ' + getFkReferencedTable(editingColumn.table, editingColumn.col) + ')'"></span>
                                            </label>
                                            <input type="text" x-model="editingColumn.constraints.fk_verify_with_str"
                                                   class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700"
                                                   placeholder="email, name (columns from referenced table)">
                                            <p class="text-xs text-gray-500 mt-1">Columns from the referenced table to verify FK exists and matches</p>
                                        </div>
                                    </template>
                                </div>
                            </template>
                            
                            <!-- Updatable-specific -->
                            <template x-if="editingColumn?.type === 'updatable'">
                                <div class="space-y-4">
                                    <!-- Constraint Type Selector -->
                                    <div>
                                        <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Constraint Type</label>
                                        <div class="grid grid-cols-2 gap-2">
                                            <button type="button" @click="editingColumn.constraints.constraint_type = 'none'"
                                                    :class="(!editingColumn.constraints.constraint_type || editingColumn.constraints.constraint_type === 'none') ? 'bg-primary-100 border-primary-500 text-primary-700 dark:bg-primary-900/30 dark:text-primary-300' : 'bg-white dark:bg-gray-700 border-gray-300 dark:border-gray-600'"
                                                    class="px-3 py-2 border rounded-lg text-sm font-medium transition-colors">
                                                No constraint
                                            </button>
                                            <button type="button" @click="editingColumn.constraints.constraint_type = 'values'"
                                                    :class="editingColumn.constraints.constraint_type === 'values' ? 'bg-primary-100 border-primary-500 text-primary-700 dark:bg-primary-900/30 dark:text-primary-300' : 'bg-white dark:bg-gray-700 border-gray-300 dark:border-gray-600'"
                                                    class="px-3 py-2 border rounded-lg text-sm font-medium transition-colors">
                                                Restrict values
                                            </button>
                                            <button type="button" @click="editingColumn.constraints.constraint_type = 'conditions'"
                                                    :class="editingColumn.constraints.constraint_type === 'conditions' ? 'bg-primary-100 border-primary-500 text-primary-700 dark:bg-primary-900/30 dark:text-primary-300' : 'bg-white dark:bg-gray-700 border-gray-300 dark:border-gray-600'"
                                                    class="px-3 py-2 border rounded-lg text-sm font-medium transition-colors">
                                                Conditional rules
                                            </button>
                                            <button type="button" @click="editingColumn.constraints.constraint_type = 'comparison'"
                                                    :class="editingColumn.constraints.constraint_type === 'comparison' ? 'bg-primary-100 border-primary-500 text-primary-700 dark:bg-primary-900/30 dark:text-primary-300' : 'bg-white dark:bg-gray-700 border-gray-300 dark:border-gray-600'"
                                                    class="px-3 py-2 border rounded-lg text-sm font-medium transition-colors">
                                                Comparison
                                            </button>
                                        </div>
                                    </div>
                                    
                                    <!-- Restrict Values -->
                                    <template x-if="editingColumn.constraints.constraint_type === 'values'">
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">Allowed New Values</label>
                                            <input type="text" x-model="editingColumn.constraints.only_when_values"
                                                   class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700"
                                                   placeholder="value1, value2, value3">
                                            <p class="text-xs text-gray-500 mt-1">Comma-separated list of allowed values</p>
                                        </div>
                                    </template>
                                    
                                    <!-- Conditional Rules (replaces State Transitions) -->
                                    <template x-if="editingColumn.constraints.constraint_type === 'conditions'">
                                        <div class="space-y-3">
                                            <p class="text-xs text-gray-500">Define conditional rules using <code class="bg-gray-100 dark:bg-gray-700 px-1 rounded">old.</code> (current) and <code class="bg-gray-100 dark:bg-gray-700 px-1 rounded">new.</code> (incoming) column references. Rules are OR'd together.</p>
                                            <template x-for="(rule, ruleIdx) in editingColumn.constraints.rules || []" :key="ruleIdx">
                                                <div class="p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg border border-gray-200 dark:border-gray-600">
                                                    <div class="flex items-center justify-between mb-2">
                                                        <span class="text-xs font-medium text-gray-500">Rule <span x-text="ruleIdx + 1"></span> <span class="text-gray-400">(conditions are AND'd)</span></span>
                                                        <button type="button" @click="editingColumn.constraints.rules.splice(ruleIdx, 1)"
                                                                class="p-1 text-red-500 hover:text-red-700">
                                                            <i class="fas fa-trash text-xs"></i>
                                                        </button>
                                                    </div>
                                                    <div class="space-y-2">
                                                        <template x-for="(cond, condIdx) in rule.conditions || []" :key="condIdx">
                                                            <div class="flex items-center gap-2">
                                                                <select x-model="cond.ref" 
                                                                        class="w-32 px-2 py-1 text-sm border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-700">
                                                                    <optgroup label="Current (old)">
                                                                        <template x-for="c in getTableColumns(editingColumn.table)" :key="'old.' + c">
                                                                            <option :value="'old.' + c" x-text="'old.' + c"></option>
                                                                        </template>
                                                                    </optgroup>
                                                                    <optgroup label="Incoming (new)">
                                                                        <template x-for="c in getTableColumns(editingColumn.table)" :key="'new.' + c">
                                                                            <option :value="'new.' + c" x-text="'new.' + c"></option>
                                                                        </template>
                                                                    </optgroup>
                                                                </select>
                                                                <select x-model="cond.op"
                                                                        class="w-28 px-2 py-1 text-sm border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-700">
                                                                    <option value="equals">equals</option>
                                                                    <option value="in">in list</option>
                                                                    <option value="not_null">not null</option>
                                                                    <option value="is_null">is null</option>
                                                                    <option value="greater_than">&gt;</option>
                                                                    <option value="greater_than_or_equal">&gt;=</option>
                                                                    <option value="lower_than">&lt;</option>
                                                                    <option value="lower_than_or_equal">&lt;=</option>
                                                                    <option value="starts_with">starts with</option>
                                                                </select>
                                                                <template x-if="cond.op !== 'not_null' && cond.op !== 'is_null'">
                                                                    <input type="text" x-model="cond.value" 
                                                                           :placeholder="cond.op === 'in' ? 'val1, val2, val3' : (cond.op === 'starts_with' || cond.op === 'greater_than' || cond.op === 'lower_than' || cond.op === 'greater_than_or_equal' || cond.op === 'lower_than_or_equal') ? 'value or old.col' : 'value'"
                                                                           class="flex-1 px-2 py-1 text-sm border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-700">
                                                                </template>
                                                                <button type="button" @click="rule.conditions.splice(condIdx, 1)"
                                                                        class="p-1 text-gray-400 hover:text-red-500">
                                                                    <i class="fas fa-times text-xs"></i>
                                                                </button>
                                                            </div>
                                                        </template>
                                                        <button type="button" @click="if(!rule.conditions) rule.conditions = []; rule.conditions.push({{ref: 'new.' + editingColumn.col, op: 'equals', value: ''}})"
                                                                class="w-full py-1.5 border border-dashed border-gray-300 dark:border-gray-600 rounded text-xs text-gray-500 hover:border-primary-400 hover:text-primary-600 transition-colors">
                                                            <i class="fas fa-plus mr-1"></i> Add Condition
                                                        </button>
                                                    </div>
                                                </div>
                                            </template>
                                            <button type="button" @click="if(!editingColumn.constraints.rules) editingColumn.constraints.rules = []; editingColumn.constraints.rules.push({{conditions: [{{ref: 'new.' + editingColumn.col, op: 'in', value: ''}}]}})"
                                                    class="w-full py-2 border-2 border-dashed border-gray-300 dark:border-gray-600 rounded-lg text-sm text-gray-500 hover:border-primary-400 hover:text-primary-600 transition-colors">
                                                <i class="fas fa-plus mr-1"></i> Add Rule
                                            </button>
                                        </div>
                                    </template>
                                    
                                    <!-- Comparison -->
                                    <template x-if="editingColumn.constraints.constraint_type === 'comparison'">
                                        <div class="space-y-3">
                                            <div>
                                                <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">Operator</label>
                                                <select x-model="editingColumn.constraints.comparison_op"
                                                        class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700">
                                                    <option value="greater_than">Greater than old value</option>
                                                    <option value="greater_than_or_equal">Greater than or equal</option>
                                                    <option value="lower_than">Lower than old value</option>
                                                    <option value="lower_than_or_equal">Lower than or equal</option>
                                                    <option value="starts_with">Starts with old value (append only)</option>
                                                    <option value="not_null">Must not be null</option>
                                                </select>
                                            </div>
                                            <p class="text-xs text-gray-500">
                                                <template x-if="editingColumn.constraints.comparison_op === 'greater_than'">
                                                    <span>New value must be greater than current value (e.g., quantity can only increase)</span>
                                                </template>
                                                <template x-if="editingColumn.constraints.comparison_op === 'starts_with'">
                                                    <span>New value must start with current value (append-only pattern)</span>
                                                </template>
                                                <template x-if="editingColumn.constraints.comparison_op === 'not_null'">
                                                    <span>Value cannot be set to null</span>
                                                </template>
                                            </p>
                                        </div>
                                    </template>
                                    
                                    <!-- FK Verification for updatable (only for FK columns) -->
                                    <template x-if="isForeignKey(editingColumn.table, editingColumn.col)">
                                        <div>
                                            <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">
                                                <i class="fas fa-link text-purple-500 mr-1"></i> Foreign Key Verification
                                                <span class="text-xs font-normal text-gray-500 ml-1" x-text="'(references ' + getFkReferencedTable(editingColumn.table, editingColumn.col) + ')'"></span>
                                            </label>
                                            <input type="text" x-model="editingColumn.constraints.fk_verify_with_str"
                                                   class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700"
                                                   placeholder="email, name (columns from referenced table)">
                                            <p class="text-xs text-gray-500 mt-1">Columns from the referenced table to verify FK exists and matches</p>
                                        </div>
                                    </template>
                                </div>
                            </template>
                            
                            <!-- Guidance -->
                            <div>
                                <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">AI Guidance</label>
                                <textarea x-model="editingColumn.constraints.guidance" rows="2"
                                          class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700"
                                          placeholder="Instructions for AI agent..."></textarea>
                            </div>
                        </div>
                        <div class="p-4 border-t border-gray-200 dark:border-gray-700 flex justify-end gap-3 flex-shrink-0">
                            <button type="button" @click="editingColumn = null"
                                    class="px-4 py-2 text-gray-600 dark:text-gray-400 hover:text-gray-800 font-medium">
                                Cancel
                            </button>
                            <button type="button" @click="saveColumnConstraints()"
                                    class="px-4 py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium">
                                Save
                            </button>
                        </div>
                    </div>
                </div>

                <!-- Configured tables -->
                <div class="space-y-4">
                    <template x-for="(perms, tableName) in tables" :key="tableName">
                        <div class="border border-gray-200 dark:border-gray-700 rounded-xl overflow-hidden">
                            <!-- Table header -->
                            <div class="bg-gray-50 dark:bg-gray-700/50 px-4 py-3 flex items-center justify-between">
                                <div class="flex items-center gap-3">
                                    <i class="fas fa-table text-primary-500"></i>
                                    <span class="font-semibold text-gray-900 dark:text-white" x-text="tableName"></span>
                                    <div class="flex gap-1 ml-2">
                                        <span x-show="hasCreatable(tableName)" class="px-2 py-0.5 text-xs font-medium bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-300 rounded">C</span>
                                        <span x-show="hasReadable(tableName)" class="px-2 py-0.5 text-xs font-medium bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300 rounded">R</span>
                                        <span x-show="hasUpdatable(tableName)" class="px-2 py-0.5 text-xs font-medium bg-amber-100 text-amber-700 dark:bg-amber-900/30 dark:text-amber-300 rounded">U</span>
                                        <span x-show="isDeletable(tableName)" class="px-2 py-0.5 text-xs font-medium bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300 rounded">D</span>
                                    </div>
                                </div>
                                <div class="flex items-center gap-1">
                                    <button type="button" @click="toggleTableExpand(tableName)"
                                            class="p-2 text-gray-500 hover:text-gray-700 dark:hover:text-gray-300 rounded-lg hover:bg-gray-200 dark:hover:bg-gray-600">
                                        <i class="fas" :class="expandedTables[tableName] ? 'fa-chevron-up' : 'fa-chevron-down'"></i>
                                    </button>
                                    <button type="button" @click="removeTable(tableName)"
                                            class="p-2 text-red-500 hover:text-red-700 rounded-lg hover:bg-red-50 dark:hover:bg-red-900/20">
                                        <i class="fas fa-trash"></i>
                                    </button>
                                </div>
                            </div>
                            
                            <!-- Table permissions (expanded) -->
                            <div x-show="expandedTables[tableName]" x-collapse class="p-4">
                                
                                <!-- Permission type tabs -->
                                <div class="flex border-b border-gray-200 dark:border-gray-700 mb-4">
                                    <button type="button" @click="activeTab[tableName] = 'read'"
                                            :class="activeTab[tableName] === 'read' ? 'border-b-2 border-primary-500 text-primary-600' : 'text-gray-500 hover:text-gray-700'"
                                            class="px-4 py-2 font-medium text-sm transition-colors">
                                        <i class="fas fa-eye mr-1"></i> Read
                                    </button>
                                    <button type="button" @click="activeTab[tableName] = 'create'"
                                            :class="activeTab[tableName] === 'create' ? 'border-b-2 border-green-500 text-green-600' : 'text-gray-500 hover:text-gray-700'"
                                            class="px-4 py-2 font-medium text-sm transition-colors">
                                        <i class="fas fa-plus mr-1"></i> Create
                                    </button>
                                    <button type="button" @click="activeTab[tableName] = 'update'"
                                            :class="activeTab[tableName] === 'update' ? 'border-b-2 border-amber-500 text-amber-600' : 'text-gray-500 hover:text-gray-700'"
                                            class="px-4 py-2 font-medium text-sm transition-colors">
                                        <i class="fas fa-edit mr-1"></i> Update
                                    </button>
                                    <button type="button" @click="activeTab[tableName] = 'delete'"
                                            :class="activeTab[tableName] === 'delete' ? 'border-b-2 border-red-500 text-red-600' : 'text-gray-500 hover:text-gray-700'"
                                            class="px-4 py-2 font-medium text-sm transition-colors">
                                        <i class="fas fa-trash mr-1"></i> Delete
                                    </button>
                                </div>

                                <!-- READ Tab -->
                                <div x-show="activeTab[tableName] === 'read'" class="space-y-4">
                                    <div class="flex items-center justify-between">
                                        <label class="flex items-center gap-2">
                                            <input type="checkbox" :checked="hasReadable(tableName)" 
                                                   @change="togglePermission(tableName, 'readable', $event.target.checked)"
                                                   class="w-4 h-4 text-primary-600 rounded">
                                            <span class="font-medium text-gray-700 dark:text-gray-300">Allow reading from this table</span>
                                        </label>
                                        <label x-show="hasReadable(tableName)" class="flex items-center gap-2 text-sm text-gray-500">
                                            <input type="checkbox" :checked="isAllColumns(tableName, 'readable')"
                                                   @change="setAllColumns(tableName, 'readable', $event.target.checked)"
                                                   class="w-3 h-3 text-primary-600 rounded">
                                            <span>All columns</span>
                                        </label>
                                    </div>
                                    <div x-show="hasReadable(tableName) && !isAllColumns(tableName, 'readable')" 
                                         class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-2">
                                        <template x-for="col in getTableColumns(tableName)" :key="col">
                                            <label class="flex items-center gap-2 p-2 rounded-lg cursor-pointer"
                                                   :class="isTenantColumn(tableName, col) ? 'bg-purple-50 dark:bg-purple-900/20 border border-purple-200 dark:border-purple-800 hover:bg-purple-100 dark:hover:bg-purple-900/30' : 'bg-gray-50 dark:bg-gray-700/50 hover:bg-gray-100 dark:hover:bg-gray-700'">
                                                <input type="checkbox" :checked="isColumnSelected(tableName, 'readable', col)"
                                                       @change="toggleColumn(tableName, 'readable', col, $event.target.checked)"
                                                       class="w-4 h-4 text-primary-600 rounded">
                                                <span class="text-sm truncate" 
                                                      :class="isTenantColumn(tableName, col) ? 'text-purple-700 dark:text-purple-300' : ''"
                                                      x-text="col"></span>
                                                <span x-show="isTenantColumn(tableName, col)" 
                                                      class="px-1.5 py-0.5 text-xs font-medium bg-purple-100 text-purple-700 dark:bg-purple-900/50 dark:text-purple-300 rounded"
                                                      :title="isTenantFkColumn(tableName, col) ? 'Tenant FK' : 'Tenant Column'">
                                                    <i class="fas fa-shield-alt"></i>
                                                </span>
                                            </label>
                                        </template>
                                    </div>
                                    <div x-show="hasReadable(tableName)" class="pt-2 border-t border-gray-200 dark:border-gray-700">
                                        <label class="flex items-center gap-2 text-sm">
                                            <span class="text-gray-600 dark:text-gray-400">Max rows per page:</span>
                                            <input type="number" min="1" :value="getMaxPerPage(tableName)"
                                                   @input="setMaxPerPage(tableName, $event.target.value)"
                                                   class="w-24 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-700 text-sm"
                                                   placeholder="100">
                                        </label>
                                    </div>
                                </div>

                                <!-- CREATE Tab -->
                                <div x-show="activeTab[tableName] === 'create'" class="space-y-4">
                                    <div class="flex items-center justify-between">
                                        <label class="flex items-center gap-2">
                                            <input type="checkbox" :checked="hasCreatable(tableName)" 
                                                   @change="togglePermission(tableName, 'creatable', $event.target.checked)"
                                                   class="w-4 h-4 text-green-600 rounded">
                                            <span class="font-medium text-gray-700 dark:text-gray-300">Allow creating records</span>
                                        </label>
                                        <label x-show="hasCreatable(tableName)" class="flex items-center gap-2 text-sm text-gray-500">
                                            <input type="checkbox" :checked="isAllColumns(tableName, 'creatable')"
                                                   @change="setAllColumns(tableName, 'creatable', $event.target.checked)"
                                                   class="w-3 h-3 text-green-600 rounded">
                                            <span>All columns</span>
                                        </label>
                                    </div>
                                    <div x-show="hasCreatable(tableName) && !isAllColumns(tableName, 'creatable')" 
                                         class="grid grid-cols-2 sm:grid-cols-3 gap-2">
                                        <template x-for="col in getTableColumns(tableName)" :key="col">
                                            <div class="flex items-center gap-2 p-2 rounded-lg"
                                                 :class="isTenantColumn(tableName, col) ? 'bg-purple-50 dark:bg-purple-900/20 border border-purple-200 dark:border-purple-800' : 'bg-gray-50 dark:bg-gray-700/50'">
                                                <input type="checkbox" 
                                                       :checked="isColumnSelected(tableName, 'creatable', col)"
                                                       @change="toggleColumn(tableName, 'creatable', col, $event.target.checked)"
                                                       :disabled="isTenantColumn(tableName, col)"
                                                       class="w-4 h-4 text-green-600 rounded disabled:opacity-50 disabled:cursor-not-allowed">
                                                <span class="text-sm flex-1 truncate" 
                                                      :class="isTenantColumn(tableName, col) ? 'text-purple-700 dark:text-purple-300' : ''" 
                                                      x-text="col"></span>
                                                <span x-show="isTenantColumn(tableName, col)" 
                                                      class="px-1.5 py-0.5 text-xs font-medium bg-purple-100 text-purple-700 dark:bg-purple-900/50 dark:text-purple-300 rounded"
                                                      :title="isTenantFkColumn(tableName, col) ? 'Tenant FK (auto-filtered)' : 'Tenant Column (auto-filtered)'">
                                                    <i class="fas fa-shield-alt mr-0.5"></i>
                                                    <span x-text="isTenantFkColumn(tableName, col) ? 'FK' : 'Tenant'"></span>
                                                </span>
                                                <button type="button" x-show="isColumnSelected(tableName, 'creatable', col) && !isTenantColumn(tableName, col)"
                                                        @click="openColumnEditor(tableName, col, 'creatable')"
                                                        class="p-1 text-gray-400 hover:text-primary-600">
                                                    <i class="fas fa-cog text-xs"></i>
                                                </button>
                                            </div>
                                        </template>
                                    </div>
                                    <p x-show="hasCreatable(tableName) && !isAllColumns(tableName, 'creatable')" 
                                       class="text-xs text-purple-600 dark:text-purple-400 mt-2">
                                        <i class="fas fa-info-circle mr-1"></i>
                                        Tenant columns are automatically handled by Cori and cannot be set by AI agents.
                                    </p>
                                </div>

                                <!-- UPDATE Tab -->
                                <div x-show="activeTab[tableName] === 'update'" class="space-y-4">
                                    <div class="flex items-center justify-between">
                                        <label class="flex items-center gap-2">
                                            <input type="checkbox" :checked="hasUpdatable(tableName)" 
                                                   @change="togglePermission(tableName, 'updatable', $event.target.checked)"
                                                   class="w-4 h-4 text-amber-600 rounded">
                                            <span class="font-medium text-gray-700 dark:text-gray-300">Allow updating records</span>
                                        </label>
                                        <label x-show="hasUpdatable(tableName)" class="flex items-center gap-2 text-sm text-gray-500">
                                            <input type="checkbox" :checked="isAllColumns(tableName, 'updatable')"
                                                   @change="setAllColumns(tableName, 'updatable', $event.target.checked)"
                                                   class="w-3 h-3 text-amber-600 rounded">
                                            <span>All columns</span>
                                        </label>
                                    </div>
                                    <div x-show="hasUpdatable(tableName) && !isAllColumns(tableName, 'updatable')" 
                                         class="grid grid-cols-2 sm:grid-cols-3 gap-2">
                                        <template x-for="col in getTableColumns(tableName)" :key="col">
                                            <div class="flex items-center gap-2 p-2 rounded-lg"
                                                 :class="isTenantColumn(tableName, col) ? 'bg-purple-50 dark:bg-purple-900/20 border border-purple-200 dark:border-purple-800' : 'bg-gray-50 dark:bg-gray-700/50'">
                                                <input type="checkbox" 
                                                       :checked="isColumnSelected(tableName, 'updatable', col)"
                                                       @change="toggleColumn(tableName, 'updatable', col, $event.target.checked)"
                                                       :disabled="isTenantColumn(tableName, col)"
                                                       class="w-4 h-4 text-amber-600 rounded disabled:opacity-50 disabled:cursor-not-allowed">
                                                <span class="text-sm flex-1 truncate" 
                                                      :class="isTenantColumn(tableName, col) ? 'text-purple-700 dark:text-purple-300' : ''" 
                                                      x-text="col"></span>
                                                <span x-show="isTenantColumn(tableName, col)" 
                                                      class="px-1.5 py-0.5 text-xs font-medium bg-purple-100 text-purple-700 dark:bg-purple-900/50 dark:text-purple-300 rounded"
                                                      :title="isTenantFkColumn(tableName, col) ? 'Tenant FK (auto-filtered)' : 'Tenant Column (auto-filtered)'">
                                                    <i class="fas fa-shield-alt mr-0.5"></i>
                                                    <span x-text="isTenantFkColumn(tableName, col) ? 'FK' : 'Tenant'"></span>
                                                </span>
                                                <button type="button" x-show="isColumnSelected(tableName, 'updatable', col) && !isTenantColumn(tableName, col)"
                                                        @click="openColumnEditor(tableName, col, 'updatable')"
                                                        class="p-1 text-gray-400 hover:text-primary-600">
                                                    <i class="fas fa-cog text-xs"></i>
                                                </button>
                                            </div>
                                        </template>
                                    </div>
                                    <p x-show="hasUpdatable(tableName) && !isAllColumns(tableName, 'updatable')" 
                                       class="text-xs text-purple-600 dark:text-purple-400 mt-2">
                                        <i class="fas fa-info-circle mr-1"></i>
                                        Tenant columns are automatically handled by Cori and cannot be modified by AI agents.
                                    </p>
                                </div>

                                <!-- DELETE Tab -->
                                <div x-show="activeTab[tableName] === 'delete'" class="space-y-4">
                                    <label class="flex items-center gap-2">
                                        <input type="checkbox" :checked="isDeletable(tableName)" 
                                               @change="toggleDeletable(tableName, $event.target.checked)"
                                               class="w-4 h-4 text-red-600 rounded">
                                        <span class="font-medium text-gray-700 dark:text-gray-300">Allow deleting records</span>
                                    </label>
                                    <div x-show="isDeletable(tableName)" class="ml-6 space-y-3 pt-2">
                                        <label class="flex items-center gap-3 p-3 rounded-lg bg-gray-50 dark:bg-gray-700/50 cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700">
                                            <input type="checkbox" :checked="getDeletableOption(tableName, 'requires_approval')"
                                                   @change="setDeletableOption(tableName, 'requires_approval', $event.target.checked)"
                                                   class="w-4 h-4 text-red-600 rounded">
                                            <div>
                                                <span class="font-medium text-gray-900 dark:text-white">Requires Approval</span>
                                                <p class="text-xs text-gray-500">Human must approve before deletion</p>
                                            </div>
                                        </label>
                                        <label class="flex items-center gap-3 p-3 rounded-lg bg-gray-50 dark:bg-gray-700/50 cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700">
                                            <input type="checkbox" :checked="getDeletableOption(tableName, 'soft_delete')"
                                                   @change="setDeletableOption(tableName, 'soft_delete', $event.target.checked)"
                                                   class="w-4 h-4 text-red-600 rounded">
                                            <div>
                                                <span class="font-medium text-gray-900 dark:text-white">Soft Delete</span>
                                                <p class="text-xs text-gray-500">Mark as deleted instead of removing</p>
                                            </div>
                                        </label>
                                        <div class="p-3 rounded-lg bg-gray-50 dark:bg-gray-700/50">
                                            <label class="block text-sm font-medium text-gray-900 dark:text-white mb-1">
                                                <i class="fas fa-check-double text-purple-500 mr-1"></i> Verify With Columns
                                            </label>
                                            <input type="text" :value="getDeletableVerifyWith(tableName)"
                                                   @input="setDeletableVerifyWith(tableName, $event.target.value)"
                                                   class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-700"
                                                   placeholder="email, name (columns to verify record)">
                                            <p class="text-xs text-gray-500 mt-1">Agent must provide these column values to confirm the correct record is being deleted</p>
                                        </div>
                                    </div>
                                </div>
                            </div>
                        </div>
                    </template>
                    
                    <div x-show="Object.keys(tables).length === 0" 
                         class="text-center py-12 border-2 border-dashed border-gray-300 dark:border-gray-600 rounded-xl">
                        <i class="fas fa-database text-4xl text-gray-300 dark:text-gray-600 mb-4"></i>
                        <p class="text-gray-500 dark:text-gray-400 mb-4">No tables configured yet</p>
                        <button type="button" @click="showTableSelector = true"
                                class="inline-flex items-center gap-2 text-primary-600 hover:text-primary-700 font-medium">
                            <i class="fas fa-plus"></i> Add your first table
                        </button>
                    </div>
                </div>
            </div>

            <!-- Hidden JSON field -->
            <input type="hidden" name="tables_json" :value="JSON.stringify(tables)">

            <!-- Form actions -->
            <div class="flex justify-between items-center">
                <p class="text-sm text-gray-500">
                    <i class="fas fa-info-circle mr-1"></i>
                    Configure permissions for <span x-text="Object.keys(tables).length"></span> table(s)
                </p>
                <div class="flex gap-3">
                    <a href="/roles" class="px-4 py-2 text-gray-700 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-700 rounded-lg font-medium transition-colors">
                        Cancel
                    </a>
                    <button type="submit" class="px-6 py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium transition-colors inline-flex items-center gap-2">
                        <i class="fas fa-save"></i> {submit_text}
                    </button>
                </div>
            </div>
        </form>

        <script>
        function roleEditor() {{
            return {{
                tables: {{}},
                schema: {{}},
                showTableSelector: false,
                expandedTables: {{}},
                activeTab: {{}},
                editingColumn: null,
                
                init() {{
                    const tablesAttr = this.$el.dataset.tables;
                    const schemaAttr = this.$el.dataset.schema;
                    try {{
                        this.tables = tablesAttr ? JSON.parse(tablesAttr) : {{}};
                    }} catch (e) {{
                        console.error('Failed to parse tables:', e);
                        this.tables = {{}};
                    }}
                    try {{
                        this.schema = schemaAttr ? JSON.parse(schemaAttr) : {{}};
                    }} catch (e) {{
                        console.error('Failed to parse schema:', e);
                        this.schema = {{}};
                    }}
                    Object.keys(this.tables).forEach(t => {{
                        this.expandedTables[t] = true;
                        this.activeTab[t] = 'read';
                    }});
                }},
                
                get availableTables() {{
                    return Object.values(this.schema).filter(t => !this.tables[t.name]);
                }},
                
                addTable() {{ this.showTableSelector = true; }},
                
                selectTable(name) {{
                    this.tables[name] = {{ readable: [], creatable: null, updatable: null, deletable: false }};
                    this.expandedTables[name] = true;
                    this.activeTab[name] = 'read';
                    this.showTableSelector = false;
                }},
                
                removeTable(name) {{
                    delete this.tables[name];
                    delete this.expandedTables[name];
                    delete this.activeTab[name];
                }},
                
                toggleTableExpand(name) {{
                    this.expandedTables[name] = !this.expandedTables[name];
                }},
                
                getTableColumns(tableName) {{
                    const table = Object.values(this.schema).find(t => t.name === tableName);
                    return table ? table.columns.map(c => c.name) : [];
                }},
                
                // Tenant column helpers
                isTenantColumn(tableName, colName) {{
                    const table = Object.values(this.schema).find(t => t.name === tableName);
                    if (!table) return false;
                    const col = table.columns.find(c => c.name === colName);
                    return col?.is_tenant || col?.is_tenant_fk || false;
                }},
                
                isTenantFkColumn(tableName, colName) {{
                    const table = Object.values(this.schema).find(t => t.name === tableName);
                    if (!table) return false;
                    const col = table.columns.find(c => c.name === colName);
                    return col?.is_tenant_fk || false;
                }},
                
                // Foreign key helpers
                isForeignKey(tableName, colName) {{
                    const table = Object.values(this.schema).find(t => t.name === tableName);
                    if (!table) return false;
                    const col = table.columns.find(c => c.name === colName);
                    return col?.is_fk || false;
                }},
                
                getFkReferencedTable(tableName, colName) {{
                    const table = Object.values(this.schema).find(t => t.name === tableName);
                    if (!table) return null;
                    const col = table.columns.find(c => c.name === colName);
                    return col?.fk_references || null;
                }},
                
                // Generic permission helpers
                hasReadable(t) {{
                    const r = this.tables[t]?.readable;
                    return r !== null && r !== undefined && (r === '*' || Array.isArray(r) || r?.columns !== undefined);
                }},
                hasCreatable(t) {{
                    const c = this.tables[t]?.creatable;
                    return c !== null && c !== undefined && (c === '*' || typeof c === 'object');
                }},
                hasUpdatable(t) {{
                    const u = this.tables[t]?.updatable;
                    return u !== null && u !== undefined && (u === '*' || typeof u === 'object');
                }},
                isDeletable(t) {{
                    const d = this.tables[t]?.deletable;
                    return d === true || (typeof d === 'object' && d !== null);
                }},
                
                togglePermission(t, type, enabled) {{
                    if (type === 'readable') {{
                        this.tables[t].readable = enabled ? [] : null;
                    }} else if (type === 'creatable') {{
                        this.tables[t].creatable = enabled ? {{}} : null;
                    }} else if (type === 'updatable') {{
                        this.tables[t].updatable = enabled ? {{}} : null;
                    }}
                }},
                
                isAllColumns(t, type) {{
                    const v = this.tables[t]?.[type];
                    if (type === 'readable') return v === '*' || v?.columns === '*';
                    return v === '*';
                }},
                
                setAllColumns(t, type, all) {{
                    if (type === 'readable') {{
                        this.tables[t].readable = all ? '*' : [];
                    }} else {{
                        // Keep as enabled (object) but change between '*' and empty object
                        this.tables[t][type] = all ? '*' : {{}};
                    }}
                }},
                
                isColumnSelected(t, type, col) {{
                    const v = this.tables[t]?.[type];
                    if (v === '*') return true;
                    if (type === 'readable') {{
                        if (Array.isArray(v)) return v.includes(col);
                        if (v?.columns === '*') return true;
                        return Array.isArray(v?.columns) && v.columns.includes(col);
                    }}
                    return v !== null && typeof v === 'object' && col in v;
                }},
                
                toggleColumn(t, type, col, checked) {{
                    if (type === 'readable') {{
                        let r = this.tables[t].readable;
                        if (typeof r === 'string') r = [];
                        if (!Array.isArray(r)) r = r?.columns || [];
                        if (checked && !r.includes(col)) r.push(col);
                        else if (!checked) r = r.filter(c => c !== col);
                        this.tables[t].readable = r;
                    }} else {{
                        let v = this.tables[t][type];
                        if (v === '*') return;
                        if (v === null || typeof v !== 'object') v = {{}};
                        if (checked) v[col] = {{}};
                        else delete v[col];
                        this.tables[t][type] = v;
                    }}
                }},
                
                getMaxPerPage(t) {{
                    const r = this.tables[t]?.readable;
                    return r?.max_per_page || '';
                }},
                
                setMaxPerPage(t, value) {{
                    let r = this.tables[t].readable;
                    const v = parseInt(value) || null;
                    if (typeof r === 'string' || Array.isArray(r)) {{
                        if (v) this.tables[t].readable = {{ columns: r === '*' ? '*' : r, max_per_page: v }};
                    }} else if (r) {{
                        if (v) r.max_per_page = v;
                        else delete r.max_per_page;
                    }}
                }},
                
                toggleDeletable(t, enabled) {{
                    this.tables[t].deletable = enabled ? true : false;
                }},
                
                getDeletableOption(t, key) {{
                    const d = this.tables[t]?.deletable;
                    return typeof d === 'object' ? (d[key] || false) : false;
                }},
                
                setDeletableOption(t, key, value) {{
                    let d = this.tables[t].deletable;
                    if (d === true) {{ d = {{}}; this.tables[t].deletable = d; }}
                    if (typeof d === 'object') {{
                        if (value) d[key] = value;
                        else delete d[key];
                        if (Object.keys(d).length === 0) this.tables[t].deletable = true;
                    }}
                }},
                
                getDeletableVerifyWith(t) {{
                    const d = this.tables[t]?.deletable;
                    if (typeof d === 'object' && Array.isArray(d.verify_with)) {{
                        return d.verify_with.join(', ');
                    }}
                    return '';
                }},
                
                setDeletableVerifyWith(t, value) {{
                    let d = this.tables[t].deletable;
                    if (d === true) {{ d = {{}}; this.tables[t].deletable = d; }}
                    if (typeof d === 'object') {{
                        const vals = value.split(',').map(s => s.trim()).filter(s => s);
                        if (vals.length > 0) d.verify_with = vals;
                        else delete d.verify_with;
                        if (Object.keys(d).length === 0) this.tables[t].deletable = true;
                    }}
                }},
                
                openColumnEditor(t, col, type) {{
                    const v = this.tables[t]?.[type];
                    const constraints = (v !== '*' && typeof v === 'object' && v[col]) ? {{ ...v[col] }} : {{}};
                    
                    // Convert arrays to strings for editing
                    if (type === 'creatable' && constraints.restrict_to) {{
                        constraints.restrict_to_str = constraints.restrict_to.join(', ');
                    }}
                    // Convert foreign_key.verify_with to string
                    if (constraints.foreign_key?.verify_with) {{
                        constraints.fk_verify_with_str = constraints.foreign_key.verify_with.join(', ');
                    }}
                    if (type === 'updatable' && constraints.only_when) {{
                        // Detect the type of only_when constraint and parse into UI model
                        const ow = constraints.only_when;
                        
                        // Helper to parse a single condition object into rule conditions
                        const parseConditionObj = (condObj) => {{
                            const conditions = [];
                            for (const [key, val] of Object.entries(condObj)) {{
                                if (!key.startsWith('old.') && !key.startsWith('new.')) continue;
                                
                                if (Array.isArray(val)) {{
                                    conditions.push({{ ref: key, op: 'in', value: val.join(', ') }});
                                }} else if (typeof val === 'object' && val !== null) {{
                                    // Comparison operator
                                    if (val.greater_than !== undefined) conditions.push({{ ref: key, op: 'greater_than', value: String(val.greater_than) }});
                                    else if (val.greater_than_or_equal !== undefined) conditions.push({{ ref: key, op: 'greater_than_or_equal', value: String(val.greater_than_or_equal) }});
                                    else if (val.lower_than !== undefined) conditions.push({{ ref: key, op: 'lower_than', value: String(val.lower_than) }});
                                    else if (val.lower_than_or_equal !== undefined) conditions.push({{ ref: key, op: 'lower_than_or_equal', value: String(val.lower_than_or_equal) }});
                                    else if (val.starts_with !== undefined) conditions.push({{ ref: key, op: 'starts_with', value: String(val.starts_with) }});
                                    else if (val.not_null) conditions.push({{ ref: key, op: 'not_null', value: '' }});
                                    else if (val.is_null) conditions.push({{ ref: key, op: 'is_null', value: '' }});
                                    else if (val.in) conditions.push({{ ref: key, op: 'in', value: val.in.join(', ') }});
                                    else if (val.not_in) conditions.push({{ ref: key, op: 'not_in', value: val.not_in.join(', ') }});
                                    else if (val.equals !== undefined) conditions.push({{ ref: key, op: 'equals', value: String(val.equals) }});
                                    else if (val.not_equals !== undefined) conditions.push({{ ref: key, op: 'not_equals', value: String(val.not_equals) }});
                                }} else {{
                                    // Simple value - equals
                                    conditions.push({{ ref: key, op: 'equals', value: String(val) }});
                                }}
                            }}
                            return conditions;
                        }};
                        
                        // Check if it's a simple single-column values restriction
                        const newKey = 'new.' + col;
                        if (!Array.isArray(ow) && typeof ow === 'object') {{
                            const keys = Object.keys(ow).filter(k => k.startsWith('old.') || k.startsWith('new.'));
                            if (keys.length === 1 && keys[0] === newKey) {{
                                const val = ow[newKey];
                                if (Array.isArray(val)) {{
                                    // Simple values restriction
                                    constraints.constraint_type = 'values';
                                    constraints.only_when_values = val.join(', ');
                                }} else if (typeof val === 'object' && val !== null) {{
                                    // Simple comparison on current column
                                    constraints.constraint_type = 'comparison';
                                    if (val.greater_than) constraints.comparison_op = 'greater_than';
                                    else if (val.greater_than_or_equal) constraints.comparison_op = 'greater_than_or_equal';
                                    else if (val.lower_than) constraints.comparison_op = 'lower_than';
                                    else if (val.lower_than_or_equal) constraints.comparison_op = 'lower_than_or_equal';
                                    else if (val.starts_with) constraints.comparison_op = 'starts_with';
                                    else if (val.not_null) constraints.comparison_op = 'not_null';
                                    else {{
                                        // Complex, use conditions
                                        constraints.constraint_type = 'conditions';
                                        constraints.rules = [{{ conditions: parseConditionObj(ow) }}];
                                    }}
                                }} else {{
                                    // Single value, use conditions
                                    constraints.constraint_type = 'conditions';
                                    constraints.rules = [{{ conditions: parseConditionObj(ow) }}];
                                }}
                            }} else {{
                                // Multiple columns in single condition object - use conditions
                                constraints.constraint_type = 'conditions';
                                constraints.rules = [{{ conditions: parseConditionObj(ow) }}];
                            }}
                        }} else if (Array.isArray(ow)) {{
                            // Array of condition sets (OR logic) - use conditions
                            constraints.constraint_type = 'conditions';
                            constraints.rules = ow.map(condObj => ({{ conditions: parseConditionObj(condObj) }}));
                        }} else {{
                            constraints.constraint_type = 'none';
                        }}
                    }} else if (type === 'updatable') {{
                        constraints.constraint_type = 'none';
                    }}
                    
                    this.editingColumn = {{ table: t, col, type, constraints }};
                }},
                
                saveColumnConstraints() {{
                    if (!this.editingColumn) return;
                    const {{ table, col, type, constraints }} = this.editingColumn;
                    let v = this.tables[table][type];
                    if (v === '*' || typeof v !== 'object') return;
                    
                    // Build clean constraints object
                    const clean = {{}};
                    if (constraints.required) clean.required = true;
                    if (constraints.requires_approval) clean.requires_approval = true;
                    if (constraints.default) clean.default = constraints.default;
                    if (constraints.guidance) clean.guidance = constraints.guidance;
                    
                    if (type === 'creatable' && constraints.restrict_to_str) {{
                        const vals = constraints.restrict_to_str.split(',').map(s => s.trim()).filter(s => s);
                        if (vals.length > 0) clean.restrict_to = vals;
                    }}
                    
                    if (type === 'updatable') {{
                        const ct = constraints.constraint_type;
                        if (ct === 'values' && constraints.only_when_values) {{
                            const vals = constraints.only_when_values.split(',').map(s => s.trim()).filter(s => s);
                            if (vals.length > 0) {{
                                clean.only_when = {{}};
                                clean.only_when['new.' + col] = vals;
                            }}
                        }} else if (ct === 'conditions' && constraints.rules?.length > 0) {{
                            // Helper to build condition value
                            const buildCondValue = (cond) => {{
                                const op = cond.op;
                                const val = cond.value?.trim();
                                
                                if (op === 'equals') {{
                                    return val || '';
                                }} else if (op === 'in') {{
                                    const vals = val ? val.split(',').map(s => s.trim()).filter(s => s) : [];
                                    return vals.length === 1 ? vals[0] : vals;
                                }} else if (op === 'not_null') {{
                                    return {{ not_null: true }};
                                }} else if (op === 'is_null') {{
                                    return {{ is_null: true }};
                                }} else if (['greater_than', 'greater_than_or_equal', 'lower_than', 'lower_than_or_equal', 'starts_with'].includes(op)) {{
                                    // Check if value is a column reference or a literal
                                    if (val && (val.startsWith('old.') || val.startsWith('new.'))) {{
                                        return {{ [op]: val }};
                                    }} else {{
                                        // Try to parse as number, otherwise use string
                                        const num = parseFloat(val);
                                        return {{ [op]: isNaN(num) ? val : num }};
                                    }}
                                }}
                                return val;
                            }};
                            
                            // Build only_when from rules
                            const onlyWhenRules = constraints.rules
                                .filter(r => r.conditions?.length > 0)
                                .map(rule => {{
                                    const condObj = {{}};
                                    for (const cond of rule.conditions) {{
                                        if (cond.ref) {{
                                            condObj[cond.ref] = buildCondValue(cond);
                                        }}
                                    }}
                                    return condObj;
                                }})
                                .filter(obj => Object.keys(obj).length > 0);
                            
                            if (onlyWhenRules.length === 1) {{
                                // Single rule - use object directly
                                clean.only_when = onlyWhenRules[0];
                            }} else if (onlyWhenRules.length > 1) {{
                                // Multiple rules - use array (OR logic)
                                clean.only_when = onlyWhenRules;
                            }}
                        }} else if (ct === 'comparison' && constraints.comparison_op) {{
                            const op = constraints.comparison_op;
                            clean.only_when = {{}};
                            if (op === 'not_null') {{
                                clean.only_when['new.' + col] = {{ not_null: true }};
                            }} else {{
                                clean.only_when['new.' + col] = {{ [op]: 'old.' + col }};
                            }}
                        }}
                    }}
                    
                    // Add foreign_key constraint if verify_with is specified
                    if (constraints.fk_verify_with_str) {{
                        const fkVals = constraints.fk_verify_with_str.split(',').map(s => s.trim()).filter(s => s);
                        if (fkVals.length > 0) {{
                            clean.foreign_key = {{ verify_with: fkVals }};
                        }}
                    }}
                    
                    v[col] = clean;
                    this.editingColumn = null;
                }}
            }};
        }}
        </script>"##,
        title = title,
        method = method,
        submit_url = submit_url,
        name_input = if is_new {
            input("name", "Role Name", "text", name, "e.g., support_agent")
        } else {
            format!(
                r#"<div>
                    <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">Role Name</label>
                    <input type="text" name="name" value="{}" readonly
                           class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-gray-100 dark:bg-gray-700 text-gray-500 dark:text-gray-400">
                </div>"#,
                name
            )
        },
        description_input = input(
            "description",
            "Description",
            "text",
            description,
            "What this role is for"
        ),
        approval_group = approval_group,
        submit_text = if is_new {
            "Create Role"
        } else {
            "Save Changes"
        },
        tables_json_escaped = tables_json_escaped,
        schema_info_json_escaped = schema_info_json_escaped,
    );

    layout(title, &content)
}

/// Convert role tables to JSON for the editor
fn role_tables_to_json(tables: &HashMap<String, cori_core::config::role_definition::TablePermissions>) -> String {
    use cori_core::config::role_definition::{
        CreatableColumns, ReadableConfig, UpdatableColumns, DeletablePermission,
    };

    let mut json_tables = serde_json::Map::new();

    for (name, perms) in tables {
        let mut table_obj = serde_json::Map::new();

        // Readable
        let readable_val = match &perms.readable {
            ReadableConfig::All(_) => serde_json::Value::String("*".to_string()),
            ReadableConfig::List(cols) => serde_json::Value::Array(
                cols.iter().map(|s| serde_json::Value::String(s.clone())).collect()
            ),
            ReadableConfig::Config(cfg) => {
                let mut obj = serde_json::Map::new();
                if cfg.columns.is_all() {
                    obj.insert("columns".to_string(), serde_json::Value::String("*".to_string()));
                } else if let Some(cols) = cfg.columns.as_list() {
                    obj.insert("columns".to_string(), serde_json::Value::Array(
                        cols.iter().map(|s| serde_json::Value::String(s.clone())).collect()
                    ));
                }
                if let Some(max) = cfg.max_per_page {
                    obj.insert("max_per_page".to_string(), serde_json::Value::Number(max.into()));
                }
                serde_json::Value::Object(obj)
            }
        };
        table_obj.insert("readable".to_string(), readable_val);

        // Creatable - use null for empty maps (no creatable permission)
        let creatable_val = match &perms.creatable {
            CreatableColumns::All(_) => serde_json::Value::String("*".to_string()),
            CreatableColumns::Map(cols) if cols.is_empty() => serde_json::Value::Null,
            CreatableColumns::Map(cols) => {
                let mut obj = serde_json::Map::new();
                for (col, constraints) in cols {
                    let c = serde_json::to_value(constraints).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                    obj.insert(col.clone(), c);
                }
                serde_json::Value::Object(obj)
            }
        };
        table_obj.insert("creatable".to_string(), creatable_val);

        // Updatable - use null for empty maps (no updatable permission)
        let updatable_val = match &perms.updatable {
            UpdatableColumns::All(_) => serde_json::Value::String("*".to_string()),
            UpdatableColumns::Map(cols) if cols.is_empty() => serde_json::Value::Null,
            UpdatableColumns::Map(cols) => {
                let mut obj = serde_json::Map::new();
                for (col, constraints) in cols {
                    let c = serde_json::to_value(constraints).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                    obj.insert(col.clone(), c);
                }
                serde_json::Value::Object(obj)
            }
        };
        table_obj.insert("updatable".to_string(), updatable_val);

        // Deletable
        let deletable_val = match &perms.deletable {
            DeletablePermission::Allowed(b) => serde_json::Value::Bool(*b),
            DeletablePermission::WithConstraints(c) => {
                serde_json::to_value(c).unwrap_or(serde_json::Value::Bool(true))
            }
        };
        table_obj.insert("deletable".to_string(), deletable_val);

        json_tables.insert(name.clone(), serde_json::Value::Object(table_obj));
    }

    serde_json::to_string(&json_tables).unwrap_or_else(|_| "{}".to_string())
}

/// Convert schema info to JSON for the editor, including tenant column info from rules
fn schema_to_json(schema: &SchemaInfo, rules: Option<&RulesDefinition>) -> String {
    let mut json_schema = serde_json::Map::new();

    for table in &schema.tables {
        let mut table_obj = serde_json::Map::new();
        table_obj.insert("name".to_string(), serde_json::Value::String(table.name.clone()));
        table_obj.insert("schema".to_string(), serde_json::Value::String(table.schema.clone()));
        
        // Get tenant column info from rules
        let tenant_column = rules
            .and_then(|r| r.get_table_rules(&table.name))
            .and_then(|tr| tr.get_direct_tenant_column())
            .map(|s| s.to_string());
        
        // Get inherited tenant FK column
        let tenant_fk_column = rules
            .and_then(|r| r.get_table_rules(&table.name))
            .and_then(|tr| tr.get_inherited_tenant())
            .map(|it| it.via.clone());
        
        // Check if table is global
        let is_global = rules
            .and_then(|r| r.get_table_rules(&table.name))
            .map(|tr| tr.global.unwrap_or(false))
            .unwrap_or(false);
        
        if let Some(tc) = &tenant_column {
            table_obj.insert("tenant_column".to_string(), serde_json::Value::String(tc.clone()));
        }
        if let Some(fk) = &tenant_fk_column {
            table_obj.insert("tenant_fk_column".to_string(), serde_json::Value::String(fk.clone()));
        }
        table_obj.insert("is_global".to_string(), serde_json::Value::Bool(is_global));
        
        // Build a map of column -> referenced table for FK columns
        let fk_map: std::collections::HashMap<String, String> = table.foreign_keys.iter()
            .flat_map(|fk| {
                fk.columns.iter().map(move |col| (col.clone(), fk.references_table.clone()))
            })
            .collect();
        
        let columns: Vec<serde_json::Value> = table.columns.iter().map(|c| {
            let mut col_obj = serde_json::Map::new();
            col_obj.insert("name".to_string(), serde_json::Value::String(c.name.clone()));
            col_obj.insert("data_type".to_string(), serde_json::Value::String(c.data_type.clone()));
            col_obj.insert("nullable".to_string(), serde_json::Value::Bool(c.nullable));
            
            // Mark if this is the tenant column
            let is_tenant = tenant_column.as_ref().map(|tc| tc == &c.name).unwrap_or(false);
            let is_tenant_fk = tenant_fk_column.as_ref().map(|fk| fk == &c.name).unwrap_or(false);
            col_obj.insert("is_tenant".to_string(), serde_json::Value::Bool(is_tenant));
            col_obj.insert("is_tenant_fk".to_string(), serde_json::Value::Bool(is_tenant_fk));
            
            // Mark if this is a foreign key column and its referenced table
            if let Some(ref_table) = fk_map.get(&c.name) {
                col_obj.insert("is_fk".to_string(), serde_json::Value::Bool(true));
                col_obj.insert("fk_references".to_string(), serde_json::Value::String(ref_table.clone()));
            } else {
                col_obj.insert("is_fk".to_string(), serde_json::Value::Bool(false));
            }
            
            serde_json::Value::Object(col_obj)
        }).collect();
        
        table_obj.insert("columns".to_string(), serde_json::Value::Array(columns));
        json_schema.insert(table.name.clone(), serde_json::Value::Object(table_obj));
    }

    serde_json::to_string(&json_schema).unwrap_or_else(|_| "{}".to_string())
}

// =============================================================================
// Role Detail Page with MCP Preview
// =============================================================================

pub fn role_detail_page(role: &RoleDefinition, mcp_tools: Option<&[serde_json::Value]>) -> String {
    use cori_core::config::role_definition::{CreatableColumns, ReadableConfig, UpdatableColumns, DeletablePermission};

    // Count stats
    let table_count = role.tables.len();
    let tool_count = mcp_tools.map(|t| t.len()).unwrap_or(0);
    let mut read_count = 0;
    let mut write_count = 0;
    for perms in role.tables.values() {
        if !perms.readable.is_empty() { read_count += 1; }
        if !perms.creatable.is_empty() || !perms.updatable.is_empty() || perms.deletable.is_allowed() { 
            write_count += 1; 
        }
    }

    // Build table rows with visual CRUD indicators
    let tables_rows: String = role.tables.iter().map(|(name, perms)| {
        // Read indicator
        let read_badge = if !perms.readable.is_empty() {
            let count = match &perms.readable {
                ReadableConfig::All(_) => "all".to_string(),
                ReadableConfig::List(cols) => cols.len().to_string(),
                ReadableConfig::Config(cfg) => if cfg.columns.is_all() { "all".to_string() } else { cfg.columns.as_list().map(|c| c.len()).unwrap_or(0).to_string() }
            };
            let max_page = match &perms.readable {
                ReadableConfig::Config(cfg) => cfg.max_per_page.map(|m| format!(" <span class=\"text-xs text-gray-400\">/{}</span>", m)).unwrap_or_default(),
                _ => String::new(),
            };
            format!(r#"<div class="flex items-center gap-1.5">
                <span class="w-2 h-2 rounded-full bg-blue-500"></span>
                <span class="text-sm">{count} cols{max_page}</span>
            </div>"#)
        } else {
            r#"<span class="text-gray-300 dark:text-gray-600">—</span>"#.to_string()
        };

        // Create indicator
        let create_badge = match &perms.creatable {
            CreatableColumns::All(_) => r#"<div class="flex items-center gap-1.5">
                <span class="w-2 h-2 rounded-full bg-green-500"></span>
                <span class="text-sm">all cols</span>
            </div>"#.to_string(),
            CreatableColumns::Map(cols) if cols.is_empty() => r#"<span class="text-gray-300 dark:text-gray-600">—</span>"#.to_string(),
            CreatableColumns::Map(cols) => {
                let has_constraints = cols.values().any(|c| c.required || c.default.is_some() || c.restrict_to.is_some() || c.requires_approval.is_some());
                format!(r#"<div class="flex items-center gap-1.5">
                    <span class="w-2 h-2 rounded-full bg-green-500"></span>
                    <span class="text-sm">{} cols{}</span>
                </div>"#, cols.len(), if has_constraints { " ⚙" } else { "" })
            }
        };

        // Update indicator
        let update_badge = match &perms.updatable {
            UpdatableColumns::All(_) => r#"<div class="flex items-center gap-1.5">
                <span class="w-2 h-2 rounded-full bg-yellow-500"></span>
                <span class="text-sm">all cols</span>
            </div>"#.to_string(),
            UpdatableColumns::Map(cols) if cols.is_empty() => r#"<span class="text-gray-300 dark:text-gray-600">—</span>"#.to_string(),
            UpdatableColumns::Map(cols) => {
                let has_constraints = cols.values().any(|c| c.only_when.is_some() || c.requires_approval.is_some());
                format!(r#"<div class="flex items-center gap-1.5">
                    <span class="w-2 h-2 rounded-full bg-yellow-500"></span>
                    <span class="text-sm">{} cols{}</span>
                </div>"#, cols.len(), if has_constraints { " ⚙" } else { "" })
            }
        };

        // Delete indicator
        let delete_badge = match &perms.deletable {
            DeletablePermission::Allowed(false) => r#"<span class="text-gray-300 dark:text-gray-600">—</span>"#.to_string(),
            DeletablePermission::Allowed(true) => r#"<div class="flex items-center gap-1.5">
                <span class="w-2 h-2 rounded-full bg-red-500"></span>
                <span class="text-sm">allowed</span>
            </div>"#.to_string(),
            DeletablePermission::WithConstraints(c) => {
                let mut extras = Vec::new();
                if c.requires_approval.as_ref().is_some_and(|r| r.is_required()) { extras.push("🔒"); }
                if c.soft_delete { extras.push("♻"); }
                if !c.verify_with.is_empty() { extras.push("✓"); }
                format!(r#"<div class="flex items-center gap-1.5">
                    <span class="w-2 h-2 rounded-full bg-red-500"></span>
                    <span class="text-sm">allowed {}</span>
                </div>"#, extras.join(""))
            }
        };

        format!(
            r##"<tr class="border-b border-gray-100 dark:border-gray-700/50 hover:bg-gray-50 dark:hover:bg-gray-700/30">
                <td class="py-3 px-4">
                    <div class="flex items-center gap-2">
                        <i class="fas fa-table text-gray-400 text-xs"></i>
                        <span class="font-medium text-gray-900 dark:text-white">{name}</span>
                    </div>
                </td>
                <td class="py-3 px-4 text-center">{read}</td>
                <td class="py-3 px-4 text-center">{create}</td>
                <td class="py-3 px-4 text-center">{update}</td>
                <td class="py-3 px-4 text-center">{delete}</td>
            </tr>"##,
            name = name,
            read = read_badge,
            create = create_badge,
            update = update_badge,
            delete = delete_badge,
        )
    }).collect();

    // Expanded table details (columns and constraints)
    let table_details: String = role.tables.iter().map(|(name, perms)| {
        let mut sections = Vec::new();

        // Readable section
        if !perms.readable.is_empty() {
            let cols = match &perms.readable {
                ReadableConfig::All(_) => "<span class=\"text-green-600\">All columns</span>".to_string(),
                ReadableConfig::List(cols) => cols.iter().map(|c| format!("<code class=\"bg-gray-100 dark:bg-gray-700 px-1.5 py-0.5 rounded text-xs\">{}</code>", c)).collect::<Vec<_>>().join(" "),
                ReadableConfig::Config(cfg) => {
                    if cfg.columns.is_all() {
                        "<span class=\"text-green-600\">All columns</span>".to_string()
                    } else {
                        cfg.columns.as_list().map(|cols| cols.iter().map(|c| format!("<code class=\"bg-gray-100 dark:bg-gray-700 px-1.5 py-0.5 rounded text-xs\">{}</code>", c)).collect::<Vec<_>>().join(" ")).unwrap_or_default()
                    }
                }
            };
            let max_page = match &perms.readable {
                ReadableConfig::Config(cfg) => cfg.max_per_page.map(|m| format!("<div class=\"text-xs text-gray-500 mt-1\">Max {} rows per page</div>", m)).unwrap_or_default(),
                _ => String::new(),
            };
            sections.push(format!(r#"<div class="mb-3">
                <div class="flex items-center gap-2 mb-1.5">
                    <span class="w-1.5 h-1.5 rounded-full bg-blue-500"></span>
                    <span class="text-xs font-semibold text-gray-500 uppercase tracking-wide">Read</span>
                </div>
                <div class="ml-3.5">{cols}{max_page}</div>
            </div>"#));
        }

        // Creatable section
        if let CreatableColumns::Map(cols) = &perms.creatable {
            if !cols.is_empty() {
                let cols_html: String = cols.iter().map(|(col, constraints)| {
                    let mut notes: Vec<String> = Vec::new();
                    if constraints.required { notes.push("<span class=\"text-red-500 text-xs\">required</span>".to_string()); }
                    if let Some(d) = &constraints.default { notes.push(format!("<span class=\"text-blue-500 text-xs\">default: {}</span>", d)); }
                    if let Some(r) = &constraints.restrict_to { notes.push(format!("<span class=\"text-purple-500 text-xs\">enum: {}</span>", r.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "))); }
                    if constraints.requires_approval.is_some() { notes.push("<span class=\"text-yellow-600 text-xs\">🔒 approval</span>".to_string()); }
                    format!(r#"<div class="flex items-baseline gap-2">
                        <code class="bg-gray-100 dark:bg-gray-700 px-1.5 py-0.5 rounded text-xs">{}</code>
                        {}
                    </div>"#, col, notes.join(" "))
                }).collect();
                sections.push(format!(r#"<div class="mb-3">
                    <div class="flex items-center gap-2 mb-1.5">
                        <span class="w-1.5 h-1.5 rounded-full bg-green-500"></span>
                        <span class="text-xs font-semibold text-gray-500 uppercase tracking-wide">Create</span>
                    </div>
                    <div class="ml-3.5 space-y-1">{cols_html}</div>
                </div>"#));
            }
        } else if let CreatableColumns::All(_) = &perms.creatable {
            sections.push(r#"<div class="mb-3">
                <div class="flex items-center gap-2 mb-1.5">
                    <span class="w-1.5 h-1.5 rounded-full bg-green-500"></span>
                    <span class="text-xs font-semibold text-gray-500 uppercase tracking-wide">Create</span>
                </div>
                <div class="ml-3.5"><span class="text-green-600">All columns</span></div>
            </div>"#.to_string());
        }

        // Updatable section
        if let UpdatableColumns::Map(cols) = &perms.updatable {
            if !cols.is_empty() {
                let cols_html: String = cols.iter().map(|(col, constraints)| {
                    let mut notes = Vec::new();
                    if constraints.only_when.is_some() { notes.push("<span class=\"text-purple-500 text-xs\">conditional</span>"); }
                    if constraints.requires_approval.is_some() { notes.push("<span class=\"text-yellow-600 text-xs\">🔒 approval</span>"); }
                    format!(r#"<div class="flex items-baseline gap-2">
                        <code class="bg-gray-100 dark:bg-gray-700 px-1.5 py-0.5 rounded text-xs">{}</code>
                        {}
                    </div>"#, col, notes.join(" "))
                }).collect();
                sections.push(format!(r#"<div class="mb-3">
                    <div class="flex items-center gap-2 mb-1.5">
                        <span class="w-1.5 h-1.5 rounded-full bg-yellow-500"></span>
                        <span class="text-xs font-semibold text-gray-500 uppercase tracking-wide">Update</span>
                    </div>
                    <div class="ml-3.5 space-y-1">{cols_html}</div>
                </div>"#));
            }
        } else if let UpdatableColumns::All(_) = &perms.updatable {
            sections.push(r#"<div class="mb-3">
                <div class="flex items-center gap-2 mb-1.5">
                    <span class="w-1.5 h-1.5 rounded-full bg-yellow-500"></span>
                    <span class="text-xs font-semibold text-gray-500 uppercase tracking-wide">Update</span>
                </div>
                <div class="ml-3.5"><span class="text-green-600">All columns</span></div>
            </div>"#.to_string());
        }

        // Deletable section
        if perms.deletable.is_allowed() {
            let delete_notes = match &perms.deletable {
                DeletablePermission::Allowed(true) => "Hard delete allowed".to_string(),
                DeletablePermission::WithConstraints(c) => {
                    let mut notes: Vec<String> = Vec::new();
                    if c.soft_delete { notes.push("soft delete".to_string()); } else { notes.push("hard delete".to_string()); }
                    if c.requires_approval.as_ref().is_some_and(|r| r.is_required()) { notes.push("requires approval".to_string()); }
                    if !c.verify_with.is_empty() { notes.push(format!("verify: {}", c.verify_with.join(", "))); }
                    notes.join(" • ")
                },
                _ => String::new(),
            };
            sections.push(format!(r#"<div class="mb-3">
                <div class="flex items-center gap-2 mb-1.5">
                    <span class="w-1.5 h-1.5 rounded-full bg-red-500"></span>
                    <span class="text-xs font-semibold text-gray-500 uppercase tracking-wide">Delete</span>
                </div>
                <div class="ml-3.5 text-sm text-gray-600 dark:text-gray-400">{delete_notes}</div>
            </div>"#));
        }

        if sections.is_empty() {
            return String::new();
        }

        format!(
            r##"<div class="border border-gray-200 dark:border-gray-700 rounded-lg overflow-hidden mb-4">
                <div class="bg-gray-50 dark:bg-gray-700/50 px-4 py-2 border-b border-gray-200 dark:border-gray-700">
                    <span class="font-medium text-gray-900 dark:text-white">{name}</span>
                </div>
                <div class="p-4 text-sm">{sections}</div>
            </div>"##,
            name = name,
            sections = sections.join("")
        )
    }).collect();

    // Approval config display
    let approval_badge = role.approvals.as_ref().map(|a| {
        format!(
            r##"<div class="flex items-center gap-2 px-3 py-1.5 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded-full text-sm">
                <i class="fas fa-shield-check text-yellow-600"></i>
                <span class="text-yellow-800 dark:text-yellow-200">Approval group: <strong>{}</strong></span>
            </div>"##,
            a.group
        )
    }).unwrap_or_default();

    // MCP tools preview - simplified
    let mcp_preview = match mcp_tools {
        Some(tools) if !tools.is_empty() => {
            let tool_list: String = tools.iter().map(|tool| {
                let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                let desc = tool.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let annotations = tool.get("annotations").cloned().unwrap_or(serde_json::json!({}));
                let read_only = annotations.get("readOnly").and_then(|v| v.as_bool()).unwrap_or(false);
                let requires_approval = annotations.get("requiresApproval").and_then(|v| v.as_bool()).unwrap_or(false);

                let icon = if name.starts_with("get") || name.starts_with("list") {
                    "fa-search text-blue-500"
                } else if name.starts_with("create") {
                    "fa-plus text-green-500"
                } else if name.starts_with("update") {
                    "fa-edit text-yellow-500"
                } else if name.starts_with("delete") {
                    "fa-trash text-red-500"
                } else {
                    "fa-cog text-gray-400"
                };

                let badges = format!("{}{}",
                    if read_only { "" } else { "<span class=\"text-xs px-1.5 py-0.5 bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 rounded\">mutation</span> " },
                    if requires_approval { "<span class=\"text-xs px-1.5 py-0.5 bg-yellow-100 dark:bg-yellow-900/30 text-yellow-700 dark:text-yellow-300 rounded\">approval</span>" } else { "" }
                );

                format!(
                    r##"<div class="flex items-start gap-3 py-2 border-b border-gray-100 dark:border-gray-700/50 last:border-0">
                        <i class="fas {icon} mt-0.5"></i>
                        <div class="flex-1 min-w-0">
                            <div class="flex items-center gap-2 flex-wrap">
                                <code class="font-semibold text-gray-900 dark:text-white">{name}</code>
                                {badges}
                            </div>
                            <p class="text-sm text-gray-500 dark:text-gray-400 truncate">{desc}</p>
                        </div>
                    </div>"##,
                    icon = icon,
                    name = name,
                    badges = badges,
                    desc = desc,
                )
            }).collect();

            format!(
                r##"<div class="divide-y divide-gray-100 dark:divide-gray-700/50">{tool_list}</div>"##
            )
        },
        _ => r#"<p class="text-gray-500 dark:text-gray-400 text-center py-8">No MCP tools generated. Configure table permissions to generate tools.</p>"#.to_string(),
    };

    let content = format!(
        r##"<div class="mb-6">
            <a href="/roles" class="text-primary-600 hover:text-primary-700 flex items-center gap-2 mb-4 text-sm">
                <i class="fas fa-arrow-left"></i> Back to Roles
            </a>
            
            <!-- Header -->
            <div class="flex items-start justify-between mb-6">
                <div>
                    <h1 class="text-2xl font-bold text-gray-900 dark:text-white mb-1">{name}</h1>
                    <p class="text-gray-600 dark:text-gray-400">{description}</p>
                </div>
                <div class="flex gap-2">
                    <a href="/roles/{name}/edit" class="px-4 py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium transition-colors text-sm">
                        <i class="fas fa-edit mr-2"></i>Edit
                    </a>
                    <a href="/tokens?role={name}" class="px-4 py-2 bg-green-600 hover:bg-green-700 text-white rounded-lg font-medium transition-colors text-sm">
                        <i class="fas fa-key mr-2"></i>Mint Token
                    </a>
                </div>
            </div>
            
            <!-- Stats cards -->
            <div class="grid grid-cols-4 gap-4 mb-6">
                <div class="bg-white dark:bg-gray-800 rounded-xl p-4 border border-gray-200 dark:border-gray-700">
                    <div class="text-2xl font-bold text-gray-900 dark:text-white">{table_count}</div>
                    <div class="text-sm text-gray-500">Tables</div>
                </div>
                <div class="bg-white dark:bg-gray-800 rounded-xl p-4 border border-gray-200 dark:border-gray-700">
                    <div class="text-2xl font-bold text-blue-600">{read_count}</div>
                    <div class="text-sm text-gray-500">Readable</div>
                </div>
                <div class="bg-white dark:bg-gray-800 rounded-xl p-4 border border-gray-200 dark:border-gray-700">
                    <div class="text-2xl font-bold text-green-600">{write_count}</div>
                    <div class="text-sm text-gray-500">Writable</div>
                </div>
                <div class="bg-white dark:bg-gray-800 rounded-xl p-4 border border-gray-200 dark:border-gray-700">
                    <div class="text-2xl font-bold text-purple-600">{tool_count}</div>
                    <div class="text-sm text-gray-500">MCP Tools</div>
                </div>
            </div>
            
            {approval_badge}
            
            <!-- Main content -->
            <div class="grid grid-cols-3 gap-6 mt-6">
                <!-- Permissions overview (2 cols) -->
                <div class="col-span-2 space-y-6">
                    <!-- Quick overview table -->
                    <div class="bg-white dark:bg-gray-800 rounded-xl border border-gray-200 dark:border-gray-700 overflow-hidden">
                        <div class="px-4 py-3 border-b border-gray-200 dark:border-gray-700">
                            <h3 class="font-semibold text-gray-900 dark:text-white">Permission Matrix</h3>
                        </div>
                        <table class="w-full text-sm">
                            <thead class="bg-gray-50 dark:bg-gray-700/50">
                                <tr>
                                    <th class="py-2 px-4 text-left font-medium text-gray-500 dark:text-gray-400">Table</th>
                                    <th class="py-2 px-4 text-center font-medium text-blue-500">Read</th>
                                    <th class="py-2 px-4 text-center font-medium text-green-500">Create</th>
                                    <th class="py-2 px-4 text-center font-medium text-yellow-500">Update</th>
                                    <th class="py-2 px-4 text-center font-medium text-red-500">Delete</th>
                                </tr>
                            </thead>
                            <tbody>{tables_rows}</tbody>
                        </table>
                    </div>
                    
                    <!-- Detailed permissions -->
                    <div class="bg-white dark:bg-gray-800 rounded-xl border border-gray-200 dark:border-gray-700 overflow-hidden">
                        <div class="px-4 py-3 border-b border-gray-200 dark:border-gray-700">
                            <h3 class="font-semibold text-gray-900 dark:text-white">Column Details</h3>
                            <p class="text-xs text-gray-500 mt-0.5">⚙ = has constraints • 🔒 = requires approval</p>
                        </div>
                        <div class="p-4">{table_details}</div>
                    </div>
                </div>
                
                <!-- MCP Tools (1 col) -->
                <div class="col-span-1">
                    <div class="bg-white dark:bg-gray-800 rounded-xl border border-gray-200 dark:border-gray-700 overflow-hidden sticky top-4">
                        <div class="px-4 py-3 border-b border-gray-200 dark:border-gray-700 flex items-center justify-between">
                            <h3 class="font-semibold text-gray-900 dark:text-white">MCP Tools</h3>
                            <span class="text-xs px-2 py-0.5 bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-300 rounded-full">{tool_count} tools</span>
                        </div>
                        <div class="p-4 max-h-[600px] overflow-y-auto">{mcp_preview}</div>
                    </div>
                </div>
            </div>
        </div>"##,
        name = role.name,
        description = role.description.as_deref().unwrap_or("No description"),
        table_count = table_count,
        read_count = read_count,
        write_count = write_count,
        tool_count = tool_count,
        approval_badge = approval_badge,
        tables_rows = tables_rows,
        table_details = if table_details.is_empty() { "<p class=\"text-gray-500 text-sm\">No detailed permissions configured.</p>".to_string() } else { table_details },
        mcp_preview = mcp_preview,
    );

    layout(&format!("Role: {}", role.name), &content)
}

// Continued in next file...
