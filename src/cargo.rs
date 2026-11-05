//! Which Cargo crates a Rust test can reach.
//!
//! A test in crate `a` can only call code in `a`, its `[dependencies]` (transitively) and its own
//! `[dev-dependencies]`. Names matched across a workspace ignore that, so a test calling
//! `Cell::new` or `x.front()` could link to a crate it cannot see. `Crates` reads the manifests
//! (lazily, per directory) and answers `allows(test_file, code_file)`. Files outside any
//! package, and projects without `Cargo.toml`, are never filtered.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Default)]
struct Manifest {
    package: bool,
    workspace: bool,
    /// Crate directories of `[dependencies]` (and target-specific ones) that are path or workspace deps.
    deps: Vec<String>,
    dev: Vec<String>,
    /// `workspace = true` keys, resolved through the workspace root.
    ws_deps: Vec<String>,
    ws_dev: Vec<String>,
    /// `[workspace.dependencies]` key -> crate directory.
    ws_table: HashMap<String, String>,
}

pub struct Crates {
    root: PathBuf,
    /// Relative directory -> crate directory that owns it (`None`: not inside a package).
    owner: HashMap<String, Option<String>>,
    manifests: HashMap<String, Option<Rc<Manifest>>>,
    reach: HashMap<String, Rc<BTreeSet<String>>>,
}

fn parent(dir: &str) -> Option<&str> {
    if dir.is_empty() {
        return None;
    }
    Some(dir.rsplit_once('/').map(|(p, _)| p).unwrap_or(""))
}

