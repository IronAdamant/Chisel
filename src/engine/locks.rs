use super::*;
use crate::extract::{self, CodeUnit, Dep};
use crate::git::{self, GitAnalyzer};
use crate::hash::sha256_hex;
use crate::impact::{self, RiskInputs};
use crate::json::Value;
use crate::store::{self, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

impl Engine {
    pub(super) fn active_locks(&self) -> Vec<Value> {
        let now = now_secs();
        self.store.array("file_locks").into_iter().filter(|l| l.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0) >= now).collect()
    }

    pub(super) fn file_lock_tool(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let file = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let agent = args.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let ttl = args.get("ttl").and_then(|v| v.as_i64()).unwrap_or(300);
        let purpose = args.get("purpose").and_then(|v| v.as_str()).map(|s| s.to_string());
        self.drop_expired_locks();
        match name {
            "acquire_file_lock" => {
                let existing = self.active_locks().into_iter().find(|l| l.get("file_path").and_then(|v| v.as_str()) == Some(file.as_str()));
                if let Some(lock) = existing {
                    let holder = lock.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
                    if holder != agent {
                        return Ok(Value::object(vec![
                            ("acquired".into(), Value::Bool(false)),
                            ("holder".into(), Value::String(holder.into())),
                            ("expires_at".into(), lock.get("expires_at").cloned().unwrap_or(Value::Null)),
                        ]));
                    }
                }
                let now = now_secs();
                let expires = now + ttl as f64;
                let kept: Vec<Value> = self.store.array("file_locks").into_iter().filter(|l| l.get("file_path").and_then(|v| v.as_str()) != Some(file.as_str())).collect();
                self.store.doc.insert("file_locks", Value::Array(kept));
                self.store.array_mut("file_locks").push(Value::object(vec![
                    ("file_path".into(), Value::String(file.clone())),
                    ("agent_id".into(), Value::String(agent)),
                    ("acquired_at".into(), Value::Float(now)),
                    ("expires_at".into(), Value::Float(expires)),
                    ("purpose".into(), purpose.map(Value::String).unwrap_or(Value::Null)),
                ]));
                self.store.save()?;
                Ok(Value::object(vec![
                    ("acquired".into(), Value::Bool(true)),
                    ("holder".into(), Value::Null),
                    ("expires_at".into(), Value::Float(expires)),
                ]))
            }
            "release_file_lock" => {
                let before = self.store.array("file_locks");
                let held = before.iter().any(|l| l.get("file_path").and_then(|v| v.as_str()) == Some(file.as_str()) && l.get("agent_id").and_then(|v| v.as_str()) == Some(agent.as_str()));
                let kept: Vec<Value> = before.into_iter().filter(|l| {
                    !(l.get("file_path").and_then(|v| v.as_str()) == Some(file.as_str()) && l.get("agent_id").and_then(|v| v.as_str()) == Some(agent.as_str()))
                }).collect();
                self.store.doc.insert("file_locks", Value::Array(kept));
                self.store.save()?;
                Ok(Value::object(vec![("released".into(), Value::Bool(held))]))
            }
            "refresh_file_lock" => {
                let now = now_secs();
                let expires = now + ttl as f64;
                let mut ok = false;
                for lock in self.store.array_mut("file_locks") {
                    if lock.get("file_path").and_then(|v| v.as_str()) == Some(file.as_str()) && lock.get("agent_id").and_then(|v| v.as_str()) == Some(agent.as_str()) {
                        lock.insert("expires_at", Value::Float(expires));
                        ok = true;
                    }
                }
                self.store.save()?;
                Ok(Value::object(vec![
                    ("refreshed".into(), Value::Bool(ok)),
                    ("expires_at".into(), if ok { Value::Float(expires) } else { Value::Null }),
                ]))
            }
            "check_file_lock" => Ok(self.lock_status(&file)),
            "check_locks" => {
                let paths = args.get("file_paths").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_else(|| if file.is_empty() { Vec::new() } else { vec![file] });
                let mut conflicts = Vec::new();
                for fp in &paths {
                    let status = self.lock_status(fp);
                    if status.get("locked").and_then(|v| v.as_bool()) == Some(true) {
                        conflicts.push(Value::object(vec![
                            ("file_path".into(), Value::String(fp.clone())),
                            ("holder".into(), status.get("holder").cloned().unwrap_or(Value::Null)),
                            ("expires_at".into(), status.get("expires_at").cloned().unwrap_or(Value::Null)),
                            ("ttl_remaining".into(), status.get("ttl_remaining").cloned().unwrap_or(Value::Null)),
                            ("stale".into(), status.get("stale").cloned().unwrap_or(Value::Bool(false))),
                        ]));
                    }
                }
                Ok(Value::object(vec![
                    ("conflicts".into(), Value::Array(conflicts)),
                    ("checked".into(), Value::Int(paths.len() as i64)),
                ]))
            }
            "list_file_locks" => {
                let agent_filter = args.get("agent_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                let now = now_secs();
                let mut locks = Vec::new();
                for lock in self.active_locks() {
                    if let Some(agent) = agent_filter.as_deref() {
                        if lock.get("agent_id").and_then(|v| v.as_str()) != Some(agent) { continue; }
                    }
                    let expires = lock.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(now);
                    let mut row = lock;
                    row.insert("ttl_remaining", Value::Float(crate::json::py_round((expires - now).max(0.0), 1)));
                    locks.push(row);
                }
                let total = locks.len() as i64;
                Ok(Value::object(vec![
                    ("locks".into(), Value::Array(locks)),
                    ("total".into(), Value::Int(total)),
                ]))
            }
            _ => Err("unreachable".into()),
        }
    }

    pub(super) fn drop_expired_locks(&mut self) {
        let now = now_secs();
        let kept: Vec<Value> = self.store.array("file_locks").into_iter().filter(|l| l.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0) >= now).collect();
        self.store.doc.insert("file_locks", Value::Array(kept));
    }

    pub(super) fn lock_status(&self, file: &str) -> Value {
        let now = now_secs();
        let lock = self.active_locks().into_iter().find(|l| l.get("file_path").and_then(|v| v.as_str()) == Some(file));
        let Some(lock) = lock else {
            return Value::object(vec![
                ("locked".into(), Value::Bool(false)),
                ("holder".into(), Value::Null),
                ("expires_at".into(), Value::Null),
                ("ttl_remaining".into(), Value::Null),
            ]);
        };
        let expires = lock.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(now);
        let ttl = crate::json::py_round((expires - now).max(0.0), 1);
        Value::object(vec![
            ("locked".into(), Value::Bool(true)),
            ("holder".into(), lock.get("agent_id").cloned().unwrap_or(Value::Null)),
            ("acquired_at".into(), lock.get("acquired_at").cloned().unwrap_or(Value::Null)),
            ("expires_at".into(), Value::Float(expires)),
            ("ttl_remaining".into(), Value::Float(ttl)),
            ("stale".into(), Value::Bool(ttl < 60.0)),
            ("purpose".into(), lock.get("purpose").cloned().unwrap_or(Value::Null)),
        ])
    }
}
