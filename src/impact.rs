//! Risk, churn, coupling, and impact ranking. Weights match risk_meta.py.

use crate::json::{py_round, quantize_gap, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const COCHANGE_CAP: f64 = 10.0;
pub const IMPORT_CAP: f64 = 8.0;
const PROXIMITY_ALPHA: f64 = 0.15;
const PROXIMITY_CAP_HOPS: f64 = 5.0;

const COMPONENTS: [&str; 6] = [
    "churn",
    "coupling",
    "coverage_gap",
    "coverage_depth",
    "author_concentration",
    "test_instability",
];

pub fn weight(name: &str) -> f64 {
    match name {
        "churn" => 0.35,
        "coupling" => 0.25,
        "coverage_gap" => 0.15,
        "coverage_depth" => 0.10,
        "author_concentration" => 0.10,
        "test_instability" => 0.05,
        _ => 0.0,
    }
}

pub fn compose_risk_score(
    churn_norm: f64,
    coupling_norm: f64,
    coverage_gap: f64,
    coverage_depth: f64,
    author_concentration: f64,
    test_instability: f64,
    hidden_risk_factor: f64,
    new_file_boost: f64,
) -> f64 {
    weight("churn") * churn_norm
        + weight("coupling") * coupling_norm
        + weight("coverage_gap") * coverage_gap
        + weight("coverage_depth") * coverage_depth
        + weight("author_concentration") * author_concentration
        + weight("test_instability") * test_instability
        + hidden_risk_factor
        + new_file_boost
}

pub fn hidden_risk(dynamic_edge_count: f64) -> f64 {
    (dynamic_edge_count / 20.0).min(1.0) * 0.15
}

pub fn coupling_threshold(commit_count: i64) -> i64 {
    if commit_count <= 0 {
        return 2;
    }
    let v = ((commit_count as f64).log2() / 2.0) as i64 + 1;
    v.max(2)
}

pub fn days_since(date: &str, now_unix: f64) -> Option<f64> {
    let ts = parse_iso_unix(date)?;
    Some(((now_unix - ts) / 86400.0).max(0.0))
}

pub fn parse_iso_unix(date: &str) -> Option<f64> {
    // 2020-01-02T03:04:05+00:00 or Z or without zone
    let date = date.trim();
    if date.len() < 19 {
        return None;
    }
    let year: i32 = date[0..4].parse().ok()?;
    let month: u32 = date[5..7].parse().ok()?;
    let day: u32 = date[8..10].parse().ok()?;
    let hour: u32 = date[11..13].parse().ok()?;
    let min: u32 = date[14..16].parse().ok()?;
    let sec: u32 = date[17..19].parse().ok()?;
    let mut rest = &date[19..];
    let mut frac = 0.0;
    if let Some(stripped) = rest.strip_prefix('.') {
        let digits: String = stripped.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            frac = format!("0.{digits}").parse().unwrap_or(0.0);
            rest = &stripped[digits.len()..];
        }
    }
    let mut offset = 0i32;
    if rest.starts_with('Z') || rest.is_empty() {
        offset = 0;
    } else if rest.starts_with('+') || rest.starts_with('-') {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let hh: i32 = rest.get(1..3)?.parse().ok()?;
        let mm: i32 = if rest.len() >= 6 { rest[4..6].parse().ok()? } else { 0 };
        offset = sign * (hh * 3600 + mm * 60);
    }
    Some(unix_utc(year, month, day, hour, min, sec) + frac - offset as f64)
}

