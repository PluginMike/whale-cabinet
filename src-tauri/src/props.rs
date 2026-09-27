//! Properties dialog backend: general info, owner/group, permissions (optionally recursive), media details
//! (image size, EXIF, audio/video tags via ffprobe) and checksums.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::Command;

#[derive(Serialize)]
pub struct Props {
    pub name: String,
    pub path: String,
    pub parent: String,
    pub mime: String,
    pub is_dir: bool,
    pub size: u64,
    pub link_target: Option<String>,
    pub created: Option<i64>,
    pub modified: i64,
    pub accessed: i64,
    pub owner: String,
    pub group: String,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    /// Can the current user change the permissions?
    pub can_chmod: bool,
}

fn name_for(file: &str, id: u32) -> String {
    fs::read_to_string(file)
        .ok()
        .and_then(|t| t.lines().find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() > 2 && f[2].parse::<u32>().ok() == Some(id)).then(|| f[0].to_owned())
        }))
        .unwrap_or_else(|| id.to_string())
}

pub fn props(p: &Path) -> Result<Props, String> {
    let lm = fs::symlink_metadata(p).map_err(|e| e.to_string())?;
    let m = fs::metadata(p).unwrap_or_else(|_| lm.clone());
    let ms = |t: std::io::Result<std::time::SystemTime>| t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
    let me = unsafe { libc::geteuid() };
    Ok(Props {
        name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
        path: p.to_string_lossy().into_owned(),
        parent: p.parent().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(),
        mime: crate::desktop::mime_of(p),
        is_dir: m.is_dir(),
        size: if m.is_file() { m.len() } else { 0 },
        link_target: lm.file_type().is_symlink().then(|| fs::read_link(p).map(|t| t.to_string_lossy().into_owned()).unwrap_or_default()),
        created: ms(m.created()),
        modified: ms(m.modified()).unwrap_or(0),
        accessed: ms(m.accessed()).unwrap_or(0),
        owner: name_for("/etc/passwd", m.uid()),
        group: name_for("/etc/group", m.gid()),
        uid: m.uid(),
        gid: m.gid(),
        mode: m.mode() & 0o7777,
        can_chmod: me == 0 || me == m.uid(),
    })
}

/// chmod; with `recursive`, folders get `mode` and files get `mode` minus execute bits
/// (unless the file was already executable), which is what you want 99% of the time.
pub fn set_mode(p: &Path, mode: u32, recursive: bool) -> Vec<String> {
    let mut errs = vec![];
    let apply = |q: &Path, m: &fs::Metadata, errs: &mut Vec<String>| {
        let new = if m.is_dir() || m.mode() & 0o111 != 0 { mode } else { mode & !0o111 };
        if let Err(e) = fs::set_permissions(q, fs::Permissions::from_mode(new)) {
            errs.push(format!("{}: {e}", q.display()));
        }
    };
    let Ok(m) = fs::symlink_metadata(p) else { return vec![format!("{}: not found", p.display())] };
    if let Err(e) = fs::set_permissions(p, fs::Permissions::from_mode(mode)) {
        errs.push(format!("{}: {e}", p.display()));
    }
    if recursive && m.is_dir() {
        let mut stack = vec![p.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let q = e.path();
                let Ok(m) = fs::symlink_metadata(&q) else { continue };
                if m.file_type().is_symlink() {
                    continue; // chmod would follow it
                }
                apply(&q, &m, &mut errs);
                if m.is_dir() {
                    stack.push(q);
                }
            }
        }
    }
    errs
}

