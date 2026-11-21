//! `fs_space` extra: mounted filesystem capacity/usage.
//!
//! Local: `/proc/mounts` + `statvfs(3)`. Remote: the `## filesystems`
//! section emitted by `df -P -T -B1`.
//!
//! Pseudo filesystems (proc/sysfs/cgroup/tmpfs/...) are skipped: they have
//! no useful capacity history and would dominate the line on container
//! hosts. Mount points with the same device+mount are deduplicated.

use std::ffi::CString;

use crate::model::Filesystem;

const MAX_FILESYSTEMS: usize = 24;

const PSEUDO_FS: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "cgroup",
    "cgroup2",
    "pstore",
    "securityfs",
    "debugfs",
    "tracefs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "bpf",
    "autofs",
    "binfmt_misc",
    "rpc_pipefs",
    "nsfs",
    "squashfs",
];

fn is_pseudo(fstype: &str) -> bool {
    PSEUDO_FS.contains(&fstype)
}

/// Decode the octal escapes used by `/proc/mounts` (e.g. `\040` space).
fn decode_mount(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let oct = &field[i + 1..i + 4];
            if let Ok(value) = u8::from_str_radix(oct, 8) {
                out.push(value);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn statvfs_row(device: &str, mount: &str, fstype: &str) -> Option<Filesystem> {
    let path = CString::new(mount).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = st.f_frsize as u64;
    let total = (st.f_blocks as u64).saturating_mul(frsize);
    if total == 0 {
        return None;
    }
    let free = (st.f_bfree as u64).saturating_mul(frsize);
    let available = (st.f_bavail as u64).saturating_mul(frsize);
    let used = total.saturating_sub(free);
    Some(Filesystem {
        device: device.to_string(),
        mount: mount.to_string(),
        fstype: fstype.to_string(),
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        used_percent: (used as f64 / total as f64 * 100.0) as f32,
        inodes_total: st.f_files as u64,
        inodes_free: st.f_ffree as u64,
    })
}

/// Local read from `/proc/mounts`, capacity from `statvfs`.
pub fn read_local() -> Option<Vec<Filesystem>> {
    let text = std::fs::read_to_string("/proc/mounts").ok()?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(device) = parts.next() else { continue };
        let Some(mount) = parts.next() else { continue };
        let Some(fstype) = parts.next() else { continue };
        if is_pseudo(fstype) {
            continue;
        }
        let mount = decode_mount(mount);
        if !seen.insert(mount.clone()) {
            continue;
        }
        if let Some(row) = statvfs_row(&decode_mount(device), &mount, fstype) {
            out.push(row);
        }
    }
    finish(out)
}

/// Parse the remote `df -P -T -B1` output.
pub fn parse_df(text: &str) -> Option<Vec<Filesystem>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 7 || t[1] == "Type" {
            continue;
        }
        let fstype = t[1];
        if is_pseudo(fstype) {
            continue;
        }
        let mount = t[6..].join(" ");
        if !seen.insert(mount.clone()) {
            continue;
        }
        let (Ok(total), Ok(used), Ok(available)) = (
            t[2].parse::<u64>(),
            t[3].parse::<u64>(),
            t[4].parse::<u64>(),
        ) else {
            // df prints "-" for some pseudo/remote mounts; skip the row,
            // not the whole family.
            continue;
        };
        if total == 0 {
            continue;
        }
        out.push(Filesystem {
            device: t[0].to_string(),
            mount,
            fstype: fstype.to_string(),
            total_bytes: total,
            used_bytes: used,
            available_bytes: available,
            used_percent: (used as f64 / total as f64 * 100.0) as f32,
            inodes_total: 0,
            inodes_free: 0,
        });
    }
    finish(out)
}

fn finish(mut rows: Vec<Filesystem>) -> Option<Vec<Filesystem>> {
    rows.sort_by_key(|a| std::cmp::Reverse(a.total_bytes));
    rows.truncate(MAX_FILESYSTEMS);
    if rows.is_empty() {
        None
    } else {
        Some(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_df_and_skips_pseudo() {
        let text = "\
Filesystem Type 1B-blocks Used Available Capacity Mounted on
/dev/sda1 ext4 1000 400 600 40% /
tmpfs tmpfs 500 0 500 0% /run
nas:/data nfs4 2000 1000 1000 50% /mnt/data
";
        let rows = parse_df(text).unwrap();
        assert_eq!(rows.len(), 2);
        // Sorted by total bytes descending: nfs (2000) before root (1000).
        assert_eq!(rows[0].mount, "/mnt/data");
        assert_eq!(rows[0].fstype, "nfs4");
        assert_eq!(rows[1].mount, "/");
        assert_eq!(rows[1].used_bytes, 400);
    }

    #[test]
    fn decodes_octal_mount_escapes() {
        assert_eq!(decode_mount("/mnt/a\\040b"), "/mnt/a b");
    }
}
