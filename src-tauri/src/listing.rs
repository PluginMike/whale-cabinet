//! Directory listing, path resolution and disk-space queries. Pure std + libc so it's unit-testable.

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

#[derive(Serialize, Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub link: bool,
    pub broken: bool,
    /// "fifo" | "socket" | "char" | "block" for special files, else empty.
    pub special: &'static str,
    pub hidden: bool,
    pub size: u64,
    /// Modification time, ms since epoch.
    pub mtime: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

// NOTE: non-UTF-8 names are shown lossily and can't be round-tripped; send raw bytes if that ever matters.
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let dot_hidden: HashSet<String> = fs::read_to_string(dir.join(".hidden"))
        .map(|s| s.lines().map(str::to_owned).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for de in fs::read_dir(dir)? {
        let Ok(de) = de else { continue };
        let name = de.file_name().to_string_lossy().into_owned();
        if let Some(mut e) = entry(&de.path()) {
            e.hidden |= dot_hidden.contains(&name);
            out.push(e);
        }
    }
    Ok(out)
}

/// Stat one path into an Entry (symlinks followed for type/size; dangling links keep their own metadata).
pub fn entry(path: &Path) -> Option<Entry> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
    let lmeta = fs::symlink_metadata(path).ok()?;
    let link = lmeta.file_type().is_symlink();
    let (meta, broken) = if link {
        match fs::metadata(path) {
            Ok(m) => (m, false),
            Err(_) => (lmeta, true),
        }
    } else {
        (lmeta, false)
    };
    let ft = meta.file_type();
    let special = if ft.is_fifo() {
        "fifo"
    } else if ft.is_socket() {
        "socket"
    } else if ft.is_char_device() {
        "char"
    } else if ft.is_block_device() {
        "block"
    } else {
        ""
    };
    Some(Entry {
        hidden: name.starts_with('.'),
        dir: ft.is_dir(),
        size: if ft.is_file() { meta.len() } else { 0 },
        mtime: meta.mtime() * 1000 + meta.mtime_nsec() / 1_000_000,
        path: path.to_string_lossy().into_owned(),
        name,
        link,
        broken,
        special,
        tags: vec![],
    })
}

/// Turn user input from the location bar (or argv) into an existing absolute directory.
/// Handles `~`, `~/x`, `file://` URIs (percent-decoded) and paths relative to `cwd`.
pub fn resolve(input: &str, cwd: &Path) -> Result<PathBuf, String> {
    let s = input.trim();
    let s = match s.strip_prefix("file://") {
        Some(rest) => percent_decode(rest),
        None => s.to_owned(),
    };
    let home = dirs::home_dir().ok_or("no home directory")?;
    let p = if s.is_empty() || s == "~" {
        home
    } else if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else {
        cwd.join(&s) // join() with an absolute path replaces cwd
    };
    let p = fs::canonicalize(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !p.is_dir() {
        return Err(format!("{} is not a folder", p.display()));
    }
    Ok(p)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A folder to open, optionally with one item selected.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Target {
    pub loc: String,
    pub select: Option<String>,
}

/// `path` may be a folder (open it) or anything else that exists (open its folder with it selected).
pub fn target_for(input: &str, cwd: &Path) -> Option<Target> {
    let s = input.trim();
    let s = match s.strip_prefix("file://") {
        Some(r) => percent_decode(r.strip_prefix("localhost").unwrap_or(r)),
        None => s.to_owned(),
    };
    let p = if let Some(rest) = s.strip_prefix("~/") { dirs::home_dir()?.join(rest) } else if s == "~" { dirs::home_dir()? } else { cwd.join(&s) };
    let p = fs::canonicalize(&p).ok().or_else(|| fs::symlink_metadata(&p).ok().map(|_| p.clone()))?;
    if p.is_dir() {
        Some(Target { loc: p.to_string_lossy().into_owned(), select: None })
    } else {
        Some(Target { loc: p.parent()?.to_string_lossy().into_owned(), select: Some(p.to_string_lossy().into_owned()) })
    }
}

/// Parse `[--select FILE]... [PATH|URI]...` (argv without the program name). Unknown flags are ignored.
pub fn parse_args(args: &[String], cwd: &Path) -> Vec<Target> {
    let mut out = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--select" | "-s" => {
                if let Some(f) = it.next() {
                    out.extend(target_for(f, cwd).map(|mut t| {
                        if t.select.is_none() {
                            // --select on a folder: show it selected in its parent
                            let p = PathBuf::from(&t.loc);
                            if let Some(parent) = p.parent() {
                                t = Target { loc: parent.to_string_lossy().into_owned(), select: Some(t.loc) };
                            }
                        }
                        t
                    }));
                }
            }
            f if f.starts_with('-') => {}
            p => out.extend(target_for(p, cwd)),
        }
    }
    out
}

