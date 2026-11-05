//! MCP tool groups, chosen per project by agents (an `agent-tools` block in the project's
//! `AGENTS.md`) or by an operator (`CHISEL_TOOLS` / `CHISEL_DISABLE`).
//!
//! Every MCP tool belongs to one group. `locks` and `history` are names shared across the
//! IronAdamant tool suite, so one word switches off the same kind of feature in every tool.
//! `core` is always on. Env vars beat the file, key by key. A bad env var stops the server
//! (exit 2); a bad file line is ignored and reported as a warning, because an agent wrote it.
//! With no env var and no `agent-tools` block nothing changes. The CLI is not affected.

use crate::json::Value;
use crate::schemas::TOOL_NAMES;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const TOOLS_VAR: &str = "CHISEL_TOOLS";
pub const DISABLE_VAR: &str = "CHISEL_DISABLE";
/// This tool's name in an `agent-tools` block.
pub const AGENT_NAME: &str = "chisel";

/// Group name and its tools, in listing order. `core` is first and always on.
pub const GROUPS: [(&str, &[&str]); 6] = [
    ("core", &["analyze", "update", "diff_impact", "impact", "suggest_tests", "record_result", "stats"]),
    ("risk", &["risk_map", "test_gaps", "triage", "stale_tests"]),
    ("history", &["churn", "ownership", "coupling", "history", "who_reviews"]),
    ("maintenance", &["start_job", "job_status", "cancel_job", "optimize_storage"]),
    ("locks", &["acquire_file_lock", "release_file_lock", "refresh_file_lock", "check_file_lock", "check_locks", "list_file_locks"]),
    ("issues", &["issues", "issue", "trace"]),
];

pub fn group_of(tool: &str) -> Option<&'static str> {
    GROUPS.iter().find(|(_, tools)| tools.contains(&tool)).map(|(g, _)| *g)
}

fn group_tools(group: &str) -> Option<&'static [&'static str]> {
    GROUPS.iter().find(|(g, _)| *g == group).map(|(_, t)| *t)
}

fn group_names() -> String {
    GROUPS.iter().map(|(g, _)| *g).collect::<Vec<_>>().join(", ")
}

fn catalogue() -> String {
    format!("groups: {}; tools: {}", group_names(), TOOL_NAMES.join(", "))
}

/// Names in a list: commas or spaces, case and whitespace ignored.
fn entries(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace()).map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect()
}

enum Problem {
    Unknown(String),
    Core(String),
}

/// Tools an entry names: a group, a tool, or `all` (allowlist only).
fn resolve(entry: &str, allow_all: bool) -> Option<Vec<&'static str>> {
    if allow_all && entry == "all" {
        return Some(TOOL_NAMES.to_vec());
    }
    if let Some(tools) = group_tools(entry) {
        return Some(tools.to_vec());
    }
    TOOL_NAMES.iter().find(|t| **t == entry).map(|t| vec![*t])
}

/// The first unknown name, or (for a denylist) the first entry that would hide `core`.
fn check(raw: &str, disable: bool) -> Result<(), Problem> {
    for entry in entries(raw) {
        let Some(tools) = resolve(&entry, !disable) else { return Err(Problem::Unknown(entry)) };
        if disable && tools.iter().any(|t| group_of(t) == Some("core")) {
            return Err(Problem::Core(entry));
        }
    }
    Ok(())
}

/// One list and where it came from (`CHISEL_DISABLE`, or the AGENTS.md line).
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub label: String,
    pub value: String,
}

impl Source {
    fn env(var: &str, value: &str) -> Source {
        Source { label: var.to_string(), value: value.to_string() }
    }

    fn file(key: &str, value: &str) -> Source {
        Source { label: format!("AGENTS.md agent-tools line \"{AGENT_NAME}: {key}\""), value: value.to_string() }
    }
}

/// Which tools are hidden, and by which source and entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolFilter {
    active: bool,
    hidden: BTreeMap<String, (String, String)>,
}

impl ToolFilter {
    /// From the two env values only. An unknown name, or core in the denylist, is the one line
    /// to print before exiting 2.
    pub fn parse(tools: Option<&str>, disable: Option<&str>) -> Result<ToolFilter, String> {
        let tools = tools.filter(|v| !v.trim().is_empty()).map(|v| Source::env(TOOLS_VAR, v));
        let disable = disable.filter(|v| !v.trim().is_empty()).map(|v| Source::env(DISABLE_VAR, v));
        for (source, is_disable) in [(&tools, false), (&disable, true)] {
            if let Some(s) = source {
                match check(&s.value, is_disable) {
                    Err(Problem::Unknown(name)) => return Err(format!("{}: unknown group \"{name}\"; {}", s.label, catalogue())),
                    Err(Problem::Core(name)) => return Err(format!("{}: group \"core\" is always on and cannot be disabled (\"{name}\"); {}", s.label, catalogue())),
                    Ok(()) => {}
                }
            }
        }
        Ok(ToolFilter::build(tools.as_ref(), disable.as_ref()))
    }

