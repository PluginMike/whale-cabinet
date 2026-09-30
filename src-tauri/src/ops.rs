//! File operations: copy/move (with conflict handling and copy-verify-delete across devices), trash,
//! permanent delete, rename, create, and undo records. UI-agnostic: progress/conflicts go through `Report`.

use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "choice", content = "name", rename_all = "lowercase")]
pub enum Choice {
    Skip,
    Overwrite,
    /// New file name, or None for an automatic "name (2).ext".
    Rename(Option<String>),
    Cancel,
}

pub trait Report: Sync {
    fn progress(&self, done: u64, total: u64, current: &Path);
    /// Ask what to do when `dst` exists. Returns the choice and whether to apply it to all later conflicts.
    fn conflict(&self, src: &Path, dst: &Path) -> (Choice, bool);
    fn cancelled(&self) -> bool;
}

/// What an operation did, enough to undo it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Undo {
    /// Created these paths: undo = move them to the trash.
    Trash { paths: Vec<String> },
    /// Moved/renamed (from, to): undo = move back.
    Move { pairs: Vec<(String, String)> },
    /// Trashed these original paths at/after `since` (unix secs): undo = restore them.
    Restore { paths: Vec<String>, since: i64 },
}

#[derive(Serialize, Debug, Default)]
pub struct Outcome {
    pub errors: Vec<String>,
    /// (source, final destination) of every top-level item that completed.
    pub done: Vec<(String, String)>,
    pub overwrote: bool,
    pub cancelled: bool,
}

const CHUNK: usize = 1 << 20;

fn cstr(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
}

/// rename(2) that never replaces an existing target.
pub fn rename_noreplace(a: &Path, b: &Path) -> io::Result<()> {
    let (ca, cb) = (cstr(a)?, cstr(b)?);
    let r = unsafe { libc::renameat2(libc::AT_FDCWD, ca.as_ptr(), libc::AT_FDCWD, cb.as_ptr(), libc::RENAME_NOREPLACE) };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        // Filesystems without RENAME_NOREPLACE (some FUSE/NFS): check-then-rename.
        Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP) => {
            if fs::symlink_metadata(b).is_ok() {
                return Err(io::Error::from_raw_os_error(libc::EEXIST));
            }
            fs::rename(a, b)
        }
        _ => Err(e),
    }
}

fn set_times(p: &Path, m: &fs::Metadata) {
    let ts = [
        libc::timespec { tv_sec: m.atime(), tv_nsec: m.atime_nsec() },
        libc::timespec { tv_sec: m.mtime(), tv_nsec: m.mtime_nsec() },
    ];
    if let Ok(c) = cstr(p) {
        unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), ts.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
    }
}

/// Total bytes of regular files under `p` (symlinks not followed).
pub fn tree_size(p: &Path) -> u64 {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::read_dir(p).map(|rd| rd.flatten().map(|e| tree_size(&e.path())).sum()).unwrap_or(0),
        Ok(m) if m.is_file() => m.len(),
        _ => 0,
    }
}

/// "name.ext" → first free "name (2).ext", "name (3).ext", … (or "name (copy).ext" style with `copy`).
pub fn unique(dst: &Path, copy: bool) -> PathBuf {
    let parent = dst.parent().unwrap_or(Path::new("/"));
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let is_dir = dst.is_dir();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 1.. {
        let cand = match (copy, n) {
            (true, 1) => format!("{stem} (copy){ext}"),
            (true, n) => format!("{stem} (copy {n}){ext}"),
            (false, 1) => continue,
            (false, n) => format!("{stem} ({n}){ext}"),
        };
        let p = parent.join(cand);
        if fs::symlink_metadata(&p).is_err() {
            return p;
        }
    }
    unreachable!()
}

pub fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(format!("\"{name}\" is not a valid name"));
    }
    Ok(())
}

struct Ctx<'a> {
    rep: &'a dyn Report,
    total: u64,
    done: u64,
    all: Option<Choice>,
    out: Outcome,
}

impl Ctx<'_> {
    fn err(&mut self, p: &Path, e: impl std::fmt::Display) {
        self.out.errors.push(format!("{}: {e}", p.display()));
    }
    fn ask(&mut self, src: &Path, dst: &Path) -> Choice {
        if let Some(c) = &self.all {
            return c.clone();
        }
        let (c, all) = self.rep.conflict(src, dst);
        if all {
            // "Rename" with a typed name can't sensibly apply to everything: fall back to auto names.
            self.all = Some(if let Choice::Rename(_) = c { Choice::Rename(None) } else { c.clone() });
        }
        c
    }
}

