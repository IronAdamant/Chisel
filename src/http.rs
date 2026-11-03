//! HTTP/1.1 server for GET /health, GET /tools, and POST /call.
//! One request per connection. Binds 127.0.0.1 by default.

use crate::engine::Engine;
use crate::json::{self, Value};
use crate::next_steps::compute_next_steps;
use crate::schemas::tool_list_json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

pub fn serve(project: &Path, storage: Option<&Path>, host: &str, port: u16) -> Result<(), String> {
    let engine = Arc::new(Mutex::new(Engine::open(project, storage)?));
    let listener = TcpListener::bind((host, port)).map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    println!("Starting HTTP server on http://{addr}");
    let _ = std::io::stdout().flush();
    let (tx, rx) = mpsc::sync_channel::<TcpStream>(128);
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..32 {
        let rx = Arc::clone(&rx);
        let engine = Arc::clone(&engine);
        thread::spawn(move || {
            loop {
                let stream = {
                    let guard = match rx.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    guard.recv()
                };
                let Ok(mut stream) = stream else { break };
                let _ = handle(&mut stream, &engine);
            }
        });
    }
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        match tx.try_send(stream) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(mut stream)) | Err(mpsc::TrySendError::Disconnected(mut stream)) => {
                let _ = write_response(&mut stream, 503, &Value::object(vec![
                    ("error".into(), Value::String("queue full".into())),
                ]));
            }
        }
    }
    Ok(())
}

fn write_response(stream: &mut TcpStream, status: u16, payload: &Value) -> std::io::Result<()> {
    let bytes = json::to_string(payload).unwrap_or_else(|_| "{\"error\":\"encode\"}".into());
    let resp = format!(
        "HTTP/1.0 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reason(status),
        bytes.len(),
        bytes
    );
    stream.write_all(resp.as_bytes())
}

