//! Places sidebar ⇄ ~/.local/share/user-places.xbel, the file Dolphin and KDE/Qt file dialogs use.
//! Edits touch only the `<bookmark>` blocks they need to; everything else (KDE metadata, hidden and
//! system items, remote/search entries) is kept byte-for-byte.

use serde::Serialize;
use std::path::PathBuf;

pub fn xbel_path() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join("user-places.xbel")
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Place {
    pub href: String,
    pub title: String,
    /// Local path for file:// entries, "trash:/" for the trash, else empty.
    pub path: String,
    pub icon: String,
    pub hidden: bool,
    pub system: bool,
}

pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let ent = &tail[1..end];
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match c {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The file split into: text before the first bookmark, the bookmark blocks, and the text after the last one.
struct Doc {
    head: String,
    blocks: Vec<String>,
    tail: String,
}

fn split(text: &str) -> Doc {
    let mut blocks = vec![];
    let mut pos = 0;
    let mut head_end = None;
    let mut tail_start = 0;
    while let Some(i) = text[pos..].find("<bookmark ") {
        let start = pos + i;
        let Some(j) = text[start..].find("</bookmark>") else { break };
        let end = start + j + "</bookmark>".len();
        head_end.get_or_insert(start);
        blocks.push(text[start..end].to_owned());
        pos = end;
        tail_start = end;
    }
    match head_end {
        Some(h) => Doc { head: text[..h].to_owned(), blocks, tail: text[tail_start..].to_owned() },
        None => {
            // no bookmarks yet: insert before </xbel>
            let at = text.rfind("</xbel>").unwrap_or(text.len());
            Doc { head: text[..at].to_owned() + " ", blocks, tail: format!("\n{}", &text[at..]) }
        }
    }
}

fn join(d: &Doc) -> String {
    format!("{}{}{}", d.head, d.blocks.join("\n "), d.tail)
}

fn tag<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let i = block.find(&open)? + open.len();
    let j = block[i..].find(&format!("</{name}>"))?;
    Some(&block[i..i + j])
}

fn attr<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let i = block.find(&key)? + key.len();
    let j = block[i..].find('"')?;
    Some(&block[i..i + j])
}

fn to_place(block: &str) -> Place {
    let href = unescape(attr(block, "href").unwrap_or(""));
    let path = if let Some(p) = href.strip_prefix("file://") {
        crate::jobs::from_uri(&format!("file://{p}")).unwrap_or_default()
    } else if href.starts_with("trash:") {
        "trash:/".into()
    } else {
        String::new()
    };
    let icon = block.find("<bookmark:icon").and_then(|i| attr(&block[i..], "name")).unwrap_or("").to_owned();
    Place {
        title: unescape(tag(block, "title").unwrap_or("")),
        hidden: tag(block, "IsHidden") == Some("true"),
        system: tag(block, "isSystemItem") == Some("true"),
        href,
        path,
        icon,
    }
}

pub fn parse(text: &str) -> Vec<Place> {
    split(text).blocks.iter().map(|b| to_place(b)).collect()
}

fn block_for(path: &str, title: &str, id: &str) -> String {
    format!(
        "<bookmark href=\"{}\">\n  <title>{}</title>\n  <info>\n   <metadata owner=\"http://freedesktop.org\">\n    <bookmark:icon name=\"folder\"/>\n   </metadata>\n   <metadata owner=\"http://www.kde.org\">\n    <ID>{}</ID>\n   </metadata>\n  </info>\n </bookmark>",
        escape(&crate::jobs::to_uri(path)),
        escape(title),
        id
    )
}

pub const EMPTY: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xbel>\n<xbel xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\" xmlns:kdepriv=\"http://www.kde.org/kdepriv\" xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\">\n</xbel>\n";

/// Add a folder bookmark at `index` (None = end). No-op if it's already there.
pub fn add(text: &str, path: &str, title: &str, index: Option<usize>) -> String {
    let mut d = split(if text.trim().is_empty() { EMPTY } else { text });
    let href = crate::jobs::to_uri(path);
    if d.blocks.iter().any(|b| unescape(attr(b, "href").unwrap_or("")) == href) {
        return join(&d);
    }
    let id = format!("{}/{}", crate::ops::now_secs(), d.blocks.len());
    let at = index.unwrap_or(d.blocks.len()).min(d.blocks.len());
    d.blocks.insert(at, block_for(path, title, &id));
    join(&d)
}

