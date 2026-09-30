//! Background jobs (copy/move/delete/trash/undo) with progress + conflict events, and the clipboard.
//! Event "op" per job: state = progress | conflict | done.

use crate::ops::{self, Choice, Report, Undo};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
pub struct Jobs(Mutex<HashMap<u64, (Arc<AtomicBool>, Sender<(Choice, bool)>)>>);
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Serialize, Clone, Default)]
pub struct Progress {
    bytes_done: u64,
    bytes_total: u64,
    items_done: u64,
    items_total: u64,
    current: String,
}

#[derive(Serialize, Clone)]
pub struct Conflict {
    src: String,
    dst: String,
    src_dir: bool,
    dst_dir: bool,
    src_size: u64,
    dst_size: u64,
    src_mtime: i64,
    dst_mtime: i64,
    suggestion: String,
}

#[derive(Serialize, Clone)]
struct OpEvent<'a> {
    id: u64,
    title: &'a str,
    state: &'a str,
    progress: Option<&'a Progress>,
    conflict: Option<&'a Conflict>,
    errors: Vec<String>,
    undo: Option<Undo>,
    cancelled: bool,
}

type R<T> = Result<T, String>;

/// Bridges ops::Report to Tauri events. `items` = progress is counted in items (trash/delete), not bytes.
struct Rep {
    app: AppHandle,
    /// Window that started the job: its progress and conflict questions go there only.
    target: String,
    id: u64,
    title: String,
    cancel: Arc<AtomicBool>,
    rx: Mutex<Receiver<(Choice, bool)>>,
    items: bool,
    last: Mutex<(Instant, Progress)>,
}

impl Rep {
    fn emit(&self, state: &str, p: Option<&Progress>, c: Option<&Conflict>) {
        let _ = self.app.emit_to(self.target.as_str(), "op", OpEvent { id: self.id, title: &self.title, state, progress: p, conflict: c, errors: vec![], undo: None, cancelled: false });
    }
}

fn meta(p: &Path) -> (bool, u64, i64) {
    std::fs::symlink_metadata(p).map(|m| (m.is_dir(), m.len(), m.mtime() * 1000)).unwrap_or((false, 0, 0))
}

