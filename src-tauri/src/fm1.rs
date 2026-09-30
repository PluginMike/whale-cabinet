//! org.freedesktop.FileManager1 on the session bus: "Show in folder" from browsers, IDEs, etc.
//! ShowFolders opens folders, ShowItems opens their folders with the items selected, ShowItemProperties
//! opens the Properties dialog. Each call is forwarded to the UI as an "open" event.

use crate::listing::{target_for, Target};
use serde::Serialize;
use std::path::Path;
use tauri::AppHandle;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct OpenRequest {
    pub targets: Vec<Target>,
    /// Open the Properties dialog for these paths instead of browsing.
    pub properties: Vec<String>,
}

/// URIs → targets. `select` = reveal the items themselves (ShowItems) rather than open them as folders.
pub fn targets(uris: &[String], select: bool) -> Vec<Target> {
    let root = Path::new("/");
    uris.iter()
        .filter_map(|u| {
            let t = target_for(u, root)?;
            if select && t.select.is_none() {
                // ShowItems on a folder: show the folder selected inside its parent
                let p = Path::new(&t.loc);
                return Some(Target { loc: p.parent().unwrap_or(root).to_string_lossy().into_owned(), select: Some(t.loc) });
            }
            Some(t)
        })
        .collect()
}

pub struct FileManager1 {
    pub app: AppHandle,
}

impl FileManager1 {
    fn send(&self, req: OpenRequest) {
        crate::route_open(&self.app, req);
    }
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager1 {
    fn show_folders(&self, uris: Vec<String>, _startup_id: String) {
        self.send(OpenRequest { targets: targets(&uris, false), properties: vec![] });
    }
    fn show_items(&self, uris: Vec<String>, _startup_id: String) {
        self.send(OpenRequest { targets: targets(&uris, true), properties: vec![] });
    }
    fn show_item_properties(&self, uris: Vec<String>, _startup_id: String) {
        let props = uris.iter().filter_map(|u| target_for(u, Path::new("/"))).map(|t| t.select.unwrap_or(t.loc)).collect();
        self.send(OpenRequest { targets: vec![], properties: props });
    }
}

/// Claim the bus name (taking it over from Dolphin if it holds it) and serve until the app exits.
pub fn serve(app: AppHandle) -> Result<zbus::blocking::Connection, String> {
    let conn = zbus::blocking::connection::Builder::session()
        .map_err(|e| e.to_string())?
        .serve_at("/org/freedesktop/FileManager1", FileManager1 { app })
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;
    use zbus::fdo::RequestNameFlags as F;
    conn.request_name_with_flags("org.freedesktop.FileManager1", F::ReplaceExisting | F::AllowReplacement | F::DoNotQueue).map_err(|e| e.to_string())?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_items_selects_and_show_folders_opens() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().canonicalize().unwrap();
        std::fs::create_dir(d.join("f")).unwrap();
        std::fs::write(d.join("f/a b.pdf"), "").unwrap();
        let ds = d.to_string_lossy();
        let uri = |p: &str| crate::jobs::to_uri(&format!("{ds}/{p}"));
        assert_eq!(targets(&[uri("f/a b.pdf")], true), vec![Target { loc: format!("{ds}/f"), select: Some(format!("{ds}/f/a b.pdf")) }]);
        assert_eq!(targets(&[uri("f")], false), vec![Target { loc: format!("{ds}/f"), select: None }]);
        assert_eq!(targets(&[uri("f")], true), vec![Target { loc: ds.to_string(), select: Some(format!("{ds}/f")) }]);
    }
}
