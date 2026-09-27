#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod jobs;
mod listing;
mod ops;
mod settings;
mod theme;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;
use tauri::{async_runtime::spawn_blocking, AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

struct Watched(Mutex<(RecommendedWatcher, HashSet<PathBuf>)>);
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
async fn list_dir(path: String) -> R<Vec<listing::Entry>> {
    blocking(move || listing::list_dir(Path::new(&path)).map_err(|e| format!("{path}: {e}"))).await?
}

#[tauri::command]
async fn resolve_path(input: String, cwd: String) -> R<String> {
    blocking(move || listing::resolve(&input, Path::new(&cwd)).map(|p| p.to_string_lossy().into_owned())).await?
}

#[tauri::command]
async fn disk_space(path: String) -> Option<(u64, u64)> {
    blocking(move || listing::disk_space(Path::new(&path))).await.ok().flatten()
}

/// Folder to open at startup: first CLI argument (path or file:// URI), else home.
#[tauri::command]
fn start_path() -> String {
    let home = dirs::home_dir().unwrap_or_else(|| "/".into());
    std::env::args()
        .nth(1)
        .and_then(|a| listing::resolve(&a, &std::env::current_dir().unwrap_or(home.clone())).ok())
        .unwrap_or(home)
        .to_string_lossy()
        .into_owned()
}

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

/// Replace the set of non-recursively watched folders (current folder + expanded ones).
#[tauri::command]
fn watch(paths: Vec<String>, state: State<Watched>) {
    let mut g = state.0.lock().unwrap();
    let (w, cur) = &mut *g;
    let want: HashSet<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
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

/// Give a window its own Wayland app id (so Hyprland rules can match `whale-cabinet-dialog`). Must run before it maps.
fn set_app_id(win: &tauri::WebviewWindow, id: &str) {
    use gtk::prelude::*;
    extern "C" {
        fn gdk_wayland_window_set_application_id(w: *mut gtk::gdk::ffi::GdkWindow, id: *const std::os::raw::c_char);
    }
    let Ok(gw) = win.gtk_window() else { return };
    gw.realize();
    let is_wayland = gtk::gdk::Display::default().map(|d| d.type_().name() == "GdkWaylandDisplay").unwrap_or(false);
    if let (true, Some(gdk_win)) = (is_wayland, gw.window()) {
        use gtk::glib::translate::ToGlibPtr;
        let c = std::ffi::CString::new(id).unwrap();
        unsafe { gdk_wayland_window_set_application_id(gdk_win.to_glib_none().0, c.as_ptr()) };
    }
    // NOTE: on X11 dialogs keep WM class whale-cabinet; add XSetClassHint if an X11 user needs rules for them.
}

/// Open a small separate window (Properties, Open With, conflicts, settings) with class `whale-cabinet-dialog`.
#[tauri::command]
async fn open_dialog(app: AppHandle, kind: String, arg: String, title: String, width: f64, height: f64) -> R<String> {
    let label = format!("dlg-{}", DIALOG_N.fetch_add(1, Ordering::Relaxed));
    let enc: String = arg.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let url = format!("index.html?dialog={kind}&arg={enc}");
    let (tx, rx) = mpsc::channel();
    let (l, a) = (label.clone(), app.clone());
    app.run_on_main_thread(move || {
        let r = WebviewWindowBuilder::new(&a, &l, WebviewUrl::App(url.into()))
            .title(title)
            .inner_size(width, height)
            .decorations(false)
            .transparent(true)
            .visible(false)
            .build()
            .map(|w| {
                set_app_id(&w, "whale-cabinet-dialog");
                let _ = w.show();
                let _ = w.set_focus();
            });
        let _ = tx.send(r.map_err(|e| e.to_string()));
    })
    .map_err(|e| e.to_string())?;
    blocking(move || rx.recv().map_err(|e| e.to_string())).await???;
    Ok(label)
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_drag::init())
        .manage(jobs::Jobs::default())
        .manage(jobs::Clip::default())
        .setup(|app| {
            let h = app.handle().clone();
            app.manage(Watched(Mutex::new((make_watcher(h.clone()), HashSet::new()))));
            app.manage(Settings(Mutex::new(settings::load())));
            watch_theme_files(h.clone());
            watch_hyprland(h);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_path, selftest, selftest_dir, selftest_suites, list_dir, resolve_path, disk_space, places, open_path, watch,
            get_settings, set_settings, get_theme, open_dialog, drag_icon,
            jobs::op_transfer, jobs::op_delete, jobs::op_trash, jobs::op_undo, jobs::op_cancel, jobs::op_resolve,
            jobs::rename_item, jobs::make_item, jobs::unique_name, jobs::trash_list, jobs::trash_restore, jobs::trash_purge,
            jobs::clip_set, jobs::clip_get
        ])
        .run(tauri::generate_context!())
        .expect("error while running Whale Cabinet");
}
