//! Tags and ratings, stored where Dolphin/Baloo keep them: `user.xdg.tags` (comma-separated) and
//! `user.baloo.rating` (0–10) extended attributes. Colours are tags named `color:<preset|#hex>`.
//! Filesystems without xattrs fall back to a JSON sidecar in ~/.config/whale-cabinet/.
//! A small path→tags index (also JSON) backs the sidebar's tag list and "show all tagged items".

use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const TAGS: &str = "user.xdg.tags";
pub const RATING: &str = "user.baloo.rating";

pub struct Store {
    dir: PathBuf,
    /// path → tags, for files whose filesystem refused xattrs
    sidecar: Mutex<Option<HashMap<String, Vec<String>>>>,
    /// path → tags, everything we've seen tagged
    index: Mutex<Option<HashMap<String, Vec<String>>>>,
}

pub fn parse(v: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in String::from_utf8_lossy(v).split(',') {
        let t = t.trim();
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_owned());
        }
    }
    out
}

fn no_xattr(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENOTSUP) | Some(libc::EPERM)) // EOPNOTSUPP == ENOTSUP on Linux
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Store { dir, sidecar: Mutex::new(None), index: Mutex::new(None) }
    }

    fn load(&self, name: &str) -> HashMap<String, Vec<String>> {
        std::fs::read_to_string(self.dir.join(name)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    fn save(&self, name: &str, m: &HashMap<String, Vec<String>>) {
        let _ = std::fs::create_dir_all(&self.dir);
        let tmp = self.dir.join(format!("{name}.tmp"));
        if std::fs::write(&tmp, serde_json::to_string(m).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(tmp, self.dir.join(name));
        }
    }

    pub fn read(&self, p: &Path) -> Vec<String> {
        match xattr::get(p, TAGS) {
            Ok(Some(v)) => parse(&v),
            Ok(None) => vec![],
            Err(e) if no_xattr(&e) => {
                let mut g = self.sidecar.lock().unwrap();
                g.get_or_insert_with(|| self.load("tags.json")).get(&*p.to_string_lossy()).cloned().unwrap_or_default()
            }
            Err(_) => vec![],
        }
    }

    pub fn write(&self, p: &Path, tags: &[String]) -> Result<(), String> {
        let joined = tags.join(",");
        let r = if tags.is_empty() {
            match xattr::remove(p, TAGS) {
                Err(e) if e.raw_os_error() == Some(libc::ENODATA) => Ok(()),
                r => r,
            }
        } else {
            xattr::set(p, TAGS, joined.as_bytes())
        };
        match r {
            Ok(()) => {}
            Err(e) if no_xattr(&e) => {
                let mut g = self.sidecar.lock().unwrap();
                let m = g.get_or_insert_with(|| self.load("tags.json"));
                let k = p.to_string_lossy().into_owned();
                if tags.is_empty() { m.remove(&k) } else { m.insert(k, tags.to_vec()) };
                self.save("tags.json", m);
            }
            Err(e) => return Err(format!("{}: {e}", p.display())),
        }
        self.observe(&[(p.to_string_lossy().into_owned(), tags.to_vec())]);
        Ok(())
    }

    /// Add/remove tags on several items at once; returns each item's new tags.
    pub fn edit(&self, paths: &[String], add: &[String], remove: &[String]) -> Result<Vec<Vec<String>>, String> {
        paths
            .iter()
            .map(|p| {
                let p = Path::new(p);
                let mut t: Vec<String> = self.read(p).into_iter().filter(|x| !remove.contains(x)).collect();
                // one colour at a time
                if add.iter().any(|a| a.starts_with("color:")) {
                    t.retain(|x| !x.starts_with("color:"));
                }
                for a in add {
                    let a = a.trim().replace(',', " ");
                    if !a.is_empty() && !t.contains(&a) {
                        t.push(a);
                    }
                }
                self.write(p, &t)?;
                Ok(t)
            })
            .collect()
    }

    pub fn rating(&self, p: &Path) -> u8 {
        xattr::get(p, RATING).ok().flatten().and_then(|v| String::from_utf8_lossy(&v).trim().parse().ok()).unwrap_or(0).min(10)
    }
    pub fn set_rating(&self, p: &Path, r: u8) -> Result<(), String> {
        let res = if r == 0 { xattr::remove(p, RATING).or_else(|e| if e.raw_os_error() == Some(libc::ENODATA) { Ok(()) } else { Err(e) }) } else { xattr::set(p, RATING, r.min(10).to_string().as_bytes()) };
        res.map_err(|e| format!("{}: {e}", p.display()))
    }

    /// Record what we saw (from listings, edits or the background scan). Saves only when something changed.
    pub fn observe(&self, seen: &[(String, Vec<String>)]) {
        let mut g = self.index.lock().unwrap();
        let idx = g.get_or_insert_with(|| self.load("tag-index.json"));
        let mut changed = false;
        for (p, t) in seen {
            let cur = idx.get(p);
            if t.is_empty() {
                changed |= idx.remove(p).is_some();
            } else if cur != Some(t) {
                idx.insert(p.clone(), t.clone());
                changed = true;
            }
        }
        if changed {
            self.save("tag-index.json", idx);
        }
    }

    /// Forget index entries whose file is gone.
    fn prune(&self) {
        let mut g = self.index.lock().unwrap();
        let idx = g.get_or_insert_with(|| self.load("tag-index.json"));
        let before = idx.len();
        idx.retain(|p, _| std::fs::symlink_metadata(p).is_ok());
        if idx.len() != before {
            self.save("tag-index.json", idx);
        }
    }

    pub fn counts(&self) -> Vec<(String, usize)> {
        self.prune();
        let g = self.index.lock().unwrap();
        let mut c: BTreeMap<String, usize> = BTreeMap::new();
        for tags in g.as_ref().map(|m| m.values().collect::<Vec<_>>()).unwrap_or_default() {
            for t in tags {
                *c.entry(t.clone()).or_default() += 1;
            }
        }
        c.into_iter().collect()
    }

    /// Paths tagged `tag`, re-checked against the real xattrs (so tags removed elsewhere drop out).
    pub fn items(&self, tag: &str) -> Vec<PathBuf> {
        let cand: Vec<String> = {
            let mut g = self.index.lock().unwrap();
            let idx = g.get_or_insert_with(|| self.load("tag-index.json"));
            idx.iter().filter(|(_, t)| t.iter().any(|x| x == tag)).map(|(p, _)| p.clone()).collect()
        };
        let fresh: Vec<(String, Vec<String>)> = cand.iter().map(|p| (p.clone(), if Path::new(p).exists() { self.read(Path::new(p)) } else { vec![] })).collect();
        self.observe(&fresh);
        fresh.into_iter().filter(|(_, t)| t.iter().any(|x| x == tag)).map(|(p, _)| PathBuf::from(p)).collect()
    }

    /// Walk `root` (skipping hidden and bulky folders) and index every tagged item. Meant for a background thread.
    pub fn scan(&self, root: &Path, max_depth: usize) {
        let mut seen = vec![];
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((d, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let name = e.file_name();
                let n = name.to_string_lossy();
                if n.starts_with('.') || matches!(n.as_ref(), "node_modules" | "target" | "__pycache__") {
                    continue;
                }
                let p = e.path();
                let t = match xattr::get(&p, TAGS) {
                    Ok(Some(v)) => parse(&v),
                    _ => vec![],
                };
                if !t.is_empty() {
                    seen.push((p.to_string_lossy().into_owned(), t));
                }
                if depth < max_depth && e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push((p, depth + 1));
                }
            }
        }
        self.observe(&seen);
    }
}

