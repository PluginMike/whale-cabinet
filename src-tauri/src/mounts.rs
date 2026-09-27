//! Devices sidebar: block devices from `lsblk -J`, network/FUSE mounts from mountinfo; mount, unmount and
//! eject through udisks2 (`udisksctl`, so polkit can ask), network mounts via gio/fusermount/umount.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Device {
    /// /dev path, or the mount point for network mounts
    pub id: String,
    pub label: String,
    /// Mount point, empty if not mounted
    pub path: String,
    pub size: u64,
    /// "usb" | "drive" | "network"
    pub kind: String,
    pub can_eject: bool,
    /// Parent disk (for power-off on eject)
    pub disk: String,
}

const SYSTEM_MOUNTS: [&str; 12] = ["/", "/boot", "/boot/efi", "/efi", "/home", "/var", "/usr", "/root", "/srv", "/opt", "/nix", "/gnu"];

fn human(n: u64) -> String {
    let mut v = n as f64;
    for u in ["B", "KB", "MB", "GB", "TB"] {
        if v < 1000.0 {
            return format!("{v:.0} {u}");
        }
        v /= 1000.0;
    }
    format!("{v:.0} PB")
}

/// Pick user-facing volumes out of `lsblk -J -b -o NAME,PATH,LABEL,SIZE,FSTYPE,MOUNTPOINTS,HOTPLUG,RM,TYPE,MODEL,PKNAME`.
pub fn parse_lsblk(json: &str) -> Vec<Device> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let mut out = vec![];
    fn walk(node: &Value, disk: &Value, out: &mut Vec<Device>) {
        let fstype = node["fstype"].as_str().unwrap_or("");
        let ty = node["type"].as_str().unwrap_or("");
        let mounts: Vec<&str> = node["mountpoints"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let removable = disk["hotplug"].as_bool().unwrap_or(false) || disk["rm"].as_bool().unwrap_or(false) || node["hotplug"].as_bool().unwrap_or(false);
        let system = mounts.iter().any(|m| SYSTEM_MOUNTS.contains(m) || m.starts_with("[SWAP]"));
        let container = matches!(fstype, "swap" | "crypto_LUKS" | "LVM2_member" | "linux_raid_member" | "zfs_member");
        let usable = !fstype.is_empty() && !container && ty != "loop";
        let name = node["name"].as_str().unwrap_or("");
        if usable && !system && !name.starts_with("zram") && !name.starts_with("loop") {
            let size = node["size"].as_u64().unwrap_or(0);
            let label = node["label"].as_str().filter(|s| !s.is_empty()).map(str::to_owned)
                .or_else(|| disk["model"].as_str().map(|m| format!("{} {}", m.trim(), human(size))))
                .unwrap_or_else(|| format!("{} Volume", human(size)));
            out.push(Device {
                id: node["path"].as_str().unwrap_or("").into(),
                label,
                path: mounts.first().map(|s| s.to_string()).unwrap_or_default(),
                size,
                kind: if removable { "usb" } else { "drive" }.into(),
                can_eject: removable,
                disk: disk["path"].as_str().unwrap_or("").into(),
            });
        }
        for c in node["children"].as_array().into_iter().flatten() {
            walk(c, disk, out);
        }
    }
    for d in v["blockdevices"].as_array().into_iter().flatten() {
        walk(d, d, &mut out);
    }
    out
}

const NET_FS: [&str; 9] = ["cifs", "smb3", "nfs", "nfs4", "fuse.sshfs", "fuse.rclone", "davfs", "fuse.gvfsd-fuse", "9p"];

/// Network / FUSE mounts from /proc/self/mountinfo (mount point is field 5; fstype and source follow " - ").
pub fn parse_mountinfo(text: &str) -> Vec<Device> {
    let mut out = vec![];
    for l in text.lines() {
        let Some((left, right)) = l.split_once(" - ") else { continue };
        let mp = left.split(' ').nth(4).unwrap_or("");
        let mut r = right.split(' ');
        let (fs, src) = (r.next().unwrap_or(""), r.next().unwrap_or(""));
        if NET_FS.contains(&fs) {
            let mp = mp.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\");
            out.push(Device { id: mp.clone(), label: src.replace("\\040", " "), path: mp, size: 0, kind: "network".into(), can_eject: false, disk: String::new() });
        }
    }
    out
}