impl Report for Rep {
    fn progress(&self, done: u64, total: u64, current: &Path) {
        let mut g = self.last.lock().unwrap();
        let p = if self.items {
            Progress { items_done: done, items_total: total, current: current.to_string_lossy().into_owned(), ..Default::default() }
        } else {
            Progress { bytes_done: done, bytes_total: total, current: current.to_string_lossy().into_owned(), ..Default::default() }
        };
        g.1 = p;
        // at most ~10 events a second
        if g.0.elapsed() >= Duration::from_millis(100) {
            g.0 = Instant::now();
            self.emit("progress", Some(&g.1), None);
        }
    }
    fn conflict(&self, src: &Path, dst: &Path) -> (Choice, bool) {
        let (sd, ss, sm) = meta(src);
        let (dd, ds, dm) = meta(dst);
        let suggestion = ops::unique(dst, false).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let c = Conflict { src: src.to_string_lossy().into_owned(), dst: dst.to_string_lossy().into_owned(), src_dir: sd, dst_dir: dd, src_size: ss, dst_size: ds, src_mtime: sm, dst_mtime: dm, suggestion };
        self.emit("conflict", None, Some(&c));
        self.rx.lock().unwrap().recv().unwrap_or((Choice::Cancel, false))
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Run `work` on a thread; returns the job id immediately. `work` returns (outcome, undo record).
fn spawn(app: &AppHandle, target: &tauri::Window, title: String, items: bool, work: impl FnOnce(&Rep) -> (ops::Outcome, Option<Undo>) + Send + 'static) -> u64 {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    app.state::<Jobs>().0.lock().unwrap().insert(id, (cancel.clone(), tx));
    let rep = Rep { app: app.clone(), target: target.label().to_owned(), id, title, cancel, rx: Mutex::new(rx), items, last: Mutex::new((Instant::now(), Progress::default())) };
    std::thread::spawn(move || {
        let (out, undo) = work(&rep);
        rep.app.state::<Jobs>().0.lock().unwrap().remove(&id);
        let p = rep.last.lock().unwrap().1.clone();
        let undo = undo.filter(|u| match u {
            Undo::Trash { paths } | Undo::Restore { paths, .. } => !paths.is_empty(),
            Undo::Move { pairs } => !pairs.is_empty(),
        });
        let _ = rep.app.emit_to(rep.target.as_str(), "op", OpEvent { id, title: &rep.title, state: "done", progress: Some(&p), conflict: None, errors: out.errors, undo, cancelled: out.cancelled });
    });
    id
}

fn paths(v: &[String]) -> Vec<PathBuf> {
    v.iter().map(PathBuf::from).collect()
}
fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// Copy/move `sources` into `dest` (optionally under new `names`, one per source).
#[tauri::command]
pub fn op_transfer(app: AppHandle, window: tauri::Window, sources: Vec<String>, dest: String, mv: bool, names: Option<Vec<String>>) -> u64 {
    let title = format!("{} {} to {}", if mv { "Moving" } else { "Copying" }, count(sources.len(), "item"), Path::new(&dest).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(dest.clone()));
    spawn(&app, &window, title, false, move |rep| {
        let pairs: Vec<(PathBuf, PathBuf)> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let s = PathBuf::from(s);
                let name = names.as_ref().and_then(|n| n.get(i)).map(PathBuf::from).or_else(|| s.file_name().map(PathBuf::from)).unwrap_or_default();
                (s, Path::new(&dest).join(name))
            })
            .collect();
        let out = ops::transfer(&pairs, mv, rep);
        // Undo::Move pairs are (where it is now, where it came from).
        let undo = if mv {
            Some(Undo::Move { pairs: out.done.iter().map(|(s, d)| (d.clone(), s.clone())).collect() })
        } else if out.overwrote {
            None // replaced files can't be brought back by trashing the copies
        } else {
            Some(Undo::Trash { paths: out.done.iter().map(|(_, d)| d.clone()).collect() })
        };
        (out, undo)
    })
}

/// A file operation that needs root: runs through pkexec + the helper, with the usual progress and conflicts.
#[tauri::command]
pub fn op_admin(app: AppHandle, window: tauri::Window, req: crate::admin::Request) -> u64 {
    use crate::admin::Request as Q;
    let (title, items) = match &req {
        Q::Transfer { pairs, mv } => (format!("{} {} as administrator", if *mv { "Moving" } else { "Copying" }, count(pairs.len(), "item")), false),
        Q::Delete { paths } => (format!("Deleting {} as administrator", count(paths.len(), "item")), true),
        _ => ("Working as administrator".into(), true),
    };
    spawn(&app, &window, title, items, move |rep| match crate::admin::run(&req, rep) {
        Ok((out, _)) => (out, None),
        Err(e) => (ops::Outcome { errors: vec![e], ..Default::default() }, None),
    })
}

#[tauri::command]
pub fn op_delete(app: AppHandle, window: tauri::Window, items: Vec<String>) -> u64 {
    spawn(&app, &window, format!("Deleting {}", count(items.len(), "item")), true, move |rep| (ops::delete(&paths(&items), rep), None))
}

#[tauri::command]
pub fn op_trash(app: AppHandle, window: tauri::Window, items: Vec<String>) -> u64 {
    spawn(&app, &window, format!("Moving {} to the trash", count(items.len(), "item")), true, move |rep| {
        let (out, undo) = ops::to_trash(&paths(&items), rep);
        (out, Some(undo))
    })
}

