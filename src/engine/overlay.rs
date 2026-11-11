//! Worktree overlays: answer for a git worktree from the main checkout's snapshot.
//!
//! Agents edit in their own `git worktree`, while the MCP server runs at the main checkout. With
//! `worktree_root` (CLI `--worktree`), a read tool answers as if the snapshot had been built
//! from that tree: the base is main's `.chisel/store`, never written to; the overlay is a copy
//! under `.chisel/overlays/<hash of the worktree path>/` that `update` brings in line with the
//! worktree (only files whose content differs are re-indexed; deleted files drop out). Each call
//! re-checks the worktree by mtime and size, so answers are as current as the files. An overlay
//! is rebuilt when main's snapshot changes, and removed once its worktree is gone.

use super::*;
use crate::hash::sha256_hex;

/// Tools that answer for a worktree. Writes that belong to the project (results, locks, jobs,
/// captured issues) always go to the base snapshot.
pub(crate) const OVERLAY_TOOLS: [&str; 15] = [
    "analyze", "update", "diff_impact", "impact", "suggest_tests", "risk_map", "test_gaps", "triage", "stale_tests", "stats",
    "churn", "ownership", "coupling", "history", "who_reviews",
];

const BASE_FILE: &str = "base.txt";
const WORKTREE_FILE: &str = "worktree.txt";