pub fn list() -> Vec<Device> {
    let mut v = Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,PATH,LABEL,SIZE,FSTYPE,MOUNTPOINTS,HOTPLUG,RM,TYPE,MODEL,PKNAME"])
        .output()
        .ok()
        .map(|o| parse_lsblk(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    v.extend(parse_mountinfo(&std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default()));
    v
}

fn run(argv: &[&str]) -> Result<String, String> {
    let o = Command::new(argv[0]).args(&argv[1..]).output().map_err(|e| format!("{}: {e}", argv[0]))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_owned())
    }
}

/// Mount a block device; returns the mount point.
pub fn mount(dev: &str) -> Result<String, String> {
    let out = run(&["udisksctl", "mount", "-b", dev])?;
    // "Mounted /dev/sdb1 at /run/media/me/STICK"
    out.split(" at ").nth(1).map(|s| s.trim().trim_end_matches('.').to_owned()).ok_or(out)
}

pub fn unmount(d: &Device) -> Result<(), String> {
    if d.kind == "network" {
        run(&["gio", "mount", "-u", &d.path]).or_else(|_| run(&["fusermount3", "-u", &d.path])).or_else(|_| run(&["umount", &d.path])).map(drop)
    } else {
        run(&["udisksctl", "unmount", "-b", &d.id]).map(drop)
    }
}

/// Unmount every mounted volume on the device's disk, then power it off so it's safe to unplug.
pub fn eject(d: &Device) -> Result<(), String> {
    for v in list().into_iter().filter(|v| v.disk == d.disk && !v.path.is_empty()) {
        unmount(&v)?;
    }
    let target = if d.disk.is_empty() { &d.id } else { &d.disk };
    run(&["udisksctl", "power-off", "-b", target]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSBLK: &str = r#"{"blockdevices":[
      {"name":"zram0","path":"/dev/zram0","label":null,"size":1,"fstype":"swap","mountpoints":["[SWAP]"],"hotplug":false,"rm":false,"type":"disk","model":null},
      {"name":"nvme0n1","path":"/dev/nvme0n1","label":null,"size":1000,"fstype":null,"mountpoints":[],"hotplug":false,"rm":false,"type":"disk","model":"Samsung SSD","children":[
        {"name":"nvme0n1p1","path":"/dev/nvme0n1p1","label":null,"size":100,"fstype":"vfat","mountpoints":["/boot"],"hotplug":false,"rm":false,"type":"part"},
        {"name":"nvme0n1p2","path":"/dev/nvme0n1p2","label":null,"size":800,"fstype":"btrfs","mountpoints":["/home","/"],"hotplug":false,"rm":false,"type":"part"},
        {"name":"nvme0n1p3","path":"/dev/nvme0n1p3","label":"Data","size":2000000000,"fstype":"ext4","mountpoints":[],"hotplug":false,"rm":false,"type":"part"}]},
      {"name":"sdb","path":"/dev/sdb","label":null,"size":16000000000,"fstype":null,"mountpoints":[],"hotplug":true,"rm":true,"type":"disk","model":"SanDisk ","children":[
        {"name":"sdb1","path":"/dev/sdb1","label":null,"size":16000000000,"fstype":"exfat","mountpoints":["/run/media/m/STICK"],"hotplug":true,"rm":true,"type":"part"}]}
    ]}"#;

    #[test]
    fn picks_user_volumes() {
        let d = parse_lsblk(LSBLK);
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].label, "Data");
        assert_eq!((d[0].kind.as_str(), d[0].can_eject, d[0].path.as_str()), ("drive", false, ""));
        assert_eq!(d[1].label, "SanDisk 16 GB");
        assert_eq!((d[1].kind.as_str(), d[1].can_eject, d[1].path.as_str(), d[1].disk.as_str()), ("usb", true, "/run/media/m/STICK", "/dev/sdb"));
    }

    #[test]
    fn network_mounts() {
        let mi = "36 25 0:32 / /mnt/nas\\040share rw,relatime shared:1 - cifs //nas/share rw\n37 25 0:33 / /home proc rw - btrfs /dev/x rw\n38 25 0:34 / /mnt/pi rw - fuse.sshfs pi@10.0.0.2:/ rw\n";
        let n = parse_mountinfo(mi);
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].path, "/mnt/nas share");
        assert_eq!(n[0].label, "//nas/share");
        assert_eq!(n[1].kind, "network");
    }
}