#[tauri::command]
pub fn op_undo(app: AppHandle, window: tauri::Window, undo: Undo) -> u64 {
    spawn(&app, &window, "Undoing".into(), false, move |rep| match undo {
        Undo::Move { pairs } => {
            let pairs: Vec<_> = pairs.into_iter().map(|(now, orig)| (PathBuf::from(now), PathBuf::from(orig))).collect();
            (ops::transfer(&pairs, true, rep), None)
        }
        Undo::Trash { paths: p } => (ops::to_trash(&paths(&p), rep).0, None),
        Undo::Restore { paths: p, since } => {
            let mut out = ops::Outcome::default();
            if let Err(e) = ops::restore_paths(&p, since) {
                out.errors.push(e);
            }
            (out, None)
        }
    })
}

#[tauri::command]
pub fn op_cancel(id: u64, jobs: State<Jobs>) {
    if let Some((c, tx)) = jobs.0.lock().unwrap().get(&id) {
        c.store(true, Ordering::Relaxed);
        let _ = tx.send((Choice::Cancel, false)); // unblock a pending conflict question
    }
}

#[tauri::command]
pub fn op_resolve(id: u64, choice: Choice, all: bool, jobs: State<Jobs>) {
    if let Some((_, tx)) = jobs.0.lock().unwrap().get(&id) {
        let _ = tx.send((choice, all));
    }
}

#[tauri::command]
pub async fn rename_item(path: String, name: String) -> R<(String, Undo)> {
    tauri::async_runtime::spawn_blocking(move || {
        let to = ops::rename(Path::new(&path), &name)?.to_string_lossy().into_owned();
        Ok((to.clone(), Undo::Move { pairs: vec![(to, path)] }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn make_item(dir: String, name: String, folder: bool) -> R<String> {
    tauri::async_runtime::spawn_blocking(move || ops::create(Path::new(&dir), &name, folder).map(|p| p.to_string_lossy().into_owned()))
        .await
        .map_err(|e| e.to_string())?
}

/// `name` if it's free in `dir`, else "name (2)", "name (3)", …
#[tauri::command]
pub fn unique_name(dir: String, name: String) -> String {
    let p = Path::new(&dir).join(&name);
    if std::fs::symlink_metadata(&p).is_err() {
        return name;
    }
    ops::unique(&p, false).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name)
}

#[tauri::command]
pub async fn trash_list() -> R<Vec<ops::TrashEntry>> {
    tauri::async_runtime::spawn_blocking(ops::trash_list).await.map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn trash_restore(ids: Vec<String>) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || ops::trash_restore(&ids)).await.map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn trash_purge(ids: Option<Vec<String>>) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || ops::trash_purge(ids.as_deref())).await.map_err(|e| e.to_string())?
}

// ---------- archives (external tools, cancellable) ----------

const ARCHIVE_EXTS: [&str; 12] = [".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tgz", ".txz", ".tar", ".zip", ".7z", ".rar", ".gz", ".xz"];

/// "photos.tar.gz" → "photos"
pub fn archive_stem(name: &str) -> String {
    let lower = name.to_lowercase();
    ARCHIVE_EXTS.iter().find(|e| lower.ends_with(*e) && lower.len() > e.len()).map(|e| name[..name.len() - e.len()].to_owned()).unwrap_or_else(|| name.to_owned())
}

/// Run a tool until it exits or the job is cancelled; on failure/cancel remove `output` so nothing half-made stays.
fn run_tool(rep: &Rep, argv: &[String], cwd: &Path, output: &Path, out: &mut ops::Outcome) -> bool {
    let child = Command::new(&argv[0]).args(&argv[1..]).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            out.errors.push(format!("{}: {e}", argv[0]));
            return false;
        }
    };
    let cleanup = || {
        let _ = if output.is_dir() { std::fs::remove_dir_all(output) } else { std::fs::remove_file(output) };
    };
    loop {
        if rep.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            cleanup();
            out.cancelled = true;
            return false;
        }
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return true,
            Ok(Some(st)) => {
                let mut err = String::new();
                let _ = std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err);
                out.errors.push(format!("{} failed ({st}): {}", argv[0], err.lines().rev().take(3).collect::<Vec<_>>().join(" / ")));
                cleanup();
                return false;
            }
            Ok(None) => {
                rep.progress(0, 0, output);
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                out.errors.push(e.to_string());
                return false;
            }
        }
    }
}

