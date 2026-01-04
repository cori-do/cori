//! Request handlers for the dashboard.

use axum::response::Html;

/// Handler for the dashboard home page.
pub async fn home() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

/// Handler for the schema browser page.
pub async fn schema_browser() -> Html<String> {
    Html(render_page("Schema Browser", "<p>Schema browser coming soon...</p>"))
}

/// Handler for the roles management page.
pub async fn roles() -> Html<String> {
    Html(render_page("Roles", "<p>Roles management coming soon...</p>"))
}

/// Handler for the tokens page.
pub async fn tokens() -> Html<String> {
    Html(render_page("Tokens", "<p>Token minting coming soon...</p>"))
}

/// Handler for the audit logs page.
pub async fn audit_logs() -> Html<String> {
    Html(render_page("Audit Logs", "<p>Audit logs coming soon...</p>"))
}

/// Handler for the approvals queue page.
pub async fn approvals() -> Html<String> {
    Html(render_page("Approvals", "<p>Approvals queue coming soon...</p>"))
}

/// Handler for the settings page.
pub async fn settings() -> Html<String> {
    Html(render_page("Settings", "<p>Settings coming soon...</p>"))
}

/// Render a page with the given title and content.
fn render_page(title: &str, content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title} - Cori Dashboard</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <script src="https://unpkg.com/htmx.org@1.9.10"></script>
    <script defer src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js"></script>
</head>
<body class="bg-gray-50 min-h-screen">
    <nav class="bg-indigo-600 text-white px-4 py-3">
        <div class="max-w-7xl mx-auto flex items-center justify-between">
            <h1 class="text-xl font-bold">Cori Dashboard</h1>
            <div class="flex gap-4">
                <a href="/" class="hover:text-indigo-200">Home</a>
                <a href="/schema" class="hover:text-indigo-200">Schema</a>
                <a href="/roles" class="hover:text-indigo-200">Roles</a>
                <a href="/tokens" class="hover:text-indigo-200">Tokens</a>
                <a href="/audit" class="hover:text-indigo-200">Audit</a>
                <a href="/approvals" class="hover:text-indigo-200">Approvals</a>
                <a href="/settings" class="hover:text-indigo-200">Settings</a>
            </div>
        </div>
    </nav>
    <main class="max-w-7xl mx-auto px-4 py-8">
        <h2 class="text-2xl font-bold text-gray-900 mb-6">{title}</h2>
        {content}
    </main>
</body>
</html>"#
    )
}

