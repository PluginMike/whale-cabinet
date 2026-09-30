#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod admin;
mod crypt;
mod desktop;
mod dupes;
mod fm1;
mod fuzzy;
mod git;
mod jobs;
mod listing;
mod mounts;
mod network;
mod ops;
mod places;
mod plugins;
mod preview;
mod props;
mod recent;
mod search;
mod settings;
mod tags;
mod term;
mod theme;
mod usage;
mod zoxide;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;
use tauri::{async_runtime::spawn_blocking, AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

/// inotify watcher + what each window wants watched (the watcher follows the union).
struct Watched(Mutex<(RecommendedWatcher, HashSet<PathBuf>, HashMap<String, HashSet<PathBuf>>)>);
/// Label of the browser window used last: D-Bus "Show in folder" requests go there.
struct LastWin(Mutex<String>);
/// Each browser window's tabs (reported by the window); saved to session.json when the last one closes.
struct Sessions(Mutex<Vec<(String, Value)>>);

fn session_file() -> PathBuf {
    settings::dir().join("session.json")
}
struct Settings(Mutex<Map<String, Value>>);

type R<T> = Result<T, String>;
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> R<T> {
    spawn_blocking(f).await.map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Place {
    name: &'static str,
    path: String,
}

// ---------- browsing ----------

#[tauri::command]
async fn list_dir(path: String, app: AppHandle) -> R<Vec<listing::Entry>> {
    blocking(move || {
        let mut v = listing::list_dir(Path::new(&path)).map_err(|e| format!("{path}: {e}"))?;
        let store = app.state::<tags::Store>();
        for e in &mut v {
            e.tags = store.read(Path::new(&e.path));
        }
        // keep the tag index in step with what's on disk (catches tags changed by Dolphin)
        store.observe(&v.iter().map(|e| (e.path.clone(), e.tags.clone())).collect::<Vec<_>>());
        Ok(v)
    })
    .await?
}

// ---------- tags ----------

#[tauri::command]
async fn tags_edit(paths: Vec<String>, add: Vec<String>, remove: Vec<String>, app: AppHandle) -> R<Vec<Vec<String>>> {
    blocking(move || app.state::<tags::Store>().edit(&paths, &add, &remove)).await?
}

#[tauri::command]
async fn tag_meta(path: String, app: AppHandle) -> R<tags::Meta> {
    blocking(move || {
        let s = app.state::<tags::Store>();
        tags::Meta { tags: s.read(Path::new(&path)), rating: s.rating(Path::new(&path)) }
    })
    .await
}

#[tauri::command]
async fn set_rating(path: String, rating: u8, app: AppHandle) -> R<()> {
    blocking(move || app.state::<tags::Store>().set_rating(Path::new(&path), rating)).await?
}

#[tauri::command]
async fn tag_counts(app: AppHandle) -> R<Vec<(String, usize)>> {
    blocking(move || app.state::<tags::Store>().counts()).await
}

#[tauri::command]
async fn tag_items(tag: String, app: AppHandle) -> R<Vec<listing::Entry>> {
    blocking(move || {
        let s = app.state::<tags::Store>();
        s.items(&tag)
            .iter()
            .filter_map(|p| listing::entry(p).map(|mut e| { e.tags = s.read(p); e }))
            .collect()
    })
    .await
}

#[tauri::command]
async fn resolve_path(input: String, cwd: String) -> R<String> {
    blocking(move || listing::resolve(&input, Path::new(&cwd)).map(|p| p.to_string_lossy().into_owned())).await?
}

#[tauri::command]
async fn disk_space(path: String) -> Option<(u64, u64)> {
    blocking(move || listing::disk_space(Path::new(&path))).await.ok().flatten()
}

/// What to open at startup, from argv: `[--select FILE] [PATH|URI]...`. Empty = home.
#[tauri::command]
fn start_args() -> Vec<listing::Target> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    listing::parse_args(&args, &std::env::current_dir().unwrap_or_else(|_| "/".into()))
}

fn home_target() -> listing::Target {
    listing::Target { loc: dirs::home_dir().unwrap_or_else(|| "/".into()).to_string_lossy().into_owned(), select: None }
}

struct Bus(#[allow(dead_code)] Mutex<Option<zbus::blocking::Connection>>);

/// Dev-only: `WC_SELFTEST=<dir>` runs src/selftest.ts, which reports here.
#[tauri::command]
fn selftest(msg: Option<String>) -> bool {
    if let Some(m) = msg {
        println!("[selftest] {m}");
    }
    cfg!(debug_assertions) && std::env::var_os("WC_SELFTEST").is_some()
}

#[tauri::command]
fn selftest_dir() -> String {
    std::env::var("WC_SELFTEST").unwrap_or_default()
}

/// Dev-only REPL: returns (and removes) $WC_SELFTEST/cmd.js so a driver script can poke the running UI.
#[tauri::command]
fn selftest_cmd() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let p = PathBuf::from(std::env::var_os("WC_SELFTEST")?).join("cmd.js");
    let s = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    Some(s)
}

#[tauri::command]
fn selftest_suites() -> String {
    std::env::var("WC_SUITES").unwrap_or_default()
}

#[tauri::command]
fn places() -> Vec<Place> {
    let mut v = vec![];
    let dirs = [
        ("Home", dirs::home_dir()),
        ("Desktop", dirs::desktop_dir()),
        ("Documents", dirs::document_dir()),
        ("Downloads", dirs::download_dir()),
        ("Pictures", dirs::picture_dir()),
        ("Music", dirs::audio_dir()),
        ("Videos", dirs::video_dir()),
        ("Root", Some(PathBuf::from("/"))),
    ];
    for (name, p) in dirs {
        if let Some(p) = p.filter(|p| p.is_dir()) {
            v.push(Place { name, path: p.to_string_lossy().into_owned() });
        }
    }
    v
}

#[tauri::command]
fn open_path(path: String) -> R<()> {
    std::process::Command::new("xdg-open").arg(&path).spawn().map(drop).map_err(|e| e.to_string())
}

/// Replace this window's set of non-recursively watched folders (current folder + expanded ones).
#[tauri::command]
fn watch(paths: Vec<String>, window: tauri::Window, state: State<Watched>) {
    let mut g = state.0.lock().unwrap();
    g.2.insert(window.label().to_owned(), paths.into_iter().map(PathBuf::from).collect());
    sync_watches(&mut g);
}

fn sync_watches(g: &mut (RecommendedWatcher, HashSet<PathBuf>, HashMap<String, HashSet<PathBuf>>)) {
    let (w, cur, per) = g;
    let want: HashSet<PathBuf> = per.values().flatten().cloned().collect();
    for p in cur.difference(&want) {
        let _ = w.unwatch(p);
    }
    for p in want.difference(cur) {
        let _ = w.watch(p, RecursiveMode::NonRecursive);
    }
    *cur = want;
}

fn make_watcher(app: AppHandle) -> RecommendedWatcher {
    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if matches!(ev.kind, notify::EventKind::Access(_)) {
            return;
        }
        // Report the parent folders that changed; the frontend re-lists the ones it shows.
        let dirs: HashSet<String> = ev.paths.iter().filter_map(|p| p.parent()).map(|p| p.to_string_lossy().into_owned()).collect();
        let _ = app.emit("fs-change", dirs);
    })
    .expect("inotify watcher")
}

