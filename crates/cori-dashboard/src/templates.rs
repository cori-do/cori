//! HTML templates for the dashboard.
//!
//! Uses a simple template approach with Tailwind CSS and HTMX.

/// Base HTML layout wrapper.
pub fn layout(title: &str, content: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en" x-data="{{
    darkMode: localStorage.getItem('darkMode') === 'true',
    sidebarOpen: true
}}" :class="{{ 'dark': darkMode }}">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title} - Cori Dashboard</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <script>
        tailwind.config = {{
            darkMode: 'class',
            theme: {{
                extend: {{
                    colors: {{
                        primary: {{
                            50: '#eef2ff',
                            100: '#e0e7ff',
                            200: '#c7d2fe',
                            300: '#a5b4fc',
                            400: '#818cf8',
                            500: '#6366f1',
                            600: '#4f46e5',
                            700: '#4338ca',
                            800: '#3730a3',
                            900: '#312e81',
                        }}
                    }}
                }}
            }}
        }}
    </script>
    <script src="https://unpkg.com/htmx.org@1.9.10"></script>
    <script defer src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js"></script>
    <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.5.1/css/all.min.css">
    <style>
        [x-cloak] {{ display: none !important; }}
        .htmx-indicator {{ display: none; }}
        .htmx-request .htmx-indicator {{ display: inline-block; }}
        .htmx-request.htmx-indicator {{ display: inline-block; }}
    </style>
</head>
<body class="bg-gray-50 dark:bg-gray-900 min-h-screen">
    {NAV}
    
    <div class="flex">
        {SIDEBAR}
        
        <main class="flex-1 p-6 lg:p-8">
            <div class="max-w-7xl mx-auto">
                {content}
            </div>
        </main>
    </div>
    
    {TOAST}
    
    <script>
        // Global HTMX configuration
        document.body.addEventListener('htmx:afterSwap', function(evt) {{
            // Re-initialize Alpine components after HTMX swap
            if (typeof Alpine !== 'undefined') {{
                Alpine.initTree(evt.detail.target);
            }}
        }});
        
        // Toast notification helper
        function showToast(message, type = 'success') {{
            const toast = document.getElementById('toast');
            const toastMessage = document.getElementById('toast-message');
            toastMessage.textContent = message;
            toast.className = toast.className.replace(/bg-\w+-500/, type === 'error' ? 'bg-red-500' : 'bg-green-500');
            toast.classList.remove('hidden');
            setTimeout(() => toast.classList.add('hidden'), 3000);
        }}
    </script>
</body>
</html>"##,
        title = title,
        NAV = nav_template(),
        SIDEBAR = sidebar_template(),
        TOAST = toast_template(),
    )
}

fn nav_template() -> &'static str {
    r##"<nav class="bg-primary-600 dark:bg-primary-900 text-white px-4 py-3 sticky top-0 z-50 shadow-lg">
        <div class="flex items-center justify-between">
            <div class="flex items-center gap-4">
                <button @click="sidebarOpen = !sidebarOpen" class="p-2 hover:bg-primary-700 rounded-lg lg:hidden">
                    <i class="fas fa-bars"></i>
                </button>
                <a href="/" class="flex items-center gap-2">
                    <img src="https://assets.cori.do/cori-logo.png" alt="Cori" class="h-8">
                    <span class="text-sm bg-primary-500 dark:bg-primary-700 px-2 py-1 rounded">Dashboard</span>
                </a>
            </div>
            <div class="flex items-center gap-4">
                <div class="hidden md:flex items-center gap-2 text-sm">
                    <span class="flex items-center gap-1">
                        <span class="w-2 h-2 bg-green-400 rounded-full animate-pulse"></span>
                        Connected
                    </span>
                </div>
                <button @click="darkMode = !darkMode; localStorage.setItem('darkMode', darkMode)"
                        class="p-2 hover:bg-primary-700 rounded-lg">
                    <i class="fas" :class="darkMode ? 'fa-sun' : 'fa-moon'"></i>
                </button>
            </div>
        </div>
    </nav>"##
}

