//! Disk usage (like Filelight): one walk of a folder counting allocated bytes (st_blocks, like du), staying on
//! its filesystem and not following symlinks; hard links counted once. The whole tree stays in memory so the UI
//! can drill in without rescanning; it gets a pruned view (a few levels, the biggest children, the rest merged).

use serde::Serialize;
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Tree {
    pub name: String,
    pub size: u64,
    pub files: u64,
    pub dir: bool,
    pub kids: Vec<Tree>,
}

/// Walk `root`; `tick(files, bytes)` is called now and then.
pub fn scan(root: &Path, cancel: &AtomicBool, tick: &mut dyn FnMut(u64, u64)) -> Option<Tree> {
    let dev = std::fs::symlink_metadata(root).ok()?.dev();
    let mut seen = HashSet::new();
    let mut n = (0u64, 0u64);
    let name = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
    walk(root, name, dev, &mut seen, cancel, &mut n, tick)
}

fn walk(p: &Path, name: String, dev: u64, seen: &mut HashSet<(u64, u64)>, cancel: &AtomicBool, n: &mut (u64, u64), tick: &mut dyn FnMut(u64, u64)) -> Option<Tree> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let m = std::fs::symlink_metadata(p).ok()?;
    let own = if m.nlink() > 1 && !m.is_dir() && !seen.insert((m.dev(), m.ino())) { 0 } else { m.blocks() * 512 };
    if !m.is_dir() {
        n.0 += 1;
        n.1 += own;
        if n.0 % 5000 == 0 {
            tick(n.0, n.1);
        }
        return Some(Tree { name, size: own, files: 1, dir: false, kids: vec![] });
    }
    let mut t = Tree { name, size: own, files: 0, dir: true, kids: vec![] };
    if m.dev() != dev {
        return Some(t); // another filesystem mounted here: show it, don't count it
    }
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let child = e.file_name().to_string_lossy().into_owned();
            if let Some(k) = walk(&e.path(), child, dev, seen, cancel, n, tick) {
                t.size += k.size;
                t.files += k.files;
                t.kids.push(k);
            } else if cancel.load(Ordering::Relaxed) {
                return None;
            }
        }
    }
    t.kids.sort_by(|a, b| b.size.cmp(&a.size));
    Some(t)
}

#[derive(Serialize, Debug, PartialEq)]
pub struct View {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub dir: bool,
    /// true for the "N smaller items" bucket
    pub rest: bool,
    pub kids: Vec<View>,
}

impl Tree {
    /// The node at `rel` (path components below the root).
    pub fn find(&self, rel: &[&str]) -> Option<&Tree> {
        rel.iter().try_fold(self, |t, c| t.kids.iter().find(|k| k.name == *c))
    }
    /// A view `depth` levels deep, keeping children that are at least `min_frac` of this node (max `max_kids`).
    pub fn view(&self, path: &Path, depth: u32, min_frac: f64, max_kids: usize) -> View {
        let mut kids = vec![];
        if depth > 0 {
            let floor = (self.size as f64 * min_frac) as u64;
            let (mut big, mut small) = (vec![], vec![]);
            for (i, k) in self.kids.iter().enumerate() {
                if i < max_kids && k.size >= floor.max(1) { big.push(k) } else { small.push(k) }
            }
            kids = big.iter().map(|k| k.view(&path.join(&k.name), depth - 1, min_frac, max_kids)).collect();
            if !small.is_empty() {
                kids.push(View {
                    name: format!("{} smaller item{}", small.len(), if small.len() == 1 { "" } else { "s" }),
                    path: String::new(),
                    size: small.iter().map(|k| k.size).sum(),
                    files: small.iter().map(|k| k.files).sum(),
                    dir: false,
                    rest: true,
                    kids: vec![],
                });
            }
        }
        View { name: self.name.clone(), path: path.to_string_lossy().into_owned(), size: self.size, files: self.files, dir: self.dir, rest: false, kids }
    }
    /// Drop the node at `rel` (it was trashed) and take its size off every ancestor.
    pub fn remove(&mut self, rel: &[&str]) -> Option<(u64, u64)> {
        let (first, rest) = rel.split_first()?;
        let i = self.kids.iter().position(|k| k.name == *first)?;
        let gone = if rest.is_empty() {
            let k = self.kids.remove(i);
            (k.size, k.files)
        } else {
            self.kids[i].remove(rest)?
        };
        self.size -= gone.0.min(self.size);
        self.files -= gone.1.min(self.files);
        self.kids.sort_by(|a, b| b.size.cmp(&a.size));
        Some(gone)
    }
}

/// `path` relative to `root` as components; None if it isn't inside.
pub fn rel<'a>(root: &Path, path: &'a Path) -> Option<Vec<&'a str>> {
    path.strip_prefix(root).ok().map(|r| r.iter().filter_map(|c| c.to_str()).collect())
}

pub type Scans = std::sync::Mutex<std::collections::HashMap<u32, (PathBuf, Tree)>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_views_and_removal() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::create_dir_all(d.join("big/deep")).unwrap();
        let noise = |n: usize| (0..n).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect::<Vec<u8>>();
        std::fs::write(d.join("big/deep/a.bin"), noise(300_000)).unwrap();
        std::fs::write(d.join("big/b.bin"), noise(100_000)).unwrap();
        for i in 0..5 {
            std::fs::write(d.join(format!("s{i}.txt")), "x").unwrap();
        }
        std::fs::hard_link(d.join("big/b.bin"), d.join("hard.bin")).unwrap();
        std::os::unix::fs::symlink(d.join("big"), d.join("link")).unwrap();
        let tree = scan(d, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        let big = tree.find(&["big"]).unwrap();
        // b.bin and hard.bin are one file: whichever is walked first carries its size
        assert!(big.size >= 300_000 && big.files == 2, "{} {}", big.size, big.files);
        assert_eq!(tree.kids[0].name, "big", "biggest first");
        assert!(tree.size < big.size + 150_000, "hard link counted once, symlink not followed");
        let v = tree.view(d, 2, 0.05, 3);
        assert_eq!(v.kids[0].name, "big");
        assert_eq!(v.kids[0].kids[0].name, "deep");
        assert!(v.kids.last().unwrap().rest, "small files merged: {:?}", v.kids.iter().map(|k| &k.name).collect::<Vec<_>>());
        assert_eq!(v.kids.iter().map(|k| k.size).sum::<u64>() + (tree.size - tree.kids.iter().map(|k| k.size).sum::<u64>()), tree.size);
        let mut tree = tree;
        let before = tree.size;
        let (gone, _) = tree.remove(&["big", "deep"]).unwrap();
        assert_eq!(tree.size, before - gone);
        assert!(tree.find(&["big", "deep"]).is_none());
        assert!(scan(d, &AtomicBool::new(true), &mut |_, _| {}).is_none(), "cancel");
        assert_eq!(rel(d, &d.join("big/deep")).unwrap(), vec!["big", "deep"]);
    }
}