// ---------- settings + theme ----------

#[tauri::command]
fn get_settings(s: State<Settings>) -> Map<String, Value> {
    s.0.lock().unwrap().clone()
}

/// Merge `patch` into the settings, save, and broadcast them (and a new theme) to every window.
#[tauri::command]
fn set_settings(patch: Map<String, Value>, app: AppHandle, s: State<Settings>) -> R<()> {
    let all = {
        let mut g = s.0.lock().unwrap();
        g.extend(patch);
        settings::save(&g).map_err(|e| e.to_string())?;
        g.clone()
    };
    let _ = app.emit("settings", &all);
    emit_theme(&app);
    Ok(())
}

fn current_theme(app: &AppHandle) -> theme::Theme {
    let s = app.state::<Settings>().0.lock().unwrap().clone();
    let src = s.get("theme").and_then(Value::as_str).unwrap_or("dms").to_owned();
    let seed = s.get("customColor").and_then(Value::as_str).map(str::to_owned);
    let dark = s.get("customDark").and_then(Value::as_bool).unwrap_or(true);
    theme::load(&src, seed.as_deref(), dark)
}

#[tauri::command]
async fn get_theme(app: AppHandle) -> R<theme::Theme> {
    blocking(move || current_theme(&app)).await
}

fn emit_theme(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let t = current_theme(&app);
        let _ = app.emit("theme", t);
    });
}

/// Re-theme when DMS rewrites its colour files (wallpaper change, dark/light switch).
fn watch_theme_files(app: AppHandle) {
    let (tx, rx) = mpsc::channel::<()>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if !matches!(ev.kind, notify::EventKind::Access(_)) && ev.paths.iter().any(|p| theme::is_theme_file(p)) {
                let _ = tx.send(());
            }
        }
    })
    .expect("inotify watcher");
    for d in theme::watch_dirs() {
        let _ = w.watch(&d, RecursiveMode::NonRecursive);
    }
    std::thread::spawn(move || {
        let _keep = w;
        while rx.recv().is_ok() {
            // DMS writes several files in a burst; settle, then re-theme once.
            std::thread::sleep(Duration::from_millis(120));
            while rx.try_recv().is_ok() {}
            emit_theme(&app);
        }
    });
}

