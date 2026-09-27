//! Desktop look: DMS (matugen) palette, Hyprland shape options, UI font, harmonized tag colours.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

pub type Roles = BTreeMap<String, String>;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Hypr {
    pub running: bool,
    pub rounding: i64,
    pub gaps_in: i64,
    pub gaps_out: i64,
    pub border_size: i64,
    /// CSS colours (#rrggbbaa) of col.active_border; more than one means a gradient.
    pub border_colors: Vec<String>,
    pub border_angle: i64,
    pub blur: bool,
}

impl Default for Hypr {
    fn default() -> Self {
        Hypr { running: false, rounding: 8, gaps_in: 5, gaps_out: 10, border_size: 2, border_colors: vec![], border_angle: 0, blur: false }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Theme {
    /// "dms" | "builtin" | "custom" — what actually got used.
    pub source: String,
    pub dark: bool,
    pub roles: Roles,
    /// Preset tag colours (red…grey) harmonized toward the palette's primary.
    pub tags: BTreeMap<String, String>,
    pub hypr: Hypr,
    pub font_family: String,
    pub font_size: f64,
    pub icon_theme: String,
    /// 16 ANSI colours for the embedded terminal (DMS dank16), empty = terminal defaults.
    pub terminal: Vec<String>,
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}
pub fn dms_colors_json() -> PathBuf {
    home().join(".cache/DankMaterialShell/dms-colors.json")
}
pub fn dms_settings() -> PathBuf {
    home().join(".config/DankMaterialShell/settings.json")
}
pub fn gtk4_css() -> PathBuf {
    home().join(".config/gtk-4.0/dank-colors.css")
}
/// Folders to watch for re-theming.
pub fn watch_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [dms_colors_json(), dms_settings(), gtk4_css()].iter().filter_map(|p| p.parent().map(Into::into)).collect();
    v.extend(custom_theme_path().and_then(|p| p.parent().map(Into::into)));
    v
}
pub fn is_theme_file(p: &std::path::Path) -> bool {
    let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(n, "dms-colors.json" | "settings.json" | "dank-colors.css") || custom_theme_path().as_deref() == Some(p)
}

// ---------- DMS palette ----------

/// `dms-colors.json` → roles for its current mode.
pub fn parse_dms_json(text: &str) -> Option<(Roles, bool)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let dark = v.get("mode").and_then(Value::as_str) != Some("light");
    let colors = v.get("colors")?.get(if dark { "dark" } else { "light" })?.as_object()?;
    let roles = colors.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect();
    Some((roles, dark))
}

/// `@define-color name #hex;` lines from dank-colors.css, mapped onto Material roles.
pub fn parse_gtk_css(text: &str) -> Roles {
    let mut raw = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("@define-color ") else { continue };
        let mut it = rest.trim_end_matches(';').split_whitespace();
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            if v.starts_with('#') {
                raw.insert(k.to_owned(), v.to_owned());
            }
        }
    }
    let map = [
        ("accent_bg_color", "primary"),
        ("accent_fg_color", "on_primary"),
        ("window_bg_color", "background"),
        ("window_bg_color", "surface"),
        ("view_bg_color", "surface_container_lowest"),
        ("window_fg_color", "on_surface"),
        ("sidebar_bg_color", "surface_container"),
        ("card_bg_color", "surface_container_high"),
        ("error_bg_color", "error"),
        ("error_fg_color", "on_error"),
    ];
    map.iter().filter_map(|(g, r)| Some((r.to_string(), raw.get(*g)?.clone()))).collect()
}

/// DMS `customThemeFile` JSON: camelCase keys, either flat or under "dark"/"light".
pub fn parse_custom_theme(text: &str, dark: bool) -> Option<Roles> {
    let v: Value = serde_json::from_str(text).ok()?;
    let obj = v.get(if dark { "dark" } else { "light" }).unwrap_or(&v).as_object()?;
    let mut out = Roles::new();
    for (k, v) in obj {
        let Some(s) = v.as_str().filter(|s| s.starts_with('#')) else { continue };
        let role = match k.as_str() {
            "primaryText" => "on_primary".into(),
            "surfaceText" => "on_surface".into(),
            "surfaceVariantText" => "on_surface_variant".into(),
            "backgroundText" => "on_background".into(),
            "primaryContainerText" => "on_primary_container".into(),
            k => camel_to_snake(k),
        };
        out.insert(role, s.to_owned());
    }
    (!out.is_empty()).then_some(out)
}

