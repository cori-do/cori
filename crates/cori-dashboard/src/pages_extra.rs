//! Page templates - Part 2: Tokens, Audit, Approvals, Settings.

use crate::templates::{badge, empty_state, input, layout, select, tabs};
use cori_audit::{AuditEvent, AuditEventType};
use cori_core::config::role_definition::RoleDefinition;
use cori_core::CoriConfig;
use cori_mcp::{ApprovalRequest, ApprovalStatus};
use std::collections::HashMap;

// =============================================================================
// Tokens Page
// =============================================================================

pub fn tokens_page(roles: &HashMap<String, RoleDefinition>, selected_role: Option<&str>) -> String {
    let role_options: Vec<(String, String, bool)> = std::iter::once(("".to_string(), "Select a role...".to_string(), selected_role.is_none()))
        .chain(roles.keys().map(|name| {
            (name.clone(), name.clone(), selected_role == Some(name.as_str()))
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
        tabs = tabs("token-tabs", &[
            ("mint", "Mint Token", &mint_token_form(&role_options)),
            ("attenuate", "Attenuate Token", &attenuate_token_form()),
            ("inspect", "Inspect Token", &inspect_token_form()),
        ]),
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
        expires_input = input("expires_in_hours", "Expires In (hours)", "number", "24", "24"),
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
        expires_input = input("expires_in_hours", "Expires In (hours)", "number", "24", "24"),
    )
}

fn inspect_token_form() -> String {
    format!(
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
        
        <div id="inspect-result" class="mt-6"></div>"##
    )
}

/// Token result HTML fragment.
pub fn token_result_fragment(token: &str, token_type: &str, role: Option<&str>, tenant: Option<&str>, expires_at: Option<&str>) -> String {
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

pub fn audit_logs_page(events: &[AuditEvent], filters: &crate::api_types::AuditQueryParams) -> String {
    let event_rows: String = events.iter().map(|e| {
        let event_type_badge = match e.event_type {
            AuditEventType::QueryExecuted => badge("Executed", "green"),
            AuditEventType::QueryFailed => badge("Failed", "red"),
            AuditEventType::AuthenticationFailed => badge("Auth Failed", "red"),
            AuditEventType::AuthorizationDenied => badge("Denied", "orange"),
            AuditEventType::ToolCalled => badge("Tool Call", "blue"),
            AuditEventType::ApprovalRequired => badge("Pending", "yellow"),
            AuditEventType::ActionApproved => badge("Approved", "green"),
            AuditEventType::ActionRejected => badge("Rejected", "red"),
            _ => badge(&format!("{:?}", e.event_type), "gray"),
        };

        format!(
            r##"<tr class="hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer" 
                    @click="selectedEvent = '{event_id}'"
                    hx-get="/api/audit/{event_id}" hx-target="#event-detail" hx-swap="innerHTML">
                <td class="px-4 py-3 text-sm text-gray-500 dark:text-gray-400 whitespace-nowrap">{time}</td>
                <td class="px-4 py-3">{event_type_badge}</td>
                <td class="px-4 py-3 text-sm text-gray-900 dark:text-white">{tenant}</td>
                <td class="px-4 py-3 text-sm text-gray-900 dark:text-white">{role}</td>
                <td class="px-4 py-3 text-sm text-gray-500 dark:text-gray-400 max-w-xs truncate font-mono">{query}</td>
                <td class="px-4 py-3 text-sm text-gray-500 dark:text-gray-400">{duration}</td>
            </tr>"##,
            event_id = e.event_id,
            time = e.occurred_at.format("%H:%M:%S"),
            event_type_badge = event_type_badge,
            tenant = e.tenant_id.as_deref().unwrap_or("-"),
            role = e.role.as_deref().unwrap_or("-"),
            query = e.original_query.as_deref().unwrap_or(e.tool_name.as_deref().unwrap_or("-")),
            duration = e.duration_ms.map(|d| format!("{}ms", d)).unwrap_or("-".to_string()),
        )
    }).collect();

    let content = format!(
        r##"<div class="flex items-center justify-between mb-6">
            <div>
                <h1 class="text-2xl font-bold text-gray-900 dark:text-white">Audit Logs</h1>
                <p class="text-gray-600 dark:text-gray-400">Query execution history and events</p>
            </div>
            <button hx-get="/api/audit" hx-target="#audit-table" hx-swap="innerHTML" 
                    class="flex items-center gap-2 px-4 py-2 bg-gray-100 dark:bg-gray-700 hover:bg-gray-200 dark:hover:bg-gray-600 rounded-lg transition-colors">
                <i class="fas fa-sync-alt"></i>
                <span>Refresh</span>
            </button>
        </div>
        
        <div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 mb-6">
            <div class="p-4 border-b border-gray-200 dark:border-gray-700">
                <form hx-get="/api/audit" hx-target="#audit-table" hx-swap="innerHTML" class="flex flex-wrap gap-4">
                    <input type="text" name="tenant_id" placeholder="Filter by tenant..." value="{tenant_filter}"
                           class="px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-sm">
                    <input type="text" name="role" placeholder="Filter by role..." value="{role_filter}"
                           class="px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-sm">
                    <select name="event_type" class="px-3 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-sm">
                        <option value="">All event types</option>
                        <option value="query_executed">Query Executed</option>
                        <option value="query_failed">Query Failed</option>
                        <option value="authentication_failed">Auth Failed</option>
                        <option value="authorization_denied">Authorization Denied</option>
                        <option value="tool_called">Tool Called</option>
                    </select>
                    <button type="submit" class="px-4 py-2 bg-primary-600 hover:bg-primary-700 text-white rounded-lg text-sm font-medium">
                        <i class="fas fa-filter mr-2"></i>Filter
                    </button>
                </form>
            </div>
            
            <div id="audit-table" class="overflow-x-auto" x-data="{{ selectedEvent: null }}">
                <table class="w-full">
                    <thead class="bg-gray-50 dark:bg-gray-900">
                        <tr>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Time</th>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Event</th>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Tenant</th>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Role</th>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Query/Tool</th>
                            <th class="px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase">Duration</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-gray-200 dark:divide-gray-700">
                        {event_rows}
                    </tbody>
                </table>
                
                {empty_state}
            </div>
        </div>
        
        <div id="event-detail"></div>"##,
        tenant_filter = filters.tenant_id.as_deref().unwrap_or(""),
        role_filter = filters.role.as_deref().unwrap_or(""),
        event_rows = event_rows,
        empty_state = if events.is_empty() {
            r##"<div class="text-center py-12">
                <i class="fas fa-scroll text-4xl text-gray-400 dark:text-gray-600 mb-4"></i>
                <h3 class="text-lg font-medium text-gray-900 dark:text-white">No audit events</h3>
                <p class="text-gray-500 dark:text-gray-400">Events will appear here as queries are executed.</p>
            </div>"##
        } else { "" },
    );

    layout("Audit Logs", &content)
}

/// Audit event detail HTML fragment.
pub fn audit_event_detail_fragment(event: &AuditEvent) -> String {
    let query_section = event.original_query.as_ref().map(|q| {
        let rewritten = event.rewritten_query.as_deref().unwrap_or(q);
        format!(
            r##"<div class="space-y-4">
                <div>
                    <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Original Query</h4>
                    <pre class="bg-gray-900 text-gray-100 p-3 rounded-lg text-sm overflow-x-auto"><code>{original}</code></pre>
                </div>
                <div>
                    <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Rewritten Query (with RLS)</h4>
                    <pre class="bg-gray-900 text-gray-100 p-3 rounded-lg text-sm overflow-x-auto"><code>{rewritten}</code></pre>
                </div>
            </div>"##,
            original = q.replace('<', "&lt;").replace('>', "&gt;"),
            rewritten = rewritten.replace('<', "&lt;").replace('>', "&gt;"),
        )
    }).unwrap_or_default();

    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <div class="flex items-center justify-between mb-4">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white">Event Details</h3>
                <button onclick="document.getElementById('event-detail').innerHTML = ''" class="text-gray-400 hover:text-gray-600">
                    <i class="fas fa-times"></i>
                </button>
            </div>
            
            <div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
                <div>
                    <span class="text-sm text-gray-500 dark:text-gray-400">Event ID</span>
                    <p class="font-mono text-sm text-gray-900 dark:text-white">{event_id}</p>
                </div>
                <div>
                    <span class="text-sm text-gray-500 dark:text-gray-400">Type</span>
                    <p class="font-medium text-gray-900 dark:text-white">{event_type:?}</p>
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
            
            {error_section}
            
            {meta_section}
        </div>"##,
        event_id = event.event_id,
        event_type = event.event_type,
        occurred_at = event.occurred_at.format("%Y-%m-%d %H:%M:%S UTC"),
        duration = event.duration_ms.map(|d| format!("{}ms", d)).unwrap_or("-".to_string()),
        query_section = query_section,
        error_section = event.error.as_ref().map(|e| format!(
            r#"<div class="mt-4 p-4 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded-lg">
                <h4 class="text-sm font-medium text-red-700 dark:text-red-300 mb-1">Error</h4>
                <p class="text-red-600 dark:text-red-400 text-sm">{}</p>
            </div>"#, e
        )).unwrap_or_default(),
        meta_section = if !event.meta.is_null() {
            format!(
                r#"<div class="mt-4">
                    <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Metadata</h4>
                    <pre class="bg-gray-900 text-gray-100 p-3 rounded-lg text-sm overflow-x-auto"><code>{}</code></pre>
                </div>"#,
                serde_json::to_string_pretty(&event.meta).unwrap_or_default()
            )
        } else { String::new() },
    )
}

// =============================================================================
// Approvals Page
// =============================================================================

pub fn approvals_page(approvals: &[ApprovalRequest]) -> String {
    let pending: Vec<_> = approvals.iter().filter(|a| a.status == ApprovalStatus::Pending).collect();
    let decided: Vec<_> = approvals.iter().filter(|a| a.status != ApprovalStatus::Pending).collect();

    let pending_cards: String = pending.iter().map(|a| approval_card(a, true)).collect();
    let history_cards: String = decided.iter().take(20).map(|a| approval_card(a, false)).collect();

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
        tabs = tabs("approval-tabs", &[
            ("pending", &format!("Pending ({})", pending.len()), &if pending.is_empty() {
                empty_state(
                    "check-double",
                    "No Pending Approvals",
                    "All caught up! Actions requiring approval will appear here.",
                    None,
                )
            } else {
                format!(r#"<div class="space-y-4">{}</div>"#, pending_cards)
            }),
            ("history", "History", &if decided.is_empty() {
                empty_state(
                    "history",
                    "No Approval History",
                    "Decided approvals will appear here.",
                    None,
                )
            } else {
                format!(r#"<div class="space-y-4">{}</div>"#, history_cards)
            }),
        ]),
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
        tabs = tabs("settings-tabs", &[
            ("connection", "Connection", &connection_settings(config)),
            ("security", "Security", &security_settings(config)),
            ("guardrails", "Guardrails", &guardrails_settings(config)),
            ("audit", "Audit", &audit_settings(config)),
            ("tenancy", "Tenancy", &tenancy_settings(config)),
        ]),
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
    let public_key_status = config.biscuit.resolve_public_key()
        .ok()
        .flatten()
        .map(|_| "Configured ✓".to_string())
        .unwrap_or_else(|| "Not configured".to_string());
    
    let private_key_status = config.biscuit.resolve_private_key()
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
        vs_enabled = if config.virtual_schema.enabled { "Yes" } else { "No" },
        always_hidden = if config.virtual_schema.always_hidden.is_empty() { "None".to_string() } else { config.virtual_schema.always_hidden.join(", ") },
        always_visible = if config.virtual_schema.always_visible.is_empty() { "None".to_string() } else { config.virtual_schema.always_visible.join(", ") },
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
        max_rows_input = input("max_rows_per_query", "Max Rows per Query", "number", &config.guardrails.max_rows_per_query.to_string(), "10000"),
        max_affected_input = input("max_affected_rows", "Max Affected Rows", "number", &config.guardrails.max_affected_rows.to_string(), "1000"),
        blocked_ops = ["TRUNCATE", "DROP", "ALTER", "CREATE"].iter().map(|op| {
            let checked = if config.guardrails.blocked_operations.contains(&op.to_string()) { "checked" } else { "" };
            format!(
                r#"<label class="flex items-center gap-2">
                    <input type="checkbox" name="blocked_operations[]" value="{op}" {checked}
                           class="w-4 h-4 text-primary-600 rounded border-gray-300">
                    <span class="text-sm">{op}</span>
                </label>"#
            )
        }).collect::<String>(),
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
        log_queries_checked = if config.audit.log_queries { "checked" } else { "" },
        log_results_checked = if config.audit.log_results { "checked" } else { "" },
        retention_input = input("retention_days", "Retention (days)", "number", &config.audit.retention_days.to_string(), "90"),
        storage_backend = config.audit.storage.backend,
    )
}

fn tenancy_settings(config: &CoriConfig) -> String {
    let rules = config.rules.as_ref();
    
    let table_rows: String = rules
        .map(|r| {
            r.tables.iter().map(|(name, tr)| {
                let tenant_column = tr.get_direct_tenant_column()
                    .unwrap_or("-");
                let is_global = tr.global.unwrap_or(false);
                
                format!(
                    r#"<tr class="hover:bg-gray-50 dark:hover:bg-gray-700/50">
                        <td class="px-4 py-3 font-medium">{name}</td>
                        <td class="px-4 py-3"><code class="text-sm">{column}</code></td>
                        <td class="px-4 py-3">{global}</td>
                    </tr>"#,
                    name = name,
                    column = tenant_column,
                    global = if is_global { badge("Global", "blue") } else { badge("Scoped", "green") },
                )
            }).collect::<String>()
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
        } else { "" },
    )
}