/// Re-read Hyprland options on `configreloaded` from socket2. Reconnects if Hyprland restarts.
fn watch_hyprland(app: AppHandle) {
    let Some(sock) = theme::hypr_socket2() else { return };
    std::thread::spawn(move || loop {
        if let Ok(s) = std::os::unix::net::UnixStream::connect(&sock) {
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { break };
                if line.starts_with("configreloaded>>") {
                    emit_theme(&app);
                }
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    });
}

// ---------- dialog windows ----------

static DIALOG_N: AtomicU32 = AtomicU32::new(0);

/// Show a window with its own app id / WM class (so Hyprland rules can match `whale-cabinet-dialog`). GTK takes
/// it from the program name when the toplevel is created, i.e. on the first show: set it just around that, so
/// the compositor sees the right class from the very first commit (setting it later races the float rules).
fn show_as(win: &tauri::WebviewWindow, id: &str) {
    use gtk::prelude::*;
    if let Ok(gw) = win.gtk_window() {
        let prev = gtk::glib::prgname();
        gtk::glib::set_prgname(Some(id));
        gw.show_all();
        gtk::glib::set_prgname(prev.as_deref());
    }
    let _ = win.show();
}

/// Open a small separate window (Properties, Open With, conflicts, settings) with class `whale-cabinet-dialog`.
#[tauri::command]
async fn open_dialog(app: AppHandle, kind: String, arg: String, title: String, width: f64, height: f64) -> R<String> {
    let label = format!("dlg-{}", DIALOG_N.fetch_add(1, Ordering::Relaxed));
    let url = format!("index.html?dialog={kind}&arg={}", url_arg(&arg));
    let (tx, rx) = mpsc::channel();
    let (l, a) = (label.clone(), app.clone());
    app.run_on_main_thread(move || {
        let title = if isolated() { format!("WCTEST {title}") } else { title };
        let r = WebviewWindowBuilder::new(&a, &l, WebviewUrl::App(url.into()))
            .title(title)
            .inner_size(width, height)
            .decorations(false)
            .transparent(true)
            .visible(false)
            .build()
            .map(|w| {
                show_as(&w, "whale-cabinet-dialog");
                let _ = w.set_focus();
            });
        let _ = tx.send(r.map_err(|e| e.to_string()));
    })
    .map_err(|e| e.to_string())?;
    blocking(move || rx.recv().map_err(|e| e.to_string())).await???;
    Ok(label)
}

fn url_arg(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

// ---------- browser windows ----------

static WIN_N: AtomicU32 = AtomicU32::new(1);
fn is_browser(label: &str) -> bool {
    label == "main" || label.starts_with("win-")
}

/// Open another browser window (same process: clipboard, jobs and tags stay shared) showing `targets`.
/// Wayland compositors map it on the workspace you're on.
fn new_window(app: &AppHandle, targets: Vec<listing::Target>) {
    open_browser(app, format!("targets={}", url_arg(&serde_json::to_string(&targets).unwrap_or_default())));
}

/// A browser window restored from a saved session (its tabs and split views).
#[tauri::command]
fn open_session_window(app: AppHandle, state: Value) {
    open_browser(&app, format!("session={}", url_arg(&state.to_string())));
}

#[tauri::command]
fn session_update(state: Value, window: tauri::Window, s: State<Sessions>) {
    let mut g = s.0.lock().unwrap();
    match g.iter_mut().find(|(l, _)| l == window.label()) {
        Some(e) => e.1 = state,
        None => g.push((window.label().to_owned(), state)),
    }
}

/// The session saved when the app last closed (and forget it: it's restored once, or declined).
#[tauri::command]
fn session_take() -> Option<Value> {
    let text = std::fs::read_to_string(session_file()).ok()?;
    let _ = std::fs::remove_file(session_file());
    serde_json::from_str(&text).ok()
}

fn open_browser(app: &AppHandle, query: String) {
    let label = format!("win-{}", WIN_N.fetch_add(1, Ordering::Relaxed));
    let url = format!("index.html?{query}");
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        let title = if isolated() { "WCTEST Whale Cabinet" } else { "Whale Cabinet" };
        match WebviewWindowBuilder::new(&a, &label, WebviewUrl::App(url.into())).title(title).inner_size(1200.0, 760.0).decorations(false).transparent(true).build() {
            Ok(w) => {
                let _ = w.set_focus();
            }
            Err(e) => eprintln!("whale-cabinet: couldn't open a window: {e}"),
        }
    });
}

