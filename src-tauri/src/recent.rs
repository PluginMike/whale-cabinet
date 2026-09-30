//! Recent files ⇄ ~/.local/share/recently-used.xbel, the list GTK apps (and KDE's file dialogs) share.
//! Reading takes the newest of modified/visited/app-modified; opening a file from here records it the way
//! GtkRecentManager does (bookmark + application with a count), leaving everything else untouched.

use crate::places::{escape, unescape};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub fn xbel_path() -> PathBuf {
    // test runs keep their own list
    if crate::isolated() {
        if let Some(d) = std::env::var_os("WC_SELFTEST") {
            return Path::new(&d).join("recently-used.xbel");
        }
    }
    dirs::data_dir().unwrap_or_default().join("recently-used.xbel")
}

const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\"\n      xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\"\n      xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\"\n>\n</xbel>\n";

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let i = tag.find(&key)? + key.len();
    Some(&tag[i..i + tag[i..].find('"')?])
}

/// (start, end) byte ranges of each `<bookmark …>…</bookmark>` block.
fn blocks(text: &str) -> Vec<(usize, usize)> {
    let mut v = vec![];
    let mut pos = 0;
    while let Some(i) = text[pos..].find("<bookmark ") {
        let s = pos + i;
        let Some(j) = text[s..].find("</bookmark>") else { break };
        let e = s + j + "</bookmark>".len();
        v.push((s, e));
        pos = e;
    }
    v
}