pub fn remove(text: &str, href: &str) -> String {
    let mut d = split(text);
    d.blocks.retain(|b| unescape(attr(b, "href").unwrap_or("")) != href);
    join(&d)
}

/// Move the bookmark `href` to position `to` (among all bookmarks, hidden ones included).
pub fn reorder(text: &str, href: &str, to: usize) -> String {
    let mut d = split(text);
    if let Some(i) = d.blocks.iter().position(|b| unescape(attr(b, "href").unwrap_or("")) == href) {
        let b = d.blocks.remove(i);
        d.blocks.insert(to.min(d.blocks.len()), b);
    }
    join(&d)
}

pub fn rename(text: &str, href: &str, title: &str) -> String {
    let mut d = split(text);
    for b in &mut d.blocks {
        if unescape(attr(b, "href").unwrap_or("")) == href {
            if let (Some(i), Some(j)) = (b.find("<title>"), b.find("</title>")) {
                b.replace_range(i + 7..j, &escape(title));
            }
        }
    }
    join(&d)
}

pub fn read() -> String {
    std::fs::read_to_string(xbel_path()).unwrap_or_default()
}
pub fn write(text: &str) -> Result<(), String> {
    let p = xbel_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let tmp = p.with_extension("xbel.whale-tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xbel>
<xbel xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks">
 <info>
  <metadata owner="http://www.kde.org">
   <kde_places_version>4</kde_places_version>
  </metadata>
 </info>
 <bookmark href="file:///home/m">
  <title>Home</title>
  <info>
   <metadata owner="http://freedesktop.org">
    <bookmark:icon name="user-home"/>
   </metadata>
   <metadata owner="http://www.kde.org">
    <ID>1/0</ID>
    <isSystemItem>true</isSystemItem>
   </metadata>
  </info>
 </bookmark>
 <bookmark href="remote:/">
  <title>Network</title>
  <info><metadata owner="http://www.kde.org"><IsHidden>true</IsHidden></metadata></info>
 </bookmark>
 <bookmark href="file:///home/m/My%20Stuff">
  <title>Tom &amp; Jerry</title>
 </bookmark>
 <bookmark href="trash:/">
  <title>Trash</title>
 </bookmark>
</xbel>
"#;

    #[test]
    fn parse_places() {
        let p = parse(SAMPLE);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], Place { href: "file:///home/m".into(), title: "Home".into(), path: "/home/m".into(), icon: "user-home".into(), hidden: false, system: true });
        assert!(p[1].hidden && p[1].path.is_empty());
        assert_eq!(p[2].title, "Tom & Jerry");
        assert_eq!(p[2].path, "/home/m/My Stuff");
        assert_eq!(p[3].path, "trash:/");
    }

    #[test]
    fn edits_preserve_everything_else() {
        let added = add(SAMPLE, "/data/a <b>", "A & B", Some(1));
        let p = parse(&added);
        assert_eq!(p[1].path, "/data/a <b>");
        assert_eq!(p[1].title, "A & B");
        assert!(added.contains("<kde_places_version>4</kde_places_version>"));
        assert!(added.contains("<IsHidden>true</IsHidden>"));
        assert_eq!(add(&added, "/data/a <b>", "dup", None), added, "no duplicates");
        // remove gives back the original byte-for-byte
        assert_eq!(remove(&added, &p[1].href), SAMPLE);
        let moved = reorder(SAMPLE, "trash:/", 0);
        assert_eq!(parse(&moved).iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), vec!["Trash", "Home", "Network", "Tom & Jerry"]);
        assert_eq!(reorder(&moved, "trash:/", 3), SAMPLE);
        assert_eq!(parse(&rename(SAMPLE, "file:///home/m", "Casa <3>"))[0].title, "Casa <3>");
    }

    #[test]
    fn creates_file_when_missing() {
        let t = add("", "/x", "X", None);
        assert!(t.starts_with("<?xml"));
        assert_eq!(parse(&t).len(), 1);
        assert!(t.trim_end().ends_with("</xbel>"));
    }
}
