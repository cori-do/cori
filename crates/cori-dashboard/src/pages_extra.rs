//! Page templates - Part 2: Tokens, Audit, Approvals, Settings.

use crate::templates::{badge, empty_state, input, layout, select, tabs};
use cori_audit::{AuditEvent, AuditEventType};
use cori_core::CoriConfig;
use cori_core::config::role_definition::RoleDefinition;
use cori_mcp::{ApprovalRequest, ApprovalStatus};
use serde_json::Value;
use std::collections::HashMap;

// =============================================================================
// Pagination Types
// =============================================================================

/// Pagination information for list pages.
#[derive(Debug, Clone)]
pub struct PaginationInfo {
    pub current_page: usize,
    pub total_pages: usize,
    pub total_count: usize,
    pub page_size: usize,
    pub has_prev: bool,
    pub has_next: bool,
}

// =============================================================================
// Tokens Page
// =============================================================================

pub fn tokens_page(roles: &HashMap<String, RoleDefinition>, selected_role: Option<&str>) -> String {
    let role_options: Vec<(String, String, bool)> = std::iter::once((
        "".to_string(),
        "Select a role...".to_string(),
        selected_role.is_none(),
    ))
    .chain(roles.keys().map(|name| {
        (
            name.clone(),
            name.clone(),
            selected_role == Some(name.as_str()),
        )
    }))
    .collect();

    let content = format!(
        r##"<div class="mb-6">
            <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Token Management</h1>
            <p class="text-gray-600 dark:text-gray-400">
                Generate Biscuit tokens for AI agents. Role tokens define permissions; agent tokens are tenant-scoped.
            </p>
        </div>
        
        {tabs}"##,
        tabs = tabs(
            "token-tabs",
            &[
                ("mint", "Mint Token", &mint_token_form(&role_options)),
                ("attenuate", "Attenuate Token", &attenuate_token_form()),
                ("inspect", "Inspect Token", &inspect_token_form()),
            ]
        ),
    );

    layout("Tokens", &content)
}

fn mint_token_form(role_options: &[(String, String, bool)]) -> String {
    format!(
        r##"<div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Mint Role Token</h3>
                <p class="text-sm text-gray-600 dark:text-gray-400 mb-4">
                    Creates a base token with role permissions. Not tenant-specific.
                </p>
                
                <form hx-post="/api/tokens/mint-role" hx-target="#token-result" hx-swap="innerHTML" class="space-y-4">
                    {role_select}
                    
                    <button type="submit" class="w-full py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium transition-colors">
                        <i class="fas fa-stamp mr-2"></i>Mint Role Token
                    </button>
                </form>
            </div>
            
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Mint Agent Token</h3>
                <p class="text-sm text-gray-600 dark:text-gray-400 mb-4">
                    Creates a tenant-scoped token directly. Combines role + tenant in one step.
                </p>
                
                <form hx-post="/api/tokens/mint-agent" hx-target="#token-result" hx-swap="innerHTML" class="space-y-4">
                    {role_select_2}
                    {tenant_input}
                    {expires_input}
                    
                    <button type="submit" class="w-full py-2 bg-green-600 hover:bg-green-700 text-white rounded-lg font-medium transition-colors">
                        <i class="fas fa-key mr-2"></i>Mint Agent Token
                    </button>
                </form>
            </div>
        </div>
        
        <div id="token-result" class="mt-6"></div>"##,
        role_select = select("role", "Role", role_options),
        role_select_2 = select("role", "Role", role_options),
        tenant_input = input("tenant", "Tenant ID", "text", "", "e.g., acme_corp"),
        expires_input = input(
            "expires_in_hours",
            "Expires In (hours)",
            "number",
            "24",
            "24"
        ),
    )
}

fn attenuate_token_form() -> String {
    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Attenuate Existing Token</h3>
            <p class="text-sm text-gray-600 dark:text-gray-400 mb-4">
                Add tenant restriction and expiration to an existing role token.
            </p>
            
            <form hx-post="/api/tokens/attenuate" hx-target="#token-result" hx-swap="innerHTML" class="space-y-4">
                <div class="space-y-1">
                    <label for="base_token" class="block text-sm font-medium text-gray-700 dark:text-gray-300">Base Token</label>
                    <textarea name="base_token" id="base_token" rows="4" placeholder="Paste your base64-encoded role token here..."
                              class="w-full px-4 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-gray-900 dark:text-white font-mono text-sm focus:ring-2 focus:ring-primary-500"></textarea>
                </div>
                
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    {tenant_input}
                    {expires_input}
                </div>
                
                <button type="submit" class="w-full py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium transition-colors">
                    <i class="fas fa-compress-arrows-alt mr-2"></i>Attenuate Token
                </button>
            </form>
        </div>
        
        <div id="token-result" class="mt-6"></div>"##,
        tenant_input = input("tenant", "Tenant ID", "text", "", "e.g., client_a"),
        expires_input = input(
            "expires_in_hours",
            "Expires In (hours)",
            "number",
            "24",
            "24"
        ),
    )
}

fn inspect_token_form() -> String {
    r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Inspect Token</h3>
            <p class="text-sm text-gray-600 dark:text-gray-400 mb-4">
                View the claims and metadata of a Biscuit token.
            </p>
            
            <form hx-post="/api/tokens/inspect" hx-target="#inspect-result" hx-swap="innerHTML" class="space-y-4">
                <div class="space-y-1">
                    <label for="token" class="block text-sm font-medium text-gray-700 dark:text-gray-300">Token</label>
                    <textarea name="token" id="token" rows="4" placeholder="Paste your base64-encoded token here..."
                              class="w-full px-4 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-gray-900 dark:text-white font-mono text-sm focus:ring-2 focus:ring-primary-500"></textarea>
                </div>
                
                <button type="submit" class="w-full py-2 bg-gray-600 hover:bg-gray-700 text-white rounded-lg font-medium transition-colors">
                    <i class="fas fa-search mr-2"></i>Inspect Token
                </button>
            </form>
        </div>
        
        <div id="inspect-result" class="mt-6"></div>"##.to_string()
}

