//! Plugins: folders in ~/.local/share/whale-cabinet/plugins/<name>/ holding a plugin.json manifest and an
//! executable (any language). Each runs as the user, once you've allowed it, as one long-lived process that
//! speaks JSON lines on stdin/stdout:
//!   → {"id": 1, "method": "list", "params": {…}}
//!   ← {"id": 1, "result": …} or {"id": 1, "error": "message"}   (answers may come in any order)
//!   ← {"event": "badges", …}                                     (unasked: broadcast to the windows)
//! The first request is always "init" (settings, secrets from the keyring, a cache folder); its result
//! (badge roots, sidebar entries…) is kept for the UI. The UI maps the rest onto its own hooks.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Setting {
    pub key: String,
    pub label: String,
    /// Kept in the keyring (secret-tool), never in settings.json.
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub placeholder: String,
}

/// A right-click action. Filters: mime globs ("image/*"), extensions, only under the plugin's roots
/// (from init), only on the plugin's own virtual items, several items at once.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Action {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub mime: Vec<String>,
    #[serde(default)]
    pub ext: Vec<String>,
    #[serde(default)]
    pub roots: bool,
    #[serde(default)]
    pub own: bool,
    #[serde(default)]
    pub multi: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Manifest {
    /// = the folder name
    #[serde(default)]
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// relative to the plugin's folder
    pub exec: String,
    #[serde(default)]
    pub settings: Vec<Setting>,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// status badges for paths under the roots init returns
    #[serde(default)]
    pub badges: bool,
    /// virtual locations "<scheme>:…" (list, thumb, open, fetch)
    #[serde(default)]
    pub scheme: String,
    /// answers "search" (palette)
    #[serde(default)]
    pub search: bool,
    /// SVG path data for its sidebar section
    #[serde(default)]
    pub icon: String,
}

pub fn dir() -> PathBuf {
    if crate::isolated() {
        if let Some(d) = std::env::var_os("WC_SELFTEST") {
            return PathBuf::from(d).join("plugins");
        }
    }
    dirs::data_dir().unwrap_or_default().join("whale-cabinet/plugins")
}
fn cache_dir(name: &str) -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("whale-cabinet/plugins").join(name)
}

/// A plugin folder's manifest, checked: its executable must be a plain file inside the folder.
pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(dir.join("plugin.json")).map_err(|e| format!("plugin.json: {e}"))?;
    let mut m: Manifest = serde_json::from_str(&text).map_err(|e| format!("plugin.json: {e}"))?;
    m.name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let exec = Path::new(&m.exec);
    if m.exec.is_empty() || exec.is_absolute() || exec.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(format!("{}: exec must be a file inside the plugin folder", m.name));
    }
    if !m.scheme.is_empty() && !m.scheme.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!("{}: scheme must be lowercase letters, digits or -", m.name));
    }
    Ok(m)
}

pub fn discover(root: &Path) -> Vec<Manifest> {
    let mut v: Vec<Manifest> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| read_manifest(&e.path()).map_err(|err| eprintln!("whale-cabinet: plugin {err}")).ok())
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

// ---------- a running plugin ----------

type Reply = mpsc::Sender<Result<Value, String>>;
pub struct Proc {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    next: AtomicU64,
    pub init: Mutex<Value>,
}

impl Proc {
    /// Start the executable; `on_event` gets unasked messages, `on_exit` runs when its stdout closes.
    pub fn spawn(dir: &Path, m: &Manifest, on_event: impl Fn(Value) + Send + 'static, on_exit: impl FnOnce() + Send + 'static) -> Result<Proc, String> {
        let mut child = Command::new(dir.join(&m.exec))
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("{}: {e}", m.exec))?;
        let out = child.stdout.take().unwrap();
        let pending: Arc<Mutex<HashMap<u64, Reply>>> = Arc::default();
        let p2 = pending.clone();
        let name = m.name.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(&line) else {
                    eprintln!("whale-cabinet: plugin {name} said something that isn't JSON: {line}");
                    continue;
                };
                match msg.get("id").and_then(Value::as_u64) {
                    Some(id) => {
                        if let Some(tx) = p2.lock().unwrap().remove(&id) {
                            let r = match msg.get("error") {
                                Some(e) => Err(e.as_str().map(str::to_owned).unwrap_or_else(|| e.to_string())),
                                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(r);
                        }
                    }
                    None => on_event(Value::Object(msg)),
                }
            }
            p2.lock().unwrap().clear(); // every waiting call fails: "plugin exited"
            on_exit();
        });
        Ok(Proc { stdin: Mutex::new(child.stdin.take().unwrap()), child: Mutex::new(child), pending, next: AtomicU64::new(1), init: Mutex::new(Value::Null) })
    }

    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let line = json!({ "id": id, "method": method, "params": params }).to_string() + "\n";
        if let Err(e) = self.stdin.lock().unwrap().write_all(line.as_bytes()) {
            self.pending.lock().unwrap().remove(&id);
            return Err(format!("plugin not running: {e}"));
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("plugin didn't answer {method} in time"))
            }
            Err(_) => Err("plugin exited".into()),
        }
    }

    pub fn stop(&self) {
        let _ = self.child.lock().unwrap().kill();
        let _ = self.child.lock().unwrap().wait();
    }
}

