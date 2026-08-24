use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    /// phys_footprint (macOS) or PSS/RSS fallback, KB
    pub footprint_kb: u64,
    /// state char: R/S/I/T/Z
    pub state: char,
    pub cmd: String,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub procs: Vec<ProcInfo>,
}

impl Snapshot {
    pub fn ppid_map(&self) -> std::collections::HashMap<i32, i32> {
        self.procs.iter().map(|p| (p.pid, p.ppid)).collect()
    }
    pub fn by_pid(&self, pid: i32) -> Option<&ProcInfo> {
        self.procs.iter().find(|p| p.pid == pid)
    }
}

/// Snapshot via libproc on macOS, /proc on Linux, ps fallback.
/// Week 1: libproc FFI. Until then, ps fallback keeps CLI functional.
pub fn snapshot_current_user() -> Snapshot {
    #[cfg(target_os = "macos")]
    {
        if let Some(s) = snapshot_libproc() {
            return s;
        }
    }
    snapshot_ps()
}

#[cfg(target_os = "macos")]
fn snapshot_libproc() -> Option<Snapshot> {
    // FFI: proc_listallpids + proc_pidinfo(PROC_PIDTBSDINFO) + proc_pid_rusage ri_phys_footprint
    // For now, return None to fall back to ps — FFI wired in next commit.
    // This keeps the hot path compile-clean while we validate ppid_map logic.
    None
}

fn snapshot_ps() -> Snapshot {
    use std::process::Command;
    let out = Command::new("ps")
        .args(["-eo", "pid=,ppid=,pgid=,rss=,state=,command="])
        .output();
    let Ok(out) = out else { return Snapshot::default() };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut procs = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.is_empty() { continue; }
        // split_whitespace then re-join command tail
        let toks: Vec<&str> = t.split_whitespace().collect();
        if toks.len() < 5 { continue; }
        let pid: i32 = toks[0].parse().unwrap_or(0);
        let ppid: i32 = toks[1].parse().unwrap_or(0);
        let pgid: i32 = toks[2].parse().unwrap_or(0);
        let rss: u64 = toks[3].parse().unwrap_or(0);
        let state = toks[4].chars().next().unwrap_or('?');
        // command is original line after the 5th token's position — recover verbatim tail
        let cmd = if toks.len() > 5 {
            // find 5th token end index in original
            let mut idx = 0usize;
            let mut count = 0;
            for (i, c) in t.char_indices() {
                if c.is_whitespace() {
                    // skip whitespace run
                    while t[idx..].chars().next().map(|x| x.is_whitespace()).unwrap_or(false) { idx+=1; }
                    let _ = &t[i..];
                }
            }
            // simpler: join remaining toks with space (lossy but stable for display)
            toks[5..].join(" ")
        } else { String::new() };
        if pid == 0 { continue; }
        procs.push(ProcInfo { pid, ppid, pgid, footprint_kb: rss, state, cmd });
    }
    Snapshot { procs }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ps_snapshot_not_empty() {
        let s = snapshot_ps();
        assert!(!s.procs.is_empty());
    }
}
