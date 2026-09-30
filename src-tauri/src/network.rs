//! Network locations through gvfs: `gio mount` (driven in a PTY so its User/Domain/Password and host-key
//! prompts can be answered from the UI), then browsed as a normal folder via the gvfs FUSE path that
//! `gio info` reports. SSH hosts come from ~/.ssh/config; SMB shares are browsed with `gio list`; passwords
//! can be kept in the keyring with `secret-tool` (libsecret). Needs gvfs (+ its smb / WebDAV / nfs backends).

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const SCHEMES: [&str; 8] = ["sftp", "smb", "dav", "davs", "nfs", "ftp", "ftps", "afp"];
pub fn is_remote(href: &str) -> bool {
    href.split_once("://").is_some_and(|(s, _)| SCHEMES.contains(&s))
}

/// `Host` aliases from an ssh config (patterns with * ? ! skipped).
pub fn parse_ssh_config(text: &str) -> Vec<String> {
    let mut v = vec![];
    for l in text.lines() {
        let l = l.trim();
        let Some((k, rest)) = l.split_once(char::is_whitespace) else { continue };
        if k.eq_ignore_ascii_case("host") {
            for h in rest.split_whitespace() {
                if !h.contains(['*', '?', '!']) && !v.iter().any(|x| x == h) {
                    v.push(h.to_owned());
                }
            }
        }
    }
    v
}
// NOTE: `Include` lines aren't followed; add that if someone keeps hosts in included files.
pub fn ssh_hosts() -> Vec<String> {
    dirs::home_dir().and_then(|h| std::fs::read_to_string(h.join(".ssh/config")).ok()).map(|t| parse_ssh_config(&t)).unwrap_or_default()
}

#[derive(Default, Clone, Debug)]
pub struct Creds {
    pub user: String,
    pub domain: String,
    pub password: String,
    /// Accept a host-key / certificate question ("Log In Anyway").
    pub trust: bool,
}

#[derive(Debug, PartialEq)]
pub enum Step {
    /// Type this line.
    Send(String),
    /// Keep reading.
    Wait,
    /// Stop: needs a password / username the user hasn't given (message = gio's text).
    NeedAuth(String),
    /// Stop: gio asks a question (message + choices) the user must decide.
    Question(String),
}

/// Decide how to answer what gio has printed so far (`out` = everything since the last answer).
pub fn answer(out: &str, c: &Creds, asked_password: &mut bool) -> Step {
    let tail = out.trim_end_matches([' ', '\r', '\n']);
    if !tail.ends_with(':') && !tail.ends_with(']') && !tail.ends_with('?') {
        return Step::Wait;
    }
    let last = tail.rsplit('\n').next().unwrap_or(tail).trim();
    let message = out.lines().map(str::trim).filter(|l| !l.is_empty() && *l != last).collect::<Vec<_>>().join(" ");
    if last.starts_with("Choice") {
        if !c.trust {
            return Step::Question(message);
        }
        // pick the first choice that isn't a cancel ("[0] Log In Anyway", "[1] Cancel Login")
        let pick = out.lines().filter_map(|l| {
            let l = l.trim();
            let (n, label) = l.strip_prefix('[')?.split_once(']')?;
            (!label.to_lowercase().contains("cancel")).then(|| n.to_owned())
        });
        return Step::Send(pick.into_iter().next().unwrap_or_else(|| "0".into()));
    }
    if last.starts_with("User") {
        return if c.user.is_empty() && last == "User:" { Step::NeedAuth(message) } else { Step::Send(c.user.clone()) };
    }
    if last.starts_with("Domain") {
        return Step::Send(c.domain.clone());
    }
    if last.starts_with("Password") {
        if c.password.is_empty() || *asked_password {
            return Step::NeedAuth(if *asked_password { format!("Wrong password? {message}") } else { message });
        }
        *asked_password = true;
        return Step::Send(c.password.clone());
    }
    if last.to_lowercase().contains("anonymous") {
        return Step::Send(if c.user.is_empty() && c.password.is_empty() { "y" } else { "n" }.into());
    }
    Step::Wait
}