/// `base` joined with a relative manifest path, `.` and `..` resolved, `/`-separated.
fn join(base: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

/// `path = "x"` and `workspace = true` from an inline table or a value list.
fn dep_fields(text: &str) -> (Option<String>, bool) {
    let mut path = None;
    let mut ws = false;
    for part in text.trim().trim_start_matches('{').trim_end_matches('}').split(',') {
        let Some((k, v)) = part.split_once('=') else { continue };
        match k.trim() {
            "path" => path = Some(v.trim().trim_matches(|c| c == '"' || c == '\'').to_string()),
            "workspace" => ws = v.trim() == "true",
            _ => {}
        }
    }
    (path, ws)
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Normal,
    Dev,
    Workspace,
    Other,
}

fn section_kind(header: &str) -> (Kind, Option<String>) {
    let h = header.trim();
    let (table, name) = match h.rsplit_once('.') {
        Some((t, n)) if !matches!(n, "dependencies" | "dev-dependencies" | "build-dependencies") => (t, Some(n.trim_matches('"').to_string())),
        _ => (h, None),
    };
    let last = table.rsplit('.').next().unwrap_or(table);
    let kind = if table == "workspace.dependencies" {
        Kind::Workspace
    } else if last == "dependencies" && (table == "dependencies" || table.starts_with("target.")) {
        Kind::Normal
    } else if last == "dev-dependencies" && (table == "dev-dependencies" || table.starts_with("target.")) {
        Kind::Dev
    } else {
        Kind::Other
    };
    (kind, name)
}

/// Reads the parts of a manifest that matter for reachability.
fn parse_manifest(dir: &str, text: &str) -> Manifest {
    let mut m = Manifest::default();
    let mut kind = Kind::Other;
    // `[dependencies.foo]` tables: the key comes from the header, fields from the lines.
    let mut table_dep: Option<String> = None;
    let mut fields = String::new();
    let flush = |m: &mut Manifest, kind: Kind, key: &str, fields: &str| {
        let (path, ws) = dep_fields(fields);
        match (kind, path, ws) {
            (Kind::Workspace, Some(p), _) => {
                m.ws_table.insert(key.to_string(), join(dir, &p));
            }
            (Kind::Normal, Some(p), _) => m.deps.push(join(dir, &p)),
            (Kind::Dev, Some(p), _) => m.dev.push(join(dir, &p)),
            (Kind::Normal, None, true) => m.ws_deps.push(key.to_string()),
            (Kind::Dev, None, true) => m.ws_dev.push(key.to_string()),
            _ => {}
        }
    };
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.starts_with('[') && !line.starts_with("[[") {
            if let Some(key) = table_dep.take() {
                flush(&mut m, kind, &key, &fields);
            }
            fields.clear();
            let header = line.trim_start_matches('[').trim_end_matches(']');
            match header.trim() {
                "package" => m.package = true,
                "workspace" => m.workspace = true,
                _ => {}
            }
            let (k, name) = section_kind(header);
            kind = k;
            table_dep = name.filter(|_| kind != Kind::Other);
            continue;
        }
        if line.starts_with("[[") {
            if let Some(key) = table_dep.take() {
                flush(&mut m, kind, &key, &fields);
            }
            kind = Kind::Other;
            continue;
        }
        if kind == Kind::Other {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        if table_dep.is_some() {
            fields.push_str(line);
            fields.push(',');
            continue;
        }
        let key = key.trim().trim_matches('"');
        // `foo.workspace = true`, `foo.path = "x"`
        if let Some((name, field)) = key.split_once('.') {
            flush(&mut m, kind, name.trim(), &format!("{} = {}", field.trim(), value.trim()));
        } else {
            flush(&mut m, kind, key, value);
        }
    }
    if let Some(key) = table_dep.take() {
        flush(&mut m, kind, &key, &fields);
    }
    m
}

impl Crates {
    pub fn new(root: &Path) -> Self {
        Crates { root: root.to_path_buf(), owner: HashMap::new(), manifests: HashMap::new(), reach: HashMap::new() }
    }

    fn manifest(&mut self, dir: &str) -> Option<Rc<Manifest>> {
        if let Some(m) = self.manifests.get(dir) {
            return m.clone();
        }
        let path = if dir.is_empty() { self.root.join("Cargo.toml") } else { self.root.join(dir).join("Cargo.toml") };
        let m = fs::read_to_string(path).ok().map(|t| Rc::new(parse_manifest(dir, &t)));
        self.manifests.insert(dir.to_string(), m.clone());
        m
    }

    /// The crate directory that owns `dir`: the nearest manifest with a `[package]`.
    fn owner_of_dir(&mut self, dir: &str) -> Option<String> {
        if let Some(o) = self.owner.get(dir) {
            return o.clone();
        }
        let found = match self.manifest(dir) {
            Some(m) if m.package => Some(dir.to_string()),
            Some(_) => None,
            None => match parent(dir) {
                Some(p) => self.owner_of_dir(p),
                None => None,
            },
        };
        self.owner.insert(dir.to_string(), found.clone());
        found
    }

    /// The crate directory a file belongs to, if any.
    pub fn crate_of(&mut self, file: &str) -> Option<String> {
        let file = file.replace('\\', "/");
        let dir = file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        self.owner_of_dir(dir)
    }

    fn workspace_table(&mut self, crate_dir: &str) -> HashMap<String, String> {
        let mut dir = Some(crate_dir.to_string());
        while let Some(d) = dir {
            if let Some(m) = self.manifest(&d) {
                if m.workspace {
                    return m.ws_table.clone();
                }
            }
            dir = parent(&d).map(|p| p.to_string());
        }
        HashMap::new()
    }

    fn deps_of(&mut self, crate_dir: &str, dev: bool) -> Vec<String> {
        let Some(m) = self.manifest(crate_dir) else { return Vec::new() };
        let (direct, ws) = if dev { (&m.dev, &m.ws_dev) } else { (&m.deps, &m.ws_deps) };
        let mut out = direct.clone();
        if !ws.is_empty() {
            let table = self.workspace_table(crate_dir);
            out.extend(ws.iter().filter_map(|k| table.get(k).cloned()));
        }
        out
    }

    /// Crates code in `crate_dir`'s tests can call: itself, its dependencies (transitively) and
    /// its dev-dependencies (and theirs).
    pub fn reachable(&mut self, crate_dir: &str) -> Rc<BTreeSet<String>> {
        if let Some(r) = self.reach.get(crate_dir) {
            return r.clone();
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        seen.insert(crate_dir.to_string());
        let mut queue: Vec<String> = self.deps_of(crate_dir, false);
        queue.extend(self.deps_of(crate_dir, true));
        while let Some(c) = queue.pop() {
            if seen.insert(c.clone()) {
                queue.extend(self.deps_of(&c, false));
            }
        }
        let r = Rc::new(seen);
        self.reach.insert(crate_dir.to_string(), r.clone());
        r
    }

    /// Whether a Rust test in `test_file` can call code in `code_file`. Anything outside a
    /// package, and tests that are not Rust, are allowed.
    pub fn allows(&mut self, test_file: &str, code_file: &str) -> bool {
        if !test_file.ends_with(".rs") {
            return true;
        }
        let (Some(t), Some(c)) = (self.crate_of(test_file), self.crate_of(code_file)) else { return true };
        t == c || self.reachable(&t).contains(&c)
    }

    /// Crates that can call into `crate_dir`: those whose dependency closure contains it.
    pub fn dependents(&mut self, crate_dir: &str, candidates: &BTreeSet<String>) -> BTreeSet<String> {
        candidates.iter().filter(|c| c.as_str() == crate_dir || self.reachable(c).contains(crate_dir)).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_path_workspace_and_table_deps() {
        let m = parse_manifest(
            "crates/a",
            r#"
[package]
name = "a" # the crate
[dependencies]
b = { workspace = true }
c = { path = "../c", version = "1" }
d.workspace = true
serde = "1"
[dev-dependencies]
e = { path = "../e" }
[build-dependencies]
f = { path = "../f" }
[dependencies.g]
path = "../g"
[target.'cfg(unix)'.dependencies]
h = { path = "../h" }
"#,
        );
        assert!(m.package && !m.workspace);
        assert_eq!(m.deps, vec!["crates/c", "crates/g", "crates/h"]);
        assert_eq!(m.ws_deps, vec!["b", "d"]);
        assert_eq!(m.dev, vec!["crates/e"]);
        let w = parse_manifest("", "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\nb = { path = \"crates/b\", version = \"0.3\" }\n");
        assert!(w.workspace && !w.package);
        assert_eq!(w.ws_table.get("b").map(|s| s.as_str()), Some("crates/b"));
    }
}
