//! ~/.config/whale-cabinet/settings.json — a flat JSON object; unknown keys are kept.

use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub fn dir() -> PathBuf {
    // test runs (WC_SELFTEST) keep their own settings, tag index and session, never touching the user's
    if crate::isolated() {
        if let Some(d) = std::env::var_os("WC_SELFTEST") {
            return PathBuf::from(d).join("config");
        }
    }
    dirs::config_dir().unwrap_or_default().join("whale-cabinet")
}

pub fn defaults() -> Map<String, Value> {
    json!({
        "theme": "dms",            // dms | builtin | custom
        "customColor": "#00897b",
        "customDark": true,
        "opacity": 1.0,            // window background opacity (Hyprland blur shows through below 1)
        "titlebar": false,         // compact in-app titlebar (for floating mode)
        "view": "cabinet",         // cabinet | compact | grid
        "showHidden": false,
        "singleClick": false,
        "infoPanel": true,
        "zoom": 1.0,               // item size (1.0 = the default "large" cabinet)
        "folderDblClick": "open",  // open (Windows style) | expand (pull out inline)
        "infoWidth": 420,          // info panel width, px (drag its edge)
        "terminal": "",            // empty = auto-detect
        "restoreSession": "ask"    // ask | always | never — reopen last windows and tabs on a plain launch
    })
    .as_object()
    .unwrap()
    .clone()
}

pub fn load() -> Map<String, Value> {
    let mut m = defaults();
    if let Ok(Value::Object(saved)) = std::fs::read_to_string(dir().join("settings.json")).map(|t| serde_json::from_str(&t).unwrap_or(Value::Null)) {
        m.extend(saved);
    }
    m
}

pub fn save(m: &Map<String, Value>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = dir().join("settings.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(m)?)?;
    std::fs::rename(tmp, dir().join("settings.json"))
}