    /// From checked sources (see `check`).
    fn build(tools: Option<&Source>, disable: Option<&Source>) -> ToolFilter {
        let mut filter = ToolFilter { active: tools.is_some() || disable.is_some(), hidden: BTreeMap::new() };
        let allowed: Option<Vec<&str>> = tools.map(|s| entries(&s.value).iter().filter_map(|e| resolve(e, true)).flatten().collect());
        let denied: Vec<(&str, String)> = disable
            .map(|s| entries(&s.value).into_iter().flat_map(|e| resolve(&e, false).unwrap_or_default().into_iter().map(move |t| (t, e.clone()))).collect())
            .unwrap_or_default();
        for tool in TOOL_NAMES {
            let group = group_of(tool).unwrap_or("core");
            if group == "core" {
                continue;
            }
            if let (Some(allowed), Some(source)) = (&allowed, tools) {
                if !allowed.contains(&tool) {
                    filter.hidden.insert(tool.to_string(), (source.label.clone(), group.to_string()));
                }
            }
            if let (Some((_, entry)), Some(source)) = (denied.iter().find(|(t, _)| *t == tool), disable) {
                filter.hidden.insert(tool.to_string(), (source.label.clone(), entry.clone()));
            }
        }
        filter
    }

    /// True when a list was given (by env or by the file).
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn enabled(&self, tool: &str) -> bool {
        !self.hidden.contains_key(tool)
    }

    /// The one-line error for a call to a hidden tool.
    pub fn disabled_message(&self, tool: &str) -> Option<String> {
        let (label, entry) = self.hidden.get(tool)?;
        let group = group_of(tool).unwrap_or("core");
        let allowlist = label == TOOLS_VAR || label.ends_with(": tools\"");
        Some(if allowlist {
            format!("tool \"{tool}\" is disabled (group \"{group}\") by {label}; add \"{group}\" to it to enable")
        } else {
            format!("tool \"{tool}\" is disabled (group \"{group}\") by {label}; remove \"{entry}\" from it to enable")
        })
    }

    /// Groups on and off. A group with some tools hidden lists the live ones.
    fn on_off(&self) -> (Vec<String>, Vec<String>) {
        let (mut on, mut off) = (Vec::new(), Vec::new());
        for (group, tools) in GROUPS {
            let live: Vec<&str> = tools.iter().copied().filter(|t| self.enabled(t)).collect();
            if live.len() == tools.len() {
                on.push(group.to_string());
            } else if live.is_empty() {
                off.push(group.to_string());
            } else {
                on.push(format!("{group} (only {})", live.join(", ")));
            }
        }
        (on, off)
    }

    /// One line: groups on and off, and how an agent changes them.
    pub fn summary(&self) -> String {
        let (on, off) = self.on_off();
        let off = if off.is_empty() { "none".to_string() } else { off.join(", ") };
        format!(
            "Tool groups on: {}. Off: {off}. Change them with a \"{AGENT_NAME}: disable <groups>\" or \"{AGENT_NAME}: tools <groups>\" line in the agent-tools block of AGENTS.md; {TOOLS_VAR} / {DISABLE_VAR} override it.",
            on.join(", ")
        )
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

    fn hidden_tools(&self) -> Vec<&str> {
        self.hidden.keys().map(|s| s.as_str()).collect()
    }
}

/// This tool's lines from the first `agent-tools` block of an AGENTS.md.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentLines {
    /// The file has an `agent-tools` block (for any tool).
    pub block: bool,
    pub tools: Option<String>,
    pub disable: Option<String>,
    /// One line per ignored entry, for the instructions and `stats`.
    pub warnings: Vec<String>,
}

/// A fence line: its character, its length, and the info string after it.
fn fence(line: &str) -> Option<(char, usize, &str)> {
    let t = line.trim_start();
    let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = t.chars().take_while(|x| *x == c).count();
    (n >= 3).then(|| (c, n, t[n..].trim()))
}