/// Token result HTML fragment.
pub fn token_result_fragment(
    token: &str,
    token_type: &str,
    role: Option<&str>,
    tenant: Option<&str>,
    expires_at: Option<&str>,
) -> String {
    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <div class="flex items-center justify-between mb-4">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white">Token Generated</h3>
                {type_badge}
            </div>
            
            <div class="space-y-4">
                <div class="grid grid-cols-2 gap-4 text-sm">
                    {role_info}
                    {tenant_info}
                    {expires_info}
                </div>
                
                <div class="relative">
                    <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Token (base64)</label>
                    <div class="bg-gray-900 text-gray-100 p-4 rounded-lg overflow-x-auto">
                        <code id="token-value" class="text-sm break-all">{token}</code>
                    </div>
                    <button onclick="navigator.clipboard.writeText(document.getElementById('token-value').textContent); showToast('Token copied to clipboard!')"
                            class="absolute top-8 right-2 p-2 bg-gray-700 hover:bg-gray-600 rounded text-gray-300 hover:text-white transition-colors">
                        <i class="fas fa-copy"></i>
                    </button>
                </div>
                
                <div class="p-4 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded-lg">
                    <div class="flex items-start gap-3">
                        <i class="fas fa-exclamation-triangle text-yellow-500 mt-0.5"></i>
                        <div class="text-sm text-yellow-700 dark:text-yellow-300">
                            <strong>Security Note:</strong> Store this token securely. It grants access to your database with the specified permissions.
                        </div>
                    </div>
                </div>
            </div>
        </div>"##,
        type_badge = badge(token_type, if token_type == "Agent" { "green" } else { "blue" }),
        token = token,
        role_info = role.map(|r| format!(r#"<div><span class="text-gray-500">Role:</span> <span class="font-medium">{}</span></div>"#, r)).unwrap_or_default(),
        tenant_info = tenant.map(|t| format!(r#"<div><span class="text-gray-500">Tenant:</span> <span class="font-medium">{}</span></div>"#, t)).unwrap_or_default(),
        expires_info = expires_at.map(|e| format!(r#"<div><span class="text-gray-500">Expires:</span> <span class="font-medium">{}</span></div>"#, e)).unwrap_or_default(),
    )
}

/// Token inspection result HTML fragment.
pub fn token_inspect_fragment(info: &crate::api_types::TokenInspectResponse) -> String {
    let status_badge = if info.valid {
        badge("Valid", "green")
    } else {
        badge("Invalid/Expired", "red")
    };

    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <div class="flex items-center justify-between mb-4">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white">Token Details</h3>
                <div class="flex gap-2">
                    {type_badge}
                    {status_badge}
                </div>
            </div>
            
            <div class="space-y-4">
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4">
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Type</span>
                        <p class="font-medium text-gray-900 dark:text-white">{token_type}</p>
                    </div>
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Role</span>
                        <p class="font-medium text-gray-900 dark:text-white">{role}</p>
                    </div>
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Tenant</span>
                        <p class="font-medium text-gray-900 dark:text-white">{tenant}</p>
                    </div>
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Blocks</span>
                        <p class="font-medium text-gray-900 dark:text-white">{block_count}</p>
                    </div>
                </div>
                
                {expires_info}
                
                <div>
                    <span class="text-sm text-gray-500 dark:text-gray-400">Accessible Tables</span>
                    <div class="flex flex-wrap gap-2 mt-2">
                        {tables}
                    </div>
                </div>
            </div>
        </div>"##,
        type_badge = badge(&info.token_type, "blue"),
        status_badge = status_badge,
        token_type = info.token_type,
        role = info.role.as_deref().unwrap_or("Unknown"),
        tenant = info.tenant.as_deref().unwrap_or("Not specified (role token)"),
        block_count = info.block_count,
        expires_info = info.expires_at.map(|e| format!(
            r#"<div class="p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                <span class="text-sm text-gray-500">Expires at:</span>
                <span class="ml-2 font-medium">{}</span>
            </div>"#, e
        )).unwrap_or_default(),
        tables = info.tables.as_ref().map_or(
            "<span class=\"text-gray-500\">None</span>".to_string(),
            |tables| if tables.is_empty() {
                "<span class=\"text-gray-500\">None</span>".to_string()
            } else {
                tables.iter().map(|t| format!(r#"<span class="text-xs bg-gray-100 dark:bg-gray-700 text-gray-600 dark:text-gray-400 px-2 py-1 rounded">{}</span>"#, t)).collect()
            }
        ),
    )
}

// =============================================================================
// Audit Logs Page
// =============================================================================

pub fn audit_logs_page(
    events: &[AuditEvent],
    filters: &crate::api_types::AuditQueryParams,
    pagination: &PaginationInfo,
) -> String {
    // Get current view mode (needed for event rendering)
    let current_view = filters.view.as_deref().unwrap_or("timeline");

    // Generate event rows with hierarchical support
    let event_rows: String = events
        .iter()
        .map(|e| {
            let event_type_badge = match e.event_type {
                AuditEventType::QueryExecuted => badge("Executed", "green"),
                AuditEventType::QueryFailed => badge("Failed", "red"),
                AuditEventType::AuthenticationFailed => badge("Auth Failed", "red"),
                AuditEventType::AuthorizationDenied => badge("Denied", "orange"),
                AuditEventType::ToolCalled => badge("Tool Call", "blue"),
                AuditEventType::ApprovalRequested => badge("Pending", "yellow"),
                AuditEventType::Approved => badge("Approved", "green"),
                AuditEventType::Denied => badge("Rejected", "red"),
            };

            // Visual hierarchy indicators
            let (indent, hierarchy_icon) = if e.parent_event_id.is_some() {
                if current_view == "timeline" {
                    // Timeline view: show visual connector and indentation
                    (
                        "pl-8",
                        r#"<span class="absolute left-2 text-gray-400"><i class="fas fa-level-up-alt fa-rotate-90"></i></span>"#
                    )
                } else {
                    // Grouped view: child rows (should be hidden by default and shown when parent is expanded)
                    ("pl-8", "")
                }
            } else {
                ("", "")
            };

            // For grouped view, add expand/collapse button for root events
            let expand_button = if current_view == "hierarchical" && e.parent_event_id.is_none() {
                format!(
                    r##"<button class="mr-2 text-gray-500 hover:text-gray-700" 
                             onclick="event.stopPropagation(); toggleChildren(this, '{}');">
                        <i class="fas fa-chevron-right"></i>
                    </button>"##,
                    e.event_id
                )
            } else {
                String::new()
            };

            format!(
                r##"<tr class="hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer {row_bg} parent-row" 
                    data-event-id="{event_id}"
                    @click="sidePanelOpen = true"
                    hx-get="/api/audit/{event_id}" hx-target="#event-detail" hx-swap="innerHTML">
                <td class="px-4 py-3 text-sm text-gray-500 dark:text-gray-400 whitespace-nowrap relative {indent}">
                    {hierarchy_icon}
                    {time}
                </td>
                <td class="px-4 py-3">
                    {expand_button}
                    {event_type_badge}
                </td>
                <td class="px-4 py-3 text-sm text-gray-900 dark:text-white">{tenant}</td>
                <td class="px-4 py-3 text-sm text-gray-900 dark:text-white">{role}</td>
                <td class="px-4 py-3 text-sm text-gray-500 dark:text-gray-400 max-w-xs truncate font-mono" title="{query_full}">{query}</td>
                <td class="px-4 py-3 text-sm text-gray-500 dark:text-gray-400">{duration}</td>
            </tr>"##,
                event_id = e.event_id,
                time = e.occurred_at.format("%Y-%m-%d %H:%M:%S"),
                event_type_badge = event_type_badge,
                expand_button = expand_button,
                tenant = html_escape(&e.tenant_id),
                role = html_escape(&e.role),
                query = html_escape(&truncate_str(e.sql.as_deref().unwrap_or(&e.action), 60)),
                query_full = html_escape(e.sql.as_deref().unwrap_or(&e.action)),
                duration = e
                    .duration_ms
                    .map(|d| format!("{}ms", d))
                    .unwrap_or_else(|| "-".to_string()),
                indent = indent,
                hierarchy_icon = hierarchy_icon,
                row_bg = if e.parent_event_id.is_some() { "bg-gray-50/50 dark:bg-gray-700/30" } else { "" },
            )
        })
        .collect();

    // Build base query params for pagination links
    let base_params = build_filter_params(filters);
    let current_sort = filters.sort_by.as_deref().unwrap_or("occurred_at");
    let current_dir = filters.sort_dir.as_deref().unwrap_or("desc");

    // Generate sortable column headers
    let sort_header = |field: &str, label: &str| -> String {
        let is_current = current_sort == field;
        let next_dir = if is_current && current_dir == "desc" {
            "asc"
        } else {
            "desc"
        };
        let icon = if is_current {
            if current_dir == "desc" {
                r#"<i class="fas fa-sort-down ml-1"></i>"#
            } else {
                r#"<i class="fas fa-sort-up ml-1"></i>"#
            }
        } else {
            r#"<i class="fas fa-sort ml-1 opacity-30"></i>"#
        };

        format!(
            r##"<th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase cursor-pointer hover:text-gray-700 dark:hover:text-gray-200"
                hx-get="/audit?{base_params}&sort_by={field}&sort_dir={next_dir}&page=1"
                hx-target="body" hx-push-url="true">
                {label}{icon}
            </th>"##,
            base_params = base_params,
            field = field,
            next_dir = next_dir,
            label = label,
            icon = icon
        )
    };

    // Pagination controls
    let pagination_html = if pagination.total_pages > 1 {
        let mut pages_html = String::new();

        // Previous button
        if pagination.has_prev {
            pages_html.push_str(&format!(
                r##"<a href="/audit?{base_params}&page={prev}&sort_by={sort}&sort_dir={dir}" 
                    class="px-3 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-600 rounded-l-lg hover:bg-gray-50 dark:hover:bg-gray-700">
                    <i class="fas fa-chevron-left"></i>
                </a>"##,
                base_params = base_params,
                prev = pagination.current_page - 1,
                sort = current_sort,
                dir = current_dir
            ));
        } else {
            pages_html.push_str(
                r##"<span class="px-3 py-2 text-sm font-medium text-gray-400 dark:text-gray-600 bg-gray-100 dark:bg-gray-900 border border-gray-300 dark:border-gray-600 rounded-l-lg cursor-not-allowed">
                    <i class="fas fa-chevron-left"></i>
                </span>"##,
            );
        }

        // Page numbers
        let start_page = (pagination.current_page.saturating_sub(2)).max(1);
        let end_page = (start_page + 4).min(pagination.total_pages);

        if start_page > 1 {
            pages_html.push_str(&format!(
                r##"<a href="/audit?{base_params}&page=1&sort_by={sort}&sort_dir={dir}" 
                    class="px-3 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 bg-white dark:bg-gray-800 border-t border-b border-gray-300 dark:border-gray-600 hover:bg-gray-50 dark:hover:bg-gray-700">1</a>"##,
                base_params = base_params,
                sort = current_sort,
                dir = current_dir
            ));
            if start_page > 2 {
                pages_html.push_str(
                    r##"<span class="px-3 py-2 text-sm text-gray-500 dark:text-gray-400 bg-white dark:bg-gray-800 border-t border-b border-gray-300 dark:border-gray-600">...</span>"##,
                );
            }
        }

        for page in start_page..=end_page {
            if page == pagination.current_page {
                pages_html.push_str(&format!(
                    r##"<span class="px-3 py-2 text-sm font-medium text-white bg-primary-600 border border-primary-600">{page}</span>"##,
                    page = page
                ));
            } else {
                pages_html.push_str(&format!(
                    r##"<a href="/audit?{base_params}&page={page}&sort_by={sort}&sort_dir={dir}" 
                        class="px-3 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 bg-white dark:bg-gray-800 border-t border-b border-gray-300 dark:border-gray-600 hover:bg-gray-50 dark:hover:bg-gray-700">{page}</a>"##,
                    base_params = base_params,
                    page = page,
                    sort = current_sort,
                    dir = current_dir
                ));
            }
        }

        if end_page < pagination.total_pages {
            if end_page < pagination.total_pages - 1 {
                pages_html.push_str(
                    r##"<span class="px-3 py-2 text-sm text-gray-500 dark:text-gray-400 bg-white dark:bg-gray-800 border-t border-b border-gray-300 dark:border-gray-600">...</span>"##,
                );
            }
            pages_html.push_str(&format!(
                r##"<a href="/audit?{base_params}&page={last}&sort_by={sort}&sort_dir={dir}" 
                    class="px-3 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 bg-white dark:bg-gray-800 border-t border-b border-gray-300 dark:border-gray-600 hover:bg-gray-50 dark:hover:bg-gray-700">{last}</a>"##,
                base_params = base_params,
                last = pagination.total_pages,
                sort = current_sort,
                dir = current_dir
            ));
        }

        // Next button
        if pagination.has_next {
            pages_html.push_str(&format!(
                r##"<a href="/audit?{base_params}&page={next}&sort_by={sort}&sort_dir={dir}" 
                    class="px-3 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 bg-white dark:bg-gray-800 border border-gray-300 dark:border-gray-600 rounded-r-lg hover:bg-gray-50 dark:hover:bg-gray-700">
                    <i class="fas fa-chevron-right"></i>
                </a>"##,
                base_params = base_params,
                next = pagination.current_page + 1,
                sort = current_sort,
                dir = current_dir
            ));
        } else {
            pages_html.push_str(
                r##"<span class="px-3 py-2 text-sm font-medium text-gray-400 dark:text-gray-600 bg-gray-100 dark:bg-gray-900 border border-gray-300 dark:border-gray-600 rounded-r-lg cursor-not-allowed">
                    <i class="fas fa-chevron-right"></i>
                </span>"##,
            );
        }

        format!(
            r##"<div class="flex items-center justify-between px-4 py-3 bg-gray-50 dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700">
                <div class="text-sm text-gray-600 dark:text-gray-400">
                    Showing {start} - {end} of {total} events
                </div>
                <div class="flex">
                    {pages_html}
                </div>
            </div>"##,
            start = ((pagination.current_page - 1) * pagination.page_size) + 1,
            end = (pagination.current_page * pagination.page_size).min(pagination.total_count),
            total = pagination.total_count,
            pages_html = pages_html
        )
    } else if pagination.total_count > 0 {
        format!(
            r##"<div class="px-4 py-3 bg-gray-50 dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700 text-sm text-gray-600 dark:text-gray-400">
                Showing all {total} events
            </div>"##,
            total = pagination.total_count
        )
    } else {
        String::new()
    };

    let content = format!(
        r##"<div class="flex items-center justify-between mb-6">
            <div>
                <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Audit Logs</h1>
                <p class="text-gray-600 dark:text-gray-400">Query execution history and events ({total_count} total)</p>
            </div>
            <div class="flex items-center gap-3">
                <!-- View Toggle -->
                <div class="flex items-center gap-1 bg-gray-100 dark:bg-gray-700 rounded-lg p-1">
                    <a href="/audit?view=timeline&{base_params}" 
                       class="{timeline_btn_class}">
                        <i class="fas fa-stream mr-2"></i>Timeline
                    </a>
                    <a href="/audit?view=hierarchical&{base_params}" 
                       class="{grouped_btn_class}">
                        <i class="fas fa-sitemap mr-2"></i>Grouped
                    </a>
                </div>
                <a href="/audit" class="flex items-center gap-2 px-4 py-2 bg-gray-100 dark:bg-gray-700 hover:bg-gray-200 dark:hover:bg-gray-600 rounded-lg transition-colors">
                    <i class="fas fa-sync-alt"></i>
                    <span>Refresh</span>
                </a>
            </div>
        </div>
        
        <div class="relative" x-data="{{ sidePanelOpen: false }}">
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 mb-6">
                <div class="p-4 border-b border-gray-200 dark:border-gray-700">
                    <form action="/audit" method="get" class="flex flex-wrap gap-4">
                        <input type="text" name="tenant_id" placeholder="Filter by tenant..." value="{tenant_filter}"
                               class="px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-sm text-gray-900 dark:text-white">
                        <input type="text" name="role" placeholder="Filter by role..." value="{role_filter}"
                               class="px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-sm text-gray-900 dark:text-white">
                        <select name="event_type" class="px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-sm text-gray-900 dark:text-white">
                            <option value="">All event types</option>
                            <option value="query_executed" {sel_executed}>Query Executed</option>
                            <option value="query_failed" {sel_failed}>Query Failed</option>
                            <option value="tool_called" {sel_tool}>Tool Called</option>
                            <option value="approval_requested" {sel_approval}>Approval Requested</option>
                            <option value="approved" {sel_approved}>Approved</option>
                            <option value="denied" {sel_denied}>Denied</option>
                            <option value="authentication_failed" {sel_auth_fail}>Auth Failed</option>
                            <option value="authorization_denied" {sel_authz_denied}>Authorization Denied</option>
                        </select>
                        <input type="hidden" name="sort_by" value="{sort_by}">
                        <input type="hidden" name="sort_dir" value="{sort_dir}">
                        <button type="submit" class="px-4 py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg text-sm font-medium">
                            <i class="fas fa-filter mr-2"></i>Filter
                        </button>
                        <a href="/audit" class="px-4 py-2 bg-gray-200 dark:bg-gray-700 hover:bg-gray-300 dark:hover:bg-gray-600 text-gray-700 dark:text-gray-300 rounded-lg text-sm font-medium">
                            Clear
                        </a>
                    </form>
                </div>
                
                <div id="audit-table" class="overflow-x-auto">
                    <table class="w-full">
                        <thead class="bg-gray-50 dark:bg-gray-900">
                            <tr>
                                {th_time}
                                <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Event</th>
                                {th_tenant}
                                {th_role}
                                {th_action}
                                {th_duration}
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-gray-200 dark:divide-gray-700">
                            {event_rows}
                        </tbody>
                    </table>
                    
                    {empty_state_html}
                </div>
                
                {pagination_html}
            </div>
            
            <!-- Side Panel -->
            <div x-show="sidePanelOpen" 
                 x-transition:enter="transition ease-out duration-300"
                 x-transition:enter-start="opacity-0"
                 x-transition:enter-end="opacity-100"
                 x-transition:leave="transition ease-in duration-200"
                 x-transition:leave-start="opacity-100"
                 x-transition:leave-end="opacity-0"
                 @click="sidePanelOpen = false"
                 class="fixed inset-0 bg-gray-900/50 z-40"
                 style="display: none;">
            </div>
            
            <div x-show="sidePanelOpen"
                 x-transition:enter="transition ease-out duration-300 transform"
                 x-transition:enter-start="translate-x-full"
                 x-transition:enter-end="translate-x-0"
                 x-transition:leave="transition ease-in duration-200 transform"
                 x-transition:leave-start="translate-x-0"
                 x-transition:leave-end="translate-x-full"
                 class="fixed right-0 top-0 h-full w-full md:w-2/3 lg:w-1/2 bg-white dark:bg-gray-800 shadow-2xl z-50 overflow-y-auto"
                 style="display: none;"
                 @click.away="sidePanelOpen = false">
                <div id="event-detail" class="p-6"></div>
            </div>
        </div>
        
        <script>
        function toggleChildren(button, eventId) {{
            const icon = button.querySelector('i');
            const parentRow = button.closest('tr');
            const existingChildren = parentRow.nextElementSibling?.classList.contains('child-row');
            
            if (existingChildren) {{
                // Collapse: remove all child rows
                let nextRow = parentRow.nextElementSibling;
                while (nextRow && nextRow.classList.contains('child-row')) {{
                    const toRemove = nextRow;
                    nextRow = nextRow.nextElementSibling;
                    toRemove.remove();
                }}
                icon.classList.remove('fa-chevron-down');
                icon.classList.add('fa-chevron-right');
            }} else {{
                // Expand: fetch and insert children
                icon.classList.remove('fa-chevron-right');
                icon.classList.add('fa-chevron-down');
                
                fetch(`/api/audit/${{eventId}}/children-rows`)
                    .then(response => response.text())
                    .then(html => {{
                        if (html) {{
                            parentRow.insertAdjacentHTML('afterend', html);
                            // Initialize HTMX for new elements
                            if (window.htmx) {{
                                htmx.process(parentRow.parentElement);
                            }}
                        }}
                    }})
                    .catch(err => console.error('Failed to load children:', err));
            }}
        }}
        </script>"##,
        total_count = pagination.total_count,
        tenant_filter = html_escape(filters.tenant_id.as_deref().unwrap_or("")),
        role_filter = html_escape(filters.role.as_deref().unwrap_or("")),
        sort_by = current_sort,
        sort_dir = current_dir,
        sel_executed = selected_if(filters.event_type.as_deref(), "query_executed"),
        sel_failed = selected_if(filters.event_type.as_deref(), "query_failed"),
        sel_tool = selected_if(filters.event_type.as_deref(), "tool_called"),
        sel_approval = selected_if(filters.event_type.as_deref(), "approval_requested"),
        sel_approved = selected_if(filters.event_type.as_deref(), "approved"),
        sel_denied = selected_if(filters.event_type.as_deref(), "denied"),
        sel_auth_fail = selected_if(filters.event_type.as_deref(), "authentication_failed"),
        sel_authz_denied = selected_if(filters.event_type.as_deref(), "authorization_denied"),
        th_time = sort_header("occurred_at", "Time"),
        th_tenant = sort_header("tenant_id", "Tenant"),
        th_role = sort_header("role", "Role"),
        th_action = sort_header("action", "Action/Query"),
        th_duration = sort_header("duration_ms", "Duration"),
        event_rows = event_rows,
        empty_state_html = if events.is_empty() {
            r##"<div class="text-center py-12">
                <i class="fas fa-scroll text-4xl text-gray-400 dark:text-gray-600 mb-4"></i>
                <h3 class="text-lg font-medium text-gray-900 dark:text-white">No audit events</h3>
                <p class="text-gray-500 dark:text-gray-400">Events will appear here as queries are executed.</p>
            </div>"##
        } else {
            ""
        },
        pagination_html = pagination_html,
        base_params = build_filter_params(filters),
        timeline_btn_class = if current_view == "timeline" {
            "px-3 py-2 text-sm font-medium text-white bg-primary-600 rounded-md transition-colors"
        } else {
            "px-3 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-600 rounded-md transition-colors"
        },
        grouped_btn_class = if current_view == "hierarchical" {
            "px-3 py-2 text-sm font-medium text-white bg-primary-600 rounded-md transition-colors"
        } else {
            "px-3 py-2 text-sm font-medium text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-600 rounded-md transition-colors"
        },
    );

    layout("Audit Logs", &content)
}

/// Build query params string from filters (excluding pagination/sort).
fn build_filter_params(filters: &crate::api_types::AuditQueryParams) -> String {
    let mut params = Vec::new();
    if let Some(ref t) = filters.tenant_id
        && !t.is_empty() {
            params.push(format!("tenant_id={}", urlencoding::encode(t)));
        }
    if let Some(ref r) = filters.role
        && !r.is_empty() {
            params.push(format!("role={}", urlencoding::encode(r)));
        }
    if let Some(ref e) = filters.event_type
        && !e.is_empty() {
            params.push(format!("event_type={}", urlencoding::encode(e)));
        }
    params.join("&")
}

/// Return "selected" if value matches target.
fn selected_if(value: Option<&str>, target: &str) -> &'static str {
    if value == Some(target) {
        "selected"
    } else {
        ""
    }
}

/// HTML escape a string.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Truncate a string to max length, adding ellipsis if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Audit event detail HTML fragment.
pub fn audit_event_detail_fragment(event: &AuditEvent) -> String {
    let query_section = event.sql.as_ref().map(|q| {
        format!(
            r##"<div class="mb-6">
                <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">SQL Query</h4>
                <pre class="bg-gray-900 text-gray-100 p-3 rounded-lg text-sm overflow-x-auto"><code>{sql}</code></pre>
            </div>"##,
            sql = q.replace('<', "&lt;").replace('>', "&gt;"),
        )
    }).unwrap_or_default();

    let json_section = |title: &str, value: &serde_json::Value| -> String {
        let pretty = serde_json::to_string_pretty(value).unwrap_or_default();
        let escaped = pretty.replace('<', "&lt;").replace('>', "&gt;");
        format!(
            r##"<div class="mb-6">
                <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">{}</h4>
                <pre class="bg-gray-900 text-gray-100 p-3 rounded-lg text-sm overflow-x-auto"><code>{}</code></pre>
            </div>"##,
            title, escaped
        )
    };

    let arguments_section = event
        .arguments
        .as_ref()
        .map(|args| json_section("Arguments", args))
        .unwrap_or_default();

    let before_section = event
        .before_state
        .as_ref()
        .map(|state| json_section("Before State", state))
        .unwrap_or_default();

    let after_section = event
        .after_state
        .as_ref()
        .map(|state| json_section("After State", state))
        .unwrap_or_default();

    let diff_section = event
        .diff
        .as_ref()
        .map(|diff| json_section("Diff", diff))
        .unwrap_or_default();

    // Related Events section (hierarchical workflow)
    let related_events_section = if event.parent_event_id.is_some()
        || event.correlation_id.is_some()
    {
        let parent_link = event.parent_event_id.map(|pid| format!(
            r##"<div class="flex items-center gap-2 p-3 bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800 rounded-lg cursor-pointer hover:bg-blue-100 dark:hover:bg-blue-900/30"
                 hx-get="/api/audit/{}" hx-target="#event-detail" hx-swap="innerHTML">
                <i class="fas fa-level-up-alt fa-rotate-90 text-blue-600"></i>
                <span class="text-sm text-gray-700 dark:text-gray-300">Parent Event</span>
                <span class="text-xs font-mono text-gray-500">{}</span>
            </div>"##, pid, pid
        )).unwrap_or_default();

        let children_section = format!(
            r##"<div hx-get="/api/audit/{}/children" hx-trigger="load" hx-swap="innerHTML">
                <div class="text-sm text-gray-500 italic">Loading children...</div>
            </div>"##,
            event.event_id
        );

        let correlation_badge = event.correlation_id.as_ref().map(|cid| format!(
            r##"<div class="flex items-center gap-2">
                <span class="text-sm text-gray-500">Workflow ID:</span>
                <span class="text-xs font-mono bg-gray-100 dark:bg-gray-700 px-2 py-1 rounded">{}</span>
                <a href="/audit?correlation_id={}" class="text-sm text-primary-600 hover:text-primary-700">
                    <i class="fas fa-external-link-alt"></i> View all
                </a>
            </div>"##, cid, cid
        )).unwrap_or_default();

        format!(
            r##"<div class="mb-6 p-4 bg-gray-50 dark:bg-gray-800/50 border border-gray-200 dark:border-gray-700 rounded-lg">
                <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-3">
                    <i class="fas fa-project-diagram mr-2"></i>Related Events
                </h4>
                <div class="space-y-3">
                    {}
                    {}
                    <div>
                        <h5 class="text-xs font-medium text-gray-600 dark:text-gray-400 mb-2">Child Events</h5>
                        {}
                    </div>
                </div>
            </div>"##,
            correlation_badge, parent_link, children_section
        )
    } else {
        String::new()
    };

    format!(
        r##"<div class="h-full flex flex-col">
            <div class="flex items-center justify-between mb-6 pb-4 border-b border-gray-200 dark:border-gray-700">
                <h3 class="text-xl font-semibold text-gray-900 dark:text-white">Event Details</h3>
                <button @click="sidePanelOpen = false" class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 p-2">
                    <i class="fas fa-times text-xl"></i>
                </button>
            </div>
            
            <div class="flex-1 overflow-y-auto">
                <div class="grid grid-cols-2 gap-4 mb-6">
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Event ID</span>
                        <p class="font-mono text-sm text-gray-900 dark:text-white break-all">{event_id}</p>
                    </div>
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Type</span>
                        <p class="font-medium text-gray-900 dark:text-white">{event_type:?}</p>
                    </div>
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Tenant</span>
                        <p class="text-gray-900 dark:text-white">{tenant_id}</p>
                    </div>
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Role</span>
                        <p class="text-gray-900 dark:text-white">{role}</p>
                    </div>
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Occurred At</span>
                        <p class="text-gray-900 dark:text-white">{occurred_at}</p>
                    </div>
                    <div>
                        <span class="text-sm text-gray-500 dark:text-gray-400">Duration</span>
                        <p class="text-gray-900 dark:text-white">{duration}</p>
                    </div>
                </div>
                
                {query_section}

                {arguments_section}
                {before_section}
                {after_section}
                {diff_section}
                
                {error_section}
                
                {meta_section}
                
                {related_events_section}
            </div>
        </div>"##,
        event_id = event.event_id,
        event_type = event.event_type,
        tenant_id = html_escape(&event.tenant_id),
        role = html_escape(&event.role),
        occurred_at = event.occurred_at.format("%Y-%m-%d %H:%M:%S UTC"),
        duration = event.duration_ms.map(|d| format!("{}ms", d)).unwrap_or("-".to_string()),
        query_section = query_section,
        arguments_section = arguments_section,
        before_section = before_section,
        after_section = after_section,
        diff_section = diff_section,
        error_section = event.error.as_ref().map(|e| format!(
            r#"<div class="mb-6 p-4 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded-lg">
                <h4 class="text-sm font-medium text-red-700 dark:text-red-300 mb-1">Error</h4>
                <p class="text-red-600 dark:text-red-400 text-sm">{}</p>
            </div>"#, html_escape(e)
        )).unwrap_or_default(),
        meta_section = if !event.meta.is_null() {
            format!(
                r#"<div class="mb-6">
                    <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Metadata</h4>
                    <pre class="bg-gray-900 text-gray-100 p-3 rounded-lg text-sm overflow-x-auto"><code>{}</code></pre>
                </div>"#,
                serde_json::to_string_pretty(&event.meta).unwrap_or_default()
            )
        } else { String::new() },
        related_events_section = related_events_section,
    )
}

// =============================================================================
// Approvals Page
// =============================================================================

pub fn approvals_page(approvals: &[ApprovalRequest]) -> String {
    let pending: Vec<_> = approvals
        .iter()
        .filter(|a| a.status == ApprovalStatus::Pending)
        .collect();
    let decided: Vec<_> = approvals
        .iter()
        .filter(|a| a.status != ApprovalStatus::Pending)
        .collect();

    let pending_cards: String = pending.iter().map(|a| approval_card(a, true)).collect();
    let history_cards: String = decided
        .iter()
        .take(20)
        .map(|a| approval_card(a, false))
        .collect();

    let content = format!(
        r##"<div class="flex items-center justify-between mb-6">
            <div>
                <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Approvals Queue</h1>
                <p class="text-gray-600 dark:text-gray-400">
                    Review and decide on actions requiring human approval
                </p>
            </div>
            <div class="flex items-center gap-2">
                <span class="flex items-center gap-2 px-3 py-1 bg-yellow-100 dark:bg-yellow-900/30 text-yellow-800 dark:text-yellow-300 rounded-full text-sm">
                    <span class="w-2 h-2 bg-yellow-500 rounded-full animate-pulse"></span>
                    {pending_count} pending
                </span>
            </div>
        </div>
        
        {tabs}"##,
        pending_count = pending.len(),
        tabs = tabs(
            "approval-tabs",
            &[
                (
                    "pending",
                    &format!("Pending ({})", pending.len()),
                    &if pending.is_empty() {
                        empty_state(
                            "check-double",
                            "No Pending Approvals",
                            "All caught up! Actions requiring approval will appear here.",
                            None,
                        )
                    } else {
                        format!(r#"<div class="space-y-4">{}</div>"#, pending_cards)
                    }
                ),
                (
                    "history",
                    "History",
                    &if decided.is_empty() {
                        empty_state(
                            "history",
                            "No Approval History",
                            "Decided approvals will appear here.",
                            None,
                        )
                    } else {
                        format!(r#"<div class="space-y-4">{}</div>"#, history_cards)
                    }
                ),
            ]
        ),
    );

    layout("Approvals", &content)
}

fn approval_card(approval: &ApprovalRequest, show_actions: bool) -> String {
    let status_badge = match approval.status {
        ApprovalStatus::Pending => badge("Pending", "yellow"),
        ApprovalStatus::Approved => badge("Approved", "green"),
        ApprovalStatus::Rejected => badge("Rejected", "red"),
        ApprovalStatus::Expired => badge("Expired", "gray"),
        ApprovalStatus::Cancelled => badge("Cancelled", "gray"),
    };

    let actions = if show_actions && approval.status == ApprovalStatus::Pending {
        format!(
            r##"<div class="flex gap-2 pt-4 border-t border-gray-200 dark:border-gray-700 mt-4">
                <button hx-post="/api/approvals/{id}/approve" hx-swap="none"
                        class="flex-1 py-2 bg-green-600 hover:bg-green-700 text-white rounded-lg font-medium transition-colors">
                    <i class="fas fa-check mr-2"></i>Approve
                </button>
                <button hx-post="/api/approvals/{id}/reject" hx-swap="none"
                        class="flex-1 py-2 bg-red-600 hover:bg-red-700 text-white rounded-lg font-medium transition-colors">
                    <i class="fas fa-times mr-2"></i>Reject
                </button>
            </div>"##,
            id = approval.id,
        )
    } else {
        String::new()
    };

    let args_json = serde_json::to_string_pretty(&approval.arguments)
        .unwrap_or_default()
        .replace('<', "&lt;")
        .replace('>', "&gt;");

    let diff_table = approval_diff_table(approval);

    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <div class="flex items-start justify-between mb-4">
                <div>
                    <div class="flex items-center gap-3 mb-1">
                        <h3 class="text-lg font-semibold text-gray-900 dark:text-white font-mono">{tool_name}</h3>
                        {status_badge}
                    </div>
                    <p class="text-sm text-gray-500 dark:text-gray-400">
                        <span class="font-medium">{tenant}</span> / {role}
                    </p>
                </div>
                <div class="text-right text-sm text-gray-500 dark:text-gray-400">
                    <div>Created: {created_at}</div>
                    <div>Expires: {expires_at}</div>
                </div>
            </div>
            
            <div class="mb-4">
                <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Fields Requiring Approval</h4>
                <div class="flex flex-wrap gap-2">
                    {approval_fields}
                </div>
            </div>

            {diff_table}
            
            <div x-data="{{ showArgs: false }}">
                <button @click="showArgs = !showArgs" class="text-sm text-primary-600 hover:text-primary-700 mb-2">
                    <i class="fas" :class="showArgs ? 'fa-chevron-up' : 'fa-chevron-down'"></i>
                    <span x-text="showArgs ? 'Hide arguments' : 'Show arguments'"></span>
                </button>
                <div x-show="showArgs" x-collapse>
                    <pre class="bg-gray-900 text-gray-100 p-3 rounded-lg text-sm overflow-x-auto"><code>{args}</code></pre>
                </div>
            </div>
            
            {decided_info}
            {actions}
        </div>"##,
        tool_name = approval.tool_name,
        status_badge = status_badge,
        tenant = approval.tenant_id,
        role = approval.role,
        created_at = approval.created_at.format("%Y-%m-%d %H:%M"),
        expires_at = approval.expires_at.format("%Y-%m-%d %H:%M"),
        approval_fields = approval.approval_fields.iter()
            .map(|f| format!(r#"<span class="text-xs bg-yellow-100 dark:bg-yellow-900/30 text-yellow-800 dark:text-yellow-300 px-2 py-1 rounded">{}</span>"#, f))
            .collect::<String>(),
        diff_table = diff_table,
        args = args_json,
        decided_info = if approval.decided_at.is_some() {
            let reason_html = approval.reason.as_ref()
                .map(|r| format!(r#"<br><span class="text-gray-500">Reason:</span> {}"#, r))
                .unwrap_or_default();
            format!(
                r#"<div class="mt-4 p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg text-sm">
                    <span class="text-gray-500">Decided by:</span> <span class="font-medium">{}</span>
                    {}
                </div>"#,
                approval.decided_by.as_deref().unwrap_or("Unknown"),
                reason_html,
            )
        } else { String::new() },
        actions = actions,
    )
}

fn approval_diff_table(approval: &ApprovalRequest) -> String {
    let args_obj = approval.arguments.as_object();
    let before_obj = approval
        .original_values
        .as_ref()
        .and_then(|v| v.as_object());

    if args_obj.is_none() && before_obj.is_none() {
        return String::new();
    }

    let before_map = before_obj.cloned().unwrap_or_default();
    let mut after_map = before_map.clone();

    let is_delete = approval.tool_name.to_lowercase().starts_with("delete");
    if is_delete {
        for (key, _) in before_map.iter() {
            after_map.insert(key.clone(), Value::Null);
        }
    } else if let Some(args) = args_obj {
        for (key, value) in args {
            if key == "id" {
                continue;
            }
            after_map.insert(key.clone(), value.clone());
        }
    }

    if before_map.is_empty() && after_map.is_empty() {
        return String::new();
    }

    let mut keys: Vec<String> = before_map.keys().chain(after_map.keys()).cloned().collect();
    keys.sort();
    keys.dedup();

    let rows: String = keys
        .into_iter()
        .map(|key| {
            let before_value = before_map.get(&key);
            let after_value = after_map.get(&key);
            let changed = before_value != after_value;
            let row_class = if changed {
                "bg-yellow-50 dark:bg-yellow-900/20"
            } else {
                ""
            };

            format!(
                r#"<tr class="border-t border-gray-200 dark:border-gray-700 {row_class}">
                    <td class="px-3 py-2 text-gray-700 dark:text-gray-300 font-mono">{field}</td>
                    <td class="px-3 py-2 text-gray-600 dark:text-gray-400">{before}</td>
                    <td class="px-3 py-2 text-gray-900 dark:text-gray-100 font-medium">{after}</td>
                </tr>"#,
                row_class = row_class,
                field = escape_html(&key),
                before = value_to_string(before_value),
                after = value_to_string(after_value),
            )
        })
        .collect();

    format!(
        r#"<div class="mb-4">
            <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Before / After</h4>
            <div class="overflow-x-auto">
                <table class="min-w-full text-sm border border-gray-200 dark:border-gray-700 rounded-lg overflow-hidden">
                    <thead class="bg-gray-50 dark:bg-gray-700/50 text-gray-600 dark:text-gray-300">
                        <tr>
                            <th class="px-3 py-2 text-left font-medium">Field</th>
                            <th class="px-3 py-2 text-left font-medium">Before</th>
                            <th class="px-3 py-2 text-left font-medium">After</th>
                        </tr>
                    </thead>
                    <tbody>{rows}</tbody>
                </table>
            </div>
        </div>"#,
        rows = rows
    )
}

fn value_to_string(value: Option<&Value>) -> String {
    let text = match value {
        None => "—".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    escape_html(&text)
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// =============================================================================
// Settings Page
// =============================================================================

pub fn settings_page(config: &CoriConfig) -> String {
    let content = format!(
        r##"<div class="mb-6">
            <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Settings</h1>
            <p class="text-gray-600 dark:text-gray-400">
                Configure Cori MCP server, security, and operational settings
            </p>
        </div>
        
        {tabs}"##,
        tabs = tabs(
            "settings-tabs",
            &[
                ("connection", "Connection", &connection_settings(config)),
                ("security", "Security", &security_settings(config)),
                ("guardrails", "Guardrails", &guardrails_settings(config)),
                ("audit", "Audit", &audit_settings(config)),
                ("tenancy", "Tenancy", &tenancy_settings(config)),
            ]
        ),
    );

    layout("Settings", &content)
}

fn connection_settings(config: &CoriConfig) -> String {
    format!(
        r##"<div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Upstream Database</h3>
                <div class="space-y-4">
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">Host</span>
                        <code class="text-sm">{host}</code>
                    </div>
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">Port</span>
                        <code class="text-sm">{port}</code>
                    </div>
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">Database</span>
                        <code class="text-sm">{database}</code>
                    </div>
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">SSL Mode</span>
                        <code class="text-sm">{ssl_mode}</code>
                    </div>
                </div>
            </div>
            
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">MCP Server</h3>
                <div class="space-y-4">
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">Enabled</span>
                        <span class="font-medium">{mcp_enabled}</span>
                    </div>
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">Transport</span>
                        <code class="text-sm">{mcp_transport}</code>
                    </div>
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">HTTP Port</span>
                        <code class="text-sm">{mcp_http_port}</code>
                    </div>
                </div>
            </div>
            
            <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Dashboard</h3>
                <div class="space-y-4">
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">Listen Port</span>
                        <code class="text-sm">{dashboard_port}</code>
                    </div>
                    <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                        <span class="text-gray-600 dark:text-gray-400">Auth Type</span>
                        <code class="text-sm">{auth_type:?}</code>
                    </div>
                </div>
            </div>
        </div>"##,
        host = config.upstream.host,
        port = config.upstream.port,
        database = config.upstream.database,
        ssl_mode = "default",
        mcp_enabled = if config.mcp.enabled { "Yes" } else { "No" },
        mcp_transport = format!("{:?}", config.mcp.transport),
        mcp_http_port = config.mcp.get_port(),
        dashboard_port = config.dashboard.get_port(),
        auth_type = config.dashboard.auth.auth_type,
    )
}

fn security_settings(config: &CoriConfig) -> String {
    let public_key_status = config
        .biscuit
        .resolve_public_key()
        .ok()
        .flatten()
        .map(|_| "Configured ✓".to_string())
        .unwrap_or_else(|| "Not configured".to_string());

    let private_key_status = config
        .biscuit
        .resolve_private_key()
        .ok()
        .flatten()
        .map(|_| "Configured ✓")
        .unwrap_or("Not configured");

    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Biscuit Configuration</h3>
            <div class="space-y-4">
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Public Key</span>
                    <span class="text-sm text-gray-900 dark:text-white">{public_key}</span>
                </div>
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Private Key</span>
                    <span class="text-gray-500">{private_key_status}</span>
                </div>
            </div>
            
            <div class="mt-6 p-4 bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800 rounded-lg">
                <div class="flex items-start gap-3">
                    <i class="fas fa-info-circle text-blue-500 mt-0.5"></i>
                    <div class="text-sm text-blue-700 dark:text-blue-300">
                        <strong>Key Management:</strong> Keys are loaded from the configuration file or environment variables. 
                        Use <code>cori keys generate</code> to create new keypairs.
                    </div>
                </div>
            </div>
        </div>
        
        <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6 mt-6">
            <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Virtual Schema</h3>
            <div class="space-y-4">
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Enabled</span>
                    <span class="font-medium">{vs_enabled}</span>
                </div>
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Always Hidden Tables</span>
                    <span class="text-sm">{always_hidden}</span>
                </div>
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Always Visible Tables</span>
                    <span class="text-sm">{always_visible}</span>
                </div>
            </div>
        </div>"##,
        public_key = public_key_status,
        private_key_status = private_key_status,
        vs_enabled = if config.virtual_schema.enabled {
            "Yes"
        } else {
            "No"
        },
        always_hidden = if config.virtual_schema.always_hidden.is_empty() {
            "None".to_string()
        } else {
            config.virtual_schema.always_hidden.join(", ")
        },
        always_visible = if config.virtual_schema.always_visible.is_empty() {
            "None".to_string()
        } else {
            config.virtual_schema.always_visible.join(", ")
        },
    )
}

fn guardrails_settings(config: &CoriConfig) -> String {
    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Query Limits</h3>
            <form hx-put="/api/settings/guardrails" hx-swap="none" class="space-y-4">
                {max_rows_input}
                {max_affected_input}
                
                <div class="space-y-2">
                    <label class="block text-sm font-medium text-gray-700 dark:text-gray-300">Blocked Operations</label>
                    <div class="flex flex-wrap gap-2">
                        {blocked_ops}
                    </div>
                </div>
                
                <button type="submit" class="w-full py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium">
                    Save Guardrails
                </button>
            </form>
        </div>"##,
        max_rows_input = input(
            "max_rows_per_query",
            "Max Rows per Query",
            "number",
            &config.guardrails.max_rows_per_query.to_string(),
            "10000"
        ),
        max_affected_input = input(
            "max_affected_rows",
            "Max Affected Rows",
            "number",
            &config.guardrails.max_affected_rows.to_string(),
            "1000"
        ),
        blocked_ops = ["TRUNCATE", "DROP", "ALTER", "CREATE"]
            .iter()
            .map(|op| {
                let checked = if config
                    .guardrails
                    .blocked_operations
                    .contains(&op.to_string())
                {
                    "checked"
                } else {
                    ""
                };
                format!(
                    r#"<label class="flex items-center gap-2">
                    <input type="checkbox" name="blocked_operations[]" value="{op}" {checked}
                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                    <span class="text-sm">{op}</span>
                </label>"#
                )
            })
            .collect::<String>(),
    )
}

fn audit_settings(config: &CoriConfig) -> String {
    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <h3 class="text-lg font-semibold text-gray-900 dark:text-white mb-4">Audit Logging</h3>
            <form hx-put="/api/settings/audit" hx-swap="none" class="space-y-4">
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Enabled</span>
                    <label class="relative inline-flex items-center cursor-pointer">
                        <input type="checkbox" name="enabled" class="sr-only peer" {enabled_checked}>
                        <div class="w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-primary-300 dark:peer-focus:ring-primary-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-primary-600"></div>
                    </label>
                </div>
                
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Log Queries</span>
                    <label class="relative inline-flex items-center cursor-pointer">
                        <input type="checkbox" name="log_queries" class="sr-only peer" {log_queries_checked}>
                        <div class="w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-primary-300 dark:peer-focus:ring-primary-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-primary-600"></div>
                    </label>
                </div>
                
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Log Results</span>
                    <label class="relative inline-flex items-center cursor-pointer">
                        <input type="checkbox" name="log_results" class="sr-only peer" {log_results_checked}>
                        <div class="w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-primary-300 dark:peer-focus:ring-primary-800 rounded-full peer dark:bg-gray-700 peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all dark:border-gray-600 peer-checked:bg-primary-600"></div>
                    </label>
                </div>
                
                {retention_input}
                
                <div class="flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-700/50 rounded-lg">
                    <span class="text-gray-600 dark:text-gray-400">Storage Backend</span>
                    <code class="text-sm">{storage_backend:?}</code>
                </div>
                
                <button type="submit" class="w-full py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg font-medium">
                    Save Audit Settings
                </button>
            </form>
        </div>"##,
        enabled_checked = if config.audit.enabled { "checked" } else { "" },
        log_queries_checked = if config.audit.log_queries {
            "checked"
        } else {
            ""
        },
        log_results_checked = if config.audit.log_results {
            "checked"
        } else {
            ""
        },
        retention_input = input(
            "retention_days",
            "Retention (days)",
            "number",
            &config.audit.retention_days.to_string(),
            "90"
        ),
        storage_backend = config.audit.storage.backend,
    )
}

fn tenancy_settings(config: &CoriConfig) -> String {
    let rules = config.rules.as_ref();

    let table_rows: String = rules
        .map(|r| {
            r.tables
                .iter()
                .map(|(name, tr)| {
                    let tenant_column = tr.get_direct_tenant_column().unwrap_or("-");
                    let is_global = tr.global.unwrap_or(false);

                    format!(
                        r#"<tr class="hover:bg-gray-50 dark:hover:bg-gray-700/50">
                        <td class="px-4 py-3 font-medium">{name}</td>
                        <td class="px-4 py-3"><code class="text-sm">{column}</code></td>
                        <td class="px-4 py-3">{global}</td>
                    </tr>"#,
                        name = name,
                        column = tenant_column,
                        global = if is_global {
                            badge("Global", "blue")
                        } else {
                            badge("Scoped", "green")
                        },
                    )
                })
                .collect::<String>()
        })
        .unwrap_or_default();

    let table_count = rules.map(|r| r.tables.len()).unwrap_or(0);

    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 overflow-hidden">
            <div class="px-6 py-4 border-b border-gray-200 dark:border-gray-700">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white">Table Configuration (from schema/rules.yaml)</h3>
            </div>
            <div class="overflow-x-auto">
                <table class="w-full">
                    <thead class="bg-gray-50 dark:bg-gray-900">
                        <tr>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Table</th>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Tenant Column</th>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Scope</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-gray-200 dark:divide-gray-700">
                        {table_rows}
                    </tbody>
                </table>
            </div>
            {empty_state}
        </div>
        
        <div class="mt-6 p-4 bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800 rounded-lg">
            <div class="flex items-start gap-3">
                <i class="fas fa-info-circle text-blue-500 mt-0.5"></i>
                <div class="text-sm text-blue-700 dark:text-blue-300">
                    <strong>Rules Configuration:</strong> Edit the <code>schema/rules.yaml</code> file to modify tenancy configuration. 
                    Changes require a server restart.
                </div>
            </div>
        </div>"##,
        table_rows = table_rows,
        empty_state = if table_count == 0 {
            r#"<div class="text-center py-8 text-gray-500">No tables configured in schema/rules.yaml</div>"#
        } else {
            ""
        },
    )
}