fn sidebar_template() -> &'static str {
    r##"<aside class="w-64 bg-white dark:bg-gray-800 border-r border-gray-200 dark:border-gray-700 min-h-[calc(100vh-56px)] transition-all duration-300"
              :class="{ '-ml-64': !sidebarOpen }"
              x-cloak>
            <nav class="p-4 space-y-2">
                <a href="/" class="flex items-center gap-3 px-4 py-3 text-gray-700 dark:text-gray-200 hover:bg-primary-50 dark:hover:bg-primary-900/50 rounded-lg transition-colors"
                   :class="{ 'bg-primary-50 dark:bg-primary-900/50 text-primary-600 dark:text-primary-400': window.location.pathname === '/' }">
                    <i class="fas fa-home w-5"></i>
                    <span>Home</span>
                </a>
                
                <div class="pt-4 pb-2 px-4 text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider">
                    Database
                </div>
                <a href="/schema" class="flex items-center gap-3 px-4 py-3 text-gray-700 dark:text-gray-200 hover:bg-primary-50 dark:hover:bg-primary-900/50 rounded-lg transition-colors">
                    <i class="fas fa-database w-5"></i>
                    <span>Schema Browser</span>
                </a>
                
                <div class="pt-4 pb-2 px-4 text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider">
                    Access Control
                </div>
                <a href="/roles" class="flex items-center gap-3 px-4 py-3 text-gray-700 dark:text-gray-200 hover:bg-primary-50 dark:hover:bg-primary-900/50 rounded-lg transition-colors">
                    <i class="fas fa-user-shield w-5"></i>
                    <span>Roles</span>
                </a>
                <a href="/tokens" class="flex items-center gap-3 px-4 py-3 text-gray-700 dark:text-gray-200 hover:bg-primary-50 dark:hover:bg-primary-900/50 rounded-lg transition-colors">
                    <i class="fas fa-key w-5"></i>
                    <span>Tokens</span>
                </a>
                
                <div class="pt-4 pb-2 px-4 text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider">
                    Monitoring
                </div>
                <a href="/audit" class="flex items-center gap-3 px-4 py-3 text-gray-700 dark:text-gray-200 hover:bg-primary-50 dark:hover:bg-primary-900/50 rounded-lg transition-colors">
                    <i class="fas fa-scroll w-5"></i>
                    <span>Audit Logs</span>
                </a>
                <a href="/approvals" class="flex items-center gap-3 px-4 py-3 text-gray-700 dark:text-gray-200 hover:bg-primary-50 dark:hover:bg-primary-900/50 rounded-lg transition-colors">
                    <i class="fas fa-check-double w-5"></i>
                    <span>Approvals</span>
                    <span id="approval-badge" class="ml-auto bg-yellow-500 text-white text-xs px-2 py-0.5 rounded-full hidden">0</span>
                </a>
                
                <div class="pt-4 pb-2 px-4 text-xs font-semibold text-gray-400 dark:text-gray-500 uppercase tracking-wider">
                    Configuration
                </div>
                <a href="/settings" class="flex items-center gap-3 px-4 py-3 text-gray-700 dark:text-gray-200 hover:bg-primary-50 dark:hover:bg-primary-900/50 rounded-lg transition-colors">
                    <i class="fas fa-cog w-5"></i>
                    <span>Settings</span>
                </a>
            </nav>
        </aside>"##
}

fn toast_template() -> &'static str {
    r##"<div id="toast" class="hidden fixed bottom-4 right-4 bg-green-500 text-white px-6 py-3 rounded-lg shadow-lg z-50 transition-all">
        <span id="toast-message"></span>
    </div>"##
}

