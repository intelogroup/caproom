use caproom_core::{collector, growth, pressure, process_tree::Tree};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Session: one tree root with risk score
#[derive(Debug, Clone, serde::Serialize)]
struct SessionRisk {
    pid: i32,
    cmd: String,
    footprint_kb: u64,
    pids: usize,
    growth_kb_s: i64,
    free_pct: u8,
    risk: f64,
    secs_to_crit: Option<f64>,
}

/// risk(session) = footprint*0.5 + growth*10 + pressure_contrib*200 + fanout*50 + 1000/secs_to_crit
/// Higher risk = more responsible for machine approaching failure
fn risk_score(footprint_kb: u64, growth_kb_s: i64, free_pct: u8, fanout: usize, eff_kb: u64) -> (f64, Option<f64>) {
    let remaining = eff_kb.saturating_sub(footprint_kb) as i64;
    let secs_to_crit = if growth_kb_s > 50 && remaining > 0 {
        Some(remaining as f64 / growth_kb_s as f64)
    } else if remaining <= 0 {
        Some(0.0)
    } else {
        None
    };
    let footprint_term = (footprint_kb as f64 / 1024.0) * 0.5; // per MB
    let growth_term = (growth_kb_s.max(0) as f64) * 0.1;
    let pressure_term = if free_pct < 20 { (20 - free_pct) as f64 * 10.0 } else { 0.0 };
    let fanout_term = fanout as f64 * 2.0;
    let urgency_term = secs_to_crit.map(|s| if s < 300.0 { (300.0 - s) * 0.5 } else { 0.0 }).unwrap_or(0.0);
    (footprint_term + growth_term + pressure_term + fanout_term + urgency_term, secs_to_crit)
}

fn main() {
    let limit_mb: u64 = std::env::var("CAPROOM_LIMIT_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(4096);
    let interval = Duration::from_millis(200);
    eprintln!("caproomd v1.0 skeleton — single collector, N sessions (pid=start guard, growth cache, pressure cache)");
    eprintln!("limit={}MB interval={:?} — Ctrl-C to stop", limit_mb, interval);
    let mut growth_ring = growth::GrowthRing::new();
    let mut iter: u64 = 0;
    loop {
        iter += 1;
        let snap = collector::snapshot_current_user();
        let free = pressure::free_mem_pct();
        let eff_kb = pressure::effective_limit(limit_mb, free) * 1024;
        let roots = Tree::roots(&snap);
        let mut sessions: Vec<SessionRisk> = Vec::new();
        for r in &roots {
            if let Some(t) = Tree::build(*r, &snap) {
                let g = growth_ring.update(*r, t.footprint_kb);
                let (risk, secs) = risk_score(t.footprint_kb, g, free, t.pids.len(), eff_kb);
                sessions.push(SessionRisk {
                    pid: *r,
                    cmd: t.cmd.chars().take(40).collect(),
                    footprint_kb: t.footprint_kb,
                    pids: t.pids.len(),
                    growth_kb_s: g,
                    free_pct: free,
                    risk,
                    secs_to_crit: secs,
                });
            }
        }
        // prune stale growth
        growth_ring.prune_stale(&roots);
        sessions.sort_by(|a,b| b.risk.partial_cmp(&a.risk).unwrap());
        // print top 5 riskiest
        if iter % 5 == 0 || sessions.iter().any(|s| s.footprint_kb >= eff_kb) {
            eprintln!("--- iter {} free {}% eff {}MB sessions {} ---", iter, free, eff_kb/1024, sessions.len());
            for s in sessions.iter().take(5) {
                eprintln!(" pid {:>6} risk {:>6.1} {}MB g {:>5}KB/s fanout {} crit {:?}s -- {}", s.pid, s.risk, s.footprint_kb/1024, s.growth_kb_s, s.pids, s.secs_to_crit.map(|v| format!("{:.0}", v)).unwrap_or("∞".into()), s.cmd);
            }
            if let Some(worst) = sessions.first() {
                if worst.footprint_kb >= eff_kb {
                    eprintln!("caproomd: worst {}MB >= eff {}MB — arbiter would park idle children of pid {} (least destructive)", worst.footprint_kb/1024, eff_kb/1024, worst.pid);
                } else if worst.risk > 500.0 {
                    eprintln!("caproomd: high risk {:.1} — would warn agent pid {} before park", worst.risk, worst.pid);
                }
            }
        }
        // TODO v1.0: UDS /tmp/caproomd.sock thread-per-connection, broadcast resource_warning JSON to agents
        // TODO v1.1: dispatch_source_memorypressure single Mach source instead of poll
        std::thread::sleep(interval);
        if iter > 300 { // ~60s demo, then idle
            eprintln!("caproomd: demo 60s done — in production would run as LaunchAgent/systemd");
            break;
        }
    }
}
