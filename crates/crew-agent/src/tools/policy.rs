//! Tool policy system with allow/deny lists, groups, and wildcards.

use serde::{Deserialize, Serialize};

/// Tool policy with allow/deny lists. Deny always wins over allow.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Tools, groups, or wildcards to allow. Empty = allow all.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Tools, groups, or wildcards to deny. Always wins over allow.
    #[serde(default)]
    pub deny: Vec<String>,
}

impl ToolPolicy {
    /// Check if a tool name is permitted by this policy.
    pub fn is_allowed(&self, tool_name: &str) -> bool {
        // Deny checked first (deny-wins semantics)
        for entry in &self.deny {
            if entry_matches(entry, tool_name) {
                return false;
            }
        }

        // Empty allow list = allow everything not denied
        if self.allow.is_empty() {
            return true;
        }

        // Check allow list
        for entry in &self.allow {
            if entry_matches(entry, tool_name) {
                return true;
            }
        }

        false
    }

    /// True if the policy has no restrictions.
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }
}

/// Check if a policy entry (group, wildcard, or exact name) matches a tool name.
fn entry_matches(entry: &str, tool_name: &str) -> bool {
    // Group expansion
    if let Some(tools) = expand_group(entry) {
        return tools.contains(&tool_name);
    }
    // Wildcard: suffix `*` means prefix match
    if let Some(prefix) = entry.strip_suffix('*') {
        return tool_name.starts_with(prefix);
    }
    // Exact match
    entry == tool_name
}

/// Expand a group name to its tool names. Returns None if not a group.
fn expand_group(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "group:fs" => Some(&["read_file", "write_file", "edit_file", "diff_edit"]),
        "group:runtime" => Some(&["shell"]),
        "group:web" => Some(&["web_search", "web_fetch"]),
        "group:search" => Some(&["glob", "grep", "list_dir"]),
        "group:sessions" => Some(&["spawn"]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_policy_allows_all() {
        let policy = ToolPolicy::default();
        assert!(policy.is_allowed("shell"));
        assert!(policy.is_allowed("read_file"));
        assert!(policy.is_allowed("anything"));
        assert!(policy.is_empty());
    }

    #[test]
    fn test_deny_wins_over_allow() {
        let policy = ToolPolicy {
            allow: vec!["shell".into(), "read_file".into()],
            deny: vec!["shell".into()],
        };
        assert!(!policy.is_allowed("shell"));
        assert!(policy.is_allowed("read_file"));
        assert!(!policy.is_allowed("write_file")); // not in allow list
    }

    #[test]
    fn test_group_expansion() {
        let policy = ToolPolicy {
            allow: vec!["group:fs".into()],
            deny: vec![],
        };
        assert!(policy.is_allowed("read_file"));
        assert!(policy.is_allowed("write_file"));
        assert!(policy.is_allowed("edit_file"));
        assert!(policy.is_allowed("diff_edit"));
        assert!(!policy.is_allowed("shell"));
        assert!(!policy.is_allowed("glob"));
    }

    #[test]
    fn test_wildcard_matching() {
        let policy = ToolPolicy {
            allow: vec![],
            deny: vec!["web_*".into()],
        };
        assert!(!policy.is_allowed("web_search"));
        assert!(!policy.is_allowed("web_fetch"));
        assert!(policy.is_allowed("shell"));
        assert!(policy.is_allowed("read_file"));
    }

    #[test]
    fn test_allow_list_filters() {
        let policy = ToolPolicy {
            allow: vec!["group:fs".into(), "group:search".into()],
            deny: vec![],
        };
        assert!(policy.is_allowed("read_file"));
        assert!(policy.is_allowed("glob"));
        assert!(policy.is_allowed("grep"));
        assert!(!policy.is_allowed("shell"));
        assert!(!policy.is_allowed("spawn"));
        assert!(!policy.is_allowed("web_fetch"));
    }

    #[test]
    fn test_deny_group() {
        let policy = ToolPolicy {
            allow: vec![],
            deny: vec!["group:runtime".into()],
        };
        assert!(!policy.is_allowed("shell"));
        assert!(policy.is_allowed("read_file"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let policy = ToolPolicy {
            allow: vec!["group:fs".into()],
            deny: vec!["shell".into()],
        };
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: ToolPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.allow, policy.allow);
        assert_eq!(parsed.deny, policy.deny);
    }
}