/// Copy or move each (src, dst) pair. `dst` is the full target path (not the folder).
pub fn transfer(pairs: &[(PathBuf, PathBuf)], mv: bool, rep: &dyn Report) -> Outcome {
    let total = pairs.iter().map(|(s, _)| tree_size(s)).sum();
    let mut cx = Ctx { rep, total, done: 0, all: None, out: Outcome::default() };
    for (src, dst) in pairs {
        if rep.cancelled() {
            cx.out.cancelled = true;
            break;
        }
        let mut dst = dst.clone();
        if fs::symlink_metadata(src).is_err() {
            cx.err(src, "no longer exists");
            continue;
        }
        if &dst == src {
            if mv {
                continue; // moving onto itself: nothing to do
            }
            dst = unique(&dst, true); // paste into the same folder = duplicate
        }
        if dst.starts_with(src) {
            cx.err(src, "can't copy or move a folder into itself");
            continue;
        }
        if let Some(fin) = item(&mut cx, src, &dst, mv) {
            cx.out.done.push((src.to_string_lossy().into_owned(), fin.to_string_lossy().into_owned()));
        }
    }
    cx.out.cancelled |= rep.cancelled();
    cx.out
}

/// Transfer one item; returns the final destination if it fully completed.
fn item(cx: &mut Ctx, src: &Path, dst: &Path, mv: bool) -> Option<PathBuf> {
    let smeta = match fs::symlink_metadata(src) {
        Ok(m) => m,
        Err(e) => {
            cx.err(src, e);
            return None;
        }
    };
    let mut dst = dst.to_path_buf();
    let mut overwrite = false;
    if let Ok(dmeta) = fs::symlink_metadata(&dst) {
        if smeta.is_dir() && dmeta.is_dir() {
            return merge(cx, src, &dst, mv);
        }
        match cx.ask(src, &dst) {
            Choice::Skip => {
                cx.done += tree_size(src);
                return None;
            }
            Choice::Cancel => {
                cx.out.cancelled = true;
                return None;
            }
            Choice::Rename(name) => {
                dst = match name {
                    Some(n) if valid_name(&n).is_ok() => dst.with_file_name(n),
                    _ => unique(&dst, false),
                };
                return item(cx, src, &dst, mv);
            }
            Choice::Overwrite => {
                if dmeta.is_dir() != smeta.is_dir() {
                    // Different kinds can't be replaced atomically: send the old one to the trash, never delete it.
                    if let Err(e) = trash::delete(&dst) {
                        cx.err(&dst, format!("couldn't move the existing item to the trash: {e}"));
                        return None;
                    }
                } else {
                    overwrite = true;
                }
                cx.out.overwrote = true;
            }
        }
    }
    if mv {
        let r = if overwrite { fs::rename(src, &dst) } else { rename_noreplace(src, &dst) };
        match r {
            Ok(()) => {
                cx.done += tree_size(&dst);
                cx.rep.progress(cx.done, cx.total, &dst);
                return Some(dst);
            }
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {} // different filesystem: copy, verify, then delete
            Err(e) => {
                cx.err(src, e);
                return None;
            }
        }
    }
    if !copy_tree(cx, src, &dst, mv, overwrite) {
        return None;
    }
    if mv {
        // Every byte was copied and verified; only now remove the source.
        let r = if smeta.is_dir() { fs::remove_dir_all(src) } else { fs::remove_file(src) };
        if let Err(e) = r {
            cx.err(src, format!("copied, but couldn't remove the original: {e}"));
        }
    }
    Some(dst)
}

/// Folder onto existing folder: merge children one by one.
fn merge(cx: &mut Ctx, src: &Path, dst: &Path, mv: bool) -> Option<PathBuf> {
    let entries = match fs::read_dir(src) {
        Ok(rd) => rd.flatten().collect::<Vec<_>>(),
        Err(e) => {
            cx.err(src, e);
            return None;
        }
    };
    let mut ok = true;
    for e in entries {
        if cx.rep.cancelled() || cx.out.cancelled {
            cx.out.cancelled = true;
            return None;
        }
        ok &= item(cx, &e.path(), &dst.join(e.file_name()), mv).is_some();
    }
    if mv && ok {
        let _ = fs::remove_dir(src); // only succeeds if everything moved out
    }
    ok.then(|| dst.to_path_buf())
}