/// Media details as label → value (ordered).
pub fn details(p: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok((w, h)) = image::image_dimensions(p) {
        out.insert("Dimensions".into(), format!("{w} × {h} px"));
    }
    if let Ok(f) = fs::File::open(p) {
        if let Ok(ex) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(f)) {
            use exif::Tag;
            for (tag, label) in [
                (Tag::Make, "Camera make"),
                (Tag::Model, "Camera model"),
                (Tag::DateTimeOriginal, "Taken"),
                (Tag::ExposureTime, "Exposure"),
                (Tag::FNumber, "Aperture"),
                (Tag::PhotographicSensitivity, "ISO"),
                (Tag::FocalLength, "Focal length"),
                (Tag::LensModel, "Lens"),
                (Tag::Orientation, "Orientation"),
            ] {
                if let Some(f) = ex.get_field(tag, exif::In::PRIMARY) {
                    out.insert(label.into(), f.display_value().with_unit(&ex).to_string().trim_matches('"').to_owned());
                }
            }
            if let (Some(lat), Some(lon)) = (ex.get_field(Tag::GPSLatitude, exif::In::PRIMARY), ex.get_field(Tag::GPSLongitude, exif::In::PRIMARY)) {
                out.insert("GPS".into(), format!("{}, {}", lat.display_value(), lon.display_value()));
            }
        }
    }
    let kind = crate::desktop::mime_of(p);
    if kind.starts_with("audio/") || kind.starts_with("video/") {
        if let Ok(o) = Command::new("ffprobe").args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams"]).arg(p).output() {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
                let fmt = &v["format"];
                if let Some(d) = fmt["duration"].as_str().and_then(|d| d.parse::<f64>().ok()) {
                    let s = d.round() as u64;
                    out.insert("Duration".into(), format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60));
                }
                if let Some(b) = fmt["bit_rate"].as_str().and_then(|b| b.parse::<u64>().ok()) {
                    out.insert("Bitrate".into(), format!("{} kb/s", b / 1000));
                }
                if let Some(tags) = fmt["tags"].as_object() {
                    for (k, label) in [("title", "Title"), ("artist", "Artist"), ("album", "Album"), ("album_artist", "Album artist"), ("date", "Year"), ("genre", "Genre"), ("track", "Track")] {
                        if let Some(v) = tags.iter().find(|(tk, _)| tk.eq_ignore_ascii_case(k)).and_then(|(_, v)| v.as_str()) {
                            out.insert(label.into(), v.to_owned());
                        }
                    }
                }
                for s in v["streams"].as_array().into_iter().flatten() {
                    match s["codec_type"].as_str() {
                        Some("video") => {
                            out.insert("Video".into(), format!("{} {}×{}", s["codec_name"].as_str().unwrap_or(""), s["width"], s["height"]));
                        }
                        Some("audio") => {
                            out.entry("Audio".into()).or_insert_with(|| format!("{} {} Hz, {} ch", s["codec_name"].as_str().unwrap_or(""), s["sample_rate"].as_str().unwrap_or("?"), s["channels"]));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

pub fn checksum(p: &Path, algo: &str) -> Result<String, String> {
    use md5::Digest;
    let mut f = fs::File::open(p).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    macro_rules! run {
        ($h:expr) => {{
            let mut h = $h;
            loop {
                let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
        }};
    }
    match algo {
        "md5" => run!(md5::Md5::new()),
        "sha256" => run!(sha2::Sha256::new()),
        _ => Err(format!("unknown algorithm {algo}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_values() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("abc");
        fs::write(&f, "abc").unwrap();
        assert_eq!(checksum(&f, "md5").unwrap(), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(checksum(&f, "sha256").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(checksum(&f, "crc").is_err());
    }

    #[test]
    fn props_and_recursive_chmod() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("dir");
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("f.txt"), "x").unwrap();
        fs::write(d.join("run.sh"), "#!/bin/sh").unwrap();
        fs::set_permissions(d.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", d.join("lnk")).unwrap();
        let p = props(&d).unwrap();
        assert!(p.is_dir && p.can_chmod && !p.owner.is_empty());
        assert_eq!(props(&d.join("lnk")).unwrap().link_target.as_deref(), Some("/etc/passwd"));
        assert!(set_mode(&d, 0o750, true).is_empty());
        let mode = |n: &str| fs::symlink_metadata(d.join(n)).unwrap().mode() & 0o777;
        assert_eq!(mode("sub"), 0o750);
        assert_eq!(mode("f.txt"), 0o640, "plain files don't gain execute bits");
        assert_eq!(mode("run.sh"), 0o750);
        assert_eq!(fs::metadata("/etc/passwd").unwrap().mode() & 0o777, 0o644, "symlink targets untouched");
    }

    #[test]
    fn image_details() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("p.png");
        image::DynamicImage::new_rgb8(40, 30).save(&f).unwrap();
        assert_eq!(details(&f)["Dimensions"], "40 × 30 px");
    }
}