// ---------- state + commands ----------

/// Running plugins; `starting` makes sure one is started (and initialised) once before anyone talks to it.
#[derive(Default)]
pub struct Plugins(Mutex<HashMap<String, Arc<Proc>>>, Mutex<()>);

fn settings_of(app: &AppHandle) -> Map<String, Value> {
    app.state::<crate::Settings>().0.lock().unwrap().clone()
}
/// settings.pluginConsent[name]: true (allowed), false (refused), missing (never asked)
fn consent(app: &AppHandle, name: &str) -> Option<bool> {
    settings_of(app).get("pluginConsent")?.get(name)?.as_bool()
}
fn patch_setting(app: &AppHandle, key: &str, name: &str, f: impl FnOnce(&mut Map<String, Value>)) -> Result<(), String> {
    let s = app.state::<crate::Settings>();
    let mut all = s.0.lock().unwrap().get(key).and_then(Value::as_object).cloned().unwrap_or_default();
    let mut mine = all.get(name).and_then(Value::as_object).cloned().unwrap_or_default();
    f(&mut mine);
    all.insert(name.into(), Value::Object(mine));
    let mut patch = Map::new();
    patch.insert(key.into(), Value::Object(all));
    crate::set_settings(patch, app.clone(), s)
}

fn manifest(name: &str) -> Result<Manifest, String> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return Err(format!("bad plugin name {name}"));
    }
    read_manifest(&dir().join(name))
}

/// Settings + secrets as the plugin gets them in init.
fn values(app: &AppHandle, m: &Manifest) -> Map<String, Value> {
    let saved = settings_of(app).get("pluginSettings").and_then(|p| p.get(&m.name)).and_then(Value::as_object).cloned().unwrap_or_default();
    let mut v = Map::new();
    for s in &m.settings {
        let val = if s.secret { crate::network::secret_lookup(&[("plugin", &m.name), ("key", &s.key)]) } else { saved.get(&s.key).and_then(Value::as_str).map(str::to_owned) };
        v.insert(s.key.clone(), Value::String(val.unwrap_or_default()));
    }
    v
}

/// The running process for `name`, started (and initialised) if needed — only if you've allowed it.
fn proc(app: &AppHandle, name: &str) -> Result<Arc<Proc>, String> {
    let state = app.state::<Plugins>();
    if let Some(p) = state.0.lock().unwrap().get(name) {
        return Ok(p.clone());
    }
    let _starting = state.1.lock().unwrap();
    if let Some(p) = state.0.lock().unwrap().get(name) {
        return Ok(p.clone());
    }
    let m = manifest(name)?;
    if consent(app, name) != Some(true) {
        return Err(format!("{} isn't allowed to run (Settings → Plugins)", m.title));
    }
    let (a, a2, n, n2) = (app.clone(), app.clone(), name.to_owned(), name.to_owned());
    let me: Arc<Mutex<Option<std::sync::Weak<Proc>>>> = Arc::default();
    let me2 = me.clone();
    let p = Arc::new(Proc::spawn(
        &dir().join(name),
        &m,
        move |mut ev| {
            ev["plugin"] = Value::String(n.clone());
            let _ = a.emit("plugin-event", ev);
        },
        move || {
            // gone: forget it (unless it was already replaced), the next call starts it again
            let st = a2.state::<Plugins>();
            let mut g = st.0.lock().unwrap();
            let mine = me2.lock().unwrap().as_ref().and_then(|w| w.upgrade());
            if mine.is_some_and(|p| g.get(&n2).is_some_and(|q| Arc::ptr_eq(&p, q))) {
                g.remove(&n2);
            }
        },
    )?);
    *me.lock().unwrap() = Some(Arc::downgrade(&p));
    let cache = cache_dir(name);
    let _ = std::fs::create_dir_all(&cache);
    let params = json!({ "settings": values(app, &m), "cache": cache, "home": dirs::home_dir().unwrap_or_default() });
    match p.call("init", params, Duration::from_secs(20)) {
        Ok(init) => *p.init.lock().unwrap() = init,
        Err(e) => {
            p.stop();
            return Err(format!("{}: {e}", m.title));
        }
    }
    state.0.lock().unwrap().insert(name.to_owned(), p.clone());
    Ok(p)
}

