use std::collections::HashMap;
use std::time::{Duration, Instant};

/// In-process history ring: pid -> Vec<(Instant, kb)>. No cross-process state in v1 (CLI-first).
/// Daemon v1.1 will move this to shared UDS state. For now each `top` call estimates
/// growth from its own previous sample if called frequently; otherwise growth_kb_s = 0.
#[derive(Debug, Default)]
pub struct GrowthRing {
    // pid -> (last footprint, last instant, smoothed growth)
    history: HashMap<i32, (u64, Instant, i64)>,
}

impl GrowthRing {
    pub fn new() -> Self { Self { history: HashMap::new() } }

    /// Update with current footprint, return smoothed growth_kb_s (EWMA-ish).
    /// If no prior sample or elapsed < 500ms, return prior smoothed value.
    pub fn update(&mut self, pid: i32, footprint_kb: u64) -> i64 {
        let now = Instant::now();
        let entry = self.history.get(&pid).copied();
        if let Some((prev_kb, prev_t, prev_g)) = entry {
            let dt = now.duration_since(prev_t).as_secs_f64();
            if dt < 0.5 { return prev_g; }
            let raw = ((footprint_kb as i64 - prev_kb as i64) as f64 / dt) as i64;
            // clamp insane jumps (e.g. new tree walked), smooth
            let smoothed = (prev_g * 2 + raw) / 3;
            self.history.insert(pid, (footprint_kb, now, smoothed));
            smoothed
        } else {
            self.history.insert(pid, (footprint_kb, now, 0));
            0
        }
    }

    pub fn get(&self, pid: i32) -> i64 {
        self.history.get(&pid).map(|(_,_,g)| *g).unwrap_or(0)
    }
    pub fn prune_stale(&mut self, alive_pids: &[i32]) {
        let alive: std::collections::HashSet<i32> = alive_pids.iter().copied().collect();
        self.history.retain(|k, _| alive.contains(k));
    }

    /// For determinism in tests: inject explicit dt.
    #[cfg(test)]
    pub fn update_with_dt(&mut self, pid: i32, footprint_kb: u64, dt: Duration) -> i64 {
        let now = Instant::now();
        let prev = self.history.get(&pid).copied();
        if let Some((prev_kb, _, prev_g)) = prev {
            let raw = ((footprint_kb as i64 - prev_kb as i64) as f64 / dt.as_secs_f64()) as i64;
            let smoothed = (prev_g * 2 + raw) / 3;
            // back-date instant so next real update uses correct dt
            self.history.insert(pid, (footprint_kb, now - dt, smoothed));
            smoothed
        } else {
            self.history.insert(pid, (footprint_kb, now, 0));
            0
        }
    }
}

/// Reason code derived from growth + pressure + park.
pub fn reason_code(park_candidate: bool, growth_kb_s: i64, free_pct: u8) -> &'static str {
    if park_candidate { "PARK_IDLE" }
    else if growth_kb_s > 200 { "GROWTH_RATE" }
    else if free_pct < 15 { "PRESSURE" }
    else { "NONE" }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn growth_smoothing() {
        let mut r = GrowthRing::new();
        assert_eq!(r.update_with_dt(100, 1000, Duration::from_secs(1)), 0); // first sample
        let g1 = r.update_with_dt(100, 2000, Duration::from_secs(1)); // +1000 in 1s
        assert!(g1 > 300 && g1 < 350); // smoothed (0*2+1000)/3 ≈333
        let g2 = r.update_with_dt(100, 3000, Duration::from_secs(1));
        assert!(g2 > 500); // trending up
    }
    #[test]
    fn reason_codes() {
        assert_eq!(reason_code(true, 0, 50), "PARK_IDLE");
        assert_eq!(reason_code(false, 500, 50), "GROWTH_RATE");
        assert_eq!(reason_code(false, 0, 10), "PRESSURE");
        assert_eq!(reason_code(false, 0, 50), "NONE");
    }
}
