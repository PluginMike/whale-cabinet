//! Freedesktop integration: MIME detection (shared-mime-info globs + magic), `.desktop` entries,
//! `mimeapps.list` defaults/associations, icon-theme lookup, launching apps (Exec field codes) and terminals.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

// ---------- XDG dirs ----------

fn env_dirs(var: &str, default: &str) -> Vec<PathBuf> {
    std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.into()).split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect()
}
pub fn data_dirs() -> Vec<PathBuf> {
    let mut v = vec![dirs::data_dir().unwrap_or_default()];
    v.extend(env_dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
    v
}
fn config_dirs() -> Vec<PathBuf> {
    let mut v = vec![dirs::config_dir().unwrap_or_default()];
    v.extend(env_dirs("XDG_CONFIG_DIRS", "/etc/xdg"));
    v
}

// ---------- .desktop files ----------

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct App {
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub exec: String,
    pub icon: String,
    #[serde(skip)]
    pub mime: Vec<String>,
    #[serde(skip)]
    pub terminal: bool,
    #[serde(skip)]
    pub file: PathBuf,
    pub no_display: bool,
}

/// Parse the [Desktop Entry] group. Returns None for hidden/non-Application entries.
pub fn parse_desktop(text: &str, id: &str, file: &Path) -> Option<App> {
    let lang = std::env::var("LANG").unwrap_or_default();
    let lang = lang.split(['.', '@']).next().unwrap_or("");
    let short = lang.split('_').next().unwrap_or("");
    let mut in_group = false;
    let (mut a, mut ty, mut hidden, mut try_exec) = (App { id: id.into(), file: file.into(), ..Default::default() }, String::new(), false, String::new());
    let (mut name_full, mut name_short) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Type" => ty = v.into(),
            "Name" => a.name = v.into(),
            "Exec" => a.exec = v.into(),
            "Icon" => a.icon = v.into(),
            "MimeType" => a.mime = v.split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            "Terminal" => a.terminal = v == "true",
            "NoDisplay" => a.no_display = v == "true",
            "Hidden" => hidden = v == "true",
            "TryExec" => try_exec = v.into(),
            _ if !lang.is_empty() && k == format!("Name[{lang}]") => name_full = Some(v.to_owned()),
            _ if !short.is_empty() && k == format!("Name[{short}]") => name_short = Some(v.to_owned()),
            _ => {}
        }
    }
    if let Some(n) = name_full.or(name_short) {
        a.name = n;
    }
    if ty != "Application" || hidden || a.exec.is_empty() || (!try_exec.is_empty() && which(&try_exec).is_none()) {
        return None;
    }
    Some(a)
}

pub fn which(cmd: &str) -> Option<PathBuf> {
    if cmd.contains('/') {
        return Path::new(cmd).is_file().then(|| cmd.into());
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(cmd)).find(|f| f.is_file()))
}

/// All applications, keyed by desktop id; earlier data dirs win (user overrides system).
pub fn all_apps() -> HashMap<String, App> {
    let mut out = HashMap::new();
    for d in data_dirs() {
        let root = d.join("applications");
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("desktop") {
                    continue;
                }
                let id = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().replace('/', "-");
                if out.contains_key(&id) {
                    continue;
                }
                // Hidden/invalid entries still shadow lower-priority ones with the same id.
                let app = std::fs::read_to_string(&p).ok().and_then(|t| parse_desktop(&t, &id, &p));
                out.insert(id, app.unwrap_or_default());
            }
        }
    }
    out.retain(|_, a| !a.exec.is_empty());
    out
}

// ---------- mimeapps.list ----------

#[derive(Default, Debug, PartialEq)]
pub struct MimeApps {
    pub defaults: HashMap<String, Vec<String>>,
    pub added: HashMap<String, Vec<String>>,
    pub removed: HashMap<String, Vec<String>>,
}

pub fn parse_mimeapps(text: &str, into: &mut MimeApps) {
    let mut sect = "";
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            sect = match line {
                "[Default Applications]" => "d",
                "[Added Associations]" => "a",
                "[Removed Associations]" => "r",
                _ => "",
            };
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let m = match sect {
            "d" => &mut into.defaults,
            "a" => &mut into.added,
            "r" => &mut into.removed,
            _ => continue,
        };
        let e = m.entry(k.trim().to_owned()).or_default();
        for id in v.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            if !e.iter().any(|x| x == id) {
                e.push(id.to_owned()); // first file wins: later files append lower-priority entries
            }
        }
    }
}