fn gio(args: &[&str]) -> Result<String, String> {
    let o = Command::new("gio").args(args).stdin(Stdio::null()).output().map_err(|e| format!("gio: {e} (install gvfs)"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_owned())
    }
}

/// The FUSE path gvfs exposes for a mounted location.
pub fn local_path(uri: &str) -> Option<String> {
    let out = gio(&["info", "-a", "standard::name", uri]).ok()?;
    out.lines().find_map(|l| l.trim().strip_prefix("local path:").map(|p| p.trim().to_owned()))
}

/// Mount `uri` (answering gio's prompts from `c`) and return its local path.
/// Errors start with "auth:" (needs credentials) or "question:" (host key etc.) so the UI can ask.
pub fn mount(uri: &str, c: &Creds) -> Result<String, String> {
    if let Some(p) = local_path(uri).filter(|p| std::path::Path::new(p).exists()) {
        return Ok(p); // already mounted
    }
    let pty = native_pty_system().openpty(PtySize { rows: 24, cols: 200, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let mut cmd = CommandBuilder::new("gio");
    cmd.arg("mount");
    if c.user.is_empty() && c.password.is_empty() {
        // try anonymous first (public FTP/WebDAV); servers that need a login still prompt, and we ask the user
        cmd.arg("--anonymous");
    }
    cmd.arg(uri);
    cmd.env("LC_ALL", "C.UTF-8"); // prompts in English (so they can be recognised), text still UTF-8
    let mut child = pty.slave.spawn_command(cmd).map_err(|e| format!("gio: {e} (install gvfs)"))?;
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pty.master.take_writer().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    let (mut out, mut all, mut asked_pw) = (String::new(), String::new(), false);
    let stop = |child: &mut Box<dyn portable_pty::Child + Send + Sync>| {
        let _ = child.kill();
        let _ = child.wait();
    };
    loop {
        if let Ok(Some(st)) = child.try_wait() {
            while let Ok(b) = rx.try_recv() {
                all.push_str(&String::from_utf8_lossy(&b));
            }
            let text = all.replace('\r', "");
            if st.success() || text.contains("already mounted") {
                break;
            }
            let err = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("gio mount failed").to_owned();
            return Err(friendly(uri, err.trim_start_matches("gio: ")));
        }
        if started.elapsed() > Duration::from_secs(90) {
            stop(&mut child);
            return Err("timed out connecting".into());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(b) => {
                let s = String::from_utf8_lossy(&b).replace('\r', "");
                out.push_str(&s);
                all.push_str(&s);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(50)),
        }
        match answer(&out, c, &mut asked_pw) {
            Step::Wait => {}
            Step::Send(line) => {
                let _ = writer.write_all(format!("{line}\n").as_bytes());
                let _ = writer.flush();
                out.clear();
            }
            Step::NeedAuth(m) => {
                stop(&mut child);
                return Err(format!("auth:{m}"));
            }
            Step::Question(m) => {
                stop(&mut child);
                return Err(format!("question:{m}"));
            }
        }
    }
    local_path(uri).ok_or_else(|| "Connected, but gvfs doesn't expose it as a folder (is gvfsd-fuse running? It comes with gvfs).".into())
}

/// gio's "volume doesn't implement mount" = the gvfs backend for that protocol isn't installed.
fn friendly(uri: &str, err: &str) -> String {
    let l = err.to_lowercase();
    if l.contains("implement mount") || l.contains("not supported") {
        let scheme = uri.split("://").next().unwrap_or("");
        let pkg = match scheme {
            "smb" => "gvfs and gvfs-smb (gvfs-backends on Debian/Ubuntu)",
            "dav" | "davs" => "gvfs and its WebDAV backend (gvfs-dnssd on Arch, gvfs-backends on Debian/Ubuntu)",
            "nfs" => "gvfs and gvfs-nfs",
            _ => "gvfs",
        };
        return format!("{scheme}:// isn't available — install {pkg} (then log out and back in, or restart gvfsd)");
    }
    err.to_owned()
}

pub fn unmount(uri: &str) -> Result<(), String> {
    gio(&["mount", "-u", uri]).map(drop)
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct Mounted {
    pub name: String,
    pub uri: String,
}

/// `gio mount -l` lines like "Mount(0): pi on 10.0.0.2 -> sftp://pi@10.0.0.2/".
pub fn parse_mount_list(out: &str) -> Vec<Mounted> {
    out.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("Mount(")?;
            let (_, rest) = rest.split_once("): ")?;
            let (name, uri) = rest.rsplit_once(" -> ")?;
            is_remote(uri).then(|| Mounted { name: name.to_owned(), uri: uri.to_owned() })
        })
        .collect()
}
pub fn mounted() -> Vec<Mounted> {
    gio(&["mount", "-l"]).map(|o| parse_mount_list(&o)).unwrap_or_default()
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Found {
    pub name: String,
    pub uri: String,
}

/// `gio list -u -a standard::display-name,standard::target-uri URI` lines: "uri\tsize\t(type)\tattr=v attr=v".
pub fn parse_list(out: &str) -> Vec<Found> {
    out.lines()
        .filter_map(|l| {
            let mut cols = l.split('\t');
            let uri = cols.next()?.trim().to_owned();
            let attrs = cols.nth(2).unwrap_or("");
            let get = |k: &str| {
                let key = format!("standard::{k}=");
                let i = attrs.find(&key)? + key.len();
                let rest = &attrs[i..];
                Some(rest[..rest.find(" standard::").unwrap_or(rest.len())].to_owned())
            };
            let target = get("target-uri").filter(|t| !t.is_empty());
            let name = get("display-name").unwrap_or_else(|| uri.trim_end_matches('/').rsplit('/').next().unwrap_or(&uri).to_owned());
            (!uri.is_empty()).then(|| Found { name, uri: target.unwrap_or(uri) })
        })
        .collect()
}

/// Run gio with a time limit (the first SMB browse sits on a ~20 s NetBIOS broadcast timeout).
fn gio_timed(args: &[&str], secs: u64) -> Result<std::process::Output, String> {
    let mut child = Command::new("gio").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("gio: {e} (install gvfs)"))?;
    let t = Instant::now();
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if t.elapsed() > Duration::from_secs(secs) {
            let _ = child.kill();
            return Err("browsing the network timed out".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().map_err(|e| e.to_string())
}

/// What's inside a browsable network location (smb://, smb://WORKGROUP/, smb://server/). gvfs wants the
/// browse location itself mounted first; that's done anonymously when needed.
pub fn browse(uri: &str) -> Result<Vec<Found>, String> {
    let list = || -> Result<Vec<Found>, String> {
        let o = gio_timed(&["list", "-u", "-a", "standard::display-name,standard::target-uri", uri], 35)?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).trim().trim_start_matches("gio: ").to_owned());
        }
        Ok(parse_list(&String::from_utf8_lossy(&o.stdout)))
    };
    match list() {
        Err(e) if e.contains("not mounted") => {
            gio_timed(&["mount", "--anonymous", uri], 35)?;
            list()
        }
        r => r,
    }
}

