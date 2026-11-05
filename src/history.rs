//! One `git log -p` stream in place of a `git log -L` per function and a `git blame` per file.
//!
//! Spawning git costs 70-180 ms on Windows, so a project with 650 functions spent ~12 s in
//! `git log -L`. This module reads the whole code history once and replays git's own
//! line-log and blame rules over it:
//!
//! - `-L:<name>:<file>` finds the first *funcname line* (default diff driver: the line starts
//!   with a letter, `_` or `$`) that contains `<name>`, and the range runs to the next one.
//! - Each commit diff maps the range onto the parent the way `line-log.c` does. A commit whose
//!   diff touches the range is shown, with the `+` and `-` lines `git log -L` would print.
//! - Blame passes unchanged lines to the first parent with an identical blob, otherwise to
//!   each parent in turn, and keeps changed lines.
//!
//! Anything this cannot reproduce (a dirty or untracked file, a diff driver attribute, a
//! binary diff, an ambiguous history) is reported as `None` and the caller spawns git.

use crate::git::BlameBlock;
use std::collections::{BTreeMap, HashMap};

/// Record separator that starts each commit header in the stream.
pub const MARK: char = '\u{1e}';

/// Header: hash, tree, parents, author, email, mailmapped author, mailmapped email, date, subject.
pub const FORMAT: &str = "--format=%x1e%H|%T|%P|%an|%ae|%aN|%aE|%aI|%s";

/// Extensions of `extract::detect_language`, as case-insensitive pathspecs.
pub const CODE_EXTENSIONS: [&str; 25] = [
    "py", "pyw", "js", "jsx", "mjs", "cjs", "ts", "tsx", "go", "rs", "cs", "java", "c", "h", "cc", "cpp", "cxx", "hpp", "hxx", "kt", "kts",
    "swift", "php", "rb", "dart",
];

pub fn pathspecs() -> Vec<String> {
    CODE_EXTENSIONS.iter().map(|e| format!(":(glob,icase)**/*.{e}")).collect()
}

