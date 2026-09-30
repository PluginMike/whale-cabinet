//! Administrator actions. File operations in folders you can't write run in a helper — this same binary as
//! `pkexec whale-cabinet --admin-helper`, so polkit asks for the password every time and the GUI never runs as
//! root. The helper reads one JSON request on stdin, reuses ops.rs, and talks back in JSON lines (progress,
//! conflict questions, done); answers and cancel come on stdin.
//! "Open as Administrator" runs the default app through `pkexec env …` (Wayland/X11 variables passed on);
//! "Edit as Administrator" edits a private copy with your normal app and writes each save back through the helper.

use crate::ops::{self, Choice, Report};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    Transfer { pairs: Vec<(String, String)>, mv: bool },
    Delete { paths: Vec<String> },
    Rename { path: String, name: String },
    Create { dir: String, name: String, folder: bool },
    /// Replace `dst`'s contents with `src`'s, keeping dst's owner, mode and inode (like sudoedit).
    Write { src: String, dst: String },
    /// Copy `src` to `dst` and give it to the calling user (PKEXEC_UID), for editing a copy.
    Read { src: String, dst: String },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Msg {
    Progress { done: u64, total: u64, cur: String },
    Conflict { src: String, dst: String },
    Done { errors: Vec<String>, done: Vec<(String, String)>, cancelled: bool, result: Option<String> },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Answer {
    #[serde(default)]
    pub choice: Option<Choice>,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub cancel: bool,
}

// ---------- helper side (runs as root) ----------

struct HelperRep {
    out: Mutex<(std::io::Stdout, Instant)>,
    answers: Mutex<Receiver<Answer>>,
    cancel: std::sync::Arc<AtomicBool>,
}
impl HelperRep {
    fn send(&self, m: &Msg, force: bool) {
        let mut g = self.out.lock().unwrap();
        if force || g.1.elapsed() > Duration::from_millis(100) {
            g.1 = Instant::now();
            let _ = writeln!(g.0, "{}", serde_json::to_string(m).unwrap_or_default());
            let _ = g.0.flush();
        }
    }
}
impl Report for HelperRep {
    fn progress(&self, done: u64, total: u64, current: &Path) {
        self.send(&Msg::Progress { done, total, cur: current.to_string_lossy().into_owned() }, false);
    }
    fn conflict(&self, src: &Path, dst: &Path) -> (Choice, bool) {
        self.send(&Msg::Conflict { src: src.to_string_lossy().into_owned(), dst: dst.to_string_lossy().into_owned() }, true);
        match self.answers.lock().unwrap().recv() {
            Ok(Answer { choice: Some(c), all, cancel: false }) => (c, all),
            _ => (Choice::Cancel, false),
        }
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Carry out one request (also what the unit tests call directly, without root).
pub fn perform(req: &Request, rep: &dyn Report) -> (ops::Outcome, Option<String>) {
    let mut out = ops::Outcome::default();
    let mut result = None;
    let mut err = |e: String| out.errors.push(e);
    match req {
        Request::Transfer { pairs, mv } => {
            let pairs: Vec<(PathBuf, PathBuf)> = pairs.iter().map(|(a, b)| (a.into(), b.into())).collect();
            return (ops::transfer(&pairs, *mv, rep), None);
        }
        Request::Delete { paths } => {
            let ps: Vec<PathBuf> = paths.iter().map(PathBuf::from).filter(|p| std::fs::symlink_metadata(p).is_ok()).collect();
            return (ops::delete(&ps, rep), None);
        }
        Request::Rename { path, name } => match ops::rename(Path::new(path), name) {
            Ok(p) => result = Some(p.to_string_lossy().into_owned()),
            Err(e) => err(e),
        },
        Request::Create { dir, name, folder } => match ops::create(Path::new(dir), name, *folder) {
            Ok(p) => result = Some(p.to_string_lossy().into_owned()),
            Err(e) => err(e),
        },
        Request::Write { src, dst } => {
            let r = (|| -> std::io::Result<()> {
                let mut data = vec![];
                std::fs::File::open(src)?.read_to_end(&mut data)?;
                let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(dst)?;
                f.write_all(&data)?;
                f.sync_all()
            })();
            if let Err(e) = r {
                err(format!("{dst}: {e}"));
            }
        }
        Request::Read { src, dst } => {
            let r = (|| -> std::io::Result<()> {
                std::fs::copy(src, dst)?;
                if let Some(uid) = std::env::var("PKEXEC_UID").ok().and_then(|u| u.parse::<u32>().ok()) {
                    std::os::unix::fs::chown(dst, Some(uid), None)?;
                    std::fs::set_permissions(dst, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
                }
                Ok(())
            })();
            if let Err(e) = r {
                err(format!("{src}: {e}"));
            }
        }
    }
    (out, result)
}

/// Entry point for `whale-cabinet --admin-helper`. Returns the exit code.
pub fn helper_main() -> i32 {
    // read_line takes the stdin lock only for this call, so the answer reader below can have it
    let mut first = String::new();
    if std::io::stdin().read_line(&mut first).unwrap_or(0) == 0 {
        return 2;
    }
    let req: Request = match serde_json::from_str(&first) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bad request: {e}");
            return 2;
        }
    };
    let (tx, rx) = channel();
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        for l in std::io::stdin().lock().lines().map_while(Result::ok) {
            match serde_json::from_str::<Answer>(&l) {
                Ok(a) if a.cancel => {
                    c2.store(true, Ordering::Relaxed);
                    let _ = tx.send(a);
                }
                Ok(a) => {
                    let _ = tx.send(a);
                }
                Err(_) => {}
            }
        }
        c2.store(true, Ordering::Relaxed); // GUI went away
    });
    let rep = HelperRep { out: Mutex::new((std::io::stdout(), Instant::now())), answers: Mutex::new(rx), cancel };
    let (out, result) = perform(&req, &rep);
    rep.send(&Msg::Done { errors: out.errors, done: out.done, cancelled: out.cancelled, result }, true);
    0
}

