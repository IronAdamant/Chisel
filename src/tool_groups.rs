//! MCP tool groups: `CHISEL_TOOLS` (allowlist) and `CHISEL_DISABLE` (denylist).
//!
//! Every MCP tool belongs to one group. `locks` and `history` are names shared across the
//! IronAdamant tool suite, so one word switches off the same kind of feature in every tool.
//! `core` is always on. With neither variable set nothing changes. The CLI is not affected.

use crate::json::Value;
use crate::schemas::TOOL_NAMES;
use std::collections::BTreeMap;

pub const TOOLS_VAR: &str = "CHISEL_TOOLS";
pub const DISABLE_VAR: &str = "CHISEL_DISABLE";

/// Group name and its tools, in listing order. `core` is first and always on.
pub const GROUPS: [(&str, &[&str]); 5] = [
    ("core", &["analyze", "update", "diff_impact", "impact", "suggest_tests", "record_result", "stats"]),
    ("risk", &["risk_map", "test_gaps", "triage", "stale_tests"]),
    ("history", &["churn", "ownership", "coupling", "history", "who_reviews"]),
    ("maintenance", &["start_job", "job_status", "cancel_job", "optimize_storage"]),
    ("locks", &["acquire_file_lock", "release_file_lock", "refresh_file_lock", "check_file_lock", "check_locks", "list_file_locks"]),
];

pub fn group_of(tool: &str) -> Option<&'static str> {
    GROUPS.iter().find(|(_, tools)| tools.contains(&tool)).map(|(g, _)| *g)
}

fn group_tools(group: &str) -> Option<&'static [&'static str]> {
    GROUPS.iter().find(|(g, _)| *g == group).map(|(_, t)| *t)
}

/// Which tools are hidden, and by which variable and entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolFilter {
    active: bool,
    hidden: BTreeMap<String, (&'static str, String)>,
}

fn entries(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("").split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect()
}

fn unknown(var: &str, name: &str) -> String {
    let groups: Vec<&str> = GROUPS.iter().map(|(g, _)| *g).collect();
    format!("{var}: unknown group \"{name}\"; groups: {}; tools: {}", groups.join(", "), TOOL_NAMES.join(", "))
}

/// Tools an entry names: a group, a tool, or `all` (allowlist only).
fn resolve(var: &str, entry: &str, allow_all: bool) -> Result<Vec<&'static str>, String> {
    if allow_all && entry == "all" {
        return Ok(TOOL_NAMES.to_vec());
    }
    if let Some(tools) = group_tools(entry) {
        return Ok(tools.to_vec());
    }
    TOOL_NAMES.iter().find(|t| **t == entry).map(|t| vec![*t]).ok_or_else(|| unknown(var, entry))
}

impl ToolFilter {
    /// From `CHISEL_TOOLS` and `CHISEL_DISABLE`. An unknown name is an error to stop on.
    pub fn from_env() -> Result<ToolFilter, String> {
        ToolFilter::parse(std::env::var(TOOLS_VAR).ok().as_deref(), std::env::var(DISABLE_VAR).ok().as_deref())
    }

    pub fn parse(tools: Option<&str>, disable: Option<&str>) -> Result<ToolFilter, String> {
        let (allow, deny) = (entries(tools), entries(disable));
        let mut filter = ToolFilter { active: !allow.is_empty() || !deny.is_empty(), hidden: BTreeMap::new() };
        let mut allowed: Vec<&str> = Vec::new();
        for entry in &allow {
            allowed.extend(resolve(TOOLS_VAR, entry, true)?);
        }
        let mut denied: Vec<(&str, &String)> = Vec::new();
        for entry in &deny {
            denied.extend(resolve(DISABLE_VAR, entry, false)?.into_iter().map(|t| (t, entry)));
        }
        for tool in TOOL_NAMES {
            let group = group_of(tool).unwrap_or("core");
            if group == "core" {
                continue;
            }
            if !allow.is_empty() && !allowed.contains(&tool) {
                filter.hidden.insert(tool.to_string(), (TOOLS_VAR, group.to_string()));
            }
            if let Some((_, entry)) = denied.iter().find(|(t, _)| *t == tool) {
                filter.hidden.insert(tool.to_string(), (DISABLE_VAR, entry.to_string()));
            }
        }
        Ok(filter)
    }

    /// True when either variable named something.
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn enabled(&self, tool: &str) -> bool {
        !self.hidden.contains_key(tool)
    }