#[derive(Serialize)]
pub struct Meta {
    pub tags: Vec<String>,
    pub rating: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xattrs_work(d: &Path) -> bool {
        let f = d.join(".probe");
        std::fs::write(&f, "").unwrap();
        let ok = xattr::set(&f, "user.test", b"1").is_ok();
        std::fs::remove_file(f).unwrap();
        ok
    }

    #[test]
    fn parse_tags() {
        assert_eq!(parse(b"work, urgent,,work ,color:red"), vec!["work", "urgent", "color:red"]);
        assert!(parse(b"").is_empty());
    }

    #[test]
    fn xattr_roundtrip_dolphin_format_and_index() {
        let t = tempfile::tempdir().unwrap();
        if !xattrs_work(t.path()) {
            return;
        }
        let s = Store::new(t.path().join("cfg"));
        let f = t.path().join("doc one.txt");
        std::fs::write(&f, "").unwrap();
        let ps = vec![f.to_string_lossy().into_owned()];
        s.edit(&ps, &["work".into(), "color:red".into()], &[]).unwrap();
        // exactly what Dolphin writes/reads
        assert_eq!(xattr::get(&f, TAGS).unwrap().unwrap(), b"work,color:red");
        s.edit(&ps, &["color:#12ab34".into(), "a,b".into()], &[]).unwrap();
        assert_eq!(s.read(&f), vec!["work", "color:#12ab34", "a b"], "one colour at a time, commas stripped");
        assert_eq!(s.counts(), vec![("a b".into(), 1), ("color:#12ab34".into(), 1), ("work".into(), 1)]);
        assert_eq!(s.items("work"), vec![f.clone()]);
        // tag removed behind our back (e.g. by Dolphin) drops out of the tag view
        xattr::set(&f, TAGS, b"other").unwrap();
        assert!(s.items("work").is_empty());
        s.edit(&ps, &[], &["other".into()]).unwrap();
        assert_eq!(xattr::get(&f, TAGS).unwrap(), None, "no empty attribute left behind");
        s.set_rating(&f, 7).unwrap();
        assert_eq!(s.rating(&f), 7);
        assert_eq!(xattr::get(&f, RATING).unwrap().unwrap(), b"7");
        s.set_rating(&f, 0).unwrap();
        assert_eq!(s.rating(&f), 0);
    }

