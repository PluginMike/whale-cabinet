//! Thumbnails (freedesktop cache ~/.cache/thumbnails, shared with Dolphin/Nautilus), text previews and folder stats.

use md5::{Digest, Md5};
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn cache_root() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir)
}

pub fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn text_chunks(p: &Path) -> Option<Vec<(String, String)>> {
    let dec = png::Decoder::new(std::io::BufReader::new(fs::File::open(p).ok()?));
    let r = dec.read_info().ok()?;
    Some(r.info().uncompressed_latin1_text.iter().map(|t| (t.keyword.clone(), t.text.clone())).collect())
}

/// Is the cached thumbnail at `p` for this file version? (Thumb::MTime must match.)
fn fresh(p: &Path, mtime: i64) -> bool {
    text_chunks(p).is_some_and(|c| c.iter().any(|(k, v)| k == "Thumb::MTime" && v.parse::<i64>().ok() == Some(mtime)))
}

fn kind(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico" => "image",
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg" => "video",
        "pdf" => "pdf",
        _ => match infer::get_from_path(path).ok().flatten().map(|t| t.matcher_type()) {
            Some(infer::MatcherType::Image) => "image",
            Some(infer::MatcherType::Video) => "video",
            _ => "",
        },
    }
}

/// Render `src` to an RGBA image no larger than `size`×`size`.
fn render(src: &Path, size: u32) -> Result<image::DynamicImage, String> {
    match kind(src) {
        "image" => image::open(src).map(|i| i.thumbnail(size, size)).map_err(|e| e.to_string()),
        k @ ("video" | "pdf") => {
            let tmp = tempfile_path();
            let ok = if k == "video" {
                Command::new("ffmpegthumbnailer").arg("-i").arg(src).arg("-o").arg(&tmp).args(["-s", &size.to_string(), "-c", "png"]).output()
            } else {
                // pdftoppm appends ".png" to the output root with -singlefile
                Command::new("pdftoppm").args(["-png", "-singlefile", "-f", "1", "-l", "1", "-scale-to", &size.to_string()]).arg(src).arg(tmp.with_extension("")).output()
            }
            .map_err(|e| format!("{e} (is {} installed?)", if k == "video" { "ffmpegthumbnailer" } else { "poppler" }))?;
            let img = image::open(&tmp).map_err(|e| format!("{}: {e}", String::from_utf8_lossy(&ok.stderr).trim()));
            let _ = fs::remove_file(&tmp);
            img.map(|i| i.thumbnail(size, size))
        }
        _ => Err("no thumbnailer for this type".into()),
    }
}

fn tempfile_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("whale-cabinet-{}-{}.png", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

fn write_png(img: &image::DynamicImage, out: &Path, meta: &[(&str, String)]) -> Result<(), String> {
    let rgba = img.to_rgba8();
    fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = out.with_extension(format!("tmp{}", std::process::id()));
    let f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)); // spec: thumbnails are private
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), rgba.width(), rgba.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    for (k, v) in meta {
        enc.add_text_chunk(k.to_string(), v.clone()).map_err(|e| e.to_string())?;
    }
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(&rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// Freedesktop thumbnail for `path` (128 → normal, 256 → large), generating it if missing or stale.
/// Other sizes (bigger previews, e.g. a PDF page) go to our own cache dir. Returns the PNG path.
pub fn thumbnail(root: &Path, path: &Path, size: u32) -> Result<PathBuf, String> {
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let thumbs = root.join("thumbnails");
    if path.starts_with(&thumbs) {
        return Ok(path); // never thumbnail thumbnails
    }
    let m = fs::metadata(&path).map_err(|e| e.to_string())?;
    let uri = crate::jobs::to_uri(&path.to_string_lossy());
    let name = format!("{}.png", md5_hex(&uri));
    let (dirs, own): (Vec<PathBuf>, Option<PathBuf>) = match size {
        0..=128 => (vec![thumbs.join("normal"), thumbs.join("large"), thumbs.join("x-large")], None),
        129..=256 => (vec![thumbs.join("large"), thumbs.join("x-large")], None),
        s => {
            let d = root.join("whale-cabinet/previews").join(s.to_string());
            (vec![d.clone()], Some(d))
        }
    };
    for d in &dirs {
        let p = d.join(&name);
        if fresh(&p, m.mtime()) {
            return Ok(p);
        }
    }
    let fail = thumbs.join("fail/whale-cabinet").join(&name);
    if fresh(&fail, m.mtime()) {
        return Err("thumbnail failed before".into());
    }
    let out = own.unwrap_or_else(|| thumbs.join(if size <= 128 { "normal" } else { "large" })).join(&name);
    let meta = [("Thumb::URI", uri.clone()), ("Thumb::MTime", m.mtime().to_string()), ("Thumb::Size", m.len().to_string()), ("Software", "Whale Cabinet".into())];
    match render(&path, if size <= 128 { 128 } else { size }) {
        Ok(img) => write_png(&img, &out, &meta).map(|_| out),
        Err(e) => {
            // Remember the failure (spec: fail/<app>/) so we don't retry on every scroll.
            let _ = write_png(&image::DynamicImage::new_rgba8(1, 1), &fail, &meta);
            Err(e)
        }
    }
}

#[derive(Serialize)]
pub struct Text {
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
}

pub fn read_text(path: &Path, max: usize) -> Result<Text, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = Vec::with_capacity(max.min(1 << 20));
    f.by_ref().take(max as u64 + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let truncated = buf.len() > max;
    buf.truncate(max);
    let binary = buf.iter().take(8192).any(|&b| b == 0);
    Ok(Text { text: if binary { String::new() } else { String::from_utf8_lossy(&buf).into_owned() }, truncated, binary })
}

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct DirStats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
}

