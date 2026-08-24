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
            if p.pid <= 1 {
                continue;
            }
            if p.pid == std::process::id() as i32 {
                continue;
            }
            if p.ppid == 1 || !pids.contains(&p.ppid) {
                roots.push(p.pid);
            }
        }
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::{ProcInfo, Snapshot};

    #[test]
    fn build_single_proc() {
        let snap = Snapshot {
            procs: vec![ProcInfo {
                pid: 10,
                ppid: 1,
                pgid: 10,
                sid: 10,
                start_time: 1,
                footprint_kb: 500,
                cpu_time_ns: 0,
                state: 'S',
                cmd: "root".into(),
            }],
        };
        let t = Tree::build(10, &snap).unwrap();
        assert_eq!(t.root_pid, 10);
        assert_eq!(t.pids, vec![10]);
        assert_eq!(t.footprint_kb, 500);
        assert_eq!(t.cmd, "root");
    }

    #[test]
    fn build_nested_tree() {
        let snap = Snapshot {
            procs: vec![
                ProcInfo {
                    pid: 10,
                    ppid: 1,
                    pgid: 10,
                    sid: 10,
                    start_time: 1,
                    footprint_kb: 100,
                    cpu_time_ns: 0,
                    state: 'S',
                    cmd: "root".into(),
                },
                ProcInfo {
                    pid: 11,
                    ppid: 10,
                    pgid: 11,
                    sid: 11,
                    start_time: 2,
                    footprint_kb: 200,
                    cpu_time_ns: 0,
                    state: 'S',
                    cmd: "child".into(),
                },
                ProcInfo {
                    pid: 12,
                    ppid: 11,
                    pgid: 12,
                    sid: 12,
                    start_time: 3,
                    footprint_kb: 300,
                    cpu_time_ns: 0,
                    state: 'R',
                    cmd: "grand".into(),
                },
            ],
        };
        let t = Tree::build(10, &snap).unwrap();
        assert_eq!(t.root_pid, 10);
        assert_eq!(t.pids, vec![10, 11, 12]);
        assert_eq!(t.footprint_kb, 600);
    }

    #[test]
    fn roots_exclude_system_and_self() {
        let snap = Snapshot {
            procs: vec![
                ProcInfo {
                    pid: 1,
                    ppid: 0,
                    pgid: 1,
                    sid: 1,
                    start_time: 0,
                    footprint_kb: 0,
                    cpu_time_ns: 0,
                    state: 'S',
                    cmd: "init".into(),
                },
                ProcInfo {
                    pid: 2,
                    ppid: 100,
                    pgid: 2,
                    sid: 2,
                    start_time: 1,
                    footprint_kb: 100,
                    cpu_time_ns: 0,
                    state: 'R',
                    cmd: "child".into(),
                },
            ],
        };
        let roots = Tree::roots(&snap);
        assert!(!roots.contains(&1));
        // 2's parent 100 is missing from the snapshot -> root
        assert!(roots.contains(&2));
        // inject self pid; it must be excluded even if orphan
        let me = std::process::id() as i32;
        let mut injected = snap.procs.clone();
        injected.push(ProcInfo {
            pid: me,
            ppid: 100,
            pgid: me,
            sid: me,
            start_time: 10,
            footprint_kb: 0,
            cpu_time_ns: 0,
            state: 'R',
            cmd: "self".into(),
        });
        let snap2 = Snapshot { procs: injected };
        let roots2 = Tree::roots(&snap2);
        assert!(!roots2.contains(&me));
    }
}