/// (free bytes available to the user, total bytes) for the filesystem holding `p`.
pub fn disk_space(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let f = st.f_frsize as u64;
    Some((st.f_bavail as u64 * f, st.f_blocks as u64 * f))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn lists_odd_names_links_and_specials() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        fs::write(d.join("with space.txt"), "hi").unwrap();
        fs::write(d.join("new\nline"), "").unwrap();
        fs::write(d.join("ünï💾.md"), "x").unwrap();
        fs::create_dir(d.join(".dot")).unwrap();
        fs::write(d.join("vis"), "").unwrap();
        fs::write(d.join(".hidden"), "vis\n").unwrap();
        symlink(d.join("nowhere"), d.join("dangling")).unwrap();
        symlink(d.join(".dot"), d.join("dirlink")).unwrap();
        let fifo = std::ffi::CString::new(d.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);

        let v = list_dir(d).unwrap();
        let get = |n: &str| v.iter().find(|e| e.name == n).unwrap_or_else(|| panic!("{n} missing"));
        assert_eq!(get("with space.txt").size, 2);
        assert!(!get("new\nline").dir);
        assert_eq!(get("ünï💾.md").size, 1);
        assert!(get(".dot").hidden && get(".dot").dir);
        assert!(get("vis").hidden, ".hidden file should hide entries");
        assert!(get("dangling").link && get("dangling").broken);
        assert!(get("dirlink").link && get("dirlink").dir && !get("dirlink").broken);
        assert_eq!(get("pipe").special, "fifo");
        assert!(list_dir(&d.join("nope")).is_err());
    }

    #[test]
    fn resolves_tilde_uri_and_relative() {
        let home = dirs::home_dir().unwrap().canonicalize().unwrap();
        let t = tempfile::tempdir().unwrap();
        let d = t.path().canonicalize().unwrap();
        fs::create_dir(d.join("a b")).unwrap();
        fs::write(d.join("f"), "").unwrap();
        assert_eq!(resolve("~", &d).unwrap(), home);
        assert_eq!(resolve("  ", &d).unwrap(), home);
        assert_eq!(resolve("a b", &d).unwrap(), d.join("a b"));
        assert_eq!(resolve("a b/..", &d).unwrap(), d);
        let uri = format!("file://{}/a%20b", d.display());
        assert_eq!(resolve(&uri, Path::new("/")).unwrap(), d.join("a b"));
        assert!(resolve("f", &d).is_err(), "files are not navigable");
        assert!(resolve("missing", &d).is_err());
    }

    #[test]
    fn command_line_targets() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().canonicalize().unwrap();
        fs::create_dir(d.join("sub dir")).unwrap();
        fs::write(d.join("sub dir/f.txt"), "").unwrap();
        let args = |v: &[&str]| parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &d);
        let ds = d.to_string_lossy().into_owned();
        assert_eq!(args(&["sub dir"]), vec![Target { loc: format!("{ds}/sub dir"), select: None }]);
        assert_eq!(args(&["sub dir/f.txt"]), vec![Target { loc: format!("{ds}/sub dir"), select: Some(format!("{ds}/sub dir/f.txt")) }]);
        assert_eq!(args(&["--select", "sub dir"]), vec![Target { loc: ds.clone(), select: Some(format!("{ds}/sub dir")) }]);
        let uri = format!("file://{ds}/sub%20dir/f.txt");
        assert_eq!(args(&["--new-window", &uri])[0].select.as_deref(), Some(format!("{ds}/sub dir/f.txt").as_str()));
        assert!(args(&["missing"]).is_empty());
    }

    #[test]
    fn disk_space_reports_something() {
        let (free, total) = disk_space(Path::new("/")).unwrap();
        assert!(total > 0 && free <= total);
    }
}