/// Recursive copy into a path that doesn't exist yet (or a file to overwrite). Returns true if all of it succeeded.
fn copy_tree(cx: &mut Ctx, src: &Path, dst: &Path, verify: bool, overwrite: bool) -> bool {
    let m = match fs::symlink_metadata(src) {
        Ok(m) => m,
        Err(e) => {
            cx.err(src, e);
            return false;
        }
    };
    let ft = m.file_type();
    if ft.is_symlink() {
        let r = fs::read_link(src).and_then(|t| {
            if overwrite {
                let _ = fs::remove_file(dst);
            }
            std::os::unix::fs::symlink(t, dst)
        });
        return r.map_err(|e| cx.err(src, e)).is_ok();
    }
    if ft.is_dir() {
        if let Err(e) = fs::create_dir(dst) {
            cx.err(dst, e);
            return false;
        }
        let mut ok = true;
        match fs::read_dir(src) {
            Ok(rd) => {
                for e in rd.flatten() {
                    if cx.rep.cancelled() {
                        cx.out.cancelled = true;
                        return false;
                    }
                    ok &= copy_tree(cx, &e.path(), &dst.join(e.file_name()), verify, false);
                }
            }
            Err(e) => {
                cx.err(src, e);
                ok = false;
            }
        }
        let _ = fs::set_permissions(dst, fs::Permissions::from_mode(m.mode()));
        set_times(dst, &m);
        return ok;
    }
    if !ft.is_file() {
        cx.err(src, "special file (pipe, socket or device) skipped");
        return false;
    }
    match copy_file(cx, src, dst, &m, verify, overwrite) {
        Ok(()) => true,
        Err(e) => {
            if e.kind() != io::ErrorKind::Interrupted {
                cx.err(src, e);
            }
            false
        }
    }
}

