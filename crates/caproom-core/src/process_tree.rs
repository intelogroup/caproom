use crate::collector::Snapshot;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TreeInfo {
    pub root_pid: i32,
    pub pids: Vec<i32>,
    pub footprint_kb: u64,
    pub cmd: String,
    pub state: char,
}

/// Build tree rooted at `root` from snapshot via ppid_map ancestry.
/// Uses walk over snapshot's ppid edges, not `ps` re-read.
pub struct Tree;

impl Tree {
    pub fn build(root: i32, snap: &Snapshot) -> Option<TreeInfo> {
        let root_proc = snap.by_pid(root)?;
        let mut pids = Vec::new();
        let mut footprint = 0u64;
        let mut queue = vec![root];
        let mut visited = std::collections::HashSet::new();
        while let Some(cur) = queue.pop() {
            if !visited.insert(cur) {
                continue;
            }
            if let Some(p) = snap.by_pid(cur) {
                pids.push(cur);
                footprint += p.footprint_kb;
                for child in snap.procs.iter().filter(|c| c.ppid == cur) {
                    queue.push(child.pid);
                }
            }
        }
        Some(TreeInfo {
            root_pid: root,
            pids,
            footprint_kb: footprint,
            cmd: root_proc.cmd.clone(),
            state: root_proc.state,
        })
    }

    /// Roots: parents outside visible set or ppid==1 (like bin/caproom:244)
    /// Exclude pid 0/1 themselves — walking launchd would be whole machine.
    pub fn roots(snap: &Snapshot) -> Vec<i32> {
        let pids: std::collections::HashSet<i32> = snap.procs.iter().map(|p| p.pid).collect();
        let mut roots = Vec::new();
        for p in &snap.procs {
            if p.pid <= 1 { continue; }
            if p.pid == std::process::id() as i32 { continue; }
            if p.ppid == 1 || !pids.contains(&p.ppid) {
                roots.push(p.pid);
            }
        }
        roots
    }
}
