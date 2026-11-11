//! Stdio MCP framing. Standard clients send one JSON-RPC object per line.
//! Content-Length framing still works for clients that send it.
//! Implements initialize, tools/list, and tools/call.

use crate::engine::Engine;
use crate::json::{self, Value};
use crate::next_steps::compute_next_steps;
use crate::schemas::tool_list_json;
use crate::tool_groups::{Surface, ToolFilter};
use std::io::{self, Read, Write};
use std::path::Path;

/// A call to a hidden tool, with its one-line explanation.
struct DisabledTool(String);

pub fn serve(project: &Path, storage: Option<&Path>, worktree: Option<&Path>) -> Result<(), String> {
    // A typo in CHISEL_TOOLS / CHISEL_DISABLE stops the server, so the client shows it as failed.
    // Problems in the project's AGENTS.md block are warnings instead (see `tool_groups`).
    let mut engine = Engine::open(project, storage)?;
    engine.default_worktree = worktree.map(|p| p.to_path_buf());
    let mut surface = Surface::start(&engine.project_dir).unwrap_or_else(|line| {
        eprintln!("{line}");
        std::process::exit(2);
    });
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    loop {
        let (body, newline) = match read_incoming(&mut input) {
            Ok(Some(body)) => body,
            Ok(None) => break,
            Err(err) => {
                let err_body = Value::object(vec![
                    ("jsonrpc".into(), Value::String("2.0".into())),
                    ("id".into(), Value::Null),
                    ("error".into(), Value::object(vec![
                        ("code".into(), Value::Int(-32700)),
                        ("message".into(), Value::String(err.to_string())),
                    ])),
                ]);
                write_message(&mut output, &json::to_string(&err_body).unwrap_or_default(), true).map_err(|e| e.to_string())?;
                if err.kind() == io::ErrorKind::InvalidData {
                    continue;
                }
                return Err(err.to_string());
            }
        };
        let Ok(msg) = json::parse(&body) else {
            let err = Value::object(vec![
                ("jsonrpc".into(), Value::String("2.0".into())),
                ("id".into(), Value::Null),
                ("error".into(), Value::object(vec![
                    ("code".into(), Value::Int(-32700)),
                    ("message".into(), Value::String("parse error".into())),
                ])),
            ]);
            write_message(&mut output, &json::to_string(&err).unwrap_or_default(), newline).map_err(|e| e.to_string())?;
            continue;
        };
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();
        if (method.starts_with("notifications/") || method == "initialized" || id.is_none() && method != "initialize")
            && (method == "initialized" || method.starts_with("notifications/"))
        {
            continue;
        }
        // An agent may edit the agent-tools block mid-session. Tell the client before answering.
        if id.is_some() && surface.refresh() {
            let note = Value::object(vec![
                ("jsonrpc".into(), Value::String("2.0".into())),
                ("method".into(), Value::String("notifications/tools/list_changed".into())),
            ]);
            write_message(&mut output, &json::to_string(&note).unwrap_or_default(), newline).map_err(|e| e.to_string())?;
        }
        let groups: &ToolFilter = &surface.filter;
        let result = match method {
            "initialize" => Value::object(vec![
                ("protocolVersion".into(), Value::String("2024-11-05".into())),
                ("capabilities".into(), Value::object(vec![
                    ("tools".into(), Value::object(vec![("listChanged".into(), Value::Bool(true))])),
                ])),
                ("serverInfo".into(), Value::object(vec![
                    ("name".into(), Value::String("chisel".into())),
                    ("version".into(), Value::String(env!("CARGO_PKG_VERSION").into())),
                ])),
                ("instructions".into(), Value::String({
                    let base = "Storage is .chisel/store. Branch on status. Call analyze when status is no_data. Call diff_impact after edits, before running tests: since=main covers committed work; in a git worktree pass worktree_root. This process is stdio MCP (chisel serve-mcp).";
                    let report = surface.report_lines();
                    if report.is_empty() { base.to_string() } else { format!("{base}\n{}", report.join("\n")) }
                })),
            ]),
            "tools/list" => groups.filter_list(tool_list_json()),
            "tools/call" => {
                let params = msg.get("params");
                let name = params.and_then(|p| p.get("name")).and_then(|v| v.as_str()).unwrap_or("");
                let empty = Value::object(vec![]);
                let args = params.and_then(|p| p.get("arguments")).unwrap_or(&empty);
                let outcome = match groups.disabled_message(name) {
                    Some(line) => Err(DisabledTool(line)),
                    None => Ok(engine.dispatch(name, args)),
                };
                match outcome {
                    Err(DisabledTool(line)) => Value::object(vec![
                        ("content".into(), Value::Array(vec![Value::object(vec![
                            ("type".into(), Value::String("text".into())),
                            ("text".into(), Value::String(line)),
                        ])])),
                        ("isError".into(), Value::Bool(true)),
                    ]),
                    Ok(Ok(mut result)) => {
                        if name == "stats" && surface.configured() {
                            result.insert("tool_groups", Value::String(groups.summary()));
                            if !surface.file.warnings.is_empty() {
                                result.insert("tool_groups_warnings", Value::Array(surface.file.warnings.iter().cloned().map(Value::String).collect()));
                            }
                        }
                        let mut steps = compute_next_steps(name, &result);
                        if name == "suggest_tests" {
                            let file = args.get("file_path").and_then(|v| v.as_str());
                            if let Some(warning) = crate::next_steps::eval_warning(engine.project_dir.as_path(), file) {
                                steps.push(warning);
                            }
                        }
                        let mut payload = vec![("result".into(), result)];
                        if !steps.is_empty() {
                            payload.push(("next_steps".into(), Value::Array(steps)));
                        }
                        let text = json::to_string_pretty(&Value::object(payload)).unwrap_or_else(|_| "{\"result\":null}".into());
                        Value::object(vec![
                            ("content".into(), Value::Array(vec![Value::object(vec![
                                ("type".into(), Value::String("text".into())),
                                ("text".into(), Value::String(text)),
                            ])])),
                        ])
                    }
                    Ok(Err(e)) => Value::object(vec![
                        ("content".into(), Value::Array(vec![Value::object(vec![
                            ("type".into(), Value::String("text".into())),
                            ("text".into(), Value::String(json::to_string(&Value::object(vec![
                                ("status".into(), Value::String("error".into())),
                                ("message".into(), Value::String(e)),
                            ])).unwrap_or_default())),
                        ])])),
                        ("isError".into(), Value::Bool(true)),
                    ]),
                }
            }
            "ping" => Value::object(vec![]),
            _ => {
                if let Some(id) = id {
                    let err = Value::object(vec![
                        ("jsonrpc".into(), Value::String("2.0".into())),
                        ("id".into(), id),
                        ("error".into(), Value::object(vec![
                            ("code".into(), Value::Int(-32601)),
                            ("message".into(), Value::String("method not found".into())),
                        ])),
                    ]);
                    write_message(&mut output, &json::to_string(&err).unwrap_or_default(), newline).map_err(|e| e.to_string())?;
                }
                continue;
            }
        };
        if let Some(id) = id {
            let resp = Value::object(vec![
                ("jsonrpc".into(), Value::String("2.0".into())),
                ("id".into(), id),
                ("result".into(), result),
            ]);
            write_message(&mut output, &json::to_string(&resp).map_err(|e| e.message)?, newline).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn read_message(input: &mut impl Read) -> io::Result<Option<String>> {
    Ok(read_incoming(input)?.map(|(body, _)| body))
}

fn read_incoming(input: &mut impl Read) -> io::Result<Option<(String, bool)>> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = input.read(&mut byte)?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        if buf.is_empty() && byte[0].is_ascii_whitespace() {
            continue;
        }
        buf.push(byte[0]);
        if buf[0] == b'{' || buf[0] == b'[' {
            if byte[0] == b'\n' {
                if buf.last() == Some(&b'\n') { buf.pop(); }
                if buf.last() == Some(&b'\r') { buf.pop(); }
                return Ok(Some((String::from_utf8_lossy(&buf).into_owned(), true)));
            }
            if buf.len() > 8 * 1024 * 1024 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
            }
            continue;
        }
        if buf.ends_with(b"\r\n\r\n") || buf.ends_with(b"\n\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "header too large"));
        }
    }
    if buf.is_empty() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut len = 0usize;
    for line in text.split(['\r', '\n']) {
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    if len > 8 * 1024 * 1024 {
        discard_bytes(input, len)?;
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        input.read_exact(&mut body)?;
    }
    Ok(Some((String::from_utf8_lossy(&body).into_owned(), false)))
}

fn discard_bytes(input: &mut impl Read, mut left: usize) -> io::Result<()> {
    let mut buf = [0u8; 8192];
    while left > 0 {
        let take = left.min(buf.len());
        let n = input.read(&mut buf[..take])?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "body ended early"));
        }
        left -= n;
    }
    Ok(())
}

pub fn write_message(output: &mut impl Write, body: &str, newline: bool) -> io::Result<()> {
    if newline {
        writeln!(output, "{body}")?;
    } else {
        write!(output, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    }
    output.flush()
}
