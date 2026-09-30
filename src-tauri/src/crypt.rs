//! Encrypt / decrypt with gpg: with a password (symmetric AES-256, the password goes to gpg on stdin via
//! loopback pinentry — never on the command line) or to keys in your keyring. Folders are packed with tar first
//! (name.tar.gpg). Key-encrypted files are decrypted by gpg-agent, which asks through your pinentry.

use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn gpg() -> Command {
    let mut c = Command::new("gpg");
    c.args(["--batch", "--yes", "--no-tty"]);
    c
}

fn run(mut c: Command, stdin: Option<&[u8]>) -> Result<(), String> {
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("gpg: {e} (install gnupg)"))?;
    if let Some(data) = stdin {
        child.stdin.take().unwrap().write_all(data).map_err(|e| e.to_string())?;
    } else {
        drop(child.stdin.take());
    }
    let o = child.wait_with_output().map_err(|e| e.to_string())?;
    if o.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&o.stderr);
    let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("gpg failed").trim_start_matches("gpg: ").to_owned();
    Err(if err.contains("Bad session key") || err.contains("decryption failed") { "Wrong password".into() } else { line })
}

/// `p` if free, else "name (2).ext"…
fn free(p: PathBuf) -> PathBuf {
    if std::fs::symlink_metadata(&p).is_err() { p } else { crate::ops::unique(&p, false) }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Key {
    pub id: String,
    pub uid: String,
}

/// Keys you can encrypt to (`gpg --list-keys --with-colons`: pub lines, then their first uid).
pub fn parse_keys(out: &str) -> Vec<Key> {
    let mut v: Vec<Key> = vec![];
    let mut want_uid = false;
    for l in out.lines() {
        let f: Vec<&str> = l.split(':').collect();
        match f.first() {
            Some(&"pub") if !f.get(1).is_some_and(|s| s.contains(['r', 'e'])) => {
                v.push(Key { id: f.get(4).unwrap_or(&"").to_string(), uid: String::new() });
                want_uid = true;
            }
            Some(&"pub") => want_uid = false, // revoked / expired
            Some(&"uid") if want_uid => {
                if let Some(k) = v.last_mut() {
                    k.uid = f.get(9).unwrap_or(&"").replace("\\x3a", ":");
                }
                want_uid = false;
            }
            _ => {}
        }
    }
    v
}
pub fn keys() -> Vec<Key> {
    gpg().args(["--list-keys", "--with-colons"]).output().ok().map(|o| parse_keys(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

/// Encrypt `path` next to itself; returns the new file. `password` or `recipients` (key ids).
pub fn encrypt(path: &Path, password: Option<&str>, recipients: &[String]) -> Result<PathBuf, String> {
    let dir = path.parent().ok_or("no parent folder")?;
    let name = path.file_name().ok_or("no name")?.to_string_lossy().into_owned();
    let is_dir = path.is_dir();
    let out = free(dir.join(if is_dir { format!("{name}.tar.gpg") } else { format!("{name}.gpg") }));
    let mut c = gpg();
    match password {
        Some(_) => {
            c.args(["--pinentry-mode", "loopback", "--passphrase-fd", "0", "--symmetric", "--cipher-algo", "AES256"]);
        }
        None if !recipients.is_empty() => {
            c.arg("--encrypt");
            for r in recipients {
                c.args(["--recipient", r]);
            }
        }
        None => return Err("give a password or pick a key".into()),
    }
    c.arg("--output").arg(&out);
    // NOTE: a folder is tarred to a hidden temp file first (needs its size in free space once); pipe tar
    // into gpg on an inherited fd if that ever matters — gpg already reads the password from stdin
    let tmp = dir.join(format!(".{name}.whale-tar"));
    let src = if is_dir {
        let st = Command::new("tar").arg("-cf").arg(&tmp).arg("-C").arg(dir).arg("--").arg(&name).stderr(Stdio::null()).status().map_err(|e| format!("tar: {e}"))?;
        if !st.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("couldn't pack {name}"));
        }
        tmp.clone()
    } else {
        path.to_path_buf()
    };
    c.arg(&src);
    let r = run(c, password.map(|p| p.as_bytes()));
    if is_dir {
        let _ = std::fs::remove_file(&tmp);
    }
    r.inspect_err(|_| {
        let _ = std::fs::remove_file(&out);
    })?;
    Ok(out)
}

/// Is this file encrypted with a password (vs. to a key)?
pub fn is_symmetric(path: &Path) -> bool {
    gpg().args(["--list-packets", "--pinentry-mode", "cancel"]).arg(path).output().map(|o| String::from_utf8_lossy(&o.stdout).contains(":symkey enc packet:")).unwrap_or(false)
}

/// Decrypt next to the file ("x.pdf.gpg" → "x.pdf"). Password files need `password`; key files ask via pinentry.
pub fn decrypt(path: &Path, password: Option<&str>) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or("no name")?.to_string_lossy().into_owned();
    let stem = ["gpg", "pgp", "asc"].iter().find_map(|x| name.strip_suffix(&format!(".{x}"))).filter(|s| !s.is_empty()).unwrap_or(&name).to_owned();
    let out = free(path.with_file_name(if stem == name { format!("{name}.decrypted") } else { stem }));
    let mut c = if password.is_some() {
        let mut c = gpg();
        c.args(["--pinentry-mode", "loopback", "--passphrase-fd", "0"]);
        c
    } else {
        // not --batch: gpg-agent asks for the key's passphrase through the GUI pinentry
        let mut c = Command::new("gpg");
        c.args(["--yes"]).env_remove("GPG_TTY");
        c
    };
    c.arg("--output").arg(&out).arg("--decrypt").arg(path);
    run(c, password.map(|p| p.as_bytes())).inspect_err(|_| {
        let _ = std::fs::remove_file(&out);
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys() {
        let out = "tru::1:0:0:0:3:1\npub:u:255:22:ABCDEF0123456789:1700000000:::u:::scESC::::::23::0:\nfpr:::::::::X:\nuid:u::::1700000000::H::Me <me@x\\x3aorg>::::::::::0:\npub:r:255:22:DEADBEEF:1:::-:::sc::::::23::0:\nuid:r::::1::H::Old <old@x>::::::::::0:\n";
        assert_eq!(parse_keys(out), vec![Key { id: "ABCDEF0123456789".into(), uid: "Me <me@x:org>".into() }]);
    }

    #[test]
    fn password_roundtrip_files_and_folders() {
        if crate::desktop::which("gpg").is_none() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("GNUPGHOME", home.path()); // never touch the real keyring / agent
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::write(d.join("secret.txt"), "the whale is blue").unwrap();
        let enc = encrypt(&d.join("secret.txt"), Some("pw 1"), &[]).unwrap();
        assert_eq!(enc, d.join("secret.txt.gpg"));
        assert!(!std::fs::read(&enc).unwrap().windows(5).any(|w| w == b"whale"));
        assert!(is_symmetric(&enc));
        assert_eq!(decrypt(&enc, Some("wrong")).unwrap_err(), "Wrong password");
        let dec = decrypt(&enc, Some("pw 1")).unwrap();
        assert_eq!(dec, d.join("secret (2).txt"), "doesn't overwrite the original");
        assert_eq!(std::fs::read_to_string(dec).unwrap(), "the whale is blue");

        std::fs::create_dir_all(d.join("box/in")).unwrap();
        std::fs::write(d.join("box/in/f"), "x").unwrap();
        let e2 = encrypt(&d.join("box"), Some("pw"), &[]).unwrap();
        assert_eq!(e2, d.join("box.tar.gpg"));
        let tar = decrypt(&e2, Some("pw")).unwrap();
        assert_eq!(tar, d.join("box.tar"));
        let list = Command::new("tar").arg("-tf").arg(&tar).output().unwrap();
        assert!(String::from_utf8_lossy(&list.stdout).contains("box/in/f"));
    }
}