/// Card component.
pub fn card(title: &str, content: &str) -> String {
    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 overflow-hidden">
            <div class="px-6 py-4 border-b border-gray-200 dark:border-gray-700">
                <h3 class="text-lg font-semibold text-gray-900 dark:text-white">{title}</h3>
            </div>
            <div class="p-6">
                {content}
            </div>
        </div>"##
    )
}

/// Stats card component.
pub fn stats_card(title: &str, value: &str, icon: &str, color: &str) -> String {
    format!(
        r##"<div class="bg-white dark:bg-gray-800 rounded-xl shadow-sm border border-gray-200 dark:border-gray-700 p-6">
            <div class="flex items-center justify-between">
                <div>
                    <p class="text-sm text-gray-500 dark:text-gray-400">{title}</p>
                    <p class="text-2xl font-bold text-gray-900 dark:text-white mt-1">{value}</p>
                </div>
                <div class="w-12 h-12 rounded-full bg-{color}-100 dark:bg-{color}-900/30 flex items-center justify-center">
                    <i class="fas fa-{icon} text-{color}-500 text-xl"></i>
                </div>
            </div>
        </div>"##
    )
}

/// Button component.
pub fn button(text: &str, variant: &str, attrs: &str) -> String {
    let (bg, hover, text_color) = match variant {
        "primary" => ("bg-primary-600", "hover:bg-primary-700", "text-white"),
        "secondary" => ("bg-gray-200 dark:bg-gray-700", "hover:bg-gray-300 dark:hover:bg-gray-600", "text-gray-700 dark:text-gray-200"),
        "danger" => ("bg-red-600", "hover:bg-red-700", "text-white"),
        "success" => ("bg-green-600", "hover:bg-green-700", "text-white"),
        _ => ("bg-gray-200", "hover:bg-gray-300", "text-gray-700"),
    };
    
    format!(
        r##"<button class="{bg} {hover} {text_color} px-4 py-2 rounded-lg font-medium transition-colors disabled:opacity-50" {attrs}>{text}</button>"##
    )
}

/// Input field component.
pub fn input(name: &str, label: &str, input_type: &str, value: &str, placeholder: &str) -> String {
    format!(
        r##"<div class="space-y-1">
            <label for="{name}" class="block text-sm font-medium text-gray-700 dark:text-gray-300">{label}</label>
            <input type="{input_type}" name="{name}" id="{name}" value="{value}" placeholder="{placeholder}"
                   class="w-full px-4 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-gray-900 dark:text-white focus:ring-2 focus:ring-primary-500 focus:border-primary-500">
        </div>"##
    )
}

