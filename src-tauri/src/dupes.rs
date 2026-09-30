//! Duplicate finder: walk a folder (symlinks not followed, hard links counted once), group regular files by
//! size, then by a hash of their first 64 KiB, then by full SHA-256. Biggest sizes are checked first so the
//! sets that waste the most space show up first. Cancellable; reports sets as they're confirmed.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const HEAD: usize = 64 << 10;

fn hash(p: &Path, limit: Option<usize>) -> Option<[u8; 32]> {
    let f = std::fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut r: Box<dyn Read> = match limit {
        Some(n) => Box::new(f.take(n as u64)),
        None => Box::new(f),
    };
    loop {
        let n = r.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().into())
}

/// Calls `found(size, paths)` for every set of identical files (2+), `scanned(n)` now and then while walking.
pub fn find(root: &Path, min_size: u64, hidden: bool, cancel: &AtomicBool, mut scanned: impl FnMut(u64), mut found: impl FnMut(u64, Vec<PathBuf>)) {
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    let mut inodes = HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    let mut n = 0u64;
    while let Some(d) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if !hidden && e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                if !matches!(p.to_str(), Some("/proc" | "/sys" | "/dev" | "/run")) {
                    stack.push(p);
                }
            } else if ft.is_file() {
                let Ok(m) = e.metadata() else { continue };
                n += 1;
                if n % 2000 == 0 {
                    scanned(n);
                }
                if m.len() >= min_size.max(1) && inodes.insert((m.dev(), m.ino())) {
                    by_size.entry(m.len()).or_default().push(p);
                }
            }
        }
    }
    scanned(n);
    let mut sizes: Vec<_> = by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    sizes.sort_by(|a, b| b.0.cmp(&a.0));
    for (size, paths) in sizes {
        let mut heads: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
        for p in paths {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if let Some(h) = hash(&p, Some(HEAD)) {
                heads.entry(h).or_default().push(p);
            }
        }
        for (_, same_head) in heads.into_iter().filter(|(_, v)| v.len() > 1) {
            if size as usize <= HEAD {
                found(size, same_head); // the head was the whole file
                continue;
            }
            let mut full: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
            for p in same_head {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(h) = hash(&p, None) {
                    full.entry(h).or_default().push(p);
                }
            }
            for (_, set) in full.into_iter().filter(|(_, v)| v.len() > 1) {
                found(size, set);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_identical_files_only() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::create_dir_all(d.join("x/y")).unwrap();
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut big2 = big.clone();
        *big2.last_mut().unwrap() ^= 1; // same size, same first 64 KiB, different end
        std::fs::write(d.join("a.bin"), &big).unwrap();
        std::fs::write(d.join("x/b.bin"), &big).unwrap();
        std::fs::write(d.join("x/y/c.bin"), &big).unwrap();
        std::fs::write(d.join("near.bin"), &big2).unwrap();
        std::fs::write(d.join("s1.txt"), "same").unwrap();
        std::fs::write(d.join("s2.txt"), "same").unwrap();
        std::fs::write(d.join("other.txt"), "diff").unwrap();
        std::fs::write(d.join(".hid.txt"), "same").unwrap();
        std::fs::hard_link(d.join("a.bin"), d.join("hardlink.bin")).unwrap();
        std::os::unix::fs::symlink(d.join("a.bin"), d.join("link.bin")).unwrap();
        let run = |min: u64, hidden: bool| {
            let mut sets = vec![];
            find(d, min, hidden, &AtomicBool::new(false), |_| {}, |size, mut v| {
                v.sort();
                sets.push((size, v.iter().map(|p| p.strip_prefix(d).unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>()));
            });
            sets
        };
        let sets = run(1, false);
        assert_eq!(sets.len(), 2, "{sets:?}");
        assert_eq!(sets[0].0, 200_000, "biggest first");
        assert_eq!(sets[0].1.len(), 3, "hard link counted once, symlink and near-miss left out: {:?}", sets[0].1);
        assert!(!sets[0].1.iter().any(|p| p.contains("near") || p.contains("link.bin") && !p.contains("hard")));
        assert_eq!(sets[1].1, vec!["s1.txt", "s2.txt"]);
        assert_eq!(run(1, true)[1].1.len(), 3, "hidden included on request");
        assert_eq!(run(1000, false).len(), 1, "min size");
    }
}