/// Lines of the first top-level ```` ```agent-tools ```` block. A block inside another fence
/// (a documentation example) is text, not configuration.
fn agent_tools_block(text: &str) -> Option<Vec<&str>> {
    let mut open: Option<(char, usize, bool)> = None;
    let mut body = Vec::new();
    for line in text.lines() {
        match open {
            None => {
                if let Some((c, n, info)) = fence(line) {
                    open = Some((c, n, info == "agent-tools"));
                }
            }
            Some((c, n, is_block)) => {
                if fence(line).is_some_and(|(fc, fn_, info)| fc == c && fn_ >= n && info.is_empty()) {
                    if is_block {
                        return Some(body);
                    }
                    open = None;
                } else if is_block {
                    body.push(line);
                }
            }
        }
    }
    // An unclosed block runs to the end of the file, as in CommonMark.
    open.filter(|(_, _, is_block)| *is_block).map(|_| body)
}

pub fn parse_agents_md(text: &str) -> AgentLines {
    let mut out = AgentLines::default();
    let Some(lines) = agent_tools_block(text) else { return out };
    out.block = true;
    let warn = |msg: String| format!("AGENTS.md agent-tools: {msg} for {AGENT_NAME}; groups: {}", group_names());
    for line in lines {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((tool, rest)) = line.split_once(':') else { continue };
        if tool.trim().to_ascii_lowercase() != AGENT_NAME {
            continue;
        }
        let rest = rest.trim();
        let (key, list) = rest.split_once(char::is_whitespace).map(|(k, l)| (k, l.trim())).unwrap_or((rest, ""));
        let key = key.to_ascii_lowercase();
        let (slot, disable) = match key.as_str() {
            "tools" => (&mut out.tools, false),
            "disable" => (&mut out.disable, true),
            _ => {
                out.warnings.push(warn(format!("unknown key \"{key}\" (use tools or disable)")));
                continue;
            }
        };
        match check(list, disable) {
            Err(Problem::Unknown(name)) => out.warnings.push(warn(format!("unknown group \"{name}\""))),
            Err(Problem::Core(name)) => out.warnings.push(warn(format!("group \"core\" is always on and cannot be disabled (\"{name}\")"))),
            Ok(()) if !entries(list).is_empty() => {
                let joined = match slot.take() {
                    Some(prev) => format!("{prev},{list}"),
                    None => list.to_string(),
                };
                *slot = Some(joined);
            }
            Ok(()) => {}
        }
    }
    out
}

/// The effective switches for one MCP session: env values (fixed at start) over the
/// project's AGENTS.md (re-read when it changes).
#[derive(Debug)]
pub struct Surface {
    env_tools: Option<String>,
    env_disable: Option<String>,
    path: PathBuf,
    stamp: Option<(SystemTime, u64)>,
    pub file: AgentLines,
    pub filter: ToolFilter,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

impl Surface {
    /// Env errors are the one line to print before exiting 2. File problems become warnings.
    pub fn start(project: &Path) -> Result<Surface, String> {
        let get = |var: &str| std::env::var(var).ok().filter(|v| !v.trim().is_empty());
        let (env_tools, env_disable) = (get(TOOLS_VAR), get(DISABLE_VAR));
        ToolFilter::parse(env_tools.as_deref(), env_disable.as_deref())?;
        let mut surface = Surface { env_tools, env_disable, path: project.join("AGENTS.md"), stamp: None, file: AgentLines::default(), filter: ToolFilter::default() };
        surface.reload();
        Ok(surface)
    }

    fn reload(&mut self) {
        self.stamp = stamp(&self.path);
        self.file = std::fs::read_to_string(&self.path).map(|t| parse_agents_md(&t)).unwrap_or_default();
        let tools = match &self.env_tools {
            Some(v) => Some(Source::env(TOOLS_VAR, v)),
            None => self.file.tools.as_deref().map(|v| Source::file("tools", v)),
        };
        let disable = match &self.env_disable {
            Some(v) => Some(Source::env(DISABLE_VAR, v)),
            None => self.file.disable.as_deref().map(|v| Source::file("disable", v)),
        };
        self.filter = ToolFilter::build(tools.as_ref(), disable.as_ref());
    }

    /// Re-read AGENTS.md if its mtime or size changed. True when the visible tools changed.
    pub fn refresh(&mut self) -> bool {
        if stamp(&self.path) == self.stamp {
            return false;
        }
        let before: Vec<String> = self.filter.hidden_tools().into_iter().map(String::from).collect();
        self.reload();
        before != self.filter.hidden_tools()
    }