/// Hand an open request (D-Bus "Show in folder") to the window used last, or open a window for it.
pub fn route_open(app: &AppHandle, req: fm1::OpenRequest) {
    let last = app.state::<LastWin>().0.lock().unwrap().clone();
    let w = app.get_webview_window(&last).or_else(|| app.webview_windows().into_values().find(|w| is_browser(w.label())));
    match w {
        Some(w) => {
            let _ = app.emit_to(w.label(), "open", req);
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        None => new_window(app, req.targets),
    }
}

#[tauri::command]
fn open_window(app: AppHandle, loc: String) {
    new_window(&app, vec![listing::Target { loc, select: None }]);
}

// ---------- quick open (Ctrl+P) ----------

#[tauri::command]
async fn fuzzy_find(q: String, limit: usize, kind: Option<String>, window: tauri::Window, app: AppHandle) -> R<fuzzy::Found> {
    let kind = kind.and_then(|k| k.chars().next()).unwrap_or(' ');
    let root = dirs::home_dir().unwrap_or_else(|| "/".into());
    blocking(move || {
        let label = window.label().to_owned();
        let a = app.clone();
        app.state::<std::sync::Arc<fuzzy::Fuzzy>>().find(root, &q, limit, kind, move || {
            let _ = a.emit_to(label.as_str(), "fuzzy-ready", ());
        })
    })
    .await
}

// ---------- git ----------

/// Status of the repository holding `dir`, or None when it isn't in one.
#[tauri::command]
async fn git_status(dir: String) -> R<Option<git::Info>> {
    blocking(move || git::status(Path::new(&dir))).await
}

// ---------- duplicates ----------

#[derive(Serialize, Clone)]
struct DupSet {
    size: u64,
    files: Vec<listing::Entry>,
}
#[derive(Serialize, Clone)]
struct DupHits {
    id: u32,
    sets: Vec<DupSet>,
    scanned: u64,
    done: bool,
}

/// Scan `root` for identical files; sets stream to the asking window as "dupes-hits".
#[tauri::command]
fn dupes_start(id: u32, root: String, min_size: u64, hidden: bool, window: tauri::Window, app: AppHandle, s: State<search::Searches>) {
    let cancel = s.register(id);
    let label = window.label().to_owned();
    std::thread::spawn(move || {
        let send = |sets: Vec<DupSet>, scanned: u64, done: bool| {
            let _ = app.emit_to(label.as_str(), "dupes-hits", DupHits { id, sets, scanned, done });
        };
        let (mut batch, mut last) = (vec![], std::time::Instant::now());
        dupes::find(
            Path::new(&root),
            min_size,
            hidden,
            &cancel,
            |n| send(vec![], n, false),
            |size, paths| {
                batch.push(DupSet { size, files: paths.iter().filter_map(|p| listing::entry(p)).collect() });
                if last.elapsed() > Duration::from_millis(150) {
                    send(std::mem::take(&mut batch), 0, false);
                    last = std::time::Instant::now();
                }
            },
        );
        send(batch, 0, true);
    });
}

/// Which of these paths still exist (virtual views drop the ones that are gone).
#[tauri::command]
fn paths_exist(paths: Vec<String>) -> Vec<bool> {
    paths.iter().map(|p| std::fs::symlink_metadata(p).is_ok()).collect()
}

// ---------- shelf: files collected from anywhere, shared by all windows, kept across restarts ----------

struct Shelf(Mutex<Vec<String>>);
fn shelf_file() -> PathBuf {
    settings::dir().join("shelf.json")
}
fn shelf_changed(app: &AppHandle, list: &[String]) {
    let _ = std::fs::create_dir_all(settings::dir());
    let _ = std::fs::write(shelf_file(), serde_json::to_string(list).unwrap_or_default());
    let _ = app.emit("shelf", list);
}
/// What's on the shelf (items that no longer exist drop off).
#[tauri::command]
fn shelf_get(app: AppHandle, s: State<Shelf>) -> Vec<String> {
    let mut g = s.0.lock().unwrap();
    let before = g.len();
    g.retain(|p| std::fs::symlink_metadata(p).is_ok());
    if g.len() != before {
        shelf_changed(&app, &g);
    }
    g.clone()
}
#[tauri::command]
fn shelf_add(paths: Vec<String>, app: AppHandle, s: State<Shelf>) {
    let mut g = s.0.lock().unwrap();
    for p in paths {
        if !g.contains(&p) {
            g.push(p);
        }
    }
    shelf_changed(&app, &g);
}
/// Take these off the shelf (None = everything). The files themselves aren't touched.
#[tauri::command]
fn shelf_remove(paths: Option<Vec<String>>, app: AppHandle, s: State<Shelf>) {
    let mut g = s.0.lock().unwrap();
    match paths {
        Some(ps) => g.retain(|p| !ps.contains(p)),
        None => g.clear(),
    }
    shelf_changed(&app, &g);
}

// ---------- disk usage ----------

#[derive(Serialize, Clone)]
struct UsageTick {
    id: u32,
    files: u64,
    bytes: u64,
    done: bool,
    error: Option<String>,
}

/// Scan `root` in the background; progress and completion go to the asking window as "usage".
#[tauri::command]
fn usage_scan(id: u32, root: String, window: tauri::Window, app: AppHandle, s: State<search::Searches>) {
    let cancel = s.register(id);
    let label = window.label().to_owned();
    std::thread::spawn(move || {
        let send = |files, bytes, done, error: Option<String>| {
            let _ = app.emit_to(label.as_str(), "usage", UsageTick { id, files, bytes, done, error });
        };
        let mut last = std::time::Instant::now();
        let tree = usage::scan(Path::new(&root), &cancel, &mut |f, b| {
            if last.elapsed() > Duration::from_millis(200) {
                last = std::time::Instant::now();
                send(f, b, false, None);
            }
        });
        match tree {
            Some(t) => {
                let (f, b) = (t.files, t.size);
                app.state::<usage::Scans>().lock().unwrap().insert(id, (PathBuf::from(&root), t));
                send(f, b, true, None);
            }
            None if cancel.load(std::sync::atomic::Ordering::Relaxed) => {}
            None => send(0, 0, true, Some(format!("can't read {root}"))),
        }
    });
}

/// A pruned view of a finished scan at `path` (inside its root).
#[tauri::command]
fn usage_view(id: u32, path: String, depth: u32, s: State<usage::Scans>) -> R<usage::View> {
    let g = s.lock().unwrap();
    let (root, tree) = g.get(&id).ok_or("scan not found")?;
    let p = Path::new(&path);
    let rel = usage::rel(root, p).ok_or("outside the scanned folder")?;
    let node = tree.find(&rel).ok_or("not in the scan (deleted?)")?;
    Ok(node.view(p, depth, 0.004, 60))
}

/// Forget a finished scan, or take trashed paths out of it.
#[tauri::command]
fn usage_drop(id: u32, paths: Option<Vec<String>>, s: State<usage::Scans>) {
    let mut g = s.lock().unwrap();
    match paths {
        None => {
            g.remove(&id);
        }
        Some(ps) => {
            if let Some((root, tree)) = g.get_mut(&id) {
                for p in &ps {
                    if let Some(rel) = usage::rel(root, Path::new(p)) {
                        tree.remove(&rel);
                    }
                }
            }
        }
    }
}

// ---------- git actions, encryption ----------

#[tauri::command]
async fn git_act(root: String, action: String, paths: Vec<String>) -> R<()> {
    blocking(move || git::act(Path::new(&root), &action, &paths)).await?
}
#[tauri::command]
async fn git_diff(root: String, path: String, untracked: bool) -> R<String> {
    blocking(move || git::diff(Path::new(&root), &path, untracked)).await?
}
/// Compare a file with its last commit in an external tool (meld, kdiff3…) via git difftool.
#[tauri::command]
fn git_difftool(root: String, path: String) -> R<()> {
    let tool = ["meld", "kdiff3", "kompare"].into_iter().find(|t| desktop::which(t).is_some()).ok_or("no diff tool installed (meld, kdiff3)")?;
    desktop::spawn_detached(&["git".into(), "-C".into(), root.clone(), "difftool".into(), "-y".into(), format!("--tool={tool}"), "HEAD".into(), "--".into(), path], Path::new(&root))
}
#[tauri::command]
fn has_difftool() -> bool {
    ["meld", "kdiff3", "kompare"].into_iter().any(|t| desktop::which(t).is_some())
}

#[tauri::command]
async fn gpg_keys() -> R<Vec<crypt::Key>> {
    blocking(crypt::keys).await
}
#[tauri::command]
async fn encrypt_item(path: String, password: Option<String>, recipients: Vec<String>) -> R<String> {
    blocking(move || crypt::encrypt(Path::new(&path), password.as_deref(), &recipients).map(|p| p.to_string_lossy().into_owned())).await?
}
#[tauri::command]
async fn is_symmetric(path: String) -> R<bool> {
    blocking(move || crypt::is_symmetric(Path::new(&path))).await
}
#[tauri::command]
async fn decrypt_item(path: String, password: Option<String>) -> R<String> {
    blocking(move || crypt::decrypt(Path::new(&path), password.as_deref()).map(|p| p.to_string_lossy().into_owned())).await?
}

// ---------- zoxide ----------

#[tauri::command]
fn zoxide_add(path: String) {
    zoxide::add(&path);
}
#[tauri::command]
async fn zoxide_query(q: String, limit: usize) -> R<Vec<(f64, String)>> {
    blocking(move || zoxide::query(&q.split_whitespace().map(str::to_owned).collect::<Vec<_>>(), limit)).await?
}

// ---------- previews ----------

#[tauri::command]
async fn thumbnail(path: String, size: u32) -> R<String> {
    blocking(move || preview::thumbnail(&preview::cache_root(), Path::new(&path), size).map(|p| p.to_string_lossy().into_owned())).await?
}

/// Number of entries in a folder (hidden ones only when asked), for the "12 items" column.
#[tauri::command]
async fn dir_count(path: String, hidden: bool) -> R<u32> {
    blocking(move || {
        std::fs::read_dir(&path)
            .map(|rd| rd.flatten().filter(|e| hidden || !e.file_name().to_string_lossy().starts_with('.')).count() as u32)
            .map_err(|e| e.to_string())
    })
    .await?
}

#[tauri::command]
async fn read_text(path: String, max: usize) -> R<preview::Text> {
    blocking(move || preview::read_text(Path::new(&path), max)).await?
}

#[tauri::command]
async fn dir_stats(path: String) -> R<preview::DirStats> {
    // NOTE: not cancellable; a huge tree just finishes in the background.
    blocking(move || preview::dir_stats(Path::new(&path))).await
}

// ---------- open with / apps / terminal ----------

fn term_pref(app: &AppHandle) -> String {
    app.state::<Settings>().0.lock().unwrap().get("terminal").and_then(Value::as_str).unwrap_or("").to_owned()
}
fn icon_theme() -> String {
    theme::icon_theme()
}
fn choice(a: &desktop::App, default: bool, theme: &str) -> desktop::AppChoice {
    desktop::AppChoice { id: a.id.clone(), name: a.name.clone(), icon: desktop::find_icon(&a.icon, theme).map(|p| p.to_string_lossy().into_owned()), default }
}

/// Apps registered for this file's type, default first.
#[tauri::command]
async fn apps_for(path: String) -> R<(String, Vec<desktop::AppChoice>)> {
    blocking(move || {
        let mime = desktop::mime_of(Path::new(&path));
        let apps = desktop::all_apps();
        let ids = desktop::apps_for_mime(&mime, &apps, &desktop::load_mimeapps());
        let theme = icon_theme();
        let list = ids.iter().enumerate().filter_map(|(i, id)| apps.get(id).map(|a| choice(a, i == 0, &theme))).collect();
        (mime, list)
    })
    .await
}

#[tauri::command]
async fn all_apps() -> R<Vec<desktop::AppChoice>> {
    blocking(|| {
        let theme = icon_theme();
        let mut v: Vec<_> = desktop::all_apps().values().filter(|a| !a.no_display).map(|a| choice(a, false, &theme)).collect();
        v.sort_by_key(|a| a.name.to_lowercase());
        v
    })
    .await
}

#[tauri::command]
async fn launch_app(id: String, paths: Vec<String>, app: AppHandle) -> R<()> {
    let term = term_pref(&app);
    blocking(move || {
        let apps = desktop::all_apps();
        let a = apps.get(&id).ok_or(format!("{id} not found"))?;
        desktop::launch(a, &paths, &term)?;
        remember(&app, a, &paths);
        Ok(())
    })
    .await?
}

/// Add opened files to the shared recent list (like GTK apps do), then tell the windows.
fn remember(app: &AppHandle, a: &desktop::App, paths: &[String]) {
    let files: Vec<&String> = paths.iter().filter(|p| Path::new(p).is_file()).collect();
    if files.is_empty() {
        return;
    }
    let mut text = recent::read();
    for p in files {
        text = recent::record(&text, p, &desktop::mime_of(Path::new(p)), &a.name, &a.exec, recent::now_ms());
    }
    if recent::write(&text).is_ok() {
        let _ = app.emit("recent-changed", ());
    }
}

#[derive(Serialize)]
struct RecentEntry {
    #[serde(flatten)]
    entry: listing::Entry,
    used: i64,
    app: String,
}

/// Recently used files that still exist, newest first.
#[tauri::command]
async fn recent_list(limit: usize) -> R<Vec<RecentEntry>> {
    blocking(move || {
        recent::parse(&recent::read())
            .into_iter()
            .filter_map(|it| listing::entry(Path::new(&it.path)).filter(|e| !e.dir).map(|entry| RecentEntry { entry, used: it.used, app: it.app }))
            .take(limit)
            .collect()
    })
    .await
}

/// Forget some (or, with None, all) recent files.
#[tauri::command]
async fn recent_remove(paths: Option<Vec<String>>, app: AppHandle) -> R<()> {
    blocking(move || {
        recent::write(&recent::remove(&recent::read(), paths.as_deref()))?;
        let _ = app.emit("recent-changed", ());
        Ok(())
    })
    .await?
}

/// Open files with their default apps (grouped per app), falling back to xdg-open.
#[tauri::command]
async fn open_default(paths: Vec<String>, app: AppHandle) -> R<()> {
    let term = term_pref(&app);
    let app = app.clone();
    blocking(move || {
        let apps = desktop::all_apps();
        let ma = desktop::load_mimeapps();
        let mut groups: Vec<(String, Vec<String>)> = vec![];
        for p in paths {
            let mime = desktop::mime_of(Path::new(&p));
            match desktop::apps_for_mime(&mime, &apps, &ma).into_iter().next() {
                Some(id) => match groups.iter_mut().find(|g| g.0 == id) {
                    Some(g) => g.1.push(p),
                    None => groups.push((id, vec![p])),
                },
                None => desktop::spawn_detached(&["xdg-open".into(), p.clone()], Path::new("/"))?,
            }
        }
        for (id, files) in groups {
            desktop::launch(&apps[&id], &files, &term)?;
            remember(&app, &apps[&id], &files);
        }
        Ok(())
    })
    .await?
}

#[tauri::command]
fn set_default_app(mime: String, id: String) -> R<()> {
    desktop::set_default(&mime, &id)
}

#[tauri::command]
async fn mime_icon(path: String) -> R<Option<String>> {
    blocking(move || {
        let mime = desktop::mime_of(Path::new(&path));
        let (own, generic) = desktop::mime_icon_name(&mime);
        let theme = icon_theme();
        desktop::find_icon(&own, &theme).or_else(|| desktop::find_icon(&generic, &theme)).map(|p| p.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
fn open_terminal(dir: String, app: AppHandle) -> R<()> {
    let t = desktop::terminal(&term_pref(&app)).ok_or("no terminal emulator found (set one in Settings)")?;
    desktop::spawn_detached(&desktop::terminal_argv(&t, &dir, &[]), Path::new(&dir))
}

// ---------- administrator ----------

/// Conflicts can't happen for single-item requests; answer "cancel" if one ever does.
struct NoAsk;
impl ops::Report for NoAsk {
    fn progress(&self, _: u64, _: u64, _: &Path) {}
    fn conflict(&self, _: &Path, _: &Path) -> (ops::Choice, bool) {
        (ops::Choice::Cancel, false)
    }
    fn cancelled(&self) -> bool {
        false
    }
}

/// Can we write into this folder (access(W_OK))? Decides whether New Folder needs the administrator route.
#[tauri::command]
fn can_write(path: String) -> bool {
    std::ffi::CString::new(path).map(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0).unwrap_or(false)
}

/// Quick admin requests (rename, new folder/file): returns the resulting path.
#[tauri::command]
async fn admin_run(req: admin::Request) -> R<Option<String>> {
    blocking(move || {
        let (out, result) = admin::run(&req, &NoAsk)?;
        match out.errors.into_iter().next() {
            Some(e) => Err(e),
            None => Ok(result),
        }
    })
    .await?
}

fn default_app(path: &Path) -> R<desktop::App> {
    let apps = desktop::all_apps();
    let mime = desktop::mime_of(path);
    let id = desktop::apps_for_mime(&mime, &apps, &desktop::load_mimeapps()).into_iter().next().ok_or(format!("no app for {mime}"))?;
    apps.get(&id).cloned().ok_or(format!("{id} not found"))
}

/// Run the file's default app as root (pkexec env …). Apps that refuse root (Kate, VS Code…) need
/// Edit as Administrator instead.
#[tauri::command]
async fn open_as_admin(path: String) -> R<()> {
    blocking(move || {
        let a = default_app(Path::new(&path))?;
        let argv = desktop::expand_exec(&a, &[path.clone()], jobs::to_uri).into_iter().next().ok_or("empty Exec")?;
        desktop::spawn_detached(&admin::open_argv(&argv), Path::new("/"))
    })
    .await?
}

/// Edit a copy with your normal app; every save is written back as root (password asked each time).
#[tauri::command]
async fn edit_as_admin(path: String, app: AppHandle) -> R<()> {
    let term = term_pref(&app);
    blocking(move || {
        let orig = PathBuf::from(&path);
        let copy = admin::edit_copy_path(&orig);
        std::fs::create_dir_all(copy.parent().unwrap()).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&copy);
        if std::fs::copy(&orig, &copy).is_err() {
            // not even readable by us: let root hand us a copy
            let (out, _) = admin::run(&admin::Request::Read { src: path.clone(), dst: copy.to_string_lossy().into_owned() }, &NoAsk)?;
            if let Some(e) = out.errors.into_iter().next() {
                return Err(e);
            }
        }
        let a = default_app(&orig)?;
        desktop::launch(&a, &[copy.to_string_lossy().into_owned()], &term)?;
        let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let mut seen = mtime(&copy);
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            // NOTE: polls the copy's mtime every second for up to a day; inotify if this ever matters
            while started.elapsed() < Duration::from_secs(86400) {
                std::thread::sleep(Duration::from_secs(1));
                let Some(now) = mtime(&copy) else { break }; // copy deleted: done
                if Some(now) == seen {
                    continue;
                }
                seen = Some(now);
                let req = admin::Request::Write { src: copy.to_string_lossy().into_owned(), dst: path.clone() };
                let err = match admin::run(&req, &NoAsk) {
                    Ok((out, _)) => out.errors.into_iter().next(),
                    Err(e) => Some(e),
                };
                let _ = app.emit("admin-saved", (path.clone(), err));
            }
        });
        Ok(())
    })
    .await?
}

// ---------- properties ----------

#[tauri::command]
async fn file_props(path: String) -> R<props::Props> {
    blocking(move || props::props(Path::new(&path))).await?
}
#[tauri::command]
async fn set_mode(path: String, mode: u32, recursive: bool) -> R<Vec<String>> {
    blocking(move || props::set_mode(Path::new(&path), mode, recursive)).await
}
#[tauri::command]
async fn file_details(path: String) -> R<std::collections::BTreeMap<String, String>> {
    blocking(move || props::details(Path::new(&path))).await
}
#[tauri::command]
async fn checksum(path: String, algo: String) -> R<String> {
    blocking(move || props::checksum(Path::new(&path), &algo)).await?
}

// ---------- places (user-places.xbel) ----------

#[tauri::command]
fn places_list() -> Vec<places::Place> {
    let text = places::read();
    if text.trim().is_empty() {
        // No xbel yet: offer the usual folders (the file is created on the first edit).
        return places().into_iter().map(|p| places::Place { href: jobs::to_uri(&p.path), title: p.name.into(), path: p.path, icon: String::new(), hidden: false, system: true }).collect();
    }
    places::parse(&text)
}
fn edit_places(f: impl FnOnce(String) -> String) -> R<()> {
    let text = places::read();
    let text = if text.trim().is_empty() {
        // seed from the defaults so the first edit doesn't lose them
        places().iter().fold(places::EMPTY.to_owned(), |t, p| places::add(&t, &p.path, p.name, None))
    } else {
        text
    };
    places::write(&f(text))
}
#[tauri::command]
fn places_add(path: String, title: String, index: Option<usize>) -> R<()> {
    edit_places(|t| places::add(&t, &path, &title, index))
}
#[tauri::command]
fn places_remove(href: String) -> R<()> {
    edit_places(|t| places::remove(&t, &href))
}
#[tauri::command]
fn places_move(href: String, to: usize) -> R<()> {
    edit_places(|t| places::reorder(&t, &href, to))
}
#[tauri::command]
fn places_rename(href: String, title: String) -> R<()> {
    edit_places(|t| places::rename(&t, &href, &title))
}

/// Tell the UI when Dolphin (or anyone) edits user-places.xbel.
fn watch_places(app: AppHandle) {
    let target = places::xbel_path();
    let Some(dir) = target.parent().map(Path::to_path_buf) else { return };
    let (tx, rx) = mpsc::channel::<()>();
    let t2 = target.clone();
    let Ok(mut w) = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if !matches!(ev.kind, notify::EventKind::Access(_)) && ev.paths.iter().any(|p| p == &t2) {
                let _ = tx.send(());
            }
        }
    }) else { return };
    let _ = w.watch(&dir, RecursiveMode::NonRecursive);
    std::thread::spawn(move || {
        let _keep = w;
        while rx.recv().is_ok() {
            std::thread::sleep(Duration::from_millis(100));
            while rx.try_recv().is_ok() {}
            let _ = app.emit("places-changed", ());
        }
    });
}

