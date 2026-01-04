//! Tool registry for MCP tools.
//!
//! This module provides a simple registry for storing and retrieving
//! MCP tool definitions. The actual tool generation logic is in
//! the `tool_generator` module.

use crate::protocol::ToolDefinition;
use std::collections::HashMap;

/// Registry of available MCP tools.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, ToolDefinition>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: ToolDefinition) {
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Unregister a tool by name.
    pub fn unregister(&mut self, name: &str) -> Option<ToolDefinition> {
        self.tools.remove(name)
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools.get(name)
    }

    /// Check if a tool exists.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// List all tools.
    pub fn list(&self) -> Vec<&ToolDefinition> {
        self.tools.values().collect()
    }

    /// Get the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Clear all tools.
    pub fn clear(&mut self) {
        self.tools.clear();
    }

    /// Get tool names.
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: Some(format!("Test tool: {}", name)),
            input_schema: json!({"type": "object"}),
            annotations: None,
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(create_test_tool("test"));

        assert!(registry.get("test").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_list_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(create_test_tool("tool1"));
        registry.register(create_test_tool("tool2"));

        assert_eq!(registry.len(), 2);
        assert_eq!(registry.list().len(), 2);
    }

    #[test]
    fn test_unregister() {
        let mut registry = ToolRegistry::new();
        registry.register(create_test_tool("test"));

        let removed = registry.unregister("test");
        assert!(removed.is_some());
        assert!(registry.get("test").is_none());
    }

    #[test]
    fn test_clear() {
        let mut registry = ToolRegistry::new();
        registry.register(create_test_tool("tool1"));
        registry.register(create_test_tool("tool2"));

        registry.clear();
        assert!(registry.is_empty());
    }
}