fn camel_to_snake(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            o.push('_');
            o.push(c.to_ascii_lowercase());
        } else {
            o.push(c);
        }
    }
    o
}

fn custom_theme_path() -> Option<PathBuf> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dms_settings()).ok()?).ok()?;
    let p = v.get("customThemeFile")?.as_str().filter(|s| !s.is_empty())?;
    Some(PathBuf::from(p.replacen('~', &home().to_string_lossy(), 1)))
}

/// Best available DMS palette: customThemeFile (when DMS uses the custom theme) → dms-colors.json → gtk css.
pub fn load_dms() -> Option<(Roles, bool)> {
    let settings: Value = std::fs::read_to_string(dms_settings()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let json = std::fs::read_to_string(dms_colors_json()).ok().and_then(|t| parse_dms_json(&t));
    let dark = json.as_ref().map(|j| j.1).unwrap_or(true);
    if settings.get("currentThemeName").and_then(Value::as_str) == Some("custom") {
        if let Some(r) = custom_theme_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| parse_custom_theme(&t, dark)) {
            return Some((r, dark));
        }
    }
    json.or_else(|| {
        let r = parse_gtk_css(&std::fs::read_to_string(gtk4_css()).ok()?);
        (!r.is_empty()).then(|| {
            let dark = luminance(r.get("background").map(String::as_str).unwrap_or("#000")) < 0.4;
            (r, dark)
        })
    })
}

/// DMS's `dank16` terminal palette (color0…color15) for the given mode.
pub fn parse_dank16(text: &str, dark: bool) -> Vec<String> {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let d = &v["dank16"];
    let out: Vec<String> = (0..16).filter_map(|i| d[format!("color{i}")][if dark { "dark" } else { "light" }].as_str().map(str::to_owned)).collect();
    if out.len() == 16 { out } else { vec![] }
}

// ---------- colour maths (OKLab/OKLCH) ----------

