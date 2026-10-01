//! xdg-desktop-portal FileChooser backend: other apps' "Open file" dialogs show Whale Cabinet's picker
//! (Recent files, zoxide folders, search). Save dialogs are handed on to the GTK backend unchanged.

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::async_runtime::{channel, Sender};
use tauri::{AppHandle, Manager};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

pub const NAME: &str = "org.freedesktop.impl.portal.desktop.whalecabinet";
pub const PATH: &str = "/org/freedesktop/portal/desktop";
type Reply = (u32, HashMap<String, OwnedValue>);

/// Open pickers by window label → where the chosen URIs go (None = cancelled).
#[derive(Default)]
pub struct Pending(pub Mutex<HashMap<String, Sender<Option<Vec<String>>>>>);

/// The picker in window `label` finished (or was closed: `uris` None).
pub fn finish(app: &AppHandle, label: &str, uris: Option<Vec<String>>) {
    if let Some(tx) = app.state::<Pending>().0.lock().unwrap().remove(label) {
        let _ = tx.try_send(uris);
    }
}

/// Portal options → what the picker page needs.
pub fn picker_arg(title: &str, o: &HashMap<String, OwnedValue>) -> Json {
    let get = |k: &str| o.get(k).and_then(|v| v.try_clone().ok());
    let flag = |k: &str| get(k).and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
    // current_folder is a NUL-terminated byte string
    let folder = get("current_folder").and_then(|v| Vec::<u8>::try_from(v).ok()).map(|b| String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[])).into_owned());
    // filters: [(name, [(0 = glob | 1 = mime, pattern)])]
    let filters = get("filters").and_then(|v| Vec::<(String, Vec<(u32, String)>)>::try_from(v).ok()).unwrap_or_default();
    let current = get("current_filter").and_then(|v| <(String, Vec<(u32, String)>)>::try_from(v).ok()).map(|f| f.0);
    json!({
        "title": title,
        "multiple": flag("multiple"),
        "directory": flag("directory"),
        "accept": get("accept_label").and_then(|v| String::try_from(v).ok()),
        "folder": folder.filter(|f| !f.is_empty()),
        "filters": filters.iter().map(|(n, ps)| json!({ "name": n, "globs": ps.iter().filter(|p| p.0 == 0).map(|p| &p.1).collect::<Vec<_>>(), "mimes": ps.iter().filter(|p| p.0 == 1).map(|p| &p.1).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "current": current,
    })
}

pub struct FileChooser {
    pub app: AppHandle,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    async fn open_file(&self, _handle: OwnedObjectPath, _app_id: String, _parent: String, title: String, options: HashMap<String, OwnedValue>) -> Reply {
        let arg = picker_arg(&title, &options);
        let title = if title.is_empty() { "Open".to_owned() } else { title };
        let (tx, mut rx) = channel(1);
        let Ok(label) = crate::open_dialog(self.app.clone(), "pick".into(), arg.to_string(), title, 980.0, 640.0).await else { return (2, HashMap::new()) };
        self.app.state::<Pending>().0.lock().unwrap().insert(label, tx);
        match rx.recv().await.flatten() {
            Some(uris) => (0, HashMap::from([("uris".to_owned(), Value::from(uris).try_to_owned().unwrap())])),
            None => (1, HashMap::new()),
        }
    }

    async fn save_file(&self, #[zbus(connection)] conn: &zbus::Connection, handle: OwnedObjectPath, app_id: String, parent: String, title: String, options: HashMap<String, OwnedValue>) -> zbus::fdo::Result<Reply> {
        forward(conn, "SaveFile", &(handle, app_id, parent, title, options)).await
    }

    async fn save_files(&self, #[zbus(connection)] conn: &zbus::Connection, handle: OwnedObjectPath, app_id: String, parent: String, title: String, options: HashMap<String, OwnedValue>) -> zbus::fdo::Result<Reply> {
        forward(conn, "SaveFiles", &(handle, app_id, parent, title, options)).await
    }
}

// NOTE: always GTK (what Hyprland setups ship); read the previous FileChooser= from portals.conf if others need it.
async fn forward(conn: &zbus::Connection, method: &str, body: &(OwnedObjectPath, String, String, String, HashMap<String, OwnedValue>)) -> zbus::fdo::Result<Reply> {
    let m = conn.call_method(Some("org.freedesktop.impl.portal.desktop.gtk"), PATH, Some("org.freedesktop.impl.portal.FileChooser"), method, body).await?;
    Ok(m.body().deserialize()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_to_picker_arg() {
        let ov = |v: Value| v.try_to_owned().unwrap();
        let mut o = HashMap::new();
        o.insert("multiple".into(), ov(Value::from(true)));
        o.insert("current_folder".into(), ov(Value::from(b"/home/x\0".to_vec())));
        o.insert("filters".into(), ov(Value::from(vec![("Images".to_owned(), vec![(0u32, "*.png".to_owned()), (1, "image/jpeg".to_owned())])])));
        let a = picker_arg("Upload", &o);
        assert_eq!(a["multiple"], true);
        assert_eq!(a["directory"], false);
        assert_eq!(a["folder"], "/home/x");
        assert_eq!(a["filters"][0], json!({ "name": "Images", "globs": ["*.png"], "mimes": ["image/jpeg"] }));
        assert_eq!(picker_arg("", &HashMap::new())["folder"], Json::Null);
    }
}
