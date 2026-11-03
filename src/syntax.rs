//! Homegrown syntax tree. No tree-sitter crate.
//! Python and JavaScript/TypeScript become a module node with function and class children.
//! Python unit extraction goes through this tree. Other languages keep their scanners.

use crate::extract::CodeUnit;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: &'static str,
    pub name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub children: Vec<Node>,
}

pub fn parse(language: &str, source: &str) -> Option<Node> {
    match language {
        "python" => Some(parse_python(source)),
        "javascript" | "typescript" => Some(parse_javascript(source)),
        _ => None,
    }
}

pub fn python_units(file_path: &str, source: &str) -> Vec<CodeUnit> {
    flatten(file_path, &parse_python(source))
}

fn flatten(file_path: &str, node: &Node) -> Vec<CodeUnit> {
    let mut units = Vec::new();
    walk(file_path, node, &mut units);
    units
}

fn walk(file_path: &str, node: &Node, units: &mut Vec<CodeUnit>) {
    if node.kind == "function" || node.kind == "async_function" || node.kind == "class" {
        units.push(CodeUnit {
            file_path: file_path.to_string(),
            name: node.name.clone(),
            unit_type: node.kind.to_string(),
            line_start: node.line_start,
            line_end: node.line_end,
        });
    }
    for child in &node.children {
        walk(file_path, child, units);
    }
}

fn parse_python(source: &str) -> Node {
    let lines: Vec<&str> = source.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut module = Node { kind: "module", name: String::new(), line_start: 1, line_end: lines.len().max(1), children: Vec::new() };
    let mut depth = 0i32;
    let mut stack: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (clean, next_depth) = strip_line(lines[i], depth);
        let trimmed = clean.trim();
        if depth == 0 {
            let indent = indent_of(lines[i]);
            stack.retain(|(_, ind, _)| *ind < indent);
            if let Some(name) = class_name(trimmed) {
                let end = block_end(&lines, i, indent);
                let parent = stack.iter().rev().find(|(_, ind, _)| indent > *ind).map(|(_, _, n)| n.clone());
                let qname = match parent {
                    Some(p) => format!("{p}.{name}"),
                    None => name,
                };
                module.children.push(Node {
                    kind: "class",
                    name: qname.clone(),
                    line_start: i + 1,
                    line_end: end,
                    children: Vec::new(),
                });
                stack.push((module.children.len() - 1, indent, qname));
            } else if let Some((name, is_async)) = def_name(trimmed) {
                let parent = stack.iter().rev().find(|(_, ind, _)| indent > *ind).map(|(_, _, n)| n.clone());
                let qname = match parent {
                    Some(p) => format!("{p}.{name}"),
                    None => name,
                };
                let end = block_end(&lines, i, indent);
                module.children.push(Node {
                    kind: if is_async { "async_function" } else { "function" },
                    name: qname,
                    line_start: i + 1,
                    line_end: end,
                    children: Vec::new(),
                });
            }
        }
        depth = next_depth;
        i += 1;
    }
    module
}

fn parse_javascript(source: &str) -> Node {
    let lines: Vec<&str> = source.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut module = Node { kind: "module", name: String::new(), line_start: 1, line_end: lines.len().max(1), children: Vec::new() };
    let mut depth = 0i32;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = strip_js(line);
        if depth == 0 {
            if let Some(name) = js_name(&trimmed, "class ") {
                module.children.push(Node { kind: "class", name, line_start: i + 1, line_end: js_end(&lines, i), children: Vec::new() });
            } else if let Some(name) = js_name(&trimmed, "async function ") {
                module.children.push(Node { kind: "async_function", name, line_start: i + 1, line_end: js_end(&lines, i), children: Vec::new() });
            } else if let Some(name) = js_name(&trimmed, "function ") {
                module.children.push(Node { kind: "function", name, line_start: i + 1, line_end: js_end(&lines, i), children: Vec::new() });
            }
        }
        depth += brace_delta(line);
        if depth < 0 {
            depth = 0;
        }
        i += 1;
    }
    module
}

fn js_name(trimmed: &str, prefix: &str) -> Option<String> {
    let rest = trimmed.trim_start().strip_prefix(prefix)?;
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$').collect();
    if name.is_empty() { None } else { Some(name) }
}

fn js_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i32;
    let mut seen = false;
    let mut i = start;
    while i < lines.len() {
        depth += brace_delta(lines[i]);
        if lines[i].contains('{') {
            seen = true;
        }
        if seen && depth <= 0 {
            return i + 1;
        }
        i += 1;
    }
    lines.len().max(start + 1)
}

fn brace_delta(line: &str) -> i32 {
    let mut d = 0i32;
    for c in strip_js(line).chars() {
        match c {
            '{' => d += 1,
            '}' => d -= 1,
            _ => {}
        }
    }
    d
}

fn strip_js(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    let mut in_str: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            if c == '\\' { chars.next(); continue; }
            if c == q { in_str = None; }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if c == '"' || c == '\'' || c == '`' {
            in_str = Some(c);
            continue;
        }
        out.push(c);
    }
    out
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn class_name(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("class ")?;
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some(name) }
}

fn def_name(trimmed: &str) -> Option<(String, bool)> {
    let (rest, is_async) = if let Some(r) = trimmed.strip_prefix("async def ") {
        (r, true)
    } else {
        let r = trimmed.strip_prefix("def ")?;
        (r, false)
    };
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some((name, is_async)) }
}

fn block_end(lines: &[&str], start: usize, ind: usize) -> usize {
    let mut end = start + 1;
    let mut i = start + 1;
    let mut depth = bracket_delta(lines[start]);
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        if depth == 0 && indent_of(line) <= ind && !trimmed.starts_with('@') {
            break;
        }
        end = i + 1;
        depth += bracket_delta(line);
        if depth < 0 {
            depth = 0;
        }
        i += 1;
    }
    end
}

fn bracket_delta(line: &str) -> i32 {
    let mut d = 0i32;
    let mut chars = line.chars().peekable();
    let mut in_str: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            if c == '\\' { chars.next(); continue; }
            if c == q { in_str = None; }
            continue;
        }
        if c == '#' { break; }
        if c == '"' || c == '\'' { in_str = Some(c); continue; }
        match c {
            '(' | '[' | '{' => d += 1,
            ')' | ']' | '}' => d -= 1,
            _ => {}
        }
    }
    d
}

fn strip_line(line: &str, depth_in: i32) -> (String, i32) {
    let mut out = String::new();
    let mut depth = depth_in;
    let mut chars = line.chars().peekable();
    let mut in_str: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            if c == '\\' { chars.next(); continue; }
            if c == q { in_str = None; }
            continue;
        }
        if depth > 0 {
            match c {
                '"' | '\'' => in_str = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
            continue;
        }
        if c == '#' { break; }
        if c == '"' || c == '\'' { in_str = Some(c); continue; }
        match c {
            '(' | '[' | '{' => { depth += 1; out.push(c); }
            ')' | ']' | '}' => { depth -= 1; out.push(c); }
            _ => out.push(c),
        }
    }
    let _ = depth_in;
    (out, depth.max(0))
}