#[tauri::command]
pub fn op_compress(app: AppHandle, window: tauri::Window, items: Vec<String>, format: String) -> R<u64> {
    let first = PathBuf::from(items.first().ok_or("nothing selected")?);
    let dir = first.parent().unwrap_or(Path::new("/")).to_path_buf();
    let base = if items.len() == 1 {
        let n = first.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if first.is_dir() { n } else { archive_stem(&n).rsplit_once('.').map(|(a, _)| a.to_owned()).filter(|a| !a.is_empty()).unwrap_or(n) }
    } else {
        "Archive".into()
    };
    let want = dir.join(format!("{base}.{format}"));
    let out_path = if want.exists() { ops::unique(&want, false) } else { want };
    let names: Vec<String> = items.iter().filter_map(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned())).collect();
    let o = out_path.to_string_lossy().into_owned();
    let argv: Vec<String> = match format.as_str() {
        "zip" => ["zip", "-r", "-y", "-q", &o, "--"].iter().map(|s| s.to_string()).chain(names).collect(),
        "7z" => ["7z", "a", "-y", &o, "--"].iter().map(|s| s.to_string()).chain(names).collect(),
        f if f.starts_with("tar") => ["tar", "-caf", &o, "--"].iter().map(|s| s.to_string()).chain(names).collect(),
        f => return Err(format!("unknown format {f}")),
    };
    let title = format!("Compressing to {}", out_path.file_name().unwrap_or_default().to_string_lossy());
    Ok(spawn(&app, &window, title, false, move |rep| {
        let mut out = ops::Outcome::default();
        let ok = run_tool(rep, &argv, &dir, &out_path, &mut out);
        let undo = ok.then(|| Undo::Trash { paths: vec![out_path.to_string_lossy().into_owned()] });
        if ok {
            out.done.push((String::new(), out_path.to_string_lossy().into_owned()));
        }
        (out, undo)
    }))
}

/// Extract each archive into a new folder named after it, next to it.
#[tauri::command]
pub fn op_extract(app: AppHandle, window: tauri::Window, items: Vec<String>) -> u64 {
    spawn(&app, &window, format!("Extracting {}", count(items.len(), "archive")), false, move |rep| {
        let mut out = ops::Outcome::default();
        let mut made = vec![];
        for a in &items {
            let a = PathBuf::from(a);
            let dir = a.parent().unwrap_or(Path::new("/")).to_path_buf();
            let want = dir.join(archive_stem(&a.file_name().unwrap_or_default().to_string_lossy()));
            let dest = if want.exists() { ops::unique(&want, false) } else { want };
            if let Err(e) = std::fs::create_dir(&dest) {
                out.errors.push(format!("{}: {e}", dest.display()));
                continue;
            }
            // bsdtar reads zip/7z/rar/tar.*, refusing absolute and ../ paths by default
            let ok = run_tool(rep, &["bsdtar".into(), "-xf".into(), a.to_string_lossy().into_owned(), "-C".into(), dest.to_string_lossy().into_owned()], &dir, &dest, &mut out)
                || (crate::desktop::which("7z").is_some() && !out.cancelled && {
                    out.errors.pop();
                    let _ = std::fs::create_dir(&dest);
                    run_tool(rep, &["7z".into(), "x".into(), "-y".into(), format!("-o{}", dest.display()), a.to_string_lossy().into_owned()], &dir, &dest, &mut out)
                });
            if ok {
                made.push(dest.to_string_lossy().into_owned());
            }
            if out.cancelled {
                break;
            }
        }
        out.done = made.iter().map(|m| (String::new(), m.clone())).collect();
        (out, Some(Undo::Trash { paths: made }))
    })
}

