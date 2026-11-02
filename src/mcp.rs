//! Stdio MCP framing: Content-Length header, then one JSON-RPC body.
//! Implements initialize, tools/list, and tools/call.

use crate::engine::Engine;
use crate::json::{self, Value};
use crate::next_steps::compute_next_steps;
use crate::schemas::tool_list_json;
use std::io::{self, Read, Write};
use std::path::Path;

pub fn serve(project: &Path, storage: Option<&Path>) -> Result<(), String> {
    let mut engine = Engine::open(project, storage)?;
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    loop {
        let Some(body) = read_message(&mut input).map_err(|e| e.to_string())? else { break };
        let Ok(msg) = json::parse(&body) else {
            let err = Value::object(vec![
                ("jsonrpc".into(), Value::String("2.0".into())),
                ("id".into(), Value::Null),
                ("error".into(), Value::object(vec![
                    ("code".into(), Value::Int(-32700)),
                    ("message".into(), Value::String("parse error".into())),
                ])),
            ]);
            write_message(&mut output, &json::to_string(&err).unwrap_or_default()).map_err(|e| e.to_string())?;
            continue;
        };
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();
        if method.starts_with("notifications/") || method == "initialized" || id.is_none() && method != "initialize" {
            if method == "initialized" || method.starts_with("notifications/") {
                continue;
            }
        }
        let result = match method {
            "initialize" => Value::object(vec![
                ("protocolVersion".into(), Value::String("2024-11-05".into())),
                ("capabilities".into(), Value::object(vec![
                    ("tools".into(), Value::object(vec![("listChanged".into(), Value::Bool(false))])),
                ])),
                ("serverInfo".into(), Value::object(vec![
                    ("name".into(), Value::String("chisel".into())),
                    ("version".into(), Value::String(env!("CARGO_PKG_VERSION").into())),
                ])),
                ("instructions".into(), Value::String(
                    "Storage is .chisel/store. Branch on status. Call analyze when status is no_data. Call diff_impact after edits. This process is stdio MCP (chisel serve-mcp).".into()
                )),
            ]),
            "tools/list" => tool_list_json(),
            "tools/call" => {
                let params = msg.get("params");
                let name = params.and_then(|p| p.get("name")).and_then(|v| v.as_str()).unwrap_or("");
                let empty = Value::object(vec![]);
                let args = params.and_then(|p| p.get("arguments")).unwrap_or(&empty);
                match engine.dispatch(name, args) {
                    Ok(result) => {
                        let steps = compute_next_steps(name, &result);
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
                    Err(e) => Value::object(vec![
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
                    write_message(&mut output, &json::to_string(&err).unwrap_or_default()).map_err(|e| e.to_string())?;
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
            write_message(&mut output, &json::to_string(&resp).map_err(|e| e.message)?).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn read_message(input: &mut impl Read) -> io::Result<Option<String>> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = input.read(&mut byte)?;
        if n == 0 {
            return Ok(None);
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() > 64 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "header too large"));
        }
    }
    let text = String::from_utf8_lossy(&header);
    let mut len = 0usize;
    for line in text.split("\r\n") {
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    if len > 8 * 1024 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    Ok(Some(String::from_utf8_lossy(&body).into_owned()))
}

pub fn write_message(output: &mut impl Write, body: &str) -> io::Result<()> {
    write!(output, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    output.flush()
}
