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
}
