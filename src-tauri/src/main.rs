#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod desktop;
mod fm1;
mod jobs;
mod listing;
mod mounts;
mod ops;
mod places;
mod preview;
mod props;
mod search;
mod settings;
mod tags;
mod term;
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
        let title = if isolated() { format!("WCTEST {title}") } else { title };
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
        desktop::launch(a, &paths, &term)
    })
    .await?
}

/// Open files with their default apps (grouped per app), falling back to xdg-open.
#[tauri::command]
async fn open_default(paths: Vec<String>, app: AppHandle) -> R<()> {
    let term = term_pref(&app);
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
    let mut builder = tauri::Builder::default();
    if !isolated() {
        // Must be first: a second `whale-cabinet …` hands its arguments to this process and exits.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let mut targets = listing::parse_args(argv.get(1..).unwrap_or(&[]), Path::new(&cwd));
            if targets.is_empty() {
                targets.push(home_target());
            }
            let _ = app.emit("open", fm1::OpenRequest { targets, properties: vec![] });
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }));
    }
    builder
        .plugin(tauri_plugin_drag::init())
        .manage(jobs::Jobs::default())
        .manage(jobs::Clip::default())
        .manage(term::Terms::default())
        .manage(search::Searches::default())
        .setup(|app| {
            let h = app.handle().clone();
            app.manage(Watched(Mutex::new((make_watcher(h.clone()), HashSet::new()))));
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
            start_args, selftest, selftest_dir, selftest_suites, selftest_cmd, list_dir, resolve_path, disk_space, places, open_path, watch,
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