// ---------- network (gvfs) ----------

#[derive(Serialize)]
struct NetState {
    saved: Vec<places::Place>,
    mounted: Vec<network::Mounted>,
    ssh_hosts: Vec<String>,
    gio: bool,
    keyring: bool,
}

#[tauri::command]
async fn net_state() -> R<NetState> {
    blocking(|| NetState {
        saved: places::parse(&places::read()).into_iter().filter(|p| network::is_remote(&p.href) && !p.hidden).collect(),
        mounted: network::mounted(),
        ssh_hosts: network::ssh_hosts(),
        gio: desktop::which("gio").is_some(),
        keyring: desktop::which("secret-tool").is_some(),
    })
    .await
}

/// Mount a network location and return the folder to browse. A missing password is taken from the keyring;
/// with `remember`, a password that worked is stored there.
#[tauri::command]
async fn net_mount(uri: String, user: String, domain: String, password: String, trust: bool, remember: bool) -> R<String> {
    blocking(move || {
        let password = if password.is_empty() { network::saved_password(&uri).unwrap_or_default() } else { password };
        let path = network::mount(&uri, &network::Creds { user, domain, password: password.clone(), trust })?;
        if remember && !password.is_empty() {
            network::save_password(&uri, &password)?;
        }
        Ok(path)
    })
    .await?
}
#[tauri::command]
async fn net_unmount(uri: String) -> R<()> {
    blocking(move || network::unmount(&uri)).await?
}
#[tauri::command]
async fn net_browse(uri: String) -> R<Vec<network::Found>> {
    blocking(move || network::browse(&uri)).await?
}
#[tauri::command]
fn net_save(uri: String, title: String) -> R<()> {
    edit_places(|t| places::add_href(&t, &uri, &title, None, "folder-remote"))
}
#[tauri::command]
fn net_forget(uri: String) -> R<()> {
    network::forget_password(&uri);
    edit_places(|t| places::remove(&t, &uri))
}