// ---------- keyring (secret-tool) ----------

pub fn saved_password(uri: &str) -> Option<String> {
    secret_lookup(&[("uri", uri)])
}
pub fn save_password(uri: &str, password: &str) -> Result<(), String> {
    secret_store(&format!("Whale Cabinet: {uri}"), &[("uri", uri)], password)
}
pub fn forget_password(uri: &str) {
    secret_clear(&[("uri", uri)]);
}

/// Keyring entries are tagged `application whale-cabinet` plus these attributes.
fn secret_args<'a>(cmd: &'a str, attrs: &[(&'a str, &'a str)]) -> Vec<&'a str> {
    let mut v = vec![cmd, "application", "whale-cabinet"];
    for (k, val) in attrs {
        v.extend([*k, *val]);
    }
    v
}
pub fn secret_lookup(attrs: &[(&str, &str)]) -> Option<String> {
    let o = Command::new("secret-tool").args(secret_args("lookup", attrs)).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let p = String::from_utf8_lossy(&o.stdout).trim_end_matches('\n').to_owned();
    (o.status.success() && !p.is_empty()).then_some(p)
}
/// The secret goes to secret-tool on stdin, never on a command line.
pub fn secret_store(label: &str, attrs: &[(&str, &str)], secret: &str) -> Result<(), String> {
    let label = format!("--label={label}");
    let mut args = secret_args("store", attrs);
    args.insert(1, &label);
    let mut c = Command::new("secret-tool")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("secret-tool: {e} (install libsecret)"))?;
    c.stdin.take().unwrap().write_all(secret.as_bytes()).map_err(|e| e.to_string())?;
    let o = c.wait_with_output().map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_owned())
    }
}
pub fn secret_clear(attrs: &[(&str, &str)]) {
    let _ = Command::new("secret-tool").args(secret_args("clear", attrs)).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config_hosts() {
        let t = "Host pi nas\n  HostName 10.0.0.2\nHost *.lan\nhost  box !bad\nMatch all\nHost pi\n";
        assert_eq!(parse_ssh_config(t), vec!["pi", "nas", "box"]);
    }

    #[test]
    fn answers_gio_prompts() {
        let c = Creds { user: "me".into(), domain: "WORK".into(), password: "pw".into(), trust: false };
        let mut asked = false;
        let msg = "Authentication Required\nEnter user and password for share “x” on “nas”:\n";
        assert_eq!(answer(&format!("{msg}User [michael]: "), &c, &mut asked), Step::Send("me".into()));
        assert_eq!(answer("Domain [WORKGROUP]: ", &c, &mut asked), Step::Send("WORK".into()));
        assert_eq!(answer("Password: ", &c, &mut asked), Step::Send("pw".into()));
        assert!(matches!(answer("Password: ", &c, &mut asked), Step::NeedAuth(m) if m.starts_with("Wrong password")), "asked twice = wrong password");
        assert_eq!(answer("Password", &c, &mut false), Step::Wait, "prompt not complete yet");
        let none = Creds::default();
        assert!(matches!(answer(&format!("{msg}Password: "), &none, &mut false), Step::NeedAuth(m) if m.contains("Enter user and password")));
        let q = "The identity of the remote computer (pi) is unknown.\n[0] Log In Anyway\n[1] Cancel Login\nChoice: ";
        assert!(matches!(answer(q, &c, &mut false), Step::Question(m) if m.contains("identity")));
        let trusting = Creds { trust: true, ..c.clone() };
        assert_eq!(answer(q, &trusting, &mut false), Step::Send("0".into()));
        let q2 = "[0] Cancel\n[1] Accept\nChoice: ";
        assert_eq!(answer(q2, &trusting, &mut false), Step::Send("1".into()));
    }

    #[test]
    fn parses_gio_output() {
        let l = "Drive(0): Samsung SSD\nMount(0): pi on 10.0.0.2 -> sftp://pi@10.0.0.2/\nMount(1): STICK -> file:///run/media/m/STICK\n  Type: GProxyMount\n";
        assert_eq!(parse_mount_list(l), vec![Mounted { name: "pi on 10.0.0.2".into(), uri: "sftp://pi@10.0.0.2/".into() }]);
        let ls = "smb://WORKGROUP/\t0\t(directory)\tstandard::display-name=WORKGROUP standard::target-uri=\nsmb://nas/\t0\t(mountable)\tstandard::display-name=My NAS standard::target-uri=smb://nas/\n";
        assert_eq!(parse_list(ls), vec![Found { name: "WORKGROUP".into(), uri: "smb://WORKGROUP/".into() }, Found { name: "My NAS".into(), uri: "smb://nas/".into() }]);
        assert!(friendly("smb://nas/x", "smb://nas/x: volume doesn’t implement mount").contains("gvfs-smb"));
        assert_eq!(friendly("sftp://h/", "Permission denied"), "Permission denied");
        assert!(is_remote("sftp://x") && is_remote("davs://x/y") && !is_remote("file:///") && !is_remote("remote:/"));
    }
}