fn mimeapps_files() -> Vec<PathBuf> {
    let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|s| !s.is_empty()).map(|s| s.to_lowercase()).collect();
    let mut v = vec![];
    for d in config_dirs() {
        v.extend(desktops.iter().map(|x| d.join(format!("{x}-mimeapps.list"))));
        v.push(d.join("mimeapps.list"));
    }
    for d in data_dirs() {
        let a = d.join("applications");
        v.extend(desktops.iter().map(|x| a.join(format!("{x}-mimeapps.list"))));
        v.push(a.join("mimeapps.list"));
    }
    v
}

pub fn load_mimeapps() -> MimeApps {
    let mut m = MimeApps::default();
    for f in mimeapps_files() {
        if let Ok(t) = std::fs::read_to_string(f) {
            parse_mimeapps(&t, &mut m);
        }
    }
    m
}

/// Set the user's default app for `mime` in ~/.config/mimeapps.list, keeping everything else intact.
pub fn set_default_in(text: &str, mime: &str, id: &str) -> String {
    let mut out: Vec<String> = vec![];
    let (mut sect, mut done_d, mut done_a) = (String::new(), false, false);
    // Append our line at the end of a section, before its trailing blank lines.
    let add_line = |out: &mut Vec<String>, sect: &str, done_d: &mut bool, done_a: &mut bool| {
        let d = sect == "[Default Applications]" && !*done_d;
        let a = sect == "[Added Associations]" && !*done_a;
        if d || a {
            let at = out.len() - out.iter().rev().take_while(|l| l.trim().is_empty()).count();
            out.insert(at, format!("{mime}={id};"));
            *done_d |= d;
            *done_a |= a;
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            add_line(&mut out, &sect, &mut done_d, &mut done_a);
            sect = t.to_owned();
            out.push(line.to_owned());
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if k.trim() == mime && sect == "[Default Applications]" {
                continue; // replaced
            }
            if k.trim() == mime && sect == "[Added Associations]" {
                let rest: Vec<&str> = v.split(';').map(str::trim).filter(|s| !s.is_empty() && *s != id).collect();
                out.push(format!("{mime}={id};{}", rest.iter().map(|s| format!("{s};")).collect::<String>()));
                done_a = true;
                continue;
            }
        }
        out.push(line.to_owned());
    }
    add_line(&mut out, &sect, &mut done_d, &mut done_a);
    if !done_d {
        out.push(String::new());
        out.push("[Default Applications]".into());
        out.push(format!("{mime}={id};"));
    }
    if !done_a {
        out.push(String::new());
        out.push("[Added Associations]".into());
        out.push(format!("{mime}={id};"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

pub fn set_default(mime: &str, id: &str) -> Result<(), String> {
    let f = dirs::config_dir().ok_or("no config dir")?.join("mimeapps.list");
    let text = std::fs::read_to_string(&f).unwrap_or_default();
    let tmp = f.with_extension("list.whale-tmp");
    std::fs::write(&tmp, set_default_in(&text, mime, id)).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, f).map_err(|e| e.to_string())
}

// ---------- MIME types ----------

struct Globs {
    literal: HashMap<String, String>,
    /// (lowercased suffix without "*", mime, weight, case_sensitive)
    suffix: Vec<(String, String, u32, bool)>,
    other: Vec<(String, String, u32)>,
    parents: HashMap<String, Vec<String>>,
    icons: HashMap<String, String>,
}

fn globs() -> &'static Globs {
    static G: OnceLock<Globs> = OnceLock::new();
    G.get_or_init(|| {
        let mut g = Globs { literal: HashMap::new(), suffix: vec![], other: vec![], parents: HashMap::new(), icons: HashMap::new() };
        for d in data_dirs().iter().rev() {
            let m = d.join("mime");
            if let Ok(t) = std::fs::read_to_string(m.join("globs2")) {
                parse_globs2(&t, &mut g);
            }
            if let Ok(t) = std::fs::read_to_string(m.join("subclasses")) {
                for l in t.lines() {
                    if let Some((a, b)) = l.split_once(' ') {
                        g.parents.entry(a.into()).or_default().push(b.into());
                    }
                }
            }
            if let Ok(t) = std::fs::read_to_string(m.join("aliases")) {
                for l in t.lines() {
                    if let Some((alias, canon)) = l.split_once(' ') {
                        g.parents.entry(alias.into()).or_default().push(canon.into());
                    }
                }
            }
            for f in ["generic-icons", "icons"] {
                if let Ok(t) = std::fs::read_to_string(m.join(f)) {
                    for l in t.lines() {
                        if let Some((a, b)) = l.split_once(':') {
                            g.icons.insert(a.into(), b.into());
                        }
                    }
                }
            }
        }
        g.suffix.sort_by(|a, b| b.2.cmp(&a.2).then(b.0.len().cmp(&a.0.len())));
        g
    })
}

fn parse_globs2(t: &str, g: &mut Globs) {
    for l in t.lines() {
        if l.starts_with('#') {
            continue;
        }
        let mut it = l.splitn(4, ':');
        let (Some(w), Some(mime), Some(glob)) = (it.next(), it.next(), it.next()) else { continue };
        let cs = it.next().is_some_and(|f| f.contains("cs"));
        let w: u32 = w.parse().unwrap_or(50);
        if let Some(s) = glob.strip_prefix('*').filter(|s| !s.contains(['*', '?', '['])) {
            g.suffix.push((if cs { s.to_owned() } else { s.to_lowercase() }, mime.into(), w, cs));
        } else if !glob.contains(['*', '?', '[']) {
            g.literal.insert(glob.to_owned(), mime.into());
        } else {
            g.other.push((glob.to_owned(), mime.into(), w));
        }
    }
}

fn glob_match(pat: &str, name: &str) -> bool {
    // tiny fnmatch for the few complex globs (e.g. "[Mm]akefile", "*.[1-9]")
    fn m(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| m(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && m(&p[1..], &n[1..]),
            Some('[') => {
                let Some(end) = p.iter().position(|&c| c == ']') else { return false };
                let set = &p[1..end];
                let Some(&c) = n.first() else { return false };
                let mut ok = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        ok |= set[i] <= c && c <= set[i + 2];
                        i += 3;
                    } else {
                        ok |= set[i] == c;
                        i += 1;
                    }
                }
                ok && m(&p[end + 1..], &n[1..])
            }
            Some(&c) => n.first() == Some(&c) && m(&p[1..], &n[1..]),
        }
    }
    m(&pat.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
}

pub fn mime_by_name(name: &str) -> Option<String> {
    let g = globs();
    if let Some(m) = g.literal.get(name) {
        return Some(m.clone());
    }
    let lower = name.to_lowercase();
    // highest weight, then longest suffix wins (".tar.gz" beats ".gz")
    let best = g.suffix.iter().filter(|(s, _, _, cs)| if *cs { name.ends_with(s.as_str()) } else { lower.ends_with(s.as_str()) }).max_by(|a, b| a.2.cmp(&b.2).then(a.0.len().cmp(&b.0.len())));
    if let Some(b) = best {
        return Some(b.1.clone());
    }
    g.other.iter().find(|(p, _, _)| glob_match(p, name) || glob_match(&p.to_lowercase(), &lower)).map(|x| x.1.clone())
}

pub fn mime_of(p: &Path) -> String {
    let Ok(lm) = std::fs::symlink_metadata(p) else { return "application/octet-stream".into() };
    let Ok(m) = std::fs::metadata(p) else { return "inode/symlink".into() }; // dangling link
    let _ = lm;
    if m.is_dir() {
        return "inode/directory".into();
    }
    use std::os::unix::fs::FileTypeExt;
    let ft = m.file_type();
    if ft.is_fifo() {
        return "inode/fifo".into();
    }
    if ft.is_socket() {
        return "inode/socket".into();
    }
    if ft.is_block_device() {
        return "inode/blockdevice".into();
    }
    if ft.is_char_device() {
        return "inode/chardevice".into();
    }
    if m.len() == 0 {
        if let Some(n) = p.file_name().and_then(|n| n.to_str()).and_then(mime_by_name) {
            return n;
        }
        return "application/x-zerosize".into();
    }
    if let Some(n) = p.file_name().and_then(|n| n.to_str()).and_then(mime_by_name) {
        return n;
    }
    if let Ok(Some(t)) = infer::get_from_path(p) {
        return t.mime_type().into();
    }
    // no NUL in the first 4 KiB → plain text
    let mut buf = [0u8; 4096];
    let n = std::fs::File::open(p).and_then(|mut f| std::io::Read::read(&mut f, &mut buf)).unwrap_or(0);
    if !buf[..n].contains(&0) {
        "text/plain".into()
    } else {
        "application/octet-stream".into()
    }
}

/// `mime` followed by its ancestors (text/* → text/plain → application/octet-stream).
pub fn mime_chain(mime: &str) -> Vec<String> {
    let g = globs();
    let mut out = vec![mime.to_owned()];
    let mut i = 0;
    while i < out.len() && i < 16 {
        for p in g.parents.get(&out[i]).cloned().unwrap_or_default() {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        i += 1;
    }
    if mime.starts_with("text/") && !out.iter().any(|m| m == "text/plain") {
        out.push("text/plain".into());
    }
    if !mime.starts_with("inode/") && !out.iter().any(|m| m == "application/octet-stream") {
        out.push("application/octet-stream".into());
    }
    out
}

pub fn mime_icon_name(mime: &str) -> (String, String) {
    let g = globs();
    let own = g.icons.get(mime).cloned().unwrap_or_else(|| mime.replace('/', "-"));
    let generic = format!("{}-x-generic", mime.split('/').next().unwrap_or("application"));
    (own, generic)
}

// ---------- apps for a file ----------

#[derive(Serialize, Clone, Debug)]
pub struct AppChoice {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub default: bool,
}

/// Registered apps for `mime` (default first), per the MIME apps spec.
pub fn apps_for_mime(mime: &str, apps: &HashMap<String, App>, ma: &MimeApps) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let push = |out: &mut Vec<String>, id: &str| {
        if apps.contains_key(id) && !out.iter().any(|x| x == id) {
            out.push(id.to_owned());
        }
    };
    let chain = mime_chain(mime);
    for m in &chain {
        for id in ma.defaults.get(m).into_iter().flatten() {
            push(&mut out, id);
        }
    }
    for m in &chain {
        let removed: HashSet<&String> = ma.removed.get(m).into_iter().flatten().collect();
        for id in ma.added.get(m).into_iter().flatten() {
            push(&mut out, id);
        }
        let mut native: Vec<&App> = apps.values().filter(|a| a.mime.iter().any(|x| x == m) && !removed.contains(&a.id)).collect();
        native.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        for a in native {
            push(&mut out, &a.id);
        }
    }
    out
}

// ---------- icons ----------

fn icon_roots() -> Vec<PathBuf> {
    let mut v = vec![dirs::home_dir().unwrap_or_default().join(".icons")];
    v.extend(data_dirs().into_iter().map(|d| d.join("icons")));
    v
}

fn theme_chain(theme: &str) -> Vec<String> {
    let mut out = vec![theme.to_owned()];
    let mut i = 0;
    while i < out.len() && i < 8 {
        for r in icon_roots() {
            if let Ok(t) = std::fs::read_to_string(r.join(&out[i]).join("index.theme")) {
                if let Some(l) = t.lines().find(|l| l.starts_with("Inherits=")) {
                    for p in l[9..].split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        if !out.iter().any(|x| x == p) {
                            out.push(p.to_owned());
                        }
                    }
                }
                break;
            }
        }
        i += 1;
    }
    if !out.iter().any(|x| x == "hicolor") {
        out.push("hicolor".into());
    }
    out
}