// ---------- devices ----------

#[tauri::command]
async fn devices_list() -> R<Vec<mounts::Device>> {
    blocking(mounts::list).await
}
#[tauri::command]
async fn device_mount(id: String) -> R<String> {
    blocking(move || mounts::mount(&id)).await?
}
#[tauri::command]
async fn device_unmount(dev: mounts::Device) -> R<()> {
    blocking(move || mounts::unmount(&dev)).await?
}
#[tauri::command]
async fn device_eject(dev: mounts::Device) -> R<()> {
    blocking(move || mounts::eject(&dev)).await?
}
/// NOTE: polls lsblk/mountinfo every 2 s; switch to a udev/UDisks2 D-Bus signal watch if that ever shows up in profiles.
fn watch_devices(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last = mounts::list();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = mounts::list();
            if now != last {
                let _ = app.emit("devices", &now);
                last = now;
            }
        }
    });
}

/// PNG used as the cursor image when dragging files out to other apps.
#[tauri::command]
fn drag_icon() -> String {
    let p = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("whale-cabinet/drag.png");
    if !p.exists() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(&p, include_bytes!("../icons/32x32.png"));
    }
    p.to_string_lossy().into_owned()
}

/// Dev test runs (WC_SELFTEST) stay isolated: no single-instance hand-off, no D-Bus name, so they never touch
/// a Whale Cabinet the user is running.
fn isolated() -> bool {
    cfg!(debug_assertions) && std::env::var_os("WC_SELFTEST").is_some()
}

