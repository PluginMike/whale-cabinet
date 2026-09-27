//! Embedded terminal panel (F4): a PTY running the user's $SHELL, streamed to xterm.js.
//! It follows the current folder by typing `cd` — only when the shell is idle at its prompt.

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

struct Term {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

#[derive(Default)]
pub struct Terms(Mutex<HashMap<u32, Term>>);

#[derive(Serialize, Clone)]
struct Data {
    id: u32,
    data: String,
}

/// Split `buf` into the longest valid UTF-8 prefix and the incomplete tail (kept for the next read).
pub fn utf8_split(buf: &[u8]) -> (String, Vec<u8>) {
    match std::str::from_utf8(buf) {
        Ok(s) => (s.to_owned(), vec![]),
        Err(e) if e.error_len().is_none() => {
            let n = e.valid_up_to();
            (String::from_utf8_lossy(&buf[..n]).into_owned(), buf[n..].to_vec())
        }
        Err(_) => (String::from_utf8_lossy(buf).into_owned(), vec![]),
    }
}

/// Keystrokes that replace whatever is typed at the prompt with `cd -- '<dir>'` (leading space: kept out of history).
pub fn cd_keys(dir: &str) -> String {
    format!("\x05\x15 cd -- '{}'\r", dir.replace('\'', r"'\''"))
}

#[tauri::command]
pub fn term_open(id: u32, cwd: String, cols: u16, rows: u16, app: AppHandle, terms: State<Terms>) -> Result<(), String> {
    let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.cwd(if std::path::Path::new(&cwd).is_dir() { cwd } else { "/".into() });
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    terms.0.lock().unwrap().insert(id, Term { master: pair.master, writer, child });
    std::thread::spawn(move || {
        let mut buf = [0u8; 16384];
        let mut carry: Vec<u8> = vec![];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    carry.extend_from_slice(&buf[..n]);
                    let (s, rest) = utf8_split(&carry);
                    carry = rest;
                    if !s.is_empty() {
                        let _ = app.emit("term-data", Data { id, data: s });
                    }
                }
            }
        }
        let _ = app.emit("term-exit", id);
    });
    Ok(())
}

#[tauri::command]
pub fn term_write(id: u32, data: String, terms: State<Terms>) {
    if let Some(t) = terms.0.lock().unwrap().get_mut(&id) {
        let _ = t.writer.write_all(data.as_bytes());
        let _ = t.writer.flush();
    }
}

#[tauri::command]
pub fn term_resize(id: u32, cols: u16, rows: u16, terms: State<Terms>) {
    if let Some(t) = terms.0.lock().unwrap().get(&id) {
        let _ = t.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }
}

/// cd the shell to `dir`, unless a program (vim, htop, …) is running in the foreground.
#[tauri::command]
pub fn term_cd(id: u32, dir: String, terms: State<Terms>) -> bool {
    let mut g = terms.0.lock().unwrap();
    let Some(t) = g.get_mut(&id) else { return false };
    let idle = match (t.master.process_group_leader(), t.child.process_id()) {
        (Some(fg), Some(shell)) => fg as u32 == shell,
        _ => false,
    };
    if idle {
        let _ = t.writer.write_all(cd_keys(&dir).as_bytes());
        let _ = t.writer.flush();
    }
    idle
}

#[tauri::command]
pub fn term_close(id: u32, terms: State<Terms>) {
    if let Some(mut t) = terms.0.lock().unwrap().remove(&id) {
        let _ = t.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_carry_across_reads() {
        let s = "héllo 💾".as_bytes();
        let (a, rest) = utf8_split(&s[..s.len() - 2]);
        assert_eq!(a, "héllo ");
        assert_eq!(rest.len(), 2);
        let mut next = rest.clone();
        next.extend_from_slice(&s[s.len() - 2..]);
        assert_eq!(utf8_split(&next), ("💾".to_string(), vec![]));
    }

    #[test]
    fn cd_quoting() {
        assert_eq!(cd_keys("/a b/it's"), "\x05\x15 cd -- '/a b/it'\\''s'\r");
    }
}