/// Recursive totals (symlinks counted, not followed).
pub fn dir_stats(p: &Path) -> DirStats {
    let mut s = DirStats::default();
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    s.dirs += 1;
                    stack.push(e.path());
                }
                Ok(_) => {
                    s.files += 1;
                    s.bytes += e.metadata().map(|m| if m.is_file() { m.len() } else { 0 }).unwrap_or(0);
                }
                Err(_) => {}
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_md5_example() {
        // From the freedesktop thumbnail spec.
        assert_eq!(md5_hex("file:///home/jens/photos/me.png"), "c6ee772d9e49320e97ec29a7eb5b1697");
    }

    #[test]
    fn thumbnail_generate_cache_and_invalidate() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cache");
        let img = t.path().join("pic one.png");
        image::DynamicImage::new_rgb8(600, 300).save(&img).unwrap();
        let th = thumbnail(&root, &img, 128).unwrap();
        assert!(th.starts_with(root.join("thumbnails/normal")));
        let (w, h) = image::image_dimensions(&th).unwrap();
        assert_eq!((w, h), (128, 64));
        assert_eq!(fs::metadata(&th).unwrap().permissions().mode() & 0o777, 0o600);
        let chunks = text_chunks(&th).unwrap();
        assert!(chunks.iter().any(|(k, v)| k == "Thumb::URI" && v.ends_with("/pic%20one.png")));
        // cached: same path, no rewrite
        let before = fs::metadata(&th).unwrap().mtime_nsec();
        assert_eq!(thumbnail(&root, &img, 100).unwrap(), th);
        assert_eq!(fs::metadata(&th).unwrap().mtime_nsec(), before);
        // stale after the file changes
        let f = fs::File::options().write(true).open(&img).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        assert!(!fresh(&th, fs::metadata(&img).unwrap().mtime()));
        thumbnail(&root, &img, 128).unwrap();
        assert!(fresh(&th, fs::metadata(&img).unwrap().mtime()));
        // big preview sizes use our own cache
        assert!(thumbnail(&root, &img, 800).unwrap().starts_with(root.join("whale-cabinet/previews/800")));
    }

    #[test]
    fn failures_are_remembered() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cache");
        let bad = t.path().join("broken.png");
        fs::write(&bad, "not a png").unwrap();
        assert!(thumbnail(&root, &bad, 128).is_err());
        assert_eq!(thumbnail(&root, &bad, 128).unwrap_err(), "thumbnail failed before");
    }

    #[test]
    fn text_and_stats() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        fs::write(d.join("a.txt"), "héllo world").unwrap();
        fs::write(d.join("b.bin"), [0u8, 1, 2]).unwrap();
        fs::create_dir(d.join("sub")).unwrap();
        fs::write(d.join("sub/c"), "12345").unwrap();
        let r = read_text(&d.join("a.txt"), 5).unwrap();
        assert!(r.truncated && !r.binary);
        assert!(read_text(&d.join("b.bin"), 100).unwrap().binary);
        let n = "héllo world".len() as u64;
        assert_eq!(dir_stats(d), DirStats { files: 3, dirs: 1, bytes: n + 3 + 5 });
    }
}
