use std::collections::HashMap;

/// Still attached to tracked tree, not reparented to init(1).
/// Fix #1: ancestry walk, not ppid==root direct compare.
pub fn still_in_tree(pid: i32, root: i32, ppid_map: &HashMap<i32, i32>) -> bool {
    let mut cur = pid;
    for _ in 0..64 {
        if cur == root {
            return true;
        }
        if cur == 1 {
            return false;
        }
        match ppid_map.get(&cur) {
            Some(&p) => cur = p,
            None => return false,
        }
    }
    false
}

#[derive(Debug, Clone)]
pub struct TreeView<'a> {
    pub root: i32,
    pub pids: &'a [i32],
    pub states: &'a HashMap<i32, char>,
    pub footprints: &'a HashMap<i32, u64>,
    pub is_session_leader: &'a HashMap<i32, bool>,
    pub cpu_delta: &'a HashMap<i32, f32>,
}

pub fn is_idle_subtree(pid: i32, root: i32, view: &TreeView, ppid_map: &HashMap<i32, i32>) -> bool {
    if !still_in_tree(pid, root, ppid_map) {
        return false;
    }
    let state = view.states.get(&pid).copied().unwrap_or('?');
    if !matches!(state, 'S' | 'I') {
        return false;
    }
    if view.is_session_leader.get(&pid).copied().unwrap_or(false) {
        return false;
    }
    if view.cpu_delta.get(&pid).copied().unwrap_or(0.0) >= 0.02 {
        return false;
    }
    if view.footprints.get(&pid).copied().unwrap_or(0) < 512 * 1024 {
        return false;
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Running,
    Breached,
    Termed,
    Killed,
}

pub struct Policy {
    pub limit_mb: u64,
    pub grace_secs: u64,
}

impl Policy {
    /// Fix #2 escalation: park → resample → only TERM if still over threshold.
    /// Implemented in cli watchdog loop; this is the predicate helper.
    pub fn should_term_after_park(&self, footprint_kb: u64, free_pct: u8) -> bool {
        let eff_kb = crate::pressure::effective_limit(self.limit_mb, free_pct) * 1024;
        footprint_kb >= eff_kb
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ancestry_walk_nested() {
        // npm(100) -> vite(101) -> eslint(102)
        let map: HashMap<i32, i32> = [(101, 100), (102, 101)].into();
        assert!(still_in_tree(102, 100, &map));
        assert!(!still_in_tree(102, 999, &map));
    }
    #[test]
    fn reparented_escaped() {
        let map: HashMap<i32, i32> = [(102, 1)].into();
        assert!(!still_in_tree(102, 100, &map));
    }
}