pub fn parse_hex(s: &str) -> Option<[f64; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() < 6 {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|v| v as f64 / 255.0);
    Some([c(0)?, c(2)?, c(4)?])
}
fn to_hex(c: [f64; 3]) -> String {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}
fn lin(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}
fn gam(v: f64) -> f64 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}
/// WCAG relative luminance.
pub fn luminance(hex: &str) -> f64 {
    let c = parse_hex(hex).unwrap_or([0.0; 3]);
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}
fn to_oklch(c: [f64; 3]) -> [f64; 3] {
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let (ll, a, bb) = (
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    );
    [ll, (a * a + bb * bb).sqrt(), bb.atan2(a).to_degrees().rem_euclid(360.0)]
}
fn from_oklch(c: [f64; 3]) -> [f64; 3] {
    let (a, b) = (c[1] * c[2].to_radians().cos(), c[1] * c[2].to_radians().sin());
    let l = (c[0] + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (c[0] - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (c[0] - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        gam(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
        gam(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        gam(-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s),
    ]
}
/// Shrink chroma until the colour fits in sRGB.
fn oklch_hex(mut c: [f64; 3]) -> String {
    for _ in 0..30 {
        let rgb = from_oklch(c);
        if rgb.iter().all(|v| (-0.001..=1.001).contains(v)) {
            return to_hex(rgb);
        }
        c[1] *= 0.9;
    }
    to_hex(from_oklch(c))
}

pub const PRESETS: [(&str, &str); 9] = [
    ("red", "#e53935"),
    ("orange", "#fb8c00"),
    ("yellow", "#fdd835"),
    ("green", "#43a047"),
    ("teal", "#00897b"),
    ("blue", "#1e88e5"),
    ("purple", "#8e24aa"),
    ("pink", "#d81b60"),
    ("grey", "#757575"),
];

/// Material-style harmonization: rotate a colour's hue toward `source` by half the difference, at most 15°,
/// and set lightness for the theme mode so tags read well on it.
pub fn harmonize(color: &str, source: &str, dark: bool) -> String {
    let (Some(c), Some(s)) = (parse_hex(color), parse_hex(source)) else { return color.to_owned() };
    let (mut c, s) = (to_oklch(c), to_oklch(s));
    let diff = ((s[2] - c[2] + 540.0) % 360.0) - 180.0;
    if c[1] > 0.03 && s[1] > 0.02 {
        c[2] = (c[2] + diff.signum() * (diff.abs() * 0.5).min(15.0)).rem_euclid(360.0);
    }
    c[0] = if dark { 0.76 } else { 0.56 };
    c[1] = c[1].min(0.16);
    oklch_hex(c)
}

pub fn harmonized_tags(primary: &str, dark: bool) -> BTreeMap<String, String> {
    PRESETS.iter().map(|(n, c)| (n.to_string(), harmonize(c, primary, dark))).collect()
}

/// Material-ish scheme from one seed colour (Theme: Custom).
pub fn scheme_from_seed(seed: &str, dark: bool) -> Roles {
    let [_, sc, h] = to_oklch(parse_hex(seed).unwrap_or([0.5, 0.4, 0.8]));
    let t = |l: f64, c: f64| oklch_hex([l, c, h]);
    let n = |l: f64| oklch_hex([l, (sc * 0.12).min(0.02), h]);
    let roles: &[(&str, String)] = if dark {
        &[
            ("primary", t(0.82, sc.min(0.12))),
            ("on_primary", t(0.30, sc.min(0.10))),
            ("primary_container", t(0.40, sc.min(0.12))),
            ("on_primary_container", t(0.92, 0.05)),
            ("background", n(0.17)),
            ("surface", n(0.17)),
            ("surface_container_lowest", n(0.14)),
            ("surface_container_low", n(0.19)),
            ("surface_container", n(0.21)),
            ("surface_container_high", n(0.25)),
            ("surface_container_highest", n(0.29)),
            ("surface_bright", n(0.34)),
            ("surface_variant", n(0.33)),
            ("on_surface", n(0.92)),
            ("on_surface_variant", n(0.80)),
            ("outline", n(0.62)),
            ("outline_variant", n(0.36)),
            ("error", "#ffb4ab".into()),
            ("on_error", "#690005".into()),
        ]
    } else {
        &[
            ("primary", t(0.48, sc.min(0.16))),
            ("on_primary", "#ffffff".into()),
            ("primary_container", t(0.90, sc.min(0.07))),
            ("on_primary_container", t(0.25, sc.min(0.10))),
            ("background", n(0.98)),
            ("surface", n(0.98)),
            ("surface_container_lowest", "#ffffff".into()),
            ("surface_container_low", n(0.96)),
            ("surface_container", n(0.94)),
            ("surface_container_high", n(0.92)),
            ("surface_container_highest", n(0.90)),
            ("surface_bright", n(0.98)),
            ("surface_variant", n(0.91)),
            ("on_surface", n(0.22)),
            ("on_surface_variant", n(0.40)),
            ("outline", n(0.58)),
            ("outline_variant", n(0.82)),
            ("error", "#ba1a1a".into()),
            ("on_error", "#ffffff".into()),
        ]
    };
    roles.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

// ---------- Hyprland ----------

/// Parse one `hyprctl -j getoption` reply into (int, text, custom) views.
fn opt_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}
fn first_int(v: &Value) -> Option<i64> {
    if let Some(i) = v.get("int").and_then(Value::as_i64) {
        return Some(i);
    }
    // gaps come back as {"css": "8 8 8 8"} (or "custom")
    ["css", "custom", "str"].iter().find_map(|k| v.get(*k)?.as_str()?.split_whitespace().next()?.parse().ok())
}
/// `"e6d9b9ff 33ccffee 45deg"` → (["#d9b9ffe6", …], 45). Hyprland writes colours as AARRGGBB.
pub fn parse_gradient(s: &str) -> (Vec<String>, i64) {
    let mut cols = vec![];
    let mut angle = 0;
    for tok in s.split_whitespace() {
        if let Some(a) = tok.strip_suffix("deg") {
            angle = a.parse().unwrap_or(0);
        } else {
            let t = tok.trim_start_matches("0x").trim_start_matches("rgba(").trim_end_matches(')');
            if t.len() == 8 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                cols.push(format!("#{}{}", &t[2..], &t[..2]));
            } else if t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                cols.push(format!("#{t}"));
            }
        }
    }
    (cols, angle)
}

/// Build Hypr from getoption replies, keyed by option name.
pub fn hypr_from_replies(get: impl Fn(&str) -> Option<String>) -> Hypr {
    let mut h = Hypr::default();
    let Some(r) = get("decoration:rounding") else { return h };
    h.running = true;
    let v = |o: &str| get(o).map(|t| opt_json(&t)).unwrap_or(Value::Null);
    h.rounding = first_int(&opt_json(&r)).unwrap_or(h.rounding);
    h.gaps_in = first_int(&v("general:gaps_in")).unwrap_or(h.gaps_in);
    h.gaps_out = first_int(&v("general:gaps_out")).unwrap_or(h.gaps_out);
    h.border_size = first_int(&v("general:border_size")).unwrap_or(h.border_size);
    let b = v("general:col.active_border");
    let g = b.get("gradient").or_else(|| b.get("str")).and_then(Value::as_str).unwrap_or("");
    (h.border_colors, h.border_angle) = parse_gradient(g);
    let blur = v("decoration:blur:enabled");
    h.blur = blur.get("bool").and_then(Value::as_bool).or_else(|| blur.get("int").map(|i| i.as_i64() == Some(1))).unwrap_or(false);
    h
}

pub fn load_hypr() -> Hypr {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return Hypr::default();
    }
    hypr_from_replies(|o| {
        let out = Command::new("hyprctl").args(["-j", "getoption", o]).output().ok()?;
        let s = String::from_utf8(out.stdout).ok()?;
        s.trim_start().starts_with('{').then_some(s)
    })
}

pub fn hypr_socket2() -> Option<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let rt = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(PathBuf::from(rt).join("hypr").join(sig).join(".socket2.sock"))
}