/// Copy one file via a temp file in the target folder, then rename into place, so a cancelled or failed
/// copy never leaves a half-written file under the real name (and never truncates an existing one).
fn copy_file(cx: &mut Ctx, src: &Path, dst: &Path, m: &fs::Metadata, verify: bool, overwrite: bool) -> io::Result<()> {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dst.with_file_name(format!(".{}.wc-part-{}", name.chars().take(180).collect::<String>(), std::process::id()));
    let res = (|| {
        let mut r = File::open(src)?;
        let mut w = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let mut buf = vec![0u8; CHUNK];
        let mut crc = crc32fast::Hasher::new();
        loop {
            if cx.rep.cancelled() {
                cx.out.cancelled = true;
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            crc.update(&buf[..n]);
            w.write_all(&buf[..n])?;
            cx.done += n as u64;
            cx.rep.progress(cx.done, cx.total, src);
        }
        w.set_permissions(fs::Permissions::from_mode(m.mode()))?;
        if verify {
            w.sync_all()?;
            drop(w);
            // Read back what landed on disk and compare checksums before the caller deletes the source.
            let mut back = crc32fast::Hasher::new();
            let mut rr = File::open(&tmp)?;
            loop {
                let n = rr.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                back.update(&buf[..n]);
            }
            if back.finalize() != crc.finalize() {
                return Err(io::Error::other("verification failed: copy differs from the original"));
            }
        }
        set_times(&tmp, m);
        if overwrite {
            fs::rename(&tmp, dst)
        } else {
            rename_noreplace(&tmp, dst)
        }
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Permanently delete (symlinks are removed, never followed).
pub fn delete(paths: &[PathBuf], rep: &dyn Report) -> Outcome {
    let mut out = Outcome::default();
    for (i, p) in paths.iter().enumerate() {
        if rep.cancelled() {
            out.cancelled = true;
            break;
        }
        rep.progress(i as u64, paths.len() as u64, p);
        let r = match fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() => fs::remove_dir_all(p),
            Ok(_) => fs::remove_file(p),
            Err(e) => Err(e),
        };
        match r {
            Ok(()) => out.done.push((p.to_string_lossy().into_owned(), String::new())),
            Err(e) => out.errors.push(format!("{}: {e}", p.display())),
        }
    }
    out
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Move to the freedesktop trash. Returns the undo record.
pub fn to_trash(paths: &[PathBuf], rep: &dyn Report) -> (Outcome, Undo) {
    let since = now_secs() - 1;
    let mut out = Outcome::default();
    for (i, p) in paths.iter().enumerate() {
        if rep.cancelled() {
            out.cancelled = true;
            break;
        }
        rep.progress(i as u64, paths.len() as u64, p);
        match trash::delete(p) {
            Ok(()) => out.done.push((p.to_string_lossy().into_owned(), String::new())),
            Err(e) => out.errors.push(format!("{}: {e}", p.display())),
        }
    }
    let undo = Undo::Restore { paths: out.done.iter().map(|d| d.0.clone()).collect(), since };
    (out, undo)
}

#[derive(Serialize, Debug, Clone)]
pub struct TrashEntry {
    pub id: String,
    pub name: String,
    pub orig_path: String,
    pub deleted: i64,
    /// Where the item physically lives now (for size/preview).
    pub path: String,
    pub dir: bool,
    pub size: u64,
}

pub fn trash_list() -> Result<Vec<TrashEntry>, String> {
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;
    Ok(items
        .into_iter()
        .map(|it| {
            // On Linux the id is the .trashinfo path; the data sits in ../files/<same name>.
            let info = PathBuf::from(&it.id);
            let file = info.parent().and_then(|p| p.parent()).map(|t| t.join("files").join(info.file_stem().unwrap_or_default())).unwrap_or_default();
            let meta = fs::symlink_metadata(&file).ok();
            TrashEntry {
                dir: meta.as_ref().is_some_and(|m| m.is_dir()),
                size: meta.as_ref().filter(|m| m.is_file()).map_or(0, |m| m.len()),
                id: it.id.to_string_lossy().into_owned(),
                name: it.name.to_string_lossy().into_owned(),
                orig_path: it.original_path().to_string_lossy().into_owned(),
                deleted: it.time_deleted,
                path: file.to_string_lossy().into_owned(),
            }
        })
        .collect())
}

fn trash_items(ids: &[String]) -> Result<Vec<trash::TrashItem>, String> {
    let all = trash::os_limited::list().map_err(|e| e.to_string())?;
    Ok(all.into_iter().filter(|i| ids.iter().any(|x| x.as_str() == i.id.to_string_lossy())).collect())
}

pub fn trash_restore(ids: &[String]) -> Result<(), String> {
    trash::os_limited::restore_all(trash_items(ids)?).map_err(|e| match e {
        trash::Error::RestoreCollision { path, .. } => format!("{} already exists; restore skipped", path.display()),
        e => e.to_string(),
    })
}

pub fn trash_purge(ids: Option<&[String]>) -> Result<(), String> {
    let items = match ids {
        Some(ids) => trash_items(ids)?,
        None => trash::os_limited::list().map_err(|e| e.to_string())?,
    };
    trash::os_limited::purge_all(items).map_err(|e| e.to_string())
}

/// Undo a "Restore" record: bring back the newest trashed copy of each original path.
pub fn restore_paths(paths: &[String], since: i64) -> Result<(), String> {
    let all = trash::os_limited::list().map_err(|e| e.to_string())?;
    let mut pick: Vec<trash::TrashItem> = vec![];
    for p in paths {
        if let Some(it) = all.iter().filter(|i| i.original_path().to_string_lossy() == p.as_str() && i.time_deleted >= since).max_by_key(|i| i.time_deleted) {
            pick.push(it.clone());
        }
    }
    if pick.is_empty() {
        return Err("nothing to restore (the trash was emptied?)".into());
    }
    trash::os_limited::restore_all(pick).map_err(|e| e.to_string())
}

pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    valid_name(new_name)?;
    let dst = path.with_file_name(new_name);
    rename_noreplace(path, &dst).map_err(|e| {
        if e.raw_os_error() == Some(libc::EEXIST) {
            format!("\"{new_name}\" already exists")
        } else {
            e.to_string()
        }
    })?;
    Ok(dst)
}

pub fn create(dir: &Path, name: &str, folder: bool) -> Result<PathBuf, String> {
    valid_name(name)?;
    let p = dir.join(name);
    let r = if folder { fs::create_dir(&p) } else { OpenOptions::new().write(true).create_new(true).open(&p).map(drop) };
    r.map(|_| p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    struct Rep {
        answer: Mutex<Vec<(Choice, bool)>>,
        asked: Mutex<Vec<PathBuf>>,
        cancel: AtomicBool,
    }
    impl Rep {
        fn new(answers: Vec<(Choice, bool)>) -> Self {
            Rep { answer: Mutex::new(answers), asked: Mutex::new(vec![]), cancel: AtomicBool::new(false) }
        }
    }
    impl Report for Rep {
        fn progress(&self, _: u64, _: u64, _: &Path) {}
        fn conflict(&self, _: &Path, dst: &Path) -> (Choice, bool) {
            self.asked.lock().unwrap().push(dst.into());
            let mut a = self.answer.lock().unwrap();
            if a.is_empty() { (Choice::Skip, false) } else { a.remove(0) }
        }
        fn cancelled(&self) -> bool {
            self.cancel.load(Ordering::Relaxed)
        }
    }

    fn tree(d: &Path) {
        fs::create_dir_all(d.join("src/sub dir")).unwrap();
        fs::write(d.join("src/a.txt"), "alpha").unwrap();
        fs::write(d.join("src/sub dir/ünï\nline"), vec![7u8; 3 * CHUNK + 5]).unwrap();
        std::os::unix::fs::symlink("a.txt", d.join("src/link")).unwrap();
        std::os::unix::fs::symlink("nowhere", d.join("src/broken")).unwrap();
    }

    #[test]
    fn copy_tree_preserves_content_links_and_mode() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        tree(d);
        fs::set_permissions(d.join("src/a.txt"), fs::Permissions::from_mode(0o640)).unwrap();
        let out = transfer(&[(d.join("src"), d.join("dst"))], false, &Rep::new(vec![]));
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(fs::read(d.join("dst/sub dir/ünï\nline")).unwrap().len(), 3 * CHUNK + 5);
        assert_eq!(fs::read_link(d.join("dst/link")).unwrap(), PathBuf::from("a.txt"));
        assert_eq!(fs::read_link(d.join("dst/broken")).unwrap(), PathBuf::from("nowhere"));
        assert_eq!(fs::metadata(d.join("dst/a.txt")).unwrap().mode() & 0o777, 0o640);
        assert_eq!(fs::metadata(d.join("dst/a.txt")).unwrap().mtime(), fs::metadata(d.join("src/a.txt")).unwrap().mtime());
        assert!(d.join("src/a.txt").exists(), "copy keeps the source");
        // no temp files left behind
        assert!(fs::read_dir(d.join("dst")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("wc-part")));
    }

    #[test]
    fn move_same_device_and_into_itself() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        tree(d);
        let out = transfer(&[(d.join("src"), d.join("src/sub dir/inner"))], true, &Rep::new(vec![]));
        assert_eq!(out.errors.len(), 1, "moving a folder into itself must fail");
        let out = transfer(&[(d.join("src"), d.join("moved"))], true, &Rep::new(vec![]));
        assert!(out.errors.is_empty());
        assert!(!d.join("src").exists() && d.join("moved/a.txt").exists());
    }

    #[test]
    fn cross_device_move_copies_verifies_then_deletes() {
        // /dev/shm is tmpfs, the temp dir usually isn't; skip if they share a device.
        let a = tempfile::tempdir().unwrap();
        let Ok(b) = tempfile::tempdir_in("/dev/shm") else { return };
        if fs::metadata(a.path()).unwrap().dev() == fs::metadata(b.path()).unwrap().dev() {
            return;
        }
        tree(a.path());
        let out = transfer(&[(a.path().join("src"), b.path().join("src"))], true, &Rep::new(vec![]));
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(!a.path().join("src").exists());
        assert_eq!(fs::read(b.path().join("src/a.txt")).unwrap(), b"alpha");
    }

    #[test]
    fn conflicts_skip_overwrite_rename_and_apply_all() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        fs::create_dir_all(d.join("from")).unwrap();
        fs::create_dir_all(d.join("to")).unwrap();
        for n in ["1", "2", "3"] {
            fs::write(d.join("from").join(n), "new").unwrap();
            fs::write(d.join("to").join(n), "old").unwrap();
        }
        let pairs: Vec<_> = ["1", "2", "3"].iter().map(|n| (d.join("from").join(n), d.join("to").join(n))).collect();
        let rep = Rep::new(vec![(Choice::Skip, false), (Choice::Rename(Some("2b".into())), false), (Choice::Overwrite, true)]);
        let out = transfer(&pairs, false, &rep);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(fs::read_to_string(d.join("to/1")).unwrap(), "old");
        assert_eq!(fs::read_to_string(d.join("to/2")).unwrap(), "old");
        assert_eq!(fs::read_to_string(d.join("to/2b")).unwrap(), "new");
        assert_eq!(fs::read_to_string(d.join("to/3")).unwrap(), "new");
        assert!(out.overwrote);
        // folder onto folder merges and only asks about the clashing file
        fs::create_dir_all(d.join("m1/x")).unwrap();
        fs::create_dir_all(d.join("m2/x")).unwrap();
        fs::write(d.join("m1/x/keep"), "1").unwrap();
        fs::write(d.join("m1/x/clash"), "new").unwrap();
        fs::write(d.join("m2/x/clash"), "old").unwrap();
        let rep = Rep::new(vec![(Choice::Rename(None), false)]);
        transfer(&[(d.join("m1/x"), d.join("m2/x"))], true, &rep);
        assert_eq!(rep.asked.lock().unwrap().len(), 1);
        assert_eq!(fs::read_to_string(d.join("m2/x/clash")).unwrap(), "old");
        assert_eq!(fs::read_to_string(d.join("m2/x/clash (2)")).unwrap(), "new");
        assert!(d.join("m2/x/keep").exists() && !d.join("m1/x").exists());
    }

    #[test]
    fn paste_into_same_folder_duplicates() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        fs::write(d.join("f.txt"), "x").unwrap();
        let out = transfer(&[(d.join("f.txt"), d.join("f.txt"))], false, &Rep::new(vec![]));
        assert_eq!(out.done[0].1, d.join("f (copy).txt").to_string_lossy());
        transfer(&[(d.join("f.txt"), d.join("f.txt"))], false, &Rep::new(vec![]));
        assert!(d.join("f (copy 2).txt").exists());
    }

    #[test]
    fn cancel_leaves_no_partial_file() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        fs::write(d.join("big"), vec![1u8; 4 * CHUNK]).unwrap();
        let rep = Rep::new(vec![]);
        rep.cancel.store(true, Ordering::Relaxed);
        let out = transfer(&[(d.join("big"), d.join("copy"))], false, &rep);
        assert!(out.cancelled);
        assert!(!d.join("copy").exists());
        assert_eq!(fs::read_dir(d).unwrap().count(), 1);
    }

    #[test]
    fn specials_are_skipped_not_hung() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let c = CString::new(d.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let out = transfer(&[(d.join("pipe"), d.join("pipe2"))], false, &Rep::new(vec![]));
        assert_eq!(out.errors.len(), 1);
        assert!(!d.join("pipe2").exists());
    }

    #[test]
    fn rename_create_delete() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let f = create(d, "new file", false).unwrap();
        create(d, "folder", true).unwrap();
        assert!(create(d, "folder", true).is_err());
        assert!(create(d, "a/b", false).is_err());
        assert!(rename(&f, "folder").unwrap_err().contains("already exists"));
        let g = rename(&f, "renamed ✓").unwrap();
        assert!(g.exists() && !f.exists());
        std::os::unix::fs::symlink(d.join("folder"), d.join("lnk")).unwrap();
        let out = delete(&[d.join("lnk")], &Rep::new(vec![]));
        assert!(out.errors.is_empty());
        assert!(d.join("folder").exists(), "deleting a symlink never touches its target");
        delete(&[d.join("folder"), g], &Rep::new(vec![]));
        assert_eq!(fs::read_dir(d).unwrap().count(), 0);
    }

    #[test]
    fn unique_names() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        fs::write(d.join("a.tar.gz"), "").unwrap();
        fs::create_dir(d.join("dir.v1")).unwrap();
        assert_eq!(unique(&d.join("a.tar.gz"), false), d.join("a.tar (2).gz"));
        assert_eq!(unique(&d.join("dir.v1"), true), d.join("dir.v1 (copy)"));
        assert_eq!(unique(&d.join(".hidden"), false), d.join(".hidden (2)"));
    }
}
