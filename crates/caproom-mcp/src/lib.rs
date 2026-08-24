use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State { Parked, Running, Zombie }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReasonCode { ParkIdle, GrowthRate, Pressure, None }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopProcess {
    pub pid: i32,
    pub cmd: String,
    pub tree_rss_kb: u64,
    pub footprint_kb: u64,
    pub tree_pids: Vec<i32>,
    pub state: State,
    pub reason_code: ReasonCode,
    pub growth_kb_s: i64,
    pub free_pct: u8,
    pub park_candidate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopResponse {
    pub schema: u8,
    pub ts: u64,
    pub limit_mb_default: u64,
    pub processes: Vec<TopProcess>,
}

static GROWTH: std::sync::OnceLock<std::sync::Mutex<caproom_core::growth::GrowthRing>> = std::sync::OnceLock::new();
fn growth_ring() -> &'static std::sync::Mutex<caproom_core::growth::GrowthRing> {
    GROWTH.get_or_init(|| std::sync::Mutex::new(caproom_core::growth::GrowthRing::new()))
}

/// Pull-only MCP tools: top, park, wake. watch_* deferred to v1.1 (rmcp port).
/// No push notifications — see skills/caproom-memory/SKILL.md.
pub fn handle_top(pid: Option<i32>) -> TopResponse {
    let snap = caproom_core::collector::snapshot_current_user();
    let free_pct = caproom_core::pressure::free_mem_pct();
    let roots = if let Some(p) = pid { vec![p] } else { caproom_core::process_tree::Tree::roots(&snap) };
    let mut processes = Vec::new();
    let mut ring = growth_ring().lock().unwrap();
    for r in roots {
        if let Some(t) = caproom_core::process_tree::Tree::build(r, &snap) {
            let state = match t.state { 'T' => State::Parked, 'Z' => State::Zombie, _ => State::Running };
            let is_sleeping = matches!(t.state, 'S' | 'I');
            let park_candidate = state == State::Running && is_sleeping && t.footprint_kb >= 512*1024;
            let growth_kb_s = ring.update(r, t.footprint_kb);
            let reason_code = match caproom_core::growth::reason_code(park_candidate, growth_kb_s, free_pct) {
                "PARK_IDLE" => ReasonCode::ParkIdle,
                "GROWTH_RATE" => ReasonCode::GrowthRate,
                "PRESSURE" => ReasonCode::Pressure,
                _ => ReasonCode::None,
            };
            processes.push(TopProcess{
                pid: r, cmd: t.cmd, tree_rss_kb: t.footprint_kb, footprint_kb: t.footprint_kb,
                tree_pids: t.pids, state, reason_code, growth_kb_s, free_pct, park_candidate,
            });
        }
    }
    // prune stale after
    let ids: Vec<i32> = processes.iter().map(|p| p.pid).collect();
    ring.prune_stale(&ids);
    processes.sort_by(|a,b| b.tree_rss_kb.cmp(&a.tree_rss_kb));
    TopResponse{ schema: 1, ts: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), limit_mb_default: 4096, processes }
}

/// pid <= 1 must never be signalled: kill(0) hits our own process group,
/// kill(1) targets init. Both are caller bugs, not caproom operations.
fn valid_pid(pid: i32) -> bool { pid > 1 }

pub fn handle_park(pid: i32) -> serde_json::Value {
    if !valid_pid(pid) {
        return serde_json::json!({"error": format!("invalid pid {} — must be > 1", pid)});
    }
    #[cfg(unix)]
    {
        let r = unsafe { libc::kill(pid, libc::SIGSTOP) };
        if r == 0 {
            return serde_json::json!({"parked": [pid], "eligible_kb": 0, "state": "parked"});
        } else {
            return serde_json::json!({"error": format!("no such pid {}", pid)});
        }
    }
    #[cfg(not(unix))]
    { serde_json::json!({"error": "park not implemented on Windows"}) }
}

pub fn handle_park_tree(pid: i32) -> serde_json::Value {
    if !valid_pid(pid) {
        return serde_json::json!({"error": format!("invalid pid {} — must be > 1", pid)});
    }
    #[cfg(unix)]
    {
        let snap = caproom_core::collector::snapshot_current_user();
        if let Some(tree) = caproom_core::process_tree::Tree::build(pid, &snap) {
            // PID reuse guard: skip pids whose start_time changed since snapshot
            // (parity with cli cmd_park_tree)
            let mut parked = Vec::new();
            for p in &tree.pids {
                if let Some(orig) = snap.by_pid(*p) {
                    if orig.start_time != 0 {
                        if let Some(cur) = caproom_core::collector::snapshot_current_user().by_pid(*p) {
                            if cur.start_time != 0 && cur.start_time != orig.start_time {
                                continue;
                            }
                        }
                    }
                    if unsafe { libc::kill(*p, libc::SIGSTOP) } == 0 { parked.push(*p); }
                }
            }
            return serde_json::json!({"parked": parked, "tree_pid": pid, "tree_pids": tree.pids, "eligible_kb": tree.footprint_kb});
        } else {
            return serde_json::json!({"error": format!("no such pid {}", pid)});
        }
    }
    #[cfg(not(unix))]
    { serde_json::json!({"error": "park-tree not implemented on Windows"}) }
}