    #[test]
    fn scan_finds_tagged_items_and_prunes_deleted() {
        let t = tempfile::tempdir().unwrap();
        if !xattrs_work(t.path()) {
            return;
        }
        std::fs::create_dir_all(t.path().join("a/b")).unwrap();
        std::fs::create_dir_all(t.path().join(".hidden")).unwrap();
        let f = t.path().join("a/b/deep.md");
        std::fs::write(&f, "").unwrap();
        std::fs::write(t.path().join(".hidden/x"), "").unwrap();
        xattr::set(&f, TAGS, b"found").unwrap();
        xattr::set(t.path().join(".hidden/x"), TAGS, b"found").unwrap();
        let s = Store::new(t.path().join("cfg"));
        s.scan(t.path(), 8);
        assert_eq!(s.items("found"), vec![f.clone()]);
        std::fs::remove_file(&f).unwrap();
        assert!(s.counts().is_empty());
    }

    #[test]
    fn sidecar_used_when_xattrs_refused() {
        // /proc files refuse user xattrs with EPERM/ENOTSUP-like errors on most kernels; tmpfs may not.
        let t = tempfile::tempdir().unwrap();
        let s = Store::new(t.path().join("cfg"));
        let p = Path::new("/proc/self/status");
        match xattr::set(p, TAGS, b"x") {
            Err(e) if no_xattr(&e) => {
                s.write(p, &["side".into()]).unwrap();
                assert_eq!(s.read(p), vec!["side"]);
                assert!(std::fs::read_to_string(t.path().join("cfg/tags.json")).unwrap().contains("side"));
            }
            _ => {}
        }
    }
}
