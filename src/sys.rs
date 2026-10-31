//! Machine health for the header: disk use, whether the running kernel still has its modules on disk,
//! and Docker's disk use. Every probe can fail on its own without hiding the others.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disk {
    pub size: u64,
    pub used: u64,
    pub avail: u64,
}

impl Disk {
    /// The percentage `df` prints: used / (used + avail), rounded up.
    pub fn percent(&self) -> u64 {
        let total = self.used + self.avail;
        if total == 0 {
            0
        } else {
            (self.used * 100).div_ceil(total)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kernel {
    pub running: String,
    pub installed: Vec<String>,
}

impl Kernel {
    /// After a pacman upgrade of `linux`, the running kernel's module directory is gone and
    /// loading new modules (Docker's veth) fails until a reboot.
    pub fn reboot_needed(&self) -> bool {
        !self.installed.contains(&self.running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerRow {
    pub kind: String,
    pub size: String,
    pub reclaimable: String,
}

#[derive(Debug, Clone)]
pub struct SysInfo {
    pub disks: Vec<(String, Result<Disk, String>)>,
    pub kernel: Result<Kernel, String>,
    pub docker: Result<Vec<DockerRow>, String>,
}

/// Parses `df -B1 --output=size,used,avail -- PATH`.
pub fn parse_df(text: &str) -> Option<Disk> {
    let line = text.lines().nth(1)?;
    let mut it = line.split_whitespace().map(|f| f.parse::<u64>());
    Some(Disk {
        size: it.next()?.ok()?,
        used: it.next()?.ok()?,
        avail: it.next()?.ok()?,
    })
}

/// Parses `docker system df --format '{{.Type}}|{{.Size}}|{{.Reclaimable}}'`.
pub fn parse_docker_df(text: &str) -> Vec<DockerRow> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.splitn(3, '|');
            Some(DockerRow {
                kind: it.next()?.trim().to_string(),
                size: it.next()?.trim().to_string(),
                reclaimable: it.next()?.trim().to_string(),
            })
        })
        .collect()
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

fn disk(path: &Path) -> Result<Disk, String> {
    let out = Command::new("df")
        .args(["-B1", "--output=size,used,avail", "--"])
        .arg(path)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("could not run df: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    parse_df(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "unreadable df output".to_string())
}

fn kernel() -> Result<Kernel, String> {
    let out = Command::new("uname")
        .arg("-r")
        .output()
        .map_err(|e| format!("could not run uname: {e}"))?;
    let running = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let installed = std::fs::read_dir("/usr/lib/modules")
        .map_err(|e| format!("cannot read /usr/lib/modules: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    Ok(Kernel { running, installed })
}

/// Docker may be stopped or hang, so the call is capped at five seconds.
fn docker() -> Result<Vec<DockerRow>, String> {
    let out = Command::new("timeout")
        .args([
            "5",
            "docker",
            "system",
            "df",
            "--format",
            "{{.Type}}|{{.Size}}|{{.Reclaimable}}",
        ])
        .output()
        .map_err(|e| format!("could not run docker: {e}"))?;
    if out.status.success() {
        Ok(parse_docker_df(&String::from_utf8_lossy(&out.stdout)))
    } else if out.status.code() == Some(124) {
        Err("timed out".to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().next().unwrap_or("failed").to_string())
    }
}

pub fn collect(home: &Path) -> SysInfo {
    SysInfo {
        disks: vec![
            ("/".to_string(), disk(Path::new("/"))),
            ("~/work".to_string(), disk(&home.join("work"))),
        ],
        kernel: kernel(),
        docker: docker(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_df_output() {
        let text = "        1B-blocks          Used         Avail\n 52613349376  38654705664  11811160064\n";
        let d = parse_df(text).unwrap();
        assert_eq!(d.size, 52613349376);
        assert_eq!(d.used, 38654705664);
        assert_eq!(d.avail, 11811160064);
        assert_eq!(d.percent(), 77);
    }

    #[test]
    fn rejects_garbage_df_output() {
        assert_eq!(parse_df("header only\n"), None);
        assert_eq!(parse_df("h\nnot numbers here\n"), None);
    }

    #[test]
    fn percent_rounds_up_like_df() {
        let d = Disk {
            size: 1000,
            used: 501,
            avail: 499,
        };
        assert_eq!(d.percent(), 51);
        assert_eq!(
            Disk {
                size: 0,
                used: 0,
                avail: 0
            }
            .percent(),
            0
        );
    }

    #[test]
    fn kernel_reboot_detection() {
        let ok = Kernel {
            running: "7.2.11-arch1-1".into(),
            installed: vec!["7.2.11-arch1-1".into()],
        };
        let stale = Kernel {
            running: "7.2.9-arch1-1".into(),
            installed: vec!["7.2.11-arch1-1".into()],
        };
        assert!(!ok.reboot_needed());
        assert!(stale.reboot_needed());
    }

    #[test]
    fn parses_docker_rows() {
        let rows = parse_docker_df("Images|4.2GB|2GB (47%)\nLocal Volumes|1.1GB|0B (0%)\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, "Images");
        assert_eq!(rows[1].reclaimable, "0B (0%)");
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1536), "1.5K");
        assert_eq!(format_bytes(52613349376), "49.0G");
    }
}