/// Which archive tools exist (so the menu only offers what works).
#[tauri::command]
pub fn archive_tools() -> HashMap<&'static str, bool> {
    ["zip", "tar", "7z", "bsdtar"].into_iter().map(|t| (t, crate::desktop::which(t).is_some())).collect()
}

/// Put plain text (e.g. a path) on the clipboard.
#[tauri::command]
pub fn copy_text(text: String) -> R<()> {
    let mut cmd = if wayland() { Command::new("wl-copy") } else { let mut c = Command::new("xclip"); c.args(["-selection", "clipboard"]); c };
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    child.stdin.take().unwrap().write_all(text.as_bytes()).map_err(|e| e.to_string())
}

// ---------- clipboard (text/uri-list, interoperable with Dolphin/Nautilus) ----------

pub fn to_uri(p: &str) -> String {
    let mut s = String::from("file://");
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}
pub fn from_uri(u: &str) -> Option<String> {
    let rest = u.trim().strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 1 {
            if let Some(v) = rest.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[derive(Default)]
pub struct Clip(Mutex<(String, bool)>); // (uri-list we set, cut?)

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[tauri::command]
pub fn clip_set(paths: Vec<String>, cut: bool, clip: State<Clip>) -> R<()> {
    let list: String = paths.iter().map(|p| to_uri(p) + "\r\n").collect();
    // NOTE: one MIME type per wl-copy; Nautilus' x-special/gnome-copied-files isn't offered, so a
    // cut pasted into another file manager becomes a copy. Needs our own data-control client to fix.
    let mut cmd = if wayland() { let mut c = Command::new("wl-copy"); c.args(["--type", "text/uri-list"]); c } else { let mut c = Command::new("xclip"); c.args(["-selection", "clipboard", "-t", "text/uri-list"]); c };
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("clipboard tool missing (wl-clipboard / xclip): {e}"))?;
    child.stdin.take().unwrap().write_all(list.as_bytes()).map_err(|e| e.to_string())?;
    *clip.0.lock().unwrap() = (list, cut);
    Ok(())
}

#[derive(Serialize)]
pub struct ClipContent {
    paths: Vec<String>,
    cut: bool,
}

#[tauri::command]
pub async fn clip_get(app: AppHandle) -> R<ClipContent> {
    tauri::async_runtime::spawn_blocking(move || {
        let read = |t: &str| {
            let out = if wayland() { Command::new("wl-paste").args(["--no-newline", "--type", t]).output() } else { Command::new("xclip").args(["-selection", "clipboard", "-o", "-t", t]).output() };
            out.ok().filter(|o| o.status.success()).and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default()
        };
        let mut text = read("text/uri-list");
        let mut cut = false;
        let gnome = read("x-special/gnome-copied-files");
        if text.is_empty() && !gnome.is_empty() {
            cut = gnome.starts_with("cut");
            text = gnome.lines().skip(1).collect::<Vec<_>>().join("\n");
        }
        if read("application/x-kde-cutselection").trim() == "1" {
            cut = true;
        }
        let ours = app.state::<Clip>().0.lock().unwrap().clone();
        if !ours.0.is_empty() && ours.0.trim() == text.trim() {
            cut = ours.1;
        }
        let paths: Vec<String> = text.lines().filter_map(from_uri).collect();
        ClipContent { paths, cut }
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_stems() {
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("a.b.ZIP"), "a.b");
        assert_eq!(archive_stem(".zip"), ".zip");
        assert_eq!(archive_stem("notes.txt"), "notes.txt");
    }

    #[test]
    fn uri_roundtrip() {
        for p in ["/home/a b/ünï💾.txt", "/x/100%/#hash?.md", "/new\nline"] {
            let u = to_uri(p);
            assert!(!u.contains(' ') && !u.contains('\n'));
            assert_eq!(from_uri(&u).as_deref(), Some(p));
        }
        assert_eq!(from_uri("file://localhost/tmp/a").as_deref(), Some("/tmp/a"));
        assert_eq!(from_uri("https://x"), None);
    }
}