fn stop(app: &AppHandle, name: &str) {
    if let Some(p) = app.state::<Plugins>().0.lock().unwrap().remove(name) {
        std::thread::spawn(move || p.stop());
    }
}
fn changed(app: &AppHandle, name: &str) {
    stop(app, name);
    let _ = app.emit("plugins-changed", name);
}

#[derive(Serialize)]
pub struct Info {
    #[serde(flatten)]
    manifest: Manifest,
    /// true allowed, false refused, null never asked
    consent: Option<bool>,
}

#[tauri::command]
pub fn plugins_list(app: AppHandle) -> Vec<Info> {
    discover(&dir()).into_iter().map(|m| Info { consent: consent(&app, &m.name), manifest: m }).collect()
}

#[tauri::command]
pub fn plugin_consent(name: String, allow: bool, app: AppHandle) -> Result<(), String> {
    manifest(&name)?;
    let s = app.state::<crate::Settings>();
    let mut all = s.0.lock().unwrap().get("pluginConsent").and_then(Value::as_object).cloned().unwrap_or_default();
    all.insert(name.clone(), Value::Bool(allow));
    let mut patch = Map::new();
    patch.insert("pluginConsent".into(), Value::Object(all));
    crate::set_settings(patch, app.clone(), s)?;
    changed(&app, &name);
    Ok(())
}

/// Start it if needed; its init result (roots, sidebar…).
#[tauri::command]
pub async fn plugin_start(name: String, app: AppHandle) -> Result<Value, String> {
    crate::blocking(move || proc(&app, &name).map(|p| p.init.lock().unwrap().clone())).await?
}

#[tauri::command]
pub async fn plugin_call(name: String, method: String, params: Value, timeout: Option<u64>, app: AppHandle) -> Result<Value, String> {
    crate::blocking(move || proc(&app, &name)?.call(&method, params, Duration::from_secs(timeout.unwrap_or(30)))).await?
}

/// Current values for the settings UI; secrets only say whether one is saved.
#[tauri::command]
pub async fn plugin_settings(name: String, app: AppHandle) -> Result<Map<String, Value>, String> {
    crate::blocking(move || {
        let m = manifest(&name)?;
        let mut v = values(&app, &m);
        for s in m.settings.iter().filter(|s| s.secret) {
            let saved = v.get(&s.key).and_then(Value::as_str).is_some_and(|x| !x.is_empty());
            v.insert(s.key.clone(), Value::Bool(saved));
        }
        Ok(v)
    })
    .await?
}

/// Save one setting (secrets to the keyring; empty = forget) and restart the plugin with it.
#[tauri::command]
pub async fn plugin_set(name: String, key: String, value: String, app: AppHandle) -> Result<(), String> {
    crate::blocking(move || {
        let m = manifest(&name)?;
        let s = m.settings.iter().find(|s| s.key == key).ok_or(format!("{} has no setting {key}", m.title))?;
        if s.secret {
            let attrs = [("plugin", name.as_str()), ("key", key.as_str())];
            if value.is_empty() {
                crate::network::secret_clear(&attrs);
            } else {
                crate::network::secret_store(&format!("Whale Cabinet: {} {}", m.title, s.label), &attrs, &value)?;
            }
        } else {
            patch_setting(&app, "pluginSettings", &name, |mine| {
                mine.insert(key.clone(), Value::String(value.clone()));
            })?;
        }
        changed(&app, &name);
        Ok(())
    })
    .await?
}

/// Mime types for action filters.
#[tauri::command]
pub async fn mime_types(paths: Vec<String>) -> Result<Vec<String>, String> {
    crate::blocking(move || paths.iter().map(|p| crate::desktop::mime_of(Path::new(p))).collect()).await
}