fn handle(stream: &mut TcpStream, engine: &Arc<Mutex<Engine>>) -> std::io::Result<()> {
    let mut buf = vec![0u8; 1024 * 64];
    let n = stream.read(&mut buf)?;
    if n == 0 {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&buf[..n]);
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let mut content_length = 0usize;
    let mut too_large = false;
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
            if content_length > 1024 * 1024 {
                too_large = true;
            }
        }
    }
    if too_large {
        return write_response(stream, 400, &Value::object(vec![
            ("error".into(), Value::String("body too large".into())),
        ]));
    }
    let header_end = text.find("\r\n\r\n").map(|i| i + 4).unwrap_or(text.len());
    let mut body = text[header_end.min(text.len())..].as_bytes().to_vec();
    while body.len() < content_length && body.len() < 32 * 1024 * 1024 {
        let mut tmp = [0u8; 4096];
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    let (status, payload) = route(method, path, &body, engine);
    write_response(stream, status, &payload)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn missing_required_result(tool: &str, result: &Value) -> bool {
    if result.get("status").and_then(|v| v.as_str()) != Some("error") {
        return false;
    }
    let Some(message) = result.get("message").and_then(|v| v.as_str()) else { return false };
    crate::schemas::required_fields(tool).iter().any(|field| message == format!("{tool} requires {field}"))
}

fn route(method: &str, path: &str, body: &[u8], engine: &Arc<Mutex<Engine>>) -> (u16, Value) {
    if method == "GET" && path == "/health" {
        return (200, Value::object(vec![("status".into(), Value::String("ok".into()))]));
    }
    if method == "GET" && path == "/tools" {
        return (200, tool_list_json());
    }
    if method == "POST" && path == "/call" {
        if body.len() > 1024 * 1024 {
            return (400, Value::object(vec![("error".into(), Value::String("body too large".into()))]));
        }
        let text = String::from_utf8_lossy(body);
        let Ok(doc) = json::parse(&text) else {
            return (400, Value::object(vec![("error".into(), Value::String("Invalid JSON".into()))]));
        };
        let Some(tool) = doc.get("tool").and_then(|v| v.as_str()) else {
            return (400, Value::object(vec![("error".into(), Value::String("Missing 'tool' field in request body".into()))]));
        };
        let empty = Value::object(vec![]);
        let args = doc.get("arguments").unwrap_or(&empty);
        let mut eng = match engine.lock() {
            Ok(g) => g,
            Err(_) => return (500, Value::object(vec![("error".into(), Value::String("lock".into()))])),
        };
        return match eng.dispatch(tool, args) {
            Ok(result) => {
                let missing = missing_required_result(tool, &result);
                let mut steps = compute_next_steps(tool, &result);
                if tool == "suggest_tests" {
                    let file = args.get("file_path").and_then(|v| v.as_str());
                    if let Some(warning) = crate::next_steps::eval_warning(eng.project_dir.as_path(), file) {
                        steps.push(warning);
                    }
                }
                let mut payload = vec![("result".into(), result)];
                if !steps.is_empty() {
                    payload.push(("next_steps".into(), Value::Array(steps)));
                }
                (if missing { 400 } else { 200 }, Value::object(payload))
            }
            Err(e) if e.starts_with("unknown tool") => (404, Value::object(vec![("error".into(), Value::String(e))])),
            Err(e) => (500, Value::object(vec![
                ("status".into(), Value::String("error".into())),
                ("message".into(), Value::String(e)),
            ])),
        };
    }
    (404, Value::object(vec![("error".into(), Value::String(format!("Unknown endpoint: {path}")))]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::SharedStr;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn dummy_arg(tool: &str, field: &str) -> Value {
        match crate::schemas::field_type(tool, field).as_deref() {
            Some("boolean") => Value::Bool(false),
            Some("array") => Value::Array(vec![Value::String("x".into())]),
            Some("integer") | Some("number") => Value::Int(1),
            _ => Value::String("x".into()),
        }
    }

    fn args_omitting(tool: &str, omit: &str, bad: Option<Value>) -> Value {
        let mut pairs = Vec::new();
        for field in crate::schemas::required_fields(tool) {
            if field == omit {
                if let Some(value) = bad.clone() {
                    pairs.push((SharedStr::from(field), value));
                }
                continue;
            }
            pairs.push((SharedStr::from(field.clone()), dummy_arg(tool, &field)));
        }
        Value::object(pairs)
    }

    #[test]
    fn every_omitted_required_field_is_http_400() {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("chisel-http-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("store");
        let engine = Arc::new(Mutex::new(Engine::open(&dir, Some(&store)).unwrap()));
        let snap = {
            let eng = engine.lock().unwrap();
            eng.store.snapshot_path()
        };
        let mut checked = 0usize;
        for tool in crate::schemas::TOOL_NAMES {
            for field in crate::schemas::required_fields(tool) {
                let ty = crate::schemas::field_type(tool, &field).unwrap_or_default();
                let mut bad = vec![None, Some(Value::Null)];
                if ty == "string" {
                    bad.push(Some(Value::String(String::new())));
                }
                if ty == "array" {
                    bad.push(Some(Value::Array(vec![])));
                }
                for value in bad {
                    let args = args_omitting(tool, &field, value);
                    let before = std::fs::read(&snap).ok();
                    let result = engine.lock().unwrap().dispatch(tool, &args).unwrap();
                    let message = result.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    assert_eq!(result.get("status").and_then(|v| v.as_str()), Some("error"), "{tool} {field} {message}");
                    assert_eq!(message, format!("{tool} requires {field}"));
                    if matches!(tool, "record_result" | "start_job" | "acquire_file_lock") {
                        assert_eq!(std::fs::read(&snap).ok(), before, "{tool} {field} wrote the store");
                    }
                    let body = json::to_string(&Value::object(vec![
                        ("tool".into(), Value::String(tool.into())),
                        ("arguments".into(), args),
                    ])).unwrap();
                    let (status, payload) = route("POST", "/call", body.as_bytes(), &engine);
                    let text = json::to_string(&payload).unwrap();
                    assert_eq!(status, 400, "{tool} {field} {text}");
                    assert_eq!(payload.get("result").and_then(|v| v.get("message")).and_then(|v| v.as_str()), Some(message.as_str()));
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "schema required fields were not loaded");
        let known = br#"{"tool":"job_status","arguments":{"job_id":"missing"}}"#;
        let (status, payload) = route("POST", "/call", known, &engine);
        let text = json::to_string(&payload).unwrap();
        assert_eq!(status, 200, "{text}");
        assert!(text.contains("unknown job"), "{text}");
        let unknown = br#"{"tool":"no_such_tool","arguments":{}}"#;
        let (status, _) = route("POST", "/call", unknown, &engine);
        assert_eq!(status, 404);
    }
}
