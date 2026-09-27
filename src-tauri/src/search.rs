//! Recursive search (Ctrl+F): by name (case-insensitive substring, or a glob with * ? [..]) and optionally
//! by content. Results stream to the UI in batches; searches can be cancelled.

use crate::listing::{self, Entry};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

#[derive(Default)]
pub struct Searches(Mutex<HashMap<u32, Arc<AtomicBool>>>);

pub struct Query {
    pub text: String,
    pub glob: bool,
    pub content: bool,
    pub hidden: bool,
}

impl Query {
    pub fn new(text: &str, content: bool, hidden: bool) -> Self {
        let glob = !content && text.contains(['*', '?', '[']);
        Query { text: text.to_lowercase(), glob, content, hidden }
    }
    pub fn name_matches(&self, name: &str) -> bool {
        let n = name.to_lowercase();
        if self.glob {
            crate::desktop::glob_match(&self.text, &n)
        } else {
            n.contains(&self.text)
        }
    }
}

const MAX_CONTENT: u64 = 20 << 20;

/// Does the file contain the (lowercased) needle? Binary files (NUL in the first 8 KiB) never match.
pub fn content_matches(p: &Path, needle: &str) -> bool {
    let Ok(f) = std::fs::File::open(p) else { return false };
    let mut buf = Vec::new();
    if f.take(MAX_CONTENT).read_to_end(&mut buf).is_err() || buf.iter().take(8192).any(|&b| b == 0) {
        return false;
    }
    String::from_utf8_lossy(&buf).to_lowercase().contains(needle)
}

/// Walk `root`, calling `hit` for every match. Symlinked folders aren't followed; /proc, /sys, /dev are skipped.
pub fn walk(root: &Path, q: &Query, cancel: &AtomicBool, mut hit: impl FnMut(Entry)) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !q.hidden && name.starts_with('.') {
                continue;
            }
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() && !matches!(p.to_str(), Some("/proc" | "/sys" | "/dev" | "/run")) {
                stack.push(p.clone());
            }
            let ok = if q.content { ft.is_file() && content_matches(&p, &q.text) } else { q.name_matches(&name) };
            if ok {
                if let Some(en) = listing::entry(&p) {
                    hit(en);
                }
            }
        }
    }
}

#[derive(Serialize, Clone)]
struct Hits {
    id: u32,
    entries: Vec<Entry>,
    done: bool,
}

#[tauri::command]
pub fn search_start(id: u32, root: String, text: String, content: bool, hidden: bool, app: AppHandle, s: State<Searches>) {
    let cancel = Arc::new(AtomicBool::new(false));
    s.0.lock().unwrap().insert(id, cancel.clone());
    std::thread::spawn(move || {
        let q = Query::new(&text, content, hidden);
        let mut batch = vec![];
        let mut last = Instant::now();
        walk(Path::new(&root), &q, &cancel, |e| {
            batch.push(e);
            if batch.len() >= 500 || last.elapsed() > Duration::from_millis(150) {
                let _ = app.emit("search-hits", Hits { id, entries: std::mem::take(&mut batch), done: false });
                last = Instant::now();
            }
        });
        let _ = app.emit("search-hits", Hits { id, entries: batch, done: true });
    });
}

#[tauri::command]
pub fn search_cancel(id: u32, s: State<Searches>) {
    if let Some(c) = s.0.lock().unwrap().remove(&id) {
        c.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_glob_content_and_hidden() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::create_dir_all(d.join("a/deep/er")).unwrap();
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join("a/deep/er/Report 2026.PDF"), "x").unwrap();
        std::fs::write(d.join("a/notes.md"), "Remember the WHALE").unwrap();
        std::fs::write(d.join("a/bin.dat"), [0u8, b'w', b'h', b'a', b'l', b'e']).unwrap();
        std::fs::write(d.join(".git/report-hidden"), "").unwrap();
        std::os::unix::fs::symlink(d, d.join("a/loop")).unwrap();
        let run = |q: Query| {
            let mut v = vec![];
            walk(d, &q, &AtomicBool::new(false), |e| v.push(e.name));
            v.sort();
            v
        };
        assert_eq!(run(Query::new("report", false, false)), vec!["Report 2026.PDF"]);
        assert_eq!(run(Query::new("report", false, true)), vec!["Report 2026.PDF", "report-hidden"]);
        assert_eq!(run(Query::new("*.pdf", false, false)), vec!["Report 2026.PDF"]);
        assert_eq!(run(Query::new("whale", true, false)), vec!["notes.md"], "binary files never match content");
        let cancelled = AtomicBool::new(true);
        let mut n = 0;
        walk(d, &Query::new("", false, true), &cancelled, |_| n += 1);
        assert_eq!(n, 0);
    }
}