/// `wcplugin://localhost/<plugin>/<method>?k=v…` → the file the plugin's answer names ({"file": path}),
/// so thumbnails are plain lazy <img>s.
pub fn protocol(app: &AppHandle, uri: &str) -> tauri::http::Response<Vec<u8>> {
    let fail = |code: u16, msg: String| tauri::http::Response::builder().status(code).header("Content-Type", "text/plain").body(msg.into_bytes()).unwrap();
    let Ok(url) = tauri::Url::parse(uri) else { return fail(400, "bad url".into()) };
    let mut parts = url.path().trim_start_matches('/').splitn(2, '/');
    let (Some(name), Some(method)) = (parts.next(), parts.next()) else { return fail(400, "want /<plugin>/<method>".into()) };
    let params: Map<String, Value> = url.query_pairs().map(|(k, v)| (k.into_owned(), Value::String(v.into_owned()))).collect();
    let r = proc(app, name).and_then(|p| p.call(method, Value::Object(params), Duration::from_secs(60)));
    let file = match r {
        Ok(v) => v.get("file").and_then(Value::as_str).map(PathBuf::from),
        Err(e) => return fail(502, e),
    };
    match file.map(std::fs::read) {
        Some(Ok(bytes)) => {
            let mime = infer::get(&bytes).map(|t| t.mime_type()).unwrap_or("application/octet-stream");
            tauri::http::Response::builder().header("Content-Type", mime).header("Cache-Control", "max-age=86400").body(bytes).unwrap()
        }
        Some(Err(e)) => fail(404, e.to_string()),
        None => fail(404, "no file".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn plugin(root: &Path, name: &str, manifest: &str, script: &str) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("plugin.json"), manifest).unwrap();
        std::fs::write(d.join("run"), script).unwrap();
        std::fs::set_permissions(d.join("run"), std::fs::Permissions::from_mode(0o755)).unwrap();
        d
    }

    #[test]
    fn manifests_are_checked() {
        let t = tempfile::tempdir().unwrap();
        plugin(t.path(), "good", r#"{"title":"Good","exec":"run","scheme":"good","actions":[{"id":"a","label":"A","mime":["image/*"]}]}"#, "");
        plugin(t.path(), "escape", r#"{"title":"Bad","exec":"../../bin/sh"}"#, "");
        plugin(t.path(), "abs", r#"{"title":"Bad","exec":"/bin/sh"}"#, "");
        plugin(t.path(), "scheme", r#"{"title":"Bad","exec":"run","scheme":"file:/"}"#, "");
        plugin(t.path(), "broken", "{", "");
        let found = discover(t.path());
        assert_eq!(found.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["good"]);
        assert_eq!(found[0].actions[0].mime, ["image/*"]);
        assert!(!found[0].actions[0].multi, "single item by default");
    }

    #[test]
    fn talks_json_lines() {
        let t = tempfile::tempdir().unwrap();
        // answers out of order (slow first), sends an event, fails on "boom"
        let py = r#"#!/usr/bin/env python3
import json, sys, threading, time
out = threading.Lock()
def say(m):
    with out: print(json.dumps(m), flush=True)
def handle(r):
    if r["method"] == "boom": return say({"id": r["id"], "error": "kaboom"})
    if r["method"] == "slow": time.sleep(0.3)
    if r["method"] == "poke": say({"event": "badges", "paths": ["/x"]})
    say({"id": r["id"], "result": {"echo": r["params"], "m": r["method"]}})
for line in sys.stdin:
    threading.Thread(target=handle, args=(json.loads(line),)).start()
"#;
        let d = plugin(t.path(), "echo", r#"{"title":"Echo","exec":"run"}"#, py);
        let m = read_manifest(&d).unwrap();
        let (etx, erx) = mpsc::channel();
        let (xtx, xrx) = mpsc::channel();
        let p = Arc::new(Proc::spawn(&d, &m, move |e| { let _ = etx.send(e); }, move || { let _ = xtx.send(()); }).unwrap());
        let long = Duration::from_secs(5);
        let p2 = p.clone();
        let slow = std::thread::spawn(move || p2.call("slow", json!({}), long));
        std::thread::sleep(Duration::from_millis(50));
        let fast = p.call("hi", json!({ "a": 1 }), long).unwrap();
        assert_eq!(fast, json!({ "echo": { "a": 1 }, "m": "hi" }));
        assert_eq!(slow.join().unwrap().unwrap()["m"], "slow", "answers are matched by id, not order");
        assert_eq!(p.call("boom", json!({}), long), Err("kaboom".into()));
        p.call("poke", json!({}), long).unwrap();
        assert_eq!(erx.recv_timeout(long).unwrap()["paths"], json!(["/x"]));
        p.stop();
        xrx.recv_timeout(long).expect("exit noticed");
        assert!(p.call("hi", json!({}), long).is_err(), "calls fail once it's gone");
    }
}