/// The directory holding `.git` for `dir`, and that `.git` (a directory, or a file pointing at
/// a linked worktree's git dir).
fn find_dotgit(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut cur = fs::canonicalize(dir).ok()?;
    loop {
        let dotgit = cur.join(".git");
        if dotgit.exists() {
            return Some((cur, dotgit));
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// A linked worktree's git dir and common dir, read from its `.git` file.
fn linked_dirs(root: &Path, dotgit: &Path) -> Option<(PathBuf, PathBuf)> {
    let text = fs::read_to_string(dotgit).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = fs::canonicalize(root.join(gitdir)).ok()?;
    let common = fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = fs::canonicalize(gitdir.join(common.trim())).ok()?;
    Some((gitdir, common))
}

/// The git common dir of the checkout holding `dir`, without running git.
fn git_common_dir(dir: &Path) -> Option<PathBuf> {
    let (root, dotgit) = find_dotgit(dir)?;
    if dotgit.is_dir() {
        return fs::canonicalize(dotgit).ok();
    }
    linked_dirs(&root, &dotgit).map(|(_, common)| common)
}

/// When `dir` is inside a linked git worktree: `(main checkout root, worktree root)`. Read from
/// the `.git` file, without spawning git. `None` in a main checkout, a submodule or a bare repo.
pub fn linked_worktree(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let (root, dotgit) = find_dotgit(dir)?;
    if !dotgit.is_file() {
        return None;
    }
    let (_, common) = linked_dirs(&root, &dotgit)?;
    if common.file_name().and_then(|n| n.to_str()) != Some(".git") {
        return None;
    }
    Some((common.parent()?.to_path_buf(), root))
}

/// A path as an agent would type it (no `\\?\` prefix on Windows).
pub(crate) fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

fn stamp(path: &Path) -> String {
    let Ok(meta) = fs::metadata(path) else { return String::new() };
    let nanos = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}:{nanos}", meta.len())
}

fn error(message: String) -> Value {
    Value::object(vec![
        ("status".into(), Value::String("error".into())),
        ("message".into(), Value::String(message)),
    ])
}

/// Overlay directories whose worktree no longer exists are removed.
fn prune_overlays(base_dir: &Path) {
    let Ok(entries) = fs::read_dir(base_dir.join("overlays")) else { return };
    for entry in entries.flatten() {
        let dir = entry.path();
        let gone = match fs::read_to_string(dir.join(WORKTREE_FILE)) {
            Ok(path) => !Path::new(path.trim()).is_dir(),
            Err(_) => true,
        };
        if gone {
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

/// `args` without `worktree_root`.
fn without_worktree(args: &Value) -> Value {
    match args.as_object() {
        Some(pairs) => Value::Object(pairs.iter().filter(|(k, _)| &**k != "worktree_root").cloned().collect()),
        None => args.clone(),
    }
}

impl Engine {
    /// The worktree a call answers for: `worktree_root`, else the one this process started in.
    /// `Err` is a one-line actionable error reply.
    pub(super) fn overlay_target(&self, name: &str, args: &Value) -> Result<Option<PathBuf>, Value> {
        if !OVERLAY_TOOLS.contains(&name) || self.shard_view.is_some() {
            return Ok(None);
        }
        let given = args.get("worktree_root").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|s| PathBuf::from(s.trim()));
        let Some(raw) = given.or_else(|| self.default_worktree.clone()) else { return Ok(None) };
        let project = display_path(&self.project_dir);
        if !raw.is_absolute() {
            return Err(error(format!("worktree_root must be an absolute path, got '{}'. Pass the full path of your git worktree.", raw.display())));
        }
        let wt = match fs::canonicalize(&raw) {
            Ok(p) if p.is_dir() => p,
            _ => return Err(error(format!("worktree_root '{}' does not exist. Pass the absolute path of a git worktree of {project}.", raw.display()))),
        };
        if wt == self.project_dir {
            return Ok(None);
        }
        if let Some(base_common) = git_common_dir(&self.project_dir) {
            if git_common_dir(&wt).as_ref() != Some(&base_common) {
                return Err(error(format!("worktree_root '{}' is not a checkout of {project} (its git common dir differs). Pass a worktree made by `git worktree add` from this repository, or omit worktree_root.", display_path(&wt))));
            }
        }
        Ok(Some(wt))
    }

    /// Run `name` on the overlay for `wt`, refreshed first.
    pub(super) fn dispatch_overlay(&mut self, name: &str, args: &Value, wt: PathBuf) -> Result<Value, String> {
        self.store.reload()?;
        if !self.has_data() {
            let mut reply = Self::no_data();
            reply.insert("hint", Value::String(format!("Run analyze on the main checkout ({}) first, without worktree_root: a worktree overlays main's snapshot.", display_path(&self.project_dir))));
            return Ok(reply);
        }
        let base_dir = self.store.dir().to_path_buf();
        prune_overlays(&base_dir);
        let key = sha256_hex(display_path(&wt).as_bytes());
        let dir = base_dir.join("overlays").join(&key[..16]);
        let base_stamp = stamp(&self.store.snapshot_path());
        let recorded = fs::read_to_string(dir.join(BASE_FILE)).unwrap_or_default();
        let fresh = recorded.lines().next() == Some(base_stamp.as_str()) && dir.join(crate::store::STORE_NAME).is_file();
        let mut overlay = match self.overlays.remove(&wt) {
            Some(o) if fresh && name != "analyze" => o,
            _ => {
                fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                fs::write(dir.join(WORKTREE_FILE), display_path(&wt)).map_err(|e| e.to_string())?;
                let mut o = Box::new(Engine::open(&wt, Some(&dir))?);
                if !fresh || name == "analyze" {
                    o.store.doc = self.store.doc.clone();
                    o.store.save()?;
                    let mut base = base_stamp.clone();
                    for row in self.store.rows("file_hashes") {
                        if let (Some(p), Some(h)) = (row.get("file_path").and_then(|v| v.as_str()), row.get("hash").and_then(|v| v.as_str())) {
                            base.push('\n');
                            base.push_str(p);
                            base.push('\t');
                            base.push_str(h);
                        }
                    }
                    fs::write(dir.join(BASE_FILE), base).map_err(|e| e.to_string())?;
                }
                o
            }
        };
        overlay.restamp_unchanged = true;
        overlay.allow_repo_extractor = self.allow_repo_extractor;
        overlay.include_ignored = self.include_ignored;
        overlay.history_stream = self.history_stream;
        overlay.store.reload()?;
        let refreshed = overlay.update();
        let result = match refreshed {
            Ok(summary) if matches!(name, "analyze" | "update") => Ok(summary),
            Ok(_) => overlay.dispatch(name, &without_worktree(args)),
            Err(e) => Err(e),
        };
        let changed = overlay_files(&overlay, &dir);
        self.overlays.insert(wt.clone(), overlay);
        let result = result?;
        self.reply_meta = vec![
            ("worktree".into(), Value::String(display_path(&wt))),
            ("overlay_files".into(), Value::Int(changed as i64)),
        ];
        Ok(self.with_reply_meta(result))
    }
}

/// Files whose content in the worktree differs from main's snapshot: changed, added, deleted.
fn overlay_files(overlay: &Engine, dir: &Path) -> usize {
    let text = fs::read_to_string(dir.join(BASE_FILE)).unwrap_or_default();
    let base: HashMap<&str, &str> = text.lines().skip(1).filter_map(|l| l.split_once('\t')).collect();
    let mut seen = 0usize;
    let mut differ = 0usize;
    for row in overlay.store.rows("file_hashes") {
        let (Some(p), Some(h)) = (row.get("file_path").and_then(|v| v.as_str()), row.get("hash").and_then(|v| v.as_str())) else { continue };
        match base.get(p) {
            Some(b) => {
                seen += 1;
                if *b != h {
                    differ += 1;
                }
            }
            None => differ += 1,
        }
    }
    differ + (base.len() - seen.min(base.len()))
}
