//! Git status badges: for a folder inside a work tree, one `git status --porcelain=v2 -z` over the whole repo.
//! Codes: C conflict, M modified (not staged), S staged, U untracked, I ignored. Folders get the worst code of
//! anything changed below them (ignored doesn't bubble up). Outside a repo git is never run.

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Info {
    pub root: String,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    /// Paths relative to the root as git reported them (untracked/ignored folders included, no trailing '/').
    pub files: HashMap<String, char>,
    /// Worst code of anything changed inside each folder.
    pub dirs: HashMap<String, char>,
}

/// Nearest ancestor (or self) with a `.git` entry (folder, or file for worktrees/submodules).
pub fn find_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

fn rank(c: char) -> u8 {
    match c {
        'C' => 4,
        'M' => 3,
        'S' => 2,
        'U' => 1,
        _ => 0,
    }
}

pub fn parse(out: &[u8], root: &str) -> Info {
    let text = String::from_utf8_lossy(out);
    let mut info = Info { root: root.to_owned(), ..Default::default() };
    let mut recs = text.split('\0');
    while let Some(r) = recs.next() {
        let (code, path) = match r.as_bytes().first() {
            Some(b'#') => {
                if let Some(b) = r.strip_prefix("# branch.head ") {
                    info.branch = b.to_owned();
                } else if let Some(ab) = r.strip_prefix("# branch.ab ") {
                    let mut it = ab.split(' ');
                    info.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok()).unwrap_or(0);
                    info.behind = it.next().and_then(|b| b.trim_start_matches('-').parse().ok()).unwrap_or(0);
                }
                continue;
            }
            Some(b'1' | b'2') => {
                let rename = r.starts_with('2');
                let xy = r.get(2..4).unwrap_or("..").as_bytes();
                let path = r.splitn(if rename { 10 } else { 9 }, ' ').last().unwrap_or("");
                if rename {
                    recs.next(); // the original path of a rename/copy
                }
                (if xy[1] != b'.' { 'M' } else if xy[0] != b'.' { 'S' } else { continue }, path)
            }
            Some(b'u') => ('C', r.splitn(11, ' ').last().unwrap_or("")),
            Some(b'?') => ('U', r.get(2..).unwrap_or("")),
            Some(b'!') => ('I', r.get(2..).unwrap_or("")),
            _ => continue,
        };
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            continue;
        }
        info.files.insert(path.to_owned(), code);
        if code != 'I' {
            let mut p = path;
            while let Some(i) = p.rfind('/') {
                p = &p[..i];
                let e = info.dirs.entry(p.to_owned()).or_insert(code);
                if rank(code) > rank(*e) {
                    *e = code;
                }
            }
        }
    }
    info
}

pub fn status(dir: &Path) -> Option<Info> {
    let root = find_root(dir)?;
    let out = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["--no-optional-locks", "status", "--porcelain=v2", "-z", "--branch", "--ignored=traditional", "--untracked-files=normal"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(parse(&out.stdout, &root.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let out = b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 a a src/app.ts\0\
1 M. N... 100644 100644 100644 a b src/deep/lib/x.rs\0\
2 R. N... 100644 100644 100644 a b R100 docs/new name.md\0docs/old.md\0\
u UU N... 100644 100644 100644 100644 a b c src/deep/clash.rs\0\
? notes/\0? top.txt\0! node_modules/\0! src/build.log\0";
        let i = parse(out, "/r");
        assert_eq!((i.branch.as_str(), i.ahead, i.behind), ("main", 2, 1));
        assert_eq!(i.files["src/app.ts"], 'M');
        assert_eq!(i.files["src/deep/lib/x.rs"], 'S');
        assert_eq!(i.files["docs/new name.md"], 'S');
        assert!(!i.files.contains_key("docs/old.md"), "rename source isn't a file here");
        assert_eq!(i.files["notes"], 'U');
        assert_eq!(i.files["node_modules"], 'I');
        assert_eq!(i.dirs["src"], 'C', "conflict beats everything");
        assert_eq!(i.dirs["src/deep/lib"], 'S');
        assert!(!i.dirs.contains_key("node_modules"));
    }

    #[test]
    fn real_repo_status() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let git = |a: &[&str]| assert!(Command::new("git").arg("-C").arg(d).args(a).output().unwrap().status.success(), "git {a:?}");
        git(&["init", "-q", "-b", "trunk"]);
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/a.txt"), "1").unwrap();
        std::fs::write(d.join(".gitignore"), "*.log\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "x"]);
        std::fs::write(d.join("sub/a.txt"), "2").unwrap();
        std::fs::write(d.join("new.txt"), "").unwrap();
        std::fs::write(d.join("x.log"), "").unwrap();
        let i = status(&d.join("sub")).unwrap();
        assert_eq!(i.branch, "trunk");
        assert_eq!(i.files["sub/a.txt"], 'M');
        assert_eq!(i.dirs["sub"], 'M');
        assert_eq!(i.files["new.txt"], 'U');
        assert_eq!(i.files["x.log"], 'I');
        assert!(status(Path::new("/proc")).is_none());
    }
}
