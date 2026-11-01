//! HTTP/1.1 server for GET /health, GET /tools, and POST /call.
//! One request per connection. Binds 127.0.0.1 by default.

use crate::engine::Engine;
use crate::json::{self, Value};
use crate::schemas::tool_list_json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

pub fn serve(project: &Path, storage: Option<&Path>, host: &str, port: u16) -> Result<(), String> {
    let engine = Arc::new(Mutex::new(Engine::open(project, storage)?));
    let listener = TcpListener::bind((host, port)).map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    println!("Starting HTTP server on http://{addr}");
    let _ = std::io::stdout().flush();
    for conn in listener.incoming() {
        let Ok(mut stream) = conn else { continue };
        let engine = Arc::clone(&engine);
        thread::spawn(move || {
            let _ = handle(&mut stream, &engine);
        });
    }
    Ok(())
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
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
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
    let bytes = json::to_string(&payload).unwrap_or_else(|_| "{\"error\":\"encode\"}".into());
    let resp = format!(
        "HTTP/1.0 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reason(status),
        bytes.len(),
        bytes
    );
    stream.write_all(resp.as_bytes())?;
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    }
}

fn route(method: &str, path: &str, body: &[u8], engine: &Arc<Mutex<Engine>>) -> (u16, Value) {
    if method == "GET" && path == "/health" {
        return (200, Value::object(vec![("status".into(), Value::String("ok".into()))]));
    }
    if method == "GET" && path == "/tools" {
        return (200, tool_list_json());
    }
    if method == "POST" && path == "/call" {
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
            Ok(result) => (200, Value::object(vec![("result".into(), result)])),
            Err(e) => (404, Value::object(vec![("error".into(), Value::String(e))])),
        };
    }
    (404, Value::object(vec![("error".into(), Value::String(format!("Unknown endpoint: {path}")))]))
}
