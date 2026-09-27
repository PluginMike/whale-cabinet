#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod listing;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{async_runtime::spawn_blocking, AppHandle, Emitter, Manager, State};

struct Watched(Mutex<(RecommendedWatcher, HashSet<PathBuf>)>);

#[derive(Serialize)]
struct Place {
    name: &'static str,
    path: String,
}

#[tauri::command]
async fn list_dir(path: String) -> Result<Vec<listing::Entry>, String> {
    spawn_blocking(move || listing::list_dir(Path::new(&path)).map_err(|e| format!("{path}: {e}")))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn resolve_path(input: String, cwd: String) -> Result<String, String> {
    spawn_blocking(move || listing::resolve(&input, Path::new(&cwd)).map(|p| p.to_string_lossy().into_owned()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn disk_space(path: String) -> Option<(u64, u64)> {
    spawn_blocking(move || listing::disk_space(Path::new(&path))).await.ok().flatten()
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

/// Dev-only: `WC_SELFTEST=1` runs src/selftest.ts, which reports here.
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
fn open_path(path: String) -> Result<(), String> {
    // NOTE: xdg-open until Milestone 6 brings real Open With / mimeapps handling.
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
        let dirs: HashSet<String> = ev
            .paths
            .iter()
            .filter_map(|p| p.parent())
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let _ = app.emit("fs-change", dirs);
    })
    .expect("inotify watcher")
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let w = make_watcher(app.handle().clone());
            app.manage(Watched(Mutex::new((w, HashSet::new()))));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_path, selftest, selftest_dir, list_dir, resolve_path, disk_space, places, open_path, watch])
        .run(tauri::generate_context!())
        .expect("error while running Whale Cabinet");
}