fn main() {
    // `pkexec whale-cabinet --admin-helper`: do one file operation as root and exit, never start the UI
    if std::env::args().nth(1).as_deref() == Some("--admin-helper") {
        std::process::exit(admin::helper_main());
    }
    let mut builder = tauri::Builder::default();
    if !isolated() {
        // Must be first: a second `whale-cabinet …` hands its arguments to this process and exits.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let mut targets = listing::parse_args(argv.get(1..).unwrap_or(&[]), Path::new(&cwd));
            if targets.is_empty() {
                targets.push(home_target());
            }
            new_window(app, targets);
        }));
    }
    builder
        .plugin(tauri_plugin_drag::init())
        .on_window_event(|w, ev| match ev {
            tauri::WindowEvent::Focused(true) if is_browser(w.label()) => *w.state::<LastWin>().0.lock().unwrap() = w.label().to_owned(),
            tauri::WindowEvent::Destroyed => {
                if is_browser(w.label()) {
                    // the last browser window closing = the app closing: keep every window's tabs for next time
                    let others = w.app_handle().webview_windows().keys().any(|l| is_browser(l) && l != w.label());
                    let st = w.state::<Sessions>();
                    let mut g = st.0.lock().unwrap();
                    if !others && !g.is_empty() {
                        let all: Vec<&Value> = g.iter().map(|(_, v)| v).collect();
                        let _ = std::fs::create_dir_all(settings::dir());
                        let _ = std::fs::write(session_file(), serde_json::to_string(&all).unwrap_or_default());
                    }
                    g.retain(|(l, _)| l != w.label());
                }
                let st = w.state::<Watched>();
                let mut g = st.0.lock().unwrap();
                if g.2.remove(w.label()).is_some() {
                    sync_watches(&mut g);
                }
            }
            _ => {}
        })
        .manage(jobs::Jobs::default())
        .manage(jobs::Clip::default())
        .manage(term::Terms::default())
        .manage(search::Searches::default())
        .manage(std::sync::Arc::new(fuzzy::Fuzzy::default()))
        .manage(usage::Scans::default())
        .manage(plugins::Plugins::default())
        .register_asynchronous_uri_scheme_protocol("wcplugin", |ctx, req, responder| {
            let (app, uri) = (ctx.app_handle().clone(), req.uri().to_string());
            std::thread::spawn(move || responder.respond(plugins::protocol(&app, &uri)));
        })
        .setup(|app| {
            let h = app.handle().clone();
            app.manage(Watched(Mutex::new((make_watcher(h.clone()), HashSet::new(), HashMap::new()))));
            app.manage(LastWin(Mutex::new("main".into())));
            app.manage(Sessions(Mutex::new(vec![])));
            let saved: Vec<String> = std::fs::read_to_string(shelf_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            app.manage(Shelf(Mutex::new(saved)));
            app.manage(Settings(Mutex::new(settings::load())));
            app.manage(tags::Store::new(settings::dir()));
            // Seed the tag index from the home folder in the background (Dolphin-set tags included).
            let hs = h.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                if let Some(home) = dirs::home_dir() {
                    hs.state::<tags::Store>().scan(&home, 12);
                    let _ = hs.emit("tags-changed", ());
                }
            });
            watch_theme_files(h.clone());
            // "Show in folder" from other apps (org.freedesktop.FileManager1)
            let bus = match if isolated() { Err("isolated test run".into()) } else { fm1::serve(h.clone()) } {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!("whale-cabinet: FileManager1 D-Bus service unavailable: {e}");
                    None
                }
            };
            app.manage(Bus(Mutex::new(bus)));
            watch_places(h.clone());
            watch_devices(h.clone());
            watch_hyprland(h);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            plugins::plugins_list, plugins::plugin_consent, plugins::plugin_start, plugins::plugin_call, plugins::plugin_settings, plugins::plugin_set, plugins::mime_types,
            start_args, open_window, git_act, git_diff, git_difftool, has_difftool, gpg_keys, encrypt_item, is_symmetric, decrypt_item, usage_scan, usage_view, usage_drop, shelf_get, shelf_add, shelf_remove, open_session_window, session_update, session_take, dupes_start, paths_exist, can_write, admin_run, open_as_admin, edit_as_admin, jobs::op_admin, net_state, net_mount, net_unmount, net_browse, net_save, net_forget, recent_list, recent_remove, fuzzy_find, git_status, zoxide_add, zoxide_query, selftest, selftest_dir, selftest_suites, selftest_cmd, list_dir, resolve_path, disk_space, places, open_path, watch,
            get_settings, set_settings, get_theme, open_dialog, drag_icon, thumbnail, dir_count, read_text, dir_stats, tags_edit, tag_meta, set_rating, tag_counts, tag_items,
            apps_for, all_apps, launch_app, open_default, set_default_app, mime_icon, open_terminal, file_props, set_mode, file_details, checksum,
            jobs::op_compress, jobs::op_extract, jobs::archive_tools, jobs::copy_text,
            places_list, places_add, places_remove, places_move, places_rename, devices_list, device_mount, device_unmount, device_eject,
            term::term_open, term::term_write, term::term_resize, term::term_cd, term::term_close, search::search_start, search::search_cancel,
            jobs::op_transfer, jobs::op_delete, jobs::op_trash, jobs::op_undo, jobs::op_cancel, jobs::op_resolve,
            jobs::rename_item, jobs::make_item, jobs::unique_name, jobs::trash_list, jobs::trash_restore, jobs::trash_purge,
            jobs::clip_set, jobs::clip_get
        ])
        .run({
            let mut ctx = tauri::generate_context!();
            if isolated() {
                // Test windows are titled "WCTEST…" before they map, so a title rule can route them to a
                // throwaway monitor without ever matching the user's own Whale Cabinet windows.
                for w in &mut ctx.config_mut().app.windows {
                    w.title = "WCTEST Whale Cabinet".into();
                }
            }
            ctx
        })
        .expect("error while running Whale Cabinet");
}