/// Resolve an icon name to a file in the icon theme (or pixmaps). Cached.
pub fn find_icon(name: &str, theme: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.starts_with('/') {
        return Path::new(name).is_file().then(|| name.into());
    }
    static CACHE: std::sync::Mutex<Option<HashMap<String, Option<PathBuf>>>> = std::sync::Mutex::new(None);
    let key = format!("{theme}\0{name}");
    if let Some(hit) = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return hit.clone();
    }
    let sizes = ["48x48", "scalable", "64x64", "32x32", "128x128", "256x256", "24x24", "22x22", "16x16", "48", "64", "32"];
    let ctxs = ["apps", "mimetypes", "places", "devices", "actions", "status", "categories", "emblems"];
    let mut found = None;
    'outer: for t in theme_chain(theme) {
        for r in icon_roots() {
            let base = r.join(&t);
            if !base.is_dir() {
                continue;
            }
            for s in sizes {
                for c in ctxs {
                    for ext in ["svg", "png"] {
                        // both "48x48/apps" (hicolor, Papirus) and "apps/48" (Breeze) layouts
                        for p in [base.join(s).join(c).join(format!("{name}.{ext}")), base.join(c).join(s).join(format!("{name}.{ext}"))] {
                            if p.is_file() {
                                found = Some(p);
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }
    if found.is_none() {
        for d in data_dirs() {
            for ext in ["svg", "png", "xpm"] {
                let p = d.join("pixmaps").join(format!("{name}.{ext}"));
                if p.is_file() && ext != "xpm" {
                    found = Some(p);
                }
            }
        }
    }
    CACHE.lock().unwrap().as_mut().unwrap().insert(key, found.clone());
    found
}

// ---------- launching ----------

/// Split an Exec value into argv per the Desktop Entry spec quoting rules.
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let (mut inq, mut any) = (false, false);
    let mut it = exec.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                inq = !inq;
                any = true;
            }
            '\\' if inq => {
                if let Some(n) = it.next() {
                    cur.push(n);
                }
            }
            ' ' | '\t' if !inq => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            _ => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Expand field codes. Returns one argv per process to start (%f/%u with several files → one each).
pub fn expand_exec(app: &App, files: &[String], uri: impl Fn(&str) -> String) -> Vec<Vec<String>> {
    let argv = split_exec(&app.exec);
    let single = argv.iter().any(|a| a.contains("%f") || a.contains("%u"));
    let multi = argv.iter().any(|a| a == "%F" || a == "%U");
    let groups: Vec<Vec<String>> = if single && !multi && files.len() > 1 { files.iter().map(|f| vec![f.clone()]).collect() } else { vec![files.to_vec()] };
    groups
        .into_iter()
        .map(|g| {
            let mut out = vec![];
            for a in &argv {
                match a.as_str() {
                    "%F" => out.extend(g.iter().cloned()),
                    "%U" => out.extend(g.iter().map(|f| uri(f))),
                    "%i" => {
                        if !app.icon.is_empty() {
                            out.push("--icon".into());
                            out.push(app.icon.clone());
                        }
                    }
                    _ => {
                        let mut s = String::new();
                        let mut cs = a.chars().peekable();
                        while let Some(c) = cs.next() {
                            if c != '%' {
                                s.push(c);
                                continue;
                            }
                            match cs.next() {
                                Some('f') => s.push_str(g.first().map(String::as_str).unwrap_or("")),
                                Some('u') => s.push_str(&g.first().map(|f| uri(f)).unwrap_or_default()),
                                Some('c') => s.push_str(&app.name),
                                Some('k') => s.push_str(&app.file.to_string_lossy()),
                                Some('%') => s.push('%'),
                                _ => {} // deprecated/unknown codes are dropped
                            }
                        }
                        if !(s.is_empty() && a.starts_with('%')) {
                            out.push(s);
                        }
                    }
                }
            }
            out
        })
        .collect()
}

pub const TERMINALS: [&str; 8] = ["kitty", "foot", "ghostty", "alacritty", "wezterm", "konsole", "gnome-terminal", "xterm"];

/// The user's terminal: setting → $TERMINAL → first installed of the usual suspects.
pub fn terminal(pref: &str) -> Option<String> {
    let env = std::env::var("TERMINAL").unwrap_or_default();
    let found = [pref, env.as_str()].into_iter().filter(|s| !s.is_empty()).map(str::to_owned).chain(TERMINALS.iter().map(|s| s.to_string())).find(|t| which(t.split_whitespace().next().unwrap_or("")).is_some());
    found
}

/// argv to open `term` in `dir`, optionally running `cmd`.
pub fn terminal_argv(term: &str, dir: &str, cmd: &[String]) -> Vec<String> {
    let mut v: Vec<String> = split_exec(term);
    let name = Path::new(&v[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match name.as_str() {
        "kitty" => v.extend(["--directory".into(), dir.into()]),
        "foot" => v.push(format!("--working-directory={dir}")),
        "ghostty" | "gnome-terminal" => v.push(format!("--working-directory={dir}")),
        "alacritty" => v.extend(["--working-directory".into(), dir.into()]),
        "wezterm" => v.extend(["start".into(), "--cwd".into(), dir.into()]),
        "konsole" => v.extend(["--workdir".into(), dir.into()]),
        _ => {}
    }
    if !cmd.is_empty() {
        match name.as_str() {
            "foot" => {}
            "wezterm" | "gnome-terminal" => v.push("--".into()),
            _ => v.push("-e".into()),
        }
        v.extend(cmd.iter().cloned());
    }
    v
}

/// Start a process fully detached (own session, no inherited stdio), reaped by a thread.
pub fn spawn_detached(argv: &[String], cwd: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let (prog, args) = argv.split_first().ok_or("empty command")?;
    let mut c = Command::new(prog);
    c.args(args).current_dir(if cwd.is_dir() { cwd } else { Path::new("/") }).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = c.spawn().map_err(|e| format!("{prog}: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn launch(app: &App, files: &[String], term_pref: &str) -> Result<(), String> {
    let cwd = files.first().map(|f| Path::new(f).parent().unwrap_or(Path::new("/")).to_path_buf()).unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
    for argv in expand_exec(app, files, crate::jobs::to_uri) {
        let argv = if app.terminal {
            let t = terminal(term_pref).ok_or("no terminal emulator found")?;
            terminal_argv(&t, &cwd.to_string_lossy(), &argv)
        } else {
            argv
        };
        spawn_detached(&argv, &cwd)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = "[Desktop Entry]\nType=Application\nName=Viewer\nName[de]=Betrachter\nExec=viewer --open %U --title \"%c here\"\nIcon=viewer\nMimeType=image/png;image/jpeg;\n\n[Desktop Action new]\nName=Other\nExec=nope\n";

    #[test]
    fn desktop_entry_parsing() {
        let a = parse_desktop(ENTRY, "viewer.desktop", Path::new("/x/viewer.desktop")).unwrap();
        assert_eq!(a.exec, "viewer --open %U --title \"%c here\"");
        assert_eq!(a.mime, vec!["image/png", "image/jpeg"]);
        assert!(parse_desktop("[Desktop Entry]\nType=Link\nName=x\nExec=y\n", "l", Path::new("/l")).is_none());
        assert!(parse_desktop("[Desktop Entry]\nType=Application\nName=x\nExec=y\nHidden=true\n", "h", Path::new("/h")).is_none());
        assert!(parse_desktop("[Desktop Entry]\nType=Application\nName=x\nExec=y\nTryExec=definitely-not-installed-xyz\n", "t", Path::new("/t")).is_none());
    }

    #[test]
    fn exec_field_codes() {
        let a = parse_desktop(ENTRY, "viewer.desktop", Path::new("/x/viewer.desktop")).unwrap();
        let files = vec!["/a b/1.png".to_string(), "/c/2.png".to_string()];
        let uri = |f: &str| crate::jobs::to_uri(f);
        let v = expand_exec(&a, &files, uri);
        assert_eq!(v, vec![vec!["viewer", "--open", "file:///a%20b/1.png", "file:///c/2.png", "--title", "Viewer here"]]);
        let one = App { exec: "edit %f --line=1 %% %d".into(), ..a.clone() };
        let v = expand_exec(&one, &files, uri);
        assert_eq!(v, vec![vec!["edit", "/a b/1.png", "--line=1", "%"], vec!["edit", "/c/2.png", "--line=1", "%"]], "%f with many files = one process each");
        assert_eq!(split_exec(r#"sh -c "echo \"hi\" \\ there" ''"#), vec!["sh", "-c", "echo \"hi\" \\ there", "''"]);
    }

    #[test]
    fn mimeapps_merge_and_rewrite() {
        let mut m = MimeApps::default();
        parse_mimeapps("[Default Applications]\ntext/plain=kate.desktop;\n[Added Associations]\ntext/plain=nvim.desktop;\n[Removed Associations]\ntext/plain=gedit.desktop;\n", &mut m);
        parse_mimeapps("[Default Applications]\ntext/plain=gedit.desktop;other.desktop\n", &mut m);
        assert_eq!(m.defaults["text/plain"], vec!["kate.desktop", "gedit.desktop", "other.desktop"]);
        let mut apps = HashMap::new();
        for (id, mime) in [("kate.desktop", "text/plain"), ("gedit.desktop", "text/plain"), ("nvim.desktop", ""), ("zed.desktop", "text/plain")] {
            apps.insert(id.to_string(), App { id: id.into(), name: id.into(), exec: "x".into(), mime: vec![mime.into()], ..Default::default() });
        }
        let got = apps_for_mime("text/plain", &apps, &MimeApps { defaults: HashMap::from([("text/plain".into(), vec!["kate.desktop".into()])]), ..m });
        assert_eq!(got[0], "kate.desktop");
        assert!(got.contains(&"nvim.desktop".to_string()));

        let orig = "# my notes\n[Default Applications]\ntext/plain=kate.desktop;\nimage/png=gwenview.desktop;\n\n[Added Associations]\ntext/plain=kate.desktop;nvim.desktop;\n";
        let new = set_default_in(orig, "text/plain", "nvim.desktop");
        assert!(new.starts_with("# my notes\n[Default Applications]\n"));
        assert!(new.contains("image/png=gwenview.desktop;"), "other lines preserved");
        assert_eq!(new.matches("text/plain=").count(), 2);
        assert!(new.contains("image/png=gwenview.desktop;\ntext/plain=nvim.desktop;\n\n[Added Associations]"), "{new}");
        assert!(new.contains("text/plain=nvim.desktop;kate.desktop;"));
        let fresh = set_default_in("", "image/png", "a.desktop");
        assert!(fresh.contains("[Default Applications]\nimage/png=a.desktop;") && fresh.contains("[Added Associations]\nimage/png=a.desktop;"));
    }

    #[test]
    fn globs_and_mime_detection() {
        let mut g = Globs { literal: HashMap::new(), suffix: vec![], other: vec![], parents: HashMap::new(), icons: HashMap::new() };
        parse_globs2("50:application/x-compressed-tar:*.tar.gz\n50:application/gzip:*.gz\n50:text/x-makefile:[Mm]akefile\n50:text/x-readme:README\n", &mut g);
        assert_eq!(g.literal["README"], "text/x-readme");
        assert_eq!(g.suffix.len(), 2);
        assert!(glob_match("[Mm]akefile", "makefile") && glob_match("*.[1-9]", "ls.1") && !glob_match("*.[1-9]", "ls.x"));
        // against the real shared-mime-info database, when installed
        if Path::new("/usr/share/mime/globs2").exists() {
            assert_eq!(mime_by_name("a.TAR.GZ").as_deref(), Some("application/x-compressed-tar"));
            assert_eq!(mime_by_name("x.png").as_deref(), Some("image/png"));
            assert!(mime_chain("text/x-python").contains(&"text/plain".to_string()));
        }
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("noext"), "just text").unwrap();
        std::fs::write(t.path().join("blob"), [0u8, 159, 146, 150]).unwrap();
        assert_eq!(mime_of(&t.path().join("noext")), "text/plain");
        assert_eq!(mime_of(&t.path().join("blob")), "application/octet-stream");
        assert_eq!(mime_of(t.path()), "inode/directory");
    }

    /// Against the real machine: `cargo test real_system -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_system() {
        let apps = all_apps();
        let ma = load_mimeapps();
        for m in ["text/markdown", "image/png", "inode/directory", "application/pdf"] {
            let ids = apps_for_mime(m, &apps, &ma);
            println!("{m}: {:?}", ids.iter().take(4).collect::<Vec<_>>());
        }
        let theme = crate::theme::icon_theme();
        let (own, generic) = mime_icon_name("text/markdown");
        println!("icon theme {theme}: firefox={:?} md={:?}", find_icon("firefox", &theme), find_icon(&own, &theme).or_else(|| find_icon(&generic, &theme)));
        println!("terminal: {:?}", terminal(""));
        assert!(!apps.is_empty());
    }

    #[test]
    fn terminals() {
        assert_eq!(terminal_argv("kitty", "/a b", &[]), vec!["kitty", "--directory", "/a b"]);
        assert_eq!(terminal_argv("foot", "/x", &["htop".into()]), vec!["foot", "--working-directory=/x", "htop"]);
        assert_eq!(terminal_argv("alacritty", "/x", &["vim".into(), "f".into()]), vec!["alacritty", "--working-directory", "/x", "-e", "vim", "f"]);
        assert_eq!(terminal_argv("wezterm", "/x", &["top".into()]), vec!["wezterm", "start", "--cwd", "/x", "--", "top"]);
    }
}