// ---------- GUI side ----------

/// Run `req` through `pkexec` and the helper. `rep` gets progress/conflicts; returns the outcome and result.
pub fn run(req: &Request, rep: &dyn Report) -> Result<(ops::Outcome, Option<String>), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new("pkexec")
        .arg(&exe)
        .arg("--admin-helper")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("pkexec: {e} (install polkit)"))?;
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", serde_json::to_string(req).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let stdout = BufReader::new(child.stdout.take().unwrap());
    // relay cancel even while the helper is busy
    let stdin = std::sync::Arc::new(Mutex::new(stdin));
    let (s2, done) = (stdin.clone(), std::sync::Arc::new(AtomicBool::new(false)));
    let d2 = done.clone();
    let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
    let cf = cancel_flag.clone();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !d2.load(Ordering::Relaxed) {
                if cf.load(Ordering::Relaxed) {
                    let _ = writeln!(s2.lock().unwrap(), "{{\"cancel\":true}}");
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let mut last = None;
        for line in stdout.lines().map_while(Result::ok) {
            if rep.cancelled() {
                cancel_flag.store(true, Ordering::Relaxed);
            }
            match serde_json::from_str::<Msg>(&line) {
                Ok(Msg::Progress { done, total, cur }) => rep.progress(done, total, Path::new(&cur)),
                Ok(Msg::Conflict { src, dst }) => {
                    let (c, all) = rep.conflict(Path::new(&src), Path::new(&dst));
                    let cancel = c == Choice::Cancel;
                    let _ = writeln!(stdin.lock().unwrap(), "{}", serde_json::to_string(&Answer { choice: Some(c), all, cancel }).unwrap_or_default());
                }
                Ok(Msg::Done { errors, done, cancelled, result }) => last = Some((ops::Outcome { errors, done, overwrote: false, cancelled }, result)),
                Err(_) => {}
            }
        }
        done.store(true, Ordering::Relaxed);
        let mut err = String::new();
        let _ = child.stderr.take().map(|mut e| e.read_to_string(&mut err));
        let st = child.wait().map_err(|e| e.to_string())?;
        match last {
            Some(r) => Ok(r),
            None if st.code() == Some(126) => Err("Authentication was cancelled".into()),
            None if st.code() == Some(127) => Err(format!("Not authorized{}", if err.trim().is_empty() { String::new() } else { format!(": {}", err.trim()) })),
            None => Err(if err.trim().is_empty() { format!("administrator helper failed ({st})") } else { err.trim().to_owned() }),
        }
    })
}

