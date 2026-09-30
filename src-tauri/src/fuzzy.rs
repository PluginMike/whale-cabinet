//! Quick open (Ctrl+P): everything under home listed by `fd` (hidden and git-ignored files skipped, like fd does
//! by default), ranked with nucleo — the fzf-grade matcher from Helix — on the path relative to home, spread
//! over a few threads. zoxide's frequent folders get a small bonus. fd lists ~170k entries in well under a
//! second, so the index is simply rebuilt in the background when it's more than a few seconds old.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Index {
    pub root: PathBuf,
    /// Paths relative to `root`; folders end with '/'.
    pub items: Vec<String>,
    /// zoxide frecency of folders (absolute path → score), for the bonus.
    pub frecent: HashMap<String, f64>,
    pub built: Instant,
}

#[derive(Default)]
pub struct Fuzzy {
    index: Mutex<Option<Arc<Index>>>,
    building: AtomicBool,
}

pub const STALE: Duration = Duration::from_secs(20);

pub fn fd_bin() -> Option<&'static str> {
    // Debian/Ubuntu ship it as fdfind
    ["fd", "fdfind"].into_iter().find(|b| crate::desktop::which(b).is_some())
}

pub fn build(root: &Path) -> Result<Index, String> {
    let fd = fd_bin().ok_or("fd isn't installed (needed for quick open)")?;
    let out = Command::new(fd)
        .args(["--color=never", "--type=f", "--type=d", "--type=l", "--base-directory"])
        .arg(root)
        .arg(".")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("{fd}: {e}"))?;
    let items = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.strip_prefix("./").unwrap_or(l).to_owned()).collect();
    let frecent = crate::zoxide::query(&[], 300).unwrap_or_default().into_iter().map(|(s, p)| (p, s)).collect();
    Ok(Index { root: root.to_path_buf(), items, frecent, built: Instant::now() })
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Hit {
    pub path: String,
    /// Path relative to the index root, as matched (folders end with '/').
    pub rel: String,
    pub dir: bool,
    pub score: u32,
    /// Matched character positions in `rel`.
    pub idx: Vec<u32>,
}

fn bonus(ix: &Index, rel: &str) -> u32 {
    if !rel.ends_with('/') {
        return 0;
    }
    let abs = ix.root.join(rel.trim_end_matches('/'));
    ix.frecent.get(abs.to_str().unwrap_or("")).map_or(0, |s| ((1.0 + s).ln() * 10.0) as u32)
}

/// Best `limit` matches for `q`, best first (ties: shorter path first).
pub fn rank(ix: &Index, q: &str, limit: usize) -> Vec<Hit> {
    let pat = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let chunk = ix.items.len() / threads + 1;
    let mut best: Vec<(u32, usize)> = std::thread::scope(|s| {
        let hs: Vec<_> = ix
            .items
            .chunks(chunk)
            .enumerate()
            .map(|(ci, part)| {
                let pat = &pat;
                s.spawn(move || {
                    let mut m = Matcher::new(Config::DEFAULT.match_paths());
                    let mut buf = Vec::new();
                    let mut top: Vec<(u32, usize)> = vec![];
                    for (i, it) in part.iter().enumerate() {
                        if let Some(sc) = pat.score(Utf32Str::new(it, &mut buf), &mut m) {
                            top.push((sc + bonus(ix, it), ci * chunk + i));
                        }
                    }
                    top.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(ix.items[a.1].len().cmp(&ix.items[b.1].len())));
                    top.truncate(limit);
                    top
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    best.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(ix.items[a.1].len().cmp(&ix.items[b.1].len())));
    best.truncate(limit);
    let mut m = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();
    best.into_iter()
        .map(|(score, i)| {
            let rel = &ix.items[i];
            let mut idx = vec![];
            pat.indices(Utf32Str::new(rel, &mut buf), &mut m, &mut idx);
            idx.sort_unstable();
            idx.dedup();
            Hit { path: ix.root.join(rel.trim_end_matches('/')).to_string_lossy().into_owned(), rel: rel.clone(), dir: rel.ends_with('/'), score, idx }
        })
        .collect()
}

#[derive(Serialize)]
pub struct Found {
    pub items: Vec<Hit>,
    pub total: usize,
    pub indexing: bool,
}

impl Fuzzy {
    /// Rebuild in the background when missing or stale; `done` runs after a rebuild so the UI can re-query.
    fn refresh(self: &Arc<Self>, root: PathBuf, done: impl FnOnce() + Send + 'static) {
        if self.building.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            if let Ok(ix) = build(&root) {
                *me.index.lock().unwrap() = Some(Arc::new(ix));
            }
            me.building.store(false, Ordering::SeqCst);
            done();
        });
    }

    pub fn find(self: &Arc<Self>, root: PathBuf, q: &str, limit: usize, done: impl FnOnce() + Send + 'static) -> Found {
        let cur = self.index.lock().unwrap().clone();
        if cur.as_ref().is_none_or(|ix| ix.built.elapsed() > STALE || ix.root != root) {
            self.refresh(root, done);
        }
        match cur {
            Some(ix) => Found { items: rank(&ix, q, limit), total: ix.items.len(), indexing: self.building.load(Ordering::SeqCst) },
            None => Found { items: vec![], total: 0, indexing: true },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(items: &[&str]) -> Index {
        Index { root: "/h".into(), items: items.iter().map(|s| s.to_string()).collect(), frecent: HashMap::from([("/h/work/deep".to_string(), 50.0)]), built: Instant::now() }
    }

    #[test]
    fn ranks_fuzzy_paths_with_indices() {
        let x = ix(&["Downloads/report-final.pdf", "Documents/notes.md", "Downloads/", "src/lib/reportage.rs", "work/deep/", "work/other/"]);
        let h = rank(&x, "dl rep", 10);
        assert_eq!(h[0].rel, "Downloads/report-final.pdf");
        assert_eq!(h[0].path, "/h/Downloads/report-final.pdf");
        assert!(!h[0].dir);
        // "d" and "l" of Downloads, then "rep" of report
        assert!(h[0].idx.contains(&0) && h[0].idx.len() == 5, "{:?}", h[0].idx);
        assert!(rank(&x, "notes", 10).iter().all(|h| h.rel == "Documents/notes.md"));
        // frecent folder wins a tie
        let w = rank(&x, "work", 10);
        assert_eq!(w[0].rel, "work/deep/");
        assert!(w[0].dir);
        assert!(rank(&x, "zzzq", 10).is_empty());
        assert_eq!(rank(&x, "", 3).len(), 3, "empty query matches everything");
    }

    #[test]
    fn many_items_across_threads() {
        let items: Vec<String> = (0..50_000).map(|i| format!("d{}/file {i}.txt", i % 97)).collect();
        let x = Index { root: "/h".into(), items, frecent: HashMap::new(), built: Instant::now() };
        let h = rank(&x, "file 4242.txt", 5);
        assert_eq!(h[0].rel, format!("d{}/file 4242.txt", 4242 % 97));
    }
}