/// Select field component.
pub fn select(name: &str, label: &str, options: &[(String, String, bool)]) -> String {
    let options_html: String = options
        .iter()
        .map(|(value, text, selected)| {
            if *selected {
                format!(r#"<option value="{value}" selected>{text}</option>"#)
            } else {
                format!(r#"<option value="{value}">{text}</option>"#)
            }
        })
        .collect();
    
    format!(
        r##"<div class="space-y-1">
            <label for="{name}" class="block text-sm font-medium text-gray-700 dark:text-gray-300">{label}</label>
            <select name="{name}" id="{name}"
                    class="w-full px-4 py-2 border border-gray-300 dark:border-gray-600 rounded-lg bg-white dark:bg-gray-800 text-gray-900 dark:text-white focus:ring-2 focus:ring-primary-500 focus:border-primary-500">
                {options_html}
            </select>
        </div>"##
    )
}

/// Badge component.
pub fn badge(text: &str, color: &str) -> String {
    format!(
        r##"<span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-{color}-100 dark:bg-{color}-900/30 text-{color}-800 dark:text-{color}-300">{text}</span>"##
    )
}

/// Table component.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let headers_html: String = headers
        .iter()
        .map(|h| format!(r#"<th class="px-6 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider">{h}</th>"#))
        .collect();
    
    let rows_html: String = rows
        .iter()
        .map(|row| {
            let cells: String = row
                .iter()
                .map(|cell| format!(r#"<td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900 dark:text-gray-100">{cell}</td>"#))
                .collect();
            format!(r#"<tr class="hover:bg-gray-50 dark:hover:bg-gray-700/50">{cells}</tr>"#)
        })
        .collect();
    
    format!(
        r##"<div class="overflow-x-auto">
            <table class="min-w-full divide-y divide-gray-200 dark:divide-gray-700">
                <thead class="bg-gray-50 dark:bg-gray-800">
                    <tr>{headers_html}</tr>
                </thead>
                <tbody class="bg-white dark:bg-gray-900 divide-y divide-gray-200 dark:divide-gray-700">
                    {rows_html}
                </tbody>
            </table>
        </div>"##
    )
}

/// Empty state component.
pub fn empty_state(icon: &str, title: &str, description: &str, action: Option<(&str, &str)>) -> String {
    let action_html = action.map_or(String::new(), |(text, href)| {
        format!(r##"<a href="{href}" class="mt-4 inline-flex items-center gap-2 bg-primary-600 hover:bg-primary-700 text-white px-4 py-2 rounded-lg font-medium transition-colors">
            <i class="fas fa-plus"></i> {text}
        </a>"##)
    });
    
    format!(
        r##"<div class="text-center py-12">
            <i class="fas fa-{icon} text-4xl text-gray-400 dark:text-gray-600 mb-4"></i>
            <h3 class="text-lg font-medium text-gray-900 dark:text-white">{title}</h3>
            <p class="mt-1 text-gray-500 dark:text-gray-400">{description}</p>
            {action_html}
        </div>"##
    )
}

/// Loading spinner.
pub fn spinner() -> &'static str {
    r##"<div class="flex items-center justify-center py-8">
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary-600"></div>
    </div>"##
}

/// Code block component.
pub fn code_block(code: &str, language: &str) -> String {
    format!(
        r##"<div class="relative">
            <pre class="bg-gray-900 text-gray-100 p-4 rounded-lg overflow-x-auto text-sm"><code class="language-{language}">{code}</code></pre>
            <button onclick="navigator.clipboard.writeText(this.parentElement.querySelector('code').textContent); showToast('Copied to clipboard!')"
                    class="absolute top-2 right-2 p-2 bg-gray-700 hover:bg-gray-600 rounded text-gray-300 hover:text-white transition-colors">
                <i class="fas fa-copy"></i>
            </button>
        </div>"##
    )
}

/// JSON viewer component.
pub fn json_viewer(json: &str) -> String {
    // Pretty print and escape the JSON
    let escaped = json
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    code_block(&escaped, "json")
}

/// Tab container.
pub fn tabs(id: &str, tabs: &[(&str, &str, &str)]) -> String {
    let tab_buttons: String = tabs
        .iter()
        .enumerate()
        .map(|(i, (key, label, _))| {
            let _active = if i == 0 { "border-primary-600 text-primary-600 dark:text-primary-400" } else { "border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300" };
            format!(
                r##"<button @click="activeTab = '{key}'" 
                        :class="{{ 'border-primary-600 text-primary-600 dark:text-primary-400': activeTab === '{key}', 'border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300': activeTab !== '{key}' }}"
                        class="px-4 py-2 border-b-2 font-medium text-sm transition-colors">
                    {label}
                </button>"##
            )
        })
        .collect();
    
    let tab_contents: String = tabs
        .iter()
        .map(|(key, _, content)| {
            format!(
                r##"<div x-show="activeTab === '{key}'" x-transition>
                    {content}
                </div>"##
            )
        })
        .collect();
    
    let first_key = tabs.first().map(|(k, _, _)| *k).unwrap_or("default");
    
    format!(
        r##"<div x-data="{{ activeTab: '{first_key}' }}" id="{id}">
            <div class="border-b border-gray-200 dark:border-gray-700 mb-4">
                <nav class="flex gap-2">
                    {tab_buttons}
                </nav>
            </div>
            <div>
                {tab_contents}
            </div>
        </div>"##
    )
}