    /// The one-line error for a call to a hidden tool.
    pub fn disabled_message(&self, tool: &str) -> Option<String> {
        let (var, entry) = self.hidden.get(tool)?;
        let group = group_of(tool).unwrap_or("core");
        Some(if *var == TOOLS_VAR {
            format!("tool \"{tool}\" is disabled (group \"{group}\") by {var}; add \"{group}\" to it to enable")
        } else {
            format!("tool \"{tool}\" is disabled (group \"{group}\") by {var}; remove \"{entry}\" from it to enable")
        })
    }

    /// One short line: which groups are on, which are off (a group with some tools hidden is partial).
    pub fn summary(&self) -> String {
        let (mut on, mut off) = (Vec::new(), Vec::new());
        for (group, tools) in GROUPS {
            let hidden = tools.iter().filter(|t| !self.enabled(t)).count();
            match hidden {
                0 => on.push(group.to_string()),
                n if n == tools.len() => off.push(group.to_string()),
                _ => on.push(format!("{group} (partial)")),
            }
        }
        let off = if off.is_empty() { "none".to_string() } else { off.join(", ") };
        format!("Tool groups on: {}; off: {off}.", on.join(", "))
    }

    /// `tools/list` without the hidden tools. Unchanged when nothing is hidden.
    pub fn filter_list(&self, mut list: Value) -> Value {
        if self.hidden.is_empty() {
            return list;
        }
        if let Some(Value::Array(tools)) = list.get_mut("tools") {
            tools.retain(|t| t.get("name").and_then(|v| v.as_str()).is_none_or(|n| self.enabled(n)));
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_exactly_one_group() {
        for tool in TOOL_NAMES {
            assert_eq!(GROUPS.iter().filter(|(_, t)| t.contains(&tool)).count(), 1, "{tool}");
        }
        assert_eq!(GROUPS.iter().map(|(_, t)| t.len()).sum::<usize>(), TOOL_NAMES.len());
    }

    #[test]
    fn allowlist_and_denylist_by_group_and_tool() {
        let none = ToolFilter::parse(None, None).unwrap();
        assert!(!none.is_active() && TOOL_NAMES.iter().all(|t| none.enabled(t)));
        let f = ToolFilter::parse(Some(" Risk , churn "), None).unwrap();
        assert!(f.enabled("triage") && f.enabled("churn") && f.enabled("analyze"));
        assert!(!f.enabled("ownership") && !f.enabled("acquire_file_lock"));
        let f = ToolFilter::parse(Some("all"), Some("LOCKS,who_reviews")).unwrap();
        assert!(!f.enabled("check_locks") && !f.enabled("who_reviews") && f.enabled("churn"));
        assert_eq!(
            f.disabled_message("check_locks").unwrap(),
            "tool \"check_locks\" is disabled (group \"locks\") by CHISEL_DISABLE; remove \"locks\" from it to enable"
        );
        assert_eq!(
            f.disabled_message("who_reviews").unwrap(),
            "tool \"who_reviews\" is disabled (group \"history\") by CHISEL_DISABLE; remove \"who_reviews\" from it to enable"
        );
        let f = ToolFilter::parse(Some("risk"), None).unwrap();
        assert_eq!(f.disabled_message("churn").unwrap(), "tool \"churn\" is disabled (group \"history\") by CHISEL_TOOLS; add \"history\" to it to enable");
        assert_eq!(f.summary(), "Tool groups on: core, risk; off: history, maintenance, locks.");
    }

    #[test]
    fn core_stays_on_and_unknown_names_are_errors() {
        let f = ToolFilter::parse(Some("locks"), Some("core,analyze,diff_impact")).unwrap();
        assert!(["analyze", "update", "diff_impact", "impact", "suggest_tests", "record_result", "stats"].iter().all(|t| f.enabled(t)));
        let err = ToolFilter::parse(None, Some("lokcs")).unwrap_err();
        assert!(err.starts_with("CHISEL_DISABLE: unknown group \"lokcs\"; groups: core, risk, history, maintenance, locks; tools: analyze, "), "{err}");
        assert!(ToolFilter::parse(None, Some("all")).is_err(), "all is only for CHISEL_TOOLS");
        assert!(ToolFilter::parse(Some("nope"), None).unwrap_err().starts_with("CHISEL_TOOLS: unknown group \"nope\""));
    }
}