/// Display/session variables a root GUI app needs to reach your screen.
fn session_env() -> Vec<String> {
    let mut v = vec![];
    for k in ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XAUTHORITY", "XDG_SESSION_TYPE", "QT_QPA_PLATFORM", "GDK_BACKEND"] {
        if let Ok(val) = std::env::var(k) {
            v.push(format!("{k}={val}"));
        }
    }
    v
}

/// `pkexec env VAR=… argv…`
pub fn open_argv(argv: &[String]) -> Vec<String> {
    let mut v = vec!["pkexec".to_string(), "env".into()];
    v.extend(session_env());
    v.extend(argv.iter().cloned());
    v
}

/// Where "Edit as Administrator" keeps the private copy of `orig`.
pub fn edit_copy_path(orig: &Path) -> PathBuf {
    let tag = crate::preview::md5_hex(&orig.to_string_lossy());
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("whale-cabinet/admin-edit").join(&tag[..12]).join(orig.file_name().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Quiet;
    impl Report for Quiet {
        fn progress(&self, _: u64, _: u64, _: &Path) {}
        fn conflict(&self, _: &Path, _: &Path) -> (Choice, bool) {
            (Choice::Skip, false)
        }
        fn cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn requests_roundtrip_and_perform() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let s = |p: &str| d.join(p).to_string_lossy().into_owned();
        std::fs::write(d.join("a.conf"), "old contents, long line").unwrap();
        std::fs::write(d.join("edit.tmp"), "new").unwrap();
        std::fs::set_permissions(d.join("a.conf"), std::os::unix::fs::PermissionsExt::from_mode(0o640)).unwrap();
        let req = Request::Write { src: s("edit.tmp"), dst: s("a.conf") };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""op":"write""#));
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
        let (out, _) = perform(&req, &Quiet);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(std::fs::read_to_string(d.join("a.conf")).unwrap(), "new");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join("a.conf")).unwrap().permissions().mode() & 0o777, 0o640, "mode kept");

        let (_, made) = perform(&Request::Create { dir: s(""), name: "made".into(), folder: true }, &Quiet);
        assert_eq!(made.as_deref(), Some(s("made").as_str()));
        let (_, renamed) = perform(&Request::Rename { path: s("made"), name: "moved".into() }, &Quiet);
        assert_eq!(renamed.as_deref(), Some(s("moved").as_str()));
        let (out, _) = perform(&Request::Transfer { pairs: vec![(s("a.conf"), s("moved/a.conf"))], mv: false }, &Quiet);
        assert_eq!(out.done.len(), 1);
        let (out, _) = perform(&Request::Delete { paths: vec![s("moved"), s("never-existed")] }, &Quiet);
        assert!(out.errors.is_empty(), "missing paths are skipped: {:?}", out.errors);
        assert!(!d.join("moved").exists());

        let msg = serde_json::to_string(&Msg::Conflict { src: "a".into(), dst: "b".into() }).unwrap();
        assert!(matches!(serde_json::from_str::<Msg>(&msg).unwrap(), Msg::Conflict { .. }));
        let a: Answer = serde_json::from_str(r#"{"choice":{"choice":"rename","name":"x"},"all":true}"#).unwrap();
        assert_eq!(a.choice, Some(Choice::Rename(Some("x".into()))));
    }

    #[test]
    fn open_argv_passes_the_session() {
        let v = open_argv(&["kate".into(), "/etc/hosts".into()]);
        assert_eq!(&v[..2], &["pkexec", "env"]);
        assert_eq!(&v[v.len() - 2..], &["kate", "/etc/hosts"]);
    }
}