fn unix_utc(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> f64 {
    let mut y = year;
    let mut m = month as i32;
    if m <= 2 {
        y -= 1;
        m += 12;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (m as u64 - 3) + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era as i64) * 146097 + doe as i64 - 719468;
    (days as f64) * 86400.0 + (hour as f64) * 3600.0 + (min as f64) * 60.0 + sec as f64
}

pub fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn churn_score(dates: &[String], now: f64) -> f64 {
    let mut score = 0.0;
    for d in dates {
        if let Some(days) = days_since(d, now) {
            score += 1.0 / (1.0 + days);
        }
    }
    py_round(score, 6)
}

pub fn author_concentration(line_counts: &BTreeMap<String, i64>) -> f64 {
    let total: i64 = line_counts.values().sum();
    if total <= 0 || line_counts.is_empty() {
        return 1.0;
    }
    let hhi: f64 = line_counts.values().map(|c| {
        let f = *c as f64 / total as f64;
        f * f
    }).sum();
    py_round(hhi, 4)
}

pub fn apply_proximity(coverage_gap: f64, min_hops: i64) -> f64 {
    let factor = 1.0 - PROXIMITY_ALPHA * (PROXIMITY_CAP_HOPS - min_hops as f64).max(0.0) / PROXIMITY_CAP_HOPS;
    coverage_gap * factor.max(0.0)
}

pub struct RiskInputs {
    pub file_path: String,
    pub churn_raw: f64,
    pub co_change_count: usize,
    pub branch_co_count: usize,
    pub import_neighbors: Vec<String>,
    pub coupling_partners: Vec<(String, i64)>,
    pub tested_lines: i64,
    pub total_lines: i64,
    pub tested_units: i64,
    pub unit_count: i64,
    pub covering_tests: usize,
    pub call_edges: i64,
    pub total_edges: i64,
    pub dynamic_edges: i64,
    pub author_conc: f64,
    pub instability: f64,
    pub eval_patterns: i64,
    pub exclude_new_file_boost: bool,
    pub coverage_mode: String,
    pub proximity_hops: Option<i64>,
    pub proximity: bool,
}

pub fn score_file(inp: &RiskInputs) -> Value {
    let churn_norm = (inp.churn_raw / 5.0).min(1.0);
    let co_g = (inp.co_change_count as f64 / COCHANGE_CAP).min(1.0);
    let co_b = (inp.branch_co_count as f64 / COCHANGE_CAP).min(1.0);
    let cochange = co_g.max(co_b);
    let import_c = (inp.import_neighbors.len() as f64 / IMPORT_CAP).min(1.0);
    let coupling = cochange.max(import_c).max(0.5 * cochange + 0.5 * import_c);
    let coverage = if inp.coverage_mode == "line" && inp.total_lines > 0 {
        inp.tested_lines as f64 / inp.total_lines as f64
    } else {
        inp.tested_units as f64 / (inp.unit_count.max(1) as f64)
    };
    let mut gap = quantize_gap(1.0 - coverage);
    if inp.proximity && gap > 0.0 {
        if let Some(h) = inp.proximity_hops {
            if h > 0 {
                gap = apply_proximity(gap, h);
            }
        }
    }
    let depth = py_round((inp.covering_tests as f64 / 5.0).min(1.0), 4);
    let quality = if inp.total_edges > 0 {
        py_round(inp.call_edges as f64 / inp.total_edges as f64, 4)
    } else {
        0.0
    };
    let hidden = hidden_risk(inp.dynamic_edges as f64);
    let new_boost = if !inp.exclude_new_file_boost && churn_norm == 0.0 && coverage == 0.0 {
        0.5
    } else {
        0.0
    };
    let risk = compose_risk_score(churn_norm, coupling, gap, depth, inp.author_conc, inp.instability, hidden, new_boost);
    let partners = Value::Array(inp.coupling_partners.iter().take(3).map(|(f, n)| {
        Value::object(vec![
            ("file".into(), Value::String(f.clone())),
            ("co_commits".into(), Value::Int(*n)),
        ])
    }).collect());
    let imports = Value::Array(inp.import_neighbors.iter().take(3).map(|f| {
        Value::object(vec![("file".into(), Value::String(f.clone()))])
    }).collect());
    Value::object(vec![
        ("file_path".into(), Value::String(inp.file_path.clone())),
        ("unit_name".into(), Value::Null),
        ("risk_score".into(), Value::Float(py_round(risk, 4))),
        ("shadow_edge_count".into(), Value::Int(inp.total_edges - inp.call_edges)),
        ("dynamic_edge_count".into(), Value::Int(inp.dynamic_edges)),
        ("unknown_require_count".into(), Value::Int(inp.eval_patterns)),
        ("hidden_risk_factor".into(), Value::Float(py_round(hidden, 4))),
        ("new_file_boost".into(), Value::Float(py_round(new_boost, 4))),
        ("coupling_partners".into(), partners),
        ("import_partners".into(), imports),
        ("breakdown".into(), Value::object(vec![
            ("churn".into(), Value::Float(py_round(churn_norm, 4))),
            ("coupling".into(), Value::Float(py_round(coupling, 4))),
            ("import_coupling".into(), Value::Float(py_round(import_c, 4))),
            ("cochange_coupling".into(), Value::Float(py_round(cochange, 4))),
            ("cochange_global".into(), Value::Float(py_round(co_g, 4))),
            ("cochange_branch".into(), Value::Float(py_round(co_b, 4))),
            ("coverage_gap".into(), Value::Float(py_round(gap, 4))),
            ("coverage_fraction".into(), Value::Float(py_round(coverage, 4))),
            ("coverage_depth".into(), depth_num(depth)),
            ("edge_type_quality".into(), Value::Float(quality)),
            ("author_concentration".into(), Value::Float(py_round(inp.author_conc, 4))),
            ("test_instability".into(), Value::Float(py_round(inp.instability, 4))),
            ("hidden_risk".into(), Value::Float(py_round(hidden, 4))),
            ("new_file_boost".into(), Value::Float(py_round(new_boost, 4))),
        ])),
    ])
}

fn depth_num(depth: f64) -> Value {
    Value::Float(depth)
}

fn breakdown_f(entry: &Value, comp: &str) -> f64 {
    entry.get("breakdown").and_then(|b| b.get(comp)).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

pub fn build_risk_meta(files: &[Value], commits: i64, test_edges: i64, test_results: i64, import_edges: i64, co_changes: i64) -> Value {
    if files.is_empty() {
        return Value::object(vec![("total_files".into(), Value::Int(0))]);
    }
    let mut uniform = Vec::new();
    let mut effective = Vec::new();
    for comp in COMPONENTS {
        let mut vals = BTreeSet::new();
        for f in files {
            vals.insert(format!("{:.4}", breakdown_f(f, comp)));
        }
        if vals.len() <= 1 {
            let val = files.first().map(|f| breakdown_f(f, comp)).unwrap_or(0.0);
            let reason = diagnose(comp, val, import_edges, co_changes, commits, test_edges, test_results);
            uniform.push((comp, val, reason));
        } else {
            effective.push(comp);
        }
    }
    let mut meta = vec![
        ("total_files".into(), Value::Int(files.len() as i64)),
        ("coupling_threshold".into(), if commits > 0 { Value::Int(coupling_threshold(commits)) } else { Value::Null }),
        ("total_test_edges".into(), Value::Int(test_edges)),
        ("total_test_results".into(), Value::Int(test_results)),
        ("effective_components".into(), Value::Array(effective.into_iter().map(|s| Value::String(s.into())).collect())),
        ("uniform_components".into(), Value::object(uniform.into_iter().map(|(c, v, r)| {
            (c.into(), Value::object(vec![
                ("value".into(), Value::Float(v)),
                ("reason".into(), Value::String(r)),
            ]))
        }).collect())),
    ];
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for f in files {
        let s = format!("{:.4}", f.get("risk_score").and_then(|v| v.as_f64()).unwrap_or(0.0));
        *counts.entry(s).or_default() += 1;
    }
    let groups: Vec<_> = counts.into_iter().filter(|(_, c)| *c >= 3).collect();
    if !groups.is_empty() {
        let max_group = groups.iter().map(|(_, c)| *c).max().unwrap_or(0);
        meta.push(("uniform_risk_groups".into(), Value::object(groups.into_iter().map(|(k, c)| (k, Value::Int(c))).collect())));
        meta.push(("max_identical_risk_files".into(), Value::Int(max_group)));
        if max_group >= 5 {
            meta.push(("warnings".into(), Value::Array(vec![Value::String(format!(
                "{max_group} files share an identical risk score; risk differentiation is low (consider exclude_new_file_boost or more test results via record_result)"
            ))])));
        }
    }
    Value::object(meta)
}

fn diagnose(comp: &str, value: f64, import_edges: i64, co_changes: i64, commits: i64, test_edges: i64, test_results: i64) -> String {
    let thr = coupling_threshold(commits);
    match comp {
        "coupling" => {
            if value == 0.0 {
                if import_edges == 0 && co_changes == 0 {
                    return "no import edges and no co-change pairs found; edge builder may not match this project's import/require patterns".into();
                }
                if import_edges == 0 {
                    return "no import edges found; edge builder may not match this project's import/require patterns (co-changes exist but no structural coupling)".into();
                }
                if co_changes == 0 {
                    return format!("no co-changes above threshold ({thr}); single-author or bulk-commit history produces no co-change signal");
                }
                return format!("all files have zero coupling despite {import_edges} import edges and {co_changes} co-change pairs; coupling caps may be too restrictive");
            }
            "all files equally coupled".into()
        }
        "coverage_gap" => {
            if value == 1.0 && test_edges == 0 {
                "no test edges found; edge builder may not match this project's import/require patterns".into()
            } else if value == 1.0 {
                "no code units have test edges despite edges existing".into()
            } else if value == 0.0 {
                "all code units have test coverage".into()
            } else {
                format!("all files have identical coverage ({value})")
            }
        }
        "coverage_depth" => {
            if value == 0.0 {
                "no code units covered by multiple test files".into()
            } else {
                format!("all files have identical coverage depth ({value})")
            }
        }
        "test_instability" => {
            if value == 0.0 && test_results == 0 {
                "no test results recorded; use record_result after running tests".into()
            } else if value == 0.0 {
                "all covering tests passing".into()
            } else {
                format!("all files have identical instability ({value})")
            }
        }
        "author_concentration" => {
            if value == 1.0 {
                "single author per file (common in small teams)".into()
            } else {
                format!("all files have identical concentration ({value})")
            }
        }
        "churn" => {
            if value == 0.0 {
                "no churn data; run analyze with git history".into()
            } else {
                format!("all files have identical churn ({value})")
            }
        }
        _ => String::new(),
    }
}

pub fn apply_reweighting(files: &mut [Value]) -> Value {
    if files.len() <= 1 {
        return Value::object(vec![
            ("reweighted".into(), Value::Bool(false)),
            ("reweighting_skipped_reason".into(), Value::String("insufficient_files".into())),
        ]);
    }
    let mut uniform: Vec<(&str, f64)> = Vec::new();
    for comp in COMPONENTS {
        let mut vals = BTreeSet::new();
        for f in files.iter() {
            vals.insert(format!("{:.6}", breakdown_f(f, comp)));
        }
        if vals.len() <= 1 {
            let val = files.first().map(|f| breakdown_f(f, comp)).unwrap_or(0.0);
            uniform.push((comp, val));
        }
    }
    let zero_uniform = uniform.iter().any(|(_, v)| *v == 0.0);
    if uniform.len() < 2 && !zero_uniform {
        return Value::object(vec![("reweighted".into(), Value::Bool(false))]);
    }
    let effective: Vec<&str> = COMPONENTS.iter().copied().filter(|c| !uniform.iter().any(|(u, _)| u == c)).collect();
    if effective.is_empty() {
        return Value::object(vec![
            ("reweighted".into(), Value::Bool(false)),
            ("reweighting_skipped_reason".into(), Value::String("all_components_uniform".into())),
        ]);
    }
    let dead: f64 = uniform.iter().map(|(c, _)| weight(c)).sum();
    let sum_eff: f64 = effective.iter().map(|c| weight(c)).sum();
    let mut new_w = BTreeMap::new();
    for c in &effective {
        new_w.insert(*c, weight(c) + dead * (weight(c) / sum_eff));
    }
    for entry in files.iter_mut() {
        let risk: f64 = effective.iter().map(|c| new_w[c] * breakdown_f(entry, c)).sum();
        entry.insert("risk_score", Value::Float(py_round(risk, 4)));
    }
    Value::object(vec![
        ("reweighted".into(), Value::Bool(true)),
        ("effective_weights".into(), Value::object(effective.into_iter().map(|c| {
            (c.into(), Value::Float(py_round(new_w[c], 4)))
        }).collect())),
    ])
}

pub fn hops_to_tested(files: &BTreeSet<String>, tested: &BTreeSet<String>, neighbors: &BTreeMap<String, BTreeSet<String>>) -> BTreeMap<String, i64> {
    let mut dist = BTreeMap::new();
    let mut q: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for t in tested {
        dist.insert(t.clone(), 0);
        q.push_back(t.clone());
    }
    while let Some(u) = q.pop_front() {
        let d = dist[&u];
        if let Some(nei) = neighbors.get(&u) {
            for v in nei {
                if files.contains(v) && !dist.contains_key(v) {
                    dist.insert(v.clone(), d + 1);
                    q.push_back(v.clone());
                }
            }
        }
    }
    dist
}
