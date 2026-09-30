//! zoxide integration: every folder we browse is fed to `zoxide add` (so the shell's `z` learns from the file
//! manager too), and the jump box / Frequent drawer read its frecency ranking with `zoxide query -ls`.

use std::process::{Command, Stdio};

fn cmd() -> Command {
    let mut c = Command::new("zoxide");
    // zoxide leaves out its own cwd from queries; "/" is never a useful jump target anyway.
    c.current_dir("/").stdin(Stdio::null()).stderr(Stdio::null());
    // Test runs keep their own database so they never touch the user's.
    if crate::isolated() {
        if let Some(d) = std::env::var_os("WC_SELFTEST") {
            c.env("_ZO_DATA_DIR", std::path::Path::new(&d).join("zoxide"));
        }
    }
    c
}

pub fn add(dir: &str) {
    if let Ok(mut child) = cmd().args(["add", "--", dir]).stdout(Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// `zoxide query --list --score` output: "  12.5 /some/path" per line, best first.
pub fn parse(out: &str) -> Vec<(f64, String)> {
    out.lines()
        .filter_map(|l| {
            let (score, path) = l.trim_start().split_once(' ')?;
            Some((score.parse().ok()?, path.to_owned()))
        })
        .collect()
}

pub fn query(terms: &[String], limit: usize) -> Result<Vec<(f64, String)>, String> {
    let o = cmd().args(["query", "--list", "--score", "--"]).args(terms).output().map_err(|e| format!("zoxide: {e} (is zoxide installed?)"))?;
    // exit 1 with no output just means "no match"
    let mut v = parse(&String::from_utf8_lossy(&o.stdout));
    v.retain(|(_, p)| std::path::Path::new(p).is_dir());
    v.truncate(limit);
    Ok(v)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_scores_and_paths_with_spaces() {
        let v = super::parse("  88.0 /home/m/My Stuff\n   4.25 /tmp\ngarbage\n");
        assert_eq!(v, vec![(88.0, "/home/m/My Stuff".into()), (4.25, "/tmp".into())]);
    }
}