    /// An env var is set or the project has an `agent-tools` block. Otherwise MCP output is
    /// exactly what it was before tool groups existed.
    pub fn configured(&self) -> bool {
        self.env_tools.is_some() || self.env_disable.is_some() || self.file.block
    }

    /// Warning lines, then the groups line. Empty when not configured.
    pub fn report_lines(&self) -> Vec<String> {
        if !self.configured() {
            return Vec::new();
        }
        let mut lines = self.file.warnings.clone();
        lines.push(self.filter.summary());
        lines
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
        let none = ToolFilter::parse(None, Some("  ")).unwrap();
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
        assert!(f.summary().starts_with("Tool groups on: core, risk. Off: history, maintenance, locks, issues. Change them with a \"chisel: disable <groups>\""), "{}", f.summary());
    }

    #[test]
    fn core_cannot_be_disabled_and_unknown_names_are_errors() {
        let f = ToolFilter::parse(Some("locks"), None).unwrap();
        assert!(["analyze", "update", "diff_impact", "impact", "suggest_tests", "record_result", "stats"].iter().all(|t| f.enabled(t)));
        for name in ["core", "Analyze", "diff_impact"] {
            let err = ToolFilter::parse(None, Some(name)).unwrap_err();
            let lower = name.to_ascii_lowercase();
            assert!(err.starts_with(&format!("CHISEL_DISABLE: group \"core\" is always on and cannot be disabled (\"{lower}\"); groups: core, ")), "{err}");
        }
        let err = ToolFilter::parse(None, Some("lokcs")).unwrap_err();
        assert!(err.starts_with("CHISEL_DISABLE: unknown group \"lokcs\"; groups: core, risk, history, maintenance, locks, issues; tools: analyze, "), "{err}");
        assert!(ToolFilter::parse(None, Some("all")).is_err(), "all is only for CHISEL_TOOLS");
        assert!(ToolFilter::parse(Some("nope"), None).unwrap_err().starts_with("CHISEL_TOOLS: unknown group \"nope\""));
    }

    #[test]
    fn agents_md_block_reads_only_this_tools_lines() {
        let text = "# Project\n\n```bash\nchisel: disable risk\n```\n\n```agent-tools\n# one agent; CoordinationHub owns leases\ntrammel: disable locks, notes\nChisel: disable locks history # inline comment\nchisel: tools all\ncoordinationhub: tools core, locks, tasks\n```\n\n```agent-tools\nchisel: disable maintenance\n```\n";
        let lines = parse_agents_md(text);
        assert!(lines.block && lines.warnings.is_empty(), "{lines:?}");
        assert_eq!(lines.disable.as_deref(), Some("locks history"));
        assert_eq!(lines.tools.as_deref(), Some("all"));
        assert_eq!(parse_agents_md("no block here\n"), AgentLines::default());
        let other = parse_agents_md("```agent-tools\ntrammel: disable locks\n```\n");
        assert!(other.block && other.tools.is_none() && other.disable.is_none());
    }

    #[test]
    fn a_block_inside_another_fence_is_documentation() {
        let nested = "Example:\n\n````markdown\n```agent-tools\nchisel: disable locks\n```\n````\n\n~~~text\n```agent-tools\nchisel: disable history\n```\n~~~\n";
        assert_eq!(parse_agents_md(nested), AgentLines::default());
        // After the example, a real block still counts.
        let real = format!("{nested}\n```agent-tools\nchisel: disable risk\n```\n");
        assert_eq!(parse_agents_md(&real).disable.as_deref(), Some("risk"));
        // A longer closing fence closes; a shorter one is content.
        let long = "````agent-tools\nchisel: disable locks\n```\nchisel: disable history\n`````\n";
        assert_eq!(parse_agents_md(long).disable.as_deref(), Some("locks,history"));
    }

    #[test]
    fn bad_file_lines_are_warnings_not_errors() {
        let lines = parse_agents_md("```agent-tools\nchisel: disable lokcs\nchisel: disable core\nchisel: enable risk\nchisel: tools risk\n```\n");
        assert_eq!(lines.disable, None);
        assert_eq!(lines.tools.as_deref(), Some("risk"));
        assert_eq!(lines.warnings[0], "AGENTS.md agent-tools: unknown group \"lokcs\" for chisel; groups: core, risk, history, maintenance, locks, issues");
        assert!(lines.warnings[1].contains("group \"core\" is always on and cannot be disabled (\"core\") for chisel"), "{:?}", lines.warnings);
        assert!(lines.warnings[2].contains("unknown key \"enable\""), "{:?}", lines.warnings);
    }
}