// ---------- fonts / icons ----------

/// `'Adwaita Sans Bold 11'` → ("Adwaita Sans Bold", 11). Pango style words are left in the family; CSS copes.
pub fn parse_font_name(s: &str) -> (String, f64) {
    let s = s.trim().trim_matches('\'');
    match s.rsplit_once(' ') {
        Some((fam, size)) if size.parse::<f64>().is_ok() => (fam.trim_end_matches(',').to_owned(), size.parse().unwrap()),
        _ => (s.to_owned(), 11.0),
    }
}
fn gsetting(key: &str) -> Option<String> {
    let out = Command::new("gsettings").args(["get", "org.gnome.desktop.interface", key]).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().trim_matches('\'').to_owned();
    (!s.is_empty()).then_some(s)
}
pub fn icon_theme() -> String {
    gsetting("icon-theme").unwrap_or_else(|| "hicolor".into())
}

/// `source` is the user's setting: "dms" | "builtin" | "custom"; `custom_seed` the custom colour.
pub fn load(source: &str, custom_seed: Option<&str>, custom_dark: bool) -> Theme {
    let (family, size) = gsetting("font-name").map(|f| parse_font_name(&f)).unwrap_or(("sans-serif".into(), 11.0));
    let hypr = load_hypr();
    let (source, roles, dark) = match source {
        "custom" => ("custom", scheme_from_seed(custom_seed.unwrap_or("#6750a4"), custom_dark), custom_dark),
        "dms" => match load_dms() {
            Some((r, d)) => ("dms", r, d),
            None => ("builtin", Roles::new(), false),
        },
        _ => ("builtin", Roles::new(), false),
    };
    let tags = roles.get("primary").map(|p| harmonized_tags(p, dark)).unwrap_or_else(|| PRESETS.iter().map(|(n, c)| (n.to_string(), c.to_string())).collect());
    let terminal = if source == "dms" { std::fs::read_to_string(dms_colors_json()).map(|t| parse_dank16(&t, dark)).unwrap_or_default() } else { vec![] };
    Theme { source: source.into(), dark, roles, tags, hypr, font_family: family, font_size: size, icon_theme: icon_theme(), terminal }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dms_json_picks_mode() {
        let j = r##"{"mode":"light","colors":{"dark":{"primary":"#d9b9ff"},"light":{"primary":"#6b4fa0","surface":"#fff7fe"}}}"##;
        let (r, dark) = parse_dms_json(j).unwrap();
        assert!(!dark);
        assert_eq!(r["primary"], "#6b4fa0");
        assert_eq!(r["surface"], "#fff7fe");
        assert!(parse_dms_json("not json").is_none());
    }

    #[test]
    fn dank16_palette() {
        let mut j = serde_json::json!({"dank16": {}});
        for i in 0..16 {
            j["dank16"][format!("color{i}")] = serde_json::json!({"dark": format!("#0000{i:02x}"), "light": "#ffffff"});
        }
        let p = parse_dank16(&j.to_string(), true);
        assert_eq!((p.len(), p[15].as_str()), (16, "#00000f"));
        assert!(parse_dank16("{}", true).is_empty());
    }

    #[test]
    fn gtk_css_maps_roles() {
        let css = "/* x */\n@define-color accent_bg_color #d9b9ff;\n@define-color accent_fg_color #450086;\n@define-color window_bg_color #16111b;\n@define-color sidebar_bg_color #221e28;\n@define-color headerbar_backdrop_color @window_bg_color;\n";
        let r = parse_gtk_css(css);
        assert_eq!(r["primary"], "#d9b9ff");
        assert_eq!(r["on_primary"], "#450086");
        assert_eq!(r["background"], "#16111b");
        assert_eq!(r["surface_container"], "#221e28");
        assert_eq!(r.len(), 5, "references like @window_bg_color are skipped");
    }

    #[test]
    fn custom_theme_variants_and_keys() {
        let t = r##"{"dark":{"primary":"#aabbcc","primaryText":"#000000","surfaceContainerHigh":"#222222"},"light":{"primary":"#112233"}}"##;
        let r = parse_custom_theme(t, true).unwrap();
        assert_eq!(r["primary"], "#aabbcc");
        assert_eq!(r["on_primary"], "#000000");
        assert_eq!(r["surface_container_high"], "#222222");
        assert_eq!(parse_custom_theme(t, false).unwrap()["primary"], "#112233");
        assert_eq!(parse_custom_theme(r##"{"primary":"#010203"}"##, true).unwrap()["primary"], "#010203");
    }

    #[test]
    fn hyprctl_replies() {
        let replies = |o: &str| -> Option<String> {
            Some(match o {
                "decoration:rounding" => r#"{"option":"decoration:rounding","int":12,"set":true}"#,
                "general:gaps_in" => r#"{"option":"general:gaps_in","css":"8 8 8 8","set":true}"#,
                "general:gaps_out" => r#"{"option":"general:gaps_out","css":"14 20 14 20","set":true}"#,
                "general:border_size" => r#"{"option":"general:border_size","int":2,"set":true}"#,
                "general:col.active_border" => r#"{"option":"general:col.active_border","gradient":"e6d9b9ff ff33ccee 45deg","set":true}"#,
                "decoration:blur:enabled" => r#"{"option":"decoration:blur:enabled","bool":true,"set":true}"#,
                _ => return None,
            }.into())
        };
        let h = hypr_from_replies(replies);
        assert!(h.running && h.blur);
        assert_eq!((h.rounding, h.gaps_in, h.gaps_out, h.border_size), (12, 8, 14, 2));
        assert_eq!(h.border_colors, vec!["#d9b9ffe6", "#33cceeff"]);
        assert_eq!(h.border_angle, 45);
        assert_eq!(hypr_from_replies(|_| None), Hypr::default());
    }

    #[test]
    fn gradient_single_colour() {
        assert_eq!(parse_gradient("ee33ccff"), (vec!["#33ccffee".into()], 0));
        assert_eq!(parse_gradient(""), (vec![], 0));
    }

    #[test]
    fn font_names() {
        assert_eq!(parse_font_name("'Adwaita Sans 11'"), ("Adwaita Sans".into(), 11.0));
        assert_eq!(parse_font_name("Inter Bold 10.5"), ("Inter Bold".into(), 10.5));
        assert_eq!(parse_font_name("Cantarell"), ("Cantarell".into(), 11.0));
    }

    #[test]
    fn harmonize_moves_hue_toward_primary_by_at_most_15() {
        let blue = to_oklch(parse_hex("#1e88e5").unwrap())[2];
        let out = to_oklch(parse_hex(&harmonize("#1e88e5", "#d9b9ff", true)).unwrap())[2];
        let moved = ((out - blue + 540.0) % 360.0) - 180.0;
        assert!(moved > 0.0 && moved <= 15.5, "moved {moved}");
        // grey stays grey-ish
        let g = to_oklch(parse_hex(&harmonize("#757575", "#d9b9ff", true)).unwrap());
        assert!(g[1] < 0.03);
        assert_eq!(harmonized_tags("#d9b9ff", false).len(), 9);
    }

    #[test]
    fn seed_scheme_is_readable() {
        for dark in [true, false] {
            let r = scheme_from_seed("#00897b", dark);
            let (bg, fg) = (luminance(&r["background"]), luminance(&r["on_surface"]));
            let contrast = (bg.max(fg) + 0.05) / (bg.min(fg) + 0.05);
            assert!(contrast > 7.0, "contrast {contrast}");
        }
    }

    #[test]
    fn oklch_roundtrip() {
        for h in ["#16111b", "#d9b9ff", "#00ddda", "#ffffff", "#000000"] {
            assert_eq!(to_hex(from_oklch(to_oklch(parse_hex(h).unwrap()))), h);
        }
    }
}