// ---------- ISO 8601 (UTC) ⇄ ms, days-from-civil ----------
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
pub fn parse_time(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let days = days_from_civil(n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let secs = days * 86400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?;
    let frac = s.get(19..).and_then(|f| f.strip_prefix('.')).map(|f| f.trim_end_matches('Z')).unwrap_or("");
    let ms = format!("{frac:0<3}").get(..3).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    Some(secs * 1000 + ms)
}
pub fn fmt_time(ms: i64) -> String {
    let (secs, sub) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{sub:03}000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Item {
    pub path: String,
    /// Last use, ms since epoch.
    pub used: i64,
    /// App it was last opened with ("" if unknown).
    pub app: String,
}

pub fn parse(text: &str) -> Vec<Item> {
    let mut v: Vec<Item> = blocks(text)
        .into_iter()
        .filter_map(|(s, e)| {
            let b = &text[s..e];
            let open = &b[..b.find('>')?];
            let path = crate::jobs::from_uri(&unescape(attr(open, "href")?))?;
            let mut used = ["modified", "visited", "added"].iter().filter_map(|a| attr(open, a).and_then(parse_time)).max().unwrap_or(0);
            let mut app = String::new();
            let mut pos = 0;
            while let Some(i) = b[pos..].find("<bookmark:application ") {
                let t = &b[pos + i..];
                let t = &t[..t.find("/>").unwrap_or(t.len())];
                let when = attr(t, "modified").and_then(parse_time).unwrap_or(0);
                if when >= used || app.is_empty() {
                    used = used.max(when);
                    app = unescape(attr(t, "name").unwrap_or(""));
                }
                pos += i + 1;
            }
            Some(Item { path, used, app })
        })
        .collect();
    v.sort_by(|a, b| b.used.cmp(&a.used));
    // the same file can have several bookmarks (other apps' writers): keep the newest
    let mut seen = std::collections::HashSet::new();
    v.retain(|it| seen.insert(it.path.clone()));
    v
}

/// Record that `path` was opened with the app `name` (desktop Exec `exec`) at `now` (ms).
pub fn record(text: &str, path: &str, mime: &str, name: &str, exec: &str, now: i64) -> String {
    let text = if text.contains("</xbel>") { text.to_owned() } else { HEAD.to_owned() };
    let t = fmt_time(now);
    let href = crate::jobs::to_uri(path);
    let app_line = format!("<bookmark:application name=\"{}\" exec=\"{}\" modified=\"{t}\" count=\"1\"/>", escape(name), escape(exec).replace('\'', "&apos;"));
    for (s, e) in blocks(&text) {
        let b = &text[s..e];
        let Some(end) = b.find('>') else { continue };
        if attr(&b[..end], "href").map(unescape).as_deref() != Some(href.as_str()) {
            continue;
        }
        let mut nb = b.to_owned();
        for a in ["modified", "visited"] {
            let key = format!(" {a}=\"");
            let open_end = nb.find('>').unwrap_or(0);
            match nb[..open_end].find(&key) {
                Some(i) => {
                    let v0 = i + key.len();
                    let v1 = v0 + nb[v0..].find('"').unwrap_or(0);
                    nb.replace_range(v0..v1, &t);
                }
                None => nb.insert_str(open_end, &format!(" {a}=\"{t}\"")),
            }
        }
        let key = format!("<bookmark:application name=\"{}\"", escape(name));
        if let Some(i) = nb.find(&key) {
            let j = i + nb[i..].find("/>").unwrap_or(0);
            let tag = &nb[i..j];
            let count = attr(tag, "count").and_then(|c| c.parse::<u32>().ok()).unwrap_or(0) + 1;
            let mut new_tag = tag.to_owned();
            for (k, v) in [("modified", t.clone()), ("count", count.to_string())] {
                let kk = format!(" {k}=\"");
                if let Some(a) = new_tag.find(&kk) {
                    let v0 = a + kk.len();
                    let v1 = v0 + new_tag[v0..].find('"').unwrap_or(0);
                    new_tag.replace_range(v0..v1, &v);
                }
            }
            nb.replace_range(i..j, &new_tag);
        } else if let Some(i) = nb.find("</bookmark:applications>") {
            nb.insert_str(i, &format!("{app_line}\n        "));
        }
        return format!("{}{}{}", &text[..s], nb, &text[e..]);
    }
    let block = format!(
        "  <bookmark href=\"{}\" added=\"{t}\" modified=\"{t}\" visited=\"{t}\">\n    <info>\n      <metadata owner=\"http://freedesktop.org\">\n        <mime:mime-type type=\"{}\"/>\n        <bookmark:applications>\n          {app_line}\n        </bookmark:applications>\n      </metadata>\n    </info>\n  </bookmark>\n",
        escape(&href),
        escape(mime)
    );
    let at = text.rfind("</xbel>").unwrap_or(text.len());
    format!("{}{}{}", &text[..at], block, &text[at..])
}

/// Drop the given paths (None = everything) from the list.
pub fn remove(text: &str, paths: Option<&[String]>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e) in blocks(text) {
        let b = &text[s..e];
        let open = &b[..b.find('>').unwrap_or(0)];
        let p = attr(open, "href").map(unescape).and_then(|h| crate::jobs::from_uri(&h));
        if paths.is_none_or(|ps| p.as_ref().is_some_and(|p| ps.contains(p))) {
            // take the indentation and newline around the block with it
            let ls = text[..s].trim_end_matches([' ', '\t']).len();
            out.push_str(&text[last..ls]);
            last = if text[e..].starts_with('\n') { e + 1 } else { e };
        }
    }
    out.push_str(&text[last..]);
    out
}

pub fn read() -> String {
    std::fs::read_to_string(xbel_path()).unwrap_or_default()
}
pub fn write(text: &str) -> Result<(), String> {
    let p = xbel_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap_or(Path::new("/")));
    let tmp = p.with_extension("xbel.whale-tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, p).map_err(|e| e.to_string())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0"
      xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks"
      xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info"
>
  <bookmark href="file:///home/m/a%20b.txt" added="2026-09-01T10:00:00.000000Z" modified="2026-09-01T10:00:00.000000Z" visited="2026-09-01T10:00:00.000000Z">
    <info>
      <metadata owner="http://freedesktop.org">
        <mime:mime-type type="text/plain"/>
        <bookmark:applications>
          <bookmark:application name="Kate" exec="&apos;kate %u&apos;" modified="2026-09-02T08:30:00.500000Z" count="3"/>
        </bookmark:applications>
      </metadata>
    </info>
  </bookmark>
  <bookmark href="file:///home/m/pic.png" added="2026-09-03T00:00:00Z" modified="2026-09-03T00:00:00Z" visited="2026-09-03T00:00:00Z">
  </bookmark>
  <bookmark href="file:///home/m/pic.png" added="2026-08-01T00:00:00Z" modified="2026-08-01T00:00:00Z" visited="2026-08-01T00:00:00Z">
  </bookmark>
</xbel>
"#;

    #[test]
    fn times_roundtrip() {
        let ms = parse_time("2026-09-02T08:30:00.500000Z").unwrap();
        assert_eq!(fmt_time(ms), "2026-09-02T08:30:00.500000Z");
        assert_eq!(parse_time("1970-01-01T00:00:01Z"), Some(1000));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951868800000));
    }

    #[test]
    fn parse_record_remove() {
        let v = parse(SAMPLE);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].path, "/home/m/pic.png");
        assert_eq!(v[1], Item { path: "/home/m/a b.txt".into(), used: parse_time("2026-09-02T08:30:00.5Z").unwrap(), app: "Kate".into() });

        let now = parse_time("2026-09-30T12:00:00Z").unwrap();
        let again = record(SAMPLE, "/home/m/a b.txt", "text/plain", "Kate", "kate %u", now);
        assert!(again.contains(r#"count="4""#), "{again}");
        assert_eq!(parse(&again)[0].path, "/home/m/a b.txt");
        assert_eq!(again.matches("<bookmark ").count(), 3);
        let other = record(&again, "/home/m/a b.txt", "text/plain", "Gedit", "gedit %U", now + 1);
        assert_eq!(parse(&other)[0].app, "Gedit");
        let fresh = record(SAMPLE, "/tmp/new file.pdf", "application/pdf", "Okular", "okular %U", now);
        assert_eq!(parse(&fresh)[0], Item { path: "/tmp/new file.pdf".into(), used: now, app: "Okular".into() });
        let from_nothing = record("", "/x", "text/plain", "A", "a", now);
        assert!(from_nothing.starts_with("<?xml") && parse(&from_nothing).len() == 1);

        let gone = remove(&fresh, Some(&["/tmp/new file.pdf".into()]));
        assert_eq!(gone, SAMPLE, "removing gives back the file byte for byte");
        assert!(parse(&remove(SAMPLE, None)).is_empty());
        assert!(remove(SAMPLE, None).trim_end().ends_with("</xbel>"));
    }
}