/// Arguments for the stream over `pathspecs`. Every option that config could change is pinned.
/// `indent_heuristic` is off for line-log (which diffs with plain xdiff) and on for blame.
pub fn stream_args(indent_heuristic: bool, pathspecs: &[String]) -> Vec<String> {
    let mut args: Vec<String> = [
        "-c", "core.quotePath=false", "log", "-p", "-m", "-U0", "--root", "--full-history", "--sparse", "--diff-algorithm=myers",
        if indent_heuristic { "--indent-heuristic" } else { "--no-indent-heuristic" },
        "--inter-hunk-context=0", "--no-color", "--no-ext-diff", "--no-textconv", "--no-relative", "--src-prefix=a/", "--dst-prefix=b/",
        "--no-show-signature", FORMAT, "HEAD", "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(pathspecs.iter().cloned());
    args
}

/// For merges whose printed sections cannot be matched to parents: the first-parent diff names.
pub fn first_parent_args(hashes: &[String], pathspecs: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["-c", "core.quotePath=false", "log", "--no-walk=unsorted", "--full-history", "--sparse", "--diff-merges=first-parent", "--name-only", "--format=%x1e%H"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.extend(hashes.iter().cloned());
    args.push("--".into());
    args.extend(pathspecs.iter().cloned());
    args
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hunk {
    /// 0-based start in the parent; for an empty side, the line the change sits before.
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
}

impl Hunk {
    fn new_end(&self) -> usize {
        self.new_start + self.new_len
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PatchCommit {
    pub hash: String,
    pub tree: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub mapped_author: String,
    pub mapped_email: String,
    pub date: String,
    pub subject: String,
    /// One per printed parent diff, in parent order. Parents with an empty diff are not printed.
    pub sections: Vec<Vec<FileDiff>>,
}

/// Undo git's C-style quoting of a path (`"a\303\251\tb"`).
pub fn unquote(path: &str) -> String {
    let Some(inner) = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else { return path.to_string() };
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let c = bytes[i + 1];
        i += 2;
        match c {
            b'a' => out.push(7),
            b'b' => out.push(8),
            b't' => out.push(b'\t'),
            b'n' => out.push(b'\n'),
            b'v' => out.push(11),
            b'f' => out.push(12),
            b'r' => out.push(b'\r'),
            b'0'..=b'7' => {
                let mut v = (c - b'0') as u32;
                let mut k = 0;
                while k < 2 && i < bytes.len() && (b'0'..=b'7').contains(&bytes[i]) {
                    v = v * 8 + (bytes[i] - b'0') as u32;
                    i += 1;
                    k += 1;
                }
                out.push(v as u8);
            }
            other => out.push(other),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn side_path(rest: &str, prefix: &str) -> Option<String> {
    let rest = rest.trim_end_matches('\t');
    if rest == "/dev/null" {
        return None;
    }
    let unq = unquote(rest);
    Some(unq.strip_prefix(prefix).unwrap_or(&unq).to_string())
}

/// `-a,b` or `-a` from a hunk header, as a 0-based start and a length.
fn hunk_side(text: &str) -> Option<(usize, usize)> {
    let (start, len) = match text.split_once(',') {
        Some((s, l)) => (s.parse::<usize>().ok()?, l.parse::<usize>().ok()?),
        None => (text.parse::<usize>().ok()?, 1),
    };
    Some((if len == 0 { start } else { start.saturating_sub(1) }, len))
}

pub fn parse_stream(raw: &str) -> Vec<PatchCommit> {
    let mut commits: Vec<PatchCommit> = Vec::new();
    let mut file: Option<FileDiff> = None;
    // Content lines still owed by the current hunk: (removed, added).
    let mut owed = (0usize, 0usize);
    let flush = |commits: &mut Vec<PatchCommit>, file: &mut Option<FileDiff>| {
        if let (Some(f), Some(c)) = (file.take(), commits.last_mut()) {
            if let Some(section) = c.sections.last_mut() {
                section.push(f);
            }
        }
    };
    for line in raw.split('\n') {
        if let Some(header) = line.strip_prefix(MARK) {
            owed = (0, 0);
            flush(&mut commits, &mut file);
            let parts: Vec<&str> = header.splitn(9, '|').collect();
            if parts.len() < 9 {
                continue;
            }
            if commits.last().is_some_and(|c| c.hash == parts[0]) {
                commits.last_mut().unwrap().sections.push(Vec::new());
                continue;
            }
            commits.push(PatchCommit {
                hash: parts[0].into(),
                tree: parts[1].into(),
                parents: parts[2].split_whitespace().map(String::from).collect(),
                author: parts[3].into(),
                email: parts[4].into(),
                mapped_author: parts[5].into(),
                mapped_email: parts[6].into(),
                date: parts[7].into(),
                subject: parts[8].trim_end_matches('\r').into(),
                sections: vec![Vec::new()],
            });
            continue;
        }
        if line.starts_with("diff --git ") {
            owed = (0, 0);
            flush(&mut commits, &mut file);
            file = Some(FileDiff::default());
            continue;
        }
        let Some(f) = file.as_mut() else { continue };
        if owed.0 > 0 && line.starts_with('-') {
            owed.0 -= 1;
            continue;
        }
        if owed.1 > 0 && line.starts_with('+') {
            owed.1 -= 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ -") {
            let mut sides = rest.split(' ');
            let old = sides.next().and_then(hunk_side);
            let new = sides.next().and_then(|s| s.strip_prefix('+')).and_then(hunk_side);
            if let (Some((os, ol)), Some((ns, nl))) = (old, new) {
                f.hunks.push(Hunk { old_start: os, old_len: ol, new_start: ns, new_len: nl });
                owed = (ol, nl);
            }
        } else if let Some(rest) = line.strip_prefix("--- ") {
            f.old_path = side_path(rest, "a/");
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            f.new_path = side_path(rest, "b/");
        } else if let Some(rest) = line.strip_prefix("rename from ") {
            f.old_path = Some(unquote(rest));
        } else if let Some(rest) = line.strip_prefix("rename to ") {
            f.new_path = Some(unquote(rest));
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            f.binary = true;
        }
    }
    flush(&mut commits, &mut file);
    commits
}

/// Half-open line range, 0-based.
type Range = (usize, usize);

fn overlaps(a: Range, b: Range) -> bool {
    !(a.1 <= b.0 || b.1 <= a.0)
}

/// Sort, drop empty ranges, merge overlapping and adjacent ones (`sort_and_merge_range_set`).
fn normalize(mut rs: Vec<Range>) -> Vec<Range> {
    rs.sort();
    let mut out: Vec<Range> = Vec::with_capacity(rs.len());
    for r in rs {
        if r.0 == r.1 {
            continue;
        }
        match out.last_mut() {
            Some(last) if r.0 <= last.1 => last.1 = last.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

/// `a - b`; both sorted.
fn difference(a: &[Range], b: &[Range]) -> Vec<Range> {
    let mut out = Vec::new();
    let mut j = 0;
    for &(mut start, end) in a {
        while start < end {
            while j < b.len() && start >= b[j].1 {
                j += 1;
            }
            if j >= b.len() || end <= b[j].0 {
                out.push((start, end));
                break;
            }
            if start < b[j].0 {
                out.push((start, b[j].0));
            }
            start = b[j].1;
        }
    }
    out
}

/// Map a post-image range set onto the parent (`range_set_map_across_diff`).
/// Returns the parent ranges and the hunks that touched the set.
fn map_across<'a>(rs: &[Range], hunks: &'a [Hunk]) -> (Vec<Range>, Vec<&'a Hunk>) {
    let touched: Vec<&Hunk> = hunks.iter().filter(|h| rs.iter().any(|r| overlaps((h.new_start, h.new_end()), *r))).collect();
    let targets: Vec<Range> = touched.iter().map(|h| (h.new_start, h.new_end())).collect();
    let rest = difference(rs, &targets);
    let mut shifted = Vec::with_capacity(rest.len() + touched.len());
    let mut j = 0;
    let mut offset: i64 = 0;
    for r in rest {
        while j < hunks.len() && r.0 >= hunks[j].new_start {
            offset += hunks[j].old_len as i64 - hunks[j].new_len as i64;
            j += 1;
        }
        shifted.push(((r.0 as i64 + offset) as usize, (r.1 as i64 + offset) as usize));
    }
    shifted.extend(touched.iter().map(|h| (h.old_start, h.old_start + h.old_len)));
    (normalize(shifted), touched)
}

/// Split as git counts lines: on `\n`, with a final line that may lack one.
pub fn split_lines(content: &str) -> Vec<String> {
    let mut lines: Vec<String> = content.split('\n').map(String::from).collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// The range `git log -L:<name>:<file>` traces under the default diff driver, or `None`
/// when git would stop with "no match" (or when `name` is not a plain identifier).
pub fn funcname_range(lines: &[String], name: &str) -> Option<Range> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let is_funcname = |line: &str| line.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'$');
    let begin = lines.iter().position(|l| l.contains(name) && is_funcname(l))?;
    let end = (begin + 1..lines.len()).find(|&i| is_funcname(&lines[i])).unwrap_or(lines.len());
    Some((begin, end))
}

/// Section of `commit` that diffs it against parent `i`, or `None` when that diff is empty.
pub struct Sections<'a> {
    slots: Vec<Option<&'a [FileDiff]>>,
}

impl<'a> Sections<'a> {
    fn find(&self, i: usize, path: &str) -> Option<&'a FileDiff> {
        self.slots.get(i).copied().flatten()?.iter().find(|f| f.new_path.as_deref() == Some(path))
    }
}

pub struct History {
    pub commits: Vec<PatchCommit>,
    index: HashMap<String, usize>,
    /// Merge hash -> which parents have a non-empty diff, when the stream alone is ambiguous.
    nonempty: HashMap<String, Vec<bool>>,
}

/// A commit `git log -L` shows for a range, with the `+` and `-` lines it prints.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub commit: usize,
    pub insertions: i64,
    pub deletions: i64,
}

struct Pending {
    path: String,
    sets: Vec<Vec<Range>>,
}

impl History {
    pub fn new(commits: Vec<PatchCommit>) -> History {
        let index = commits.iter().enumerate().map(|(i, c)| (c.hash.clone(), i)).collect();
        History { commits, index, nonempty: HashMap::new() }
    }

    /// Merges with fewer printed sections than parents (some parent diffs were empty).
    pub fn ambiguous_merges(&self) -> Vec<String> {
        self.commits.iter().filter(|c| c.parents.len() > 1 && !c.sections.iter().all(|s| s.is_empty()) && c.sections.len() < c.parents.len()).map(|c| c.hash.clone()).collect()
    }

    /// Feed the `first_parent_args` output. Two-parent merges become unambiguous.
    pub fn resolve_first_parents(&mut self, raw: &str) {
        let mut current: Option<(String, bool)> = None;
        let mut done = Vec::new();
        for line in raw.split('\n') {
            if let Some(hash) = line.strip_prefix(MARK) {
                if let Some(c) = current.take() {
                    done.push(c);
                }
                current = Some((hash.trim().to_string(), false));
            } else if !line.trim().is_empty() {
                if let Some(c) = current.as_mut() {
                    c.1 = true;
                }
            }
        }
        done.extend(current);
        for (hash, first_nonempty) in done {
            let Some(&i) = self.index.get(&hash) else { continue };
            let c = &self.commits[i];
            if c.parents.len() != 2 {
                continue;
            }
            let flags = if c.sections.len() == 1 { vec![first_nonempty, !first_nonempty] } else { vec![true, true] };
            self.nonempty.insert(hash, flags);
        }
    }

    /// Sections per parent, or `None` when they cannot be matched to parents.
    fn sections<'a>(&'a self, c: &'a PatchCommit) -> Option<Sections<'a>> {
        if c.parents.len() <= 1 {
            return Some(Sections { slots: vec![Some(c.sections.first().map(|s| s.as_slice()).unwrap_or(&[]))] });
        }
        if c.sections.iter().all(|s| s.is_empty()) {
            return Some(Sections { slots: vec![None; c.parents.len()] });
        }
        if c.sections.len() == c.parents.len() {
            return Some(Sections { slots: c.sections.iter().map(|s| Some(s.as_slice())).collect() });
        }
        let flags = self.nonempty.get(&c.hash)?;
        let mut printed = c.sections.iter();
        let slots = flags.iter().map(|&nonempty| if nonempty { printed.next().map(|s| s.as_slice()) } else { None }).collect();
        Some(Sections { slots })
    }

    /// The commits `git log -L:<name>:<path>` shows for each range of the file at HEAD.
    /// `None` when the history cannot be replayed exactly.
    pub fn line_logs(&self, path: &str, ranges: &[Range]) -> Option<Vec<Vec<Shown>>> {
        let mut out: Vec<Vec<Shown>> = ranges.iter().map(|_| Vec::new()).collect();
        if self.commits.is_empty() {
            return Some(out);
        }
        let mut pending: BTreeMap<usize, Pending> = BTreeMap::new();
        pending.insert(0, Pending { path: path.to_string(), sets: ranges.iter().map(|r| normalize(vec![*r])).collect() });
        while let Some((ci, p)) = pending.pop_first() {
            let c = &self.commits[ci];
            let sections = self.sections(c)?;
            let nparents = c.parents.len();
            let slots = nparents.max(1);
            let diffs: Vec<Option<&FileDiff>> = (0..slots).map(|i| sections.find(i, &p.path)).collect();
            if diffs.iter().flatten().any(|d| d.binary) {
                return None;
            }
            let mut give: Vec<Vec<Vec<Range>>> = (0..slots).map(|_| p.sets.iter().map(|_| Vec::new()).collect()).collect();
            for (f, rs) in p.sets.iter().enumerate() {
                if rs.is_empty() {
                    continue;
                }
                let mapped: Vec<(Vec<Range>, Vec<&Hunk>)> = diffs
                    .iter()
                    .map(|d| match d {
                        Some(d) => {
                            let (cand, touched) = map_across(rs, &d.hunks);
                            (if d.old_path.is_some() { cand } else { Vec::new() }, touched)
                        }
                        None => (rs.clone(), Vec::new()),
                    })
                    .collect();
                if nparents > 1 {
                    // A parent the range did not change in takes it all, and the merge is hidden.
                    if let Some(i) = mapped.iter().position(|m| m.1.is_empty()) {
                        give[i][f] = mapped[i].0.clone();
                        continue;
                    }
                    // Changed against every parent: shown without a diff, every parent followed.
                    out[f].push(Shown { commit: ci, insertions: 0, deletions: 0 });
                    for (i, m) in mapped.into_iter().enumerate() {
                        give[i][f] = m.0;
                    }
                    continue;
                }
                let (cand, touched) = mapped.into_iter().next().unwrap_or_default();
                if !touched.is_empty() {
                    let (insertions, deletions) = printed_changes(rs, &touched);
                    out[f].push(Shown { commit: ci, insertions, deletions });
                }
                give[0][f] = cand;
            }
            for (i, sets) in give.into_iter().enumerate() {
                if i >= nparents || sets.iter().all(|s| s.is_empty()) {
                    continue;
                }
                let Some(&pi) = self.index.get(&c.parents[i]) else { continue };
                let parent_path = match diffs[i] {
                    Some(d) => d.old_path.clone()?,
                    None => p.path.clone(),
                };
                match pending.get_mut(&pi) {
                    Some(existing) => {
                        if existing.path != parent_path {
                            return None;
                        }
                        for (slot, add) in existing.sets.iter_mut().zip(sets) {
                            let mut merged = std::mem::take(slot);
                            merged.extend(add);
                            *slot = normalize(merged);
                        }
                    }
                    None => {
                        pending.insert(pi, Pending { path: parent_path, sets });
                    }
                }
            }
        }
        Some(out)
    }

    /// What `git blame --porcelain` reports for a file whose worktree equals HEAD, as blocks.
    /// `n_lines` is the line count at HEAD.
    pub fn blame(&self, path: &str, n_lines: usize) -> Option<Vec<BlameBlock>> {
        let mut owner: Vec<Option<usize>> = vec![None; n_lines];
        if self.commits.is_empty() {
            return None;
        }
        // Per pending commit: its path and (line in that commit, final line) pairs.
        let mut pending: BTreeMap<usize, (String, Vec<(usize, usize)>)> = BTreeMap::new();
        pending.insert(0, (path.to_string(), (0..n_lines).map(|l| (l, l)).collect()));
        while let Some((ci, (cpath, mut suspects))) = pending.pop_first() {
            let c = &self.commits[ci];
            let sections = self.sections(c)?;
            let nparents = c.parents.len();
            let diffs: Vec<Option<&FileDiff>> = (0..nparents.max(1)).map(|i| sections.find(i, &cpath)).collect();
            if diffs.iter().flatten().any(|d| d.binary) {
                return None;
            }
            let pass = |pi_slot: usize, lines: Vec<(usize, usize)>, ppath: String, pending: &mut BTreeMap<usize, (String, Vec<(usize, usize)>)>| -> Option<()> {
                let &pi = self.index.get(&c.parents[pi_slot])?;
                let entry = pending.entry(pi).or_insert_with(|| (ppath.clone(), Vec::new()));
                if entry.0 != ppath {
                    return None;
                }
                entry.1.extend(lines);
                Some(())
            };
            if nparents > 0 {
                if let Some(i) = diffs.iter().position(|d| d.is_none()) {
                    if self.index.contains_key(&c.parents[i]) {
                        pass(i, suspects, cpath.clone(), &mut pending)?;
                        continue;
                    }
                }
                for (i, d) in diffs.iter().enumerate() {
                    let Some(d) = d else { continue };
                    let Some(ppath) = d.old_path.clone() else { continue };
                    if !self.index.contains_key(&c.parents[i]) {
                        continue;
                    }
                    let mut keep = Vec::new();
                    let mut moved = Vec::new();
                    for (line, fin) in suspects {
                        match map_line(line, &d.hunks) {
                            Some(pl) => moved.push((pl, fin)),
                            None => keep.push((line, fin)),
                        }
                    }
                    if !moved.is_empty() {
                        pass(i, moved, ppath, &mut pending)?;
                    }
                    suspects = keep;
                    if suspects.is_empty() {
                        break;
                    }
                }
            }
            for (_, fin) in suspects {
                if let Some(slot) = owner.get_mut(fin) {
                    *slot = Some(ci);
                }
            }
        }
        let mut blocks: Vec<BlameBlock> = Vec::new();
        for (i, o) in owner.into_iter().enumerate() {
            let c = &self.commits[o?];
            let line_no = i + 1;
            if let Some(last) = blocks.last_mut() {
                if last.commit_hash == c.hash && last.line_end + 1 == line_no {
                    last.line_end = line_no;
                    continue;
                }
            }
            blocks.push(BlameBlock { commit_hash: c.hash.clone(), author: c.mapped_author.clone(), author_email: c.mapped_email.clone(), line_start: line_no, line_end: line_no });
        }
        Some(blocks)
    }
}

/// Where `line` of the child sits in the parent, or `None` when a hunk changed it.
fn map_line(line: usize, hunks: &[Hunk]) -> Option<usize> {
    let mut offset: i64 = 0;
    for h in hunks {
        if h.new_len > 0 && line >= h.new_start && line < h.new_end() {
            return None;
        }
        if h.new_end() <= line && (h.new_len > 0 || h.new_start <= line) {
            offset += h.old_len as i64 - h.new_len as i64;
        } else {
            break;
        }
    }
    Some((line as i64 + offset) as usize)
}

/// The `+` and `-` lines `git log -L` prints for a shown commit (`dump_diff_hacky_one`):
/// every removed line of a touched hunk, and its added lines that fall inside the ranges.
fn printed_changes(ranges: &[Range], touched: &[&Hunk]) -> (i64, i64) {
    let (mut ins, mut del) = (0i64, 0i64);
    let mut j = 0;
    for &(t_start, t_end) in ranges {
        while j < touched.len() && touched[j].new_end() < t_start {
            j += 1;
        }
        if j == touched.len() || touched[j].new_start > t_end {
            continue;
        }
        let mut t_cur = t_start;
        while j < touched.len() && touched[j].new_start < t_end {
            let h = touched[j];
            t_cur = t_cur.max(h.new_start);
            del += h.old_len as i64;
            let stop = h.new_end().min(t_end);
            if t_cur < stop {
                ins += (stop - t_cur) as i64;
                t_cur = stop;
            }
            j += 1;
        }
    }
    (ins, del)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        split_lines(text)
    }

    #[test]
    fn funcname_range_follows_the_default_driver() {
        let src = lines("use x;\nfn alpha() {\n    beta();\n}\n\nfn beta() {\n}\n    fn gamma() {}\n");
        assert_eq!(funcname_range(&src, "alpha"), Some((1, 5)));
        assert_eq!(funcname_range(&src, "beta"), Some((5, 8)));
        assert_eq!(funcname_range(&src, "gamma"), None, "indented lines are not funcname lines");
        assert_eq!(funcname_range(&src, "a.b"), None);
    }

    #[test]
    fn range_sets_match_line_log() {
        assert_eq!(normalize(vec![(5, 7), (0, 2), (2, 3), (4, 4)]), vec![(0, 3), (5, 7)]);
        assert_eq!(normalize(difference(&[(0, 10)], &[(2, 4), (6, 6), (8, 12)])), vec![(0, 2), (4, 8)]);
        let hunk = |os, ol, ns, nl| Hunk { old_start: os, old_len: ol, new_start: ns, new_len: nl };
        // Two lines inserted above the range shift it up by two in the parent; not touched.
        let above = [hunk(1, 0, 1, 2)];
        let (parent, touched) = map_across(&[(5, 8)], &above);
        assert_eq!((parent, touched.len()), (vec![(3, 6)], 0));
        // A replaced line inside the range: touched, and the parent side joins the range.
        let inside = [hunk(5, 3, 5, 1)];
        let (parent, touched) = map_across(&[(4, 8)], &inside);
        assert_eq!((parent, touched.len()), (vec![(4, 10)], 1));
        // A pure deletion at the range start does not touch it; strictly inside, it does.
        let at_start = [hunk(4, 2, 5, 0)];
        let within = [hunk(6, 2, 6, 0)];
        assert_eq!(map_across(&[(5, 8)], &at_start).1.len(), 0);
        assert_eq!(map_across(&[(5, 8)], &within).1.len(), 1);
    }

    #[test]
    fn stream_parses_merges_renames_and_quoted_paths() {
        let raw = "\u{1e}c2|t2|p1 p2|A|a@x|A|a@x|2024-01-02T00:00:00Z|merge\n\ndiff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -3 +3,2 @@ fn f\n-old\n+new\n+more\n\u{1e}c2|t2|p1 p2|A|a@x|A|a@x|2024-01-02T00:00:00Z|merge\n\ndiff --git a/o.rs b/n.rs\nsimilarity index 100%\nrename from o.rs\nrename to n.rs\ndiff --git \"a/\\303\\251.rs\" \"b/\\303\\251.rs\"\nnew file mode 100644\n--- /dev/null\n+++ \"b/\\303\\251.rs\"\n@@ -0,0 +1 @@\n+fn e() {}\n";
        let commits = parse_stream(raw);
        assert_eq!(commits.len(), 1);
        let c = &commits[0];
        assert_eq!((c.parents.len(), c.sections.len()), (2, 2));
        assert_eq!(c.sections[0][0].hunks, vec![Hunk { old_start: 2, old_len: 1, new_start: 2, new_len: 2 }]);
        assert_eq!((c.sections[1][0].old_path.as_deref(), c.sections[1][0].new_path.as_deref()), (Some("o.rs"), Some("n.rs")));
        assert_eq!((c.sections[1][1].old_path.as_deref(), c.sections[1][1].new_path.as_deref()), (None, Some("\u{e9}.rs")));
        assert_eq!(c.sections[1][1].hunks[0].new_start, 0);
    }

    #[test]
    fn printed_changes_clip_additions_but_not_deletions() {
        let edge = Hunk { old_start: 2, old_len: 3, new_start: 2, new_len: 4 };
        let inner = Hunk { old_start: 8, old_len: 1, new_start: 9, new_len: 0 };
        // Range 4..10 holds two of the edge hunk's four added lines; both deletions print.
        assert_eq!(printed_changes(&[(4, 10)], &[&edge, &inner]), (2, 4));
    }

    #[test]
    fn map_line_skips_changed_lines_and_shifts_the_rest() {
        let ins = Hunk { old_start: 1, old_len: 0, new_start: 1, new_len: 2 };
        let del = Hunk { old_start: 5, old_len: 3, new_start: 5, new_len: 0 };
        let hunks = [ins, del];
        assert_eq!(map_line(0, &hunks), Some(0));
        assert_eq!(map_line(1, &hunks), None);
        assert_eq!(map_line(3, &hunks), Some(1));
        assert_eq!(map_line(5, &hunks), Some(6));
    }
}