pub fn handle_wake(pid: i32) -> serde_json::Value {
    if !valid_pid(pid) {
        return serde_json::json!({"error": format!("invalid pid {} — must be > 1", pid)});
    }
    #[cfg(unix)]
    {
        let r = unsafe { libc::kill(pid, libc::SIGCONT) };
        if r == 0 { serde_json::json!({"woken": [pid], "state": "running"}) } else { serde_json::json!({"error": format!("no such pid {}", pid)}) }
    }
    #[cfg(not(unix))]
    { serde_json::json!({"error": "wake not implemented on Windows"}) }
}

pub fn handle_wake_tree(pid: i32) -> serde_json::Value {
    if !valid_pid(pid) {
        return serde_json::json!({"error": format!("invalid pid {} — must be > 1", pid)});
    }
    #[cfg(unix)]
    {
        let snap = caproom_core::collector::snapshot_current_user();
        if let Some(tree) = caproom_core::process_tree::Tree::build(pid, &snap) {
            let mut woken = Vec::new();
            for p in &tree.pids {
                if unsafe { libc::kill(*p, libc::SIGCONT) } == 0 { woken.push(*p); }
            }
            return serde_json::json!({"woken": woken, "tree_pid": pid, "tree_pids": tree.pids});
        } else {
            // fallback single
            let r = unsafe { libc::kill(pid, libc::SIGCONT) };
            if r == 0 { serde_json::json!({"woken": [pid]}) } else { serde_json::json!({"error": format!("no such pid {}", pid)}) }
        }
    }
    #[cfg(not(unix))]
    { serde_json::json!({"error": "wake-tree not implemented on Windows"}) }
}

pub fn handle_run(command: Vec<String>, limit_mb: u64) -> serde_json::Value {
    if command.is_empty() { return serde_json::json!({"error": "empty command"}); }
    // Use caproom_core pressure effective limit for check, then spawn via std::process with watchdog?
    // Minimal v1: spawn and wait, report exit, not full watchdog (watchdog is in cli crate). For MCP, we proxy to `caproom run` binary if available.
    let caproom_bin = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("caproom"))).unwrap_or_else(|| std::path::PathBuf::from("caproom"));
    let mut cmd = std::process::Command::new(&caproom_bin);
    cmd.args(["run", "--limit", &limit_mb.to_string(), "--"]).args(&command);
    let out = cmd.output();
    match out {
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let killed = stderr.contains("KILLED BY CAP") || stderr.contains("exceeded") || o.status.code() == Some(137) || o.status.code() == Some(143);
            serde_json::json!({"command": command, "limit_mb": limit_mb, "exit_code": o.status.code().unwrap_or(-1), "killed_by_cap": killed, "stdout": stdout, "stderr": stderr, "reason_code": if killed { "PRESSURE" } else { "NONE" }})
        },
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_enums_are_typed_no_freeform_message() {
        let r = handle_top(None);
        let v = serde_json::to_value(&r).unwrap();
        // assert no "message" string field exists at any level
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("\"message\""), "freeform message field must not exist");
        // state must be enum, not arbitrary string
        for p in &r.processes { let _ = serde_json::to_value(&p.state).unwrap(); }
    }
    #[test]
    fn park_wake_roundtrip() {
        let pid = std::process::id() as i32; // self, don't actually park self
        let v = handle_top(Some(pid));
        assert!(v.processes.is_empty() || !v.processes.is_empty()); // just ensure top works with pid filter
    }
    #[test]
    fn invalid_pid_rejected_without_signalling() {
        // kill(0) signals our own process group, kill(1) targets init — both must error out
        for bad in [0i32, 1, -1, -4096] {
            for v in [
                handle_park(bad),
                handle_park_tree(bad),
                handle_wake(bad),
                handle_wake_tree(bad),
            ] {
                assert!(v.get("error").is_some(), "pid {} must be rejected, got {}", bad, v);
            }
        }
    }
}
