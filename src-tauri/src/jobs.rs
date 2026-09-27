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
    id: u64,
    title: String,
    cancel: Arc<AtomicBool>,
    rx: Mutex<Receiver<(Choice, bool)>>,
    items: bool,
    last: Mutex<(Instant, Progress)>,
}

impl Rep {
    fn emit(&self, state: &str, p: Option<&Progress>, c: Option<&Conflict>) {
        let _ = self.app.emit("op", OpEvent { id: self.id, title: &self.title, state, progress: p, conflict: c, errors: vec![], undo: None, cancelled: false });
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
fn spawn(app: &AppHandle, title: String, items: bool, work: impl FnOnce(&Rep) -> (ops::Outcome, Option<Undo>) + Send + 'static) -> u64 {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    app.state::<Jobs>().0.lock().unwrap().insert(id, (cancel.clone(), tx));
    let rep = Rep { app: app.clone(), id, title, cancel, rx: Mutex::new(rx), items, last: Mutex::new((Instant::now(), Progress::default())) };
    std::thread::spawn(move || {
        let (out, undo) = work(&rep);
        rep.app.state::<Jobs>().0.lock().unwrap().remove(&id);
        let p = rep.last.lock().unwrap().1.clone();
        let undo = undo.filter(|u| match u {
            Undo::Trash { paths } | Undo::Restore { paths, .. } => !paths.is_empty(),
            Undo::Move { pairs } => !pairs.is_empty(),
        });
        let _ = rep.app.emit("op", OpEvent { id, title: &rep.title, state: "done", progress: Some(&p), conflict: None, errors: out.errors, undo, cancelled: out.cancelled });
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
pub fn op_transfer(app: AppHandle, sources: Vec<String>, dest: String, mv: bool, names: Option<Vec<String>>) -> u64 {
    let title = format!("{} {} to {}", if mv { "Moving" } else { "Copying" }, count(sources.len(), "item"), Path::new(&dest).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(dest.clone()));
    spawn(&app, title, false, move |rep| {
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

#[tauri::command]
pub fn op_delete(app: AppHandle, items: Vec<String>) -> u64 {
    spawn(&app, format!("Deleting {}", count(items.len(), "item")), true, move |rep| (ops::delete(&paths(&items), rep), None))
}

#[tauri::command]
pub fn op_trash(app: AppHandle, items: Vec<String>) -> u64 {
    spawn(&app, format!("Moving {} to the trash", count(items.len(), "item")), true, move |rep| {
        let (out, undo) = ops::to_trash(&paths(&items), rep);
        (out, Some(undo))
    })
}

#[tauri::command]
pub fn op_undo(app: AppHandle, undo: Undo) -> u64 {
    spawn(&app, "Undoing".into(), false, move |rep| match undo {
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
