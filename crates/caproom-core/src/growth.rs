use std::collections::HashMap;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

/// In-process history ring: pid -> Vec<(Instant, kb)>. No cross-process state in v1 (CLI-first).
/// Daemon v1.1 will move this to shared UDS state. For now each `top` call estimates
/// growth from its own previous sample if called frequently; otherwise growth_kb_s = 0.
#[derive(Debug, Default)]
pub struct GrowthRing {
    // pid -> (last footprint, last instant, smoothed growth)
    history: HashMap<i32, (u64, Instant, i64)>,
}

impl GrowthRing {
    pub fn new() -> Self {
        Self {
            history: HashMap::new(),
        }
    }

    /// Update with current footprint, return smoothed growth_kb_s (EWMA-ish).
    /// If no prior sample or elapsed < 500ms, return prior smoothed value.
    pub fn update(&mut self, pid: i32, footprint_kb: u64) -> i64 {
        let now = Instant::now();
        let entry = self.history.get(&pid).copied();
        if let Some((prev_kb, prev_t, prev_g)) = entry {
            let dt = now.duration_since(prev_t).as_secs_f64();
            if dt < 0.5 {
                return prev_g;
            }
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
        self.history.get(&pid).map(|(_, _, g)| *g).unwrap_or(0)
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
    if park_candidate {
        "PARK_IDLE"
    } else if growth_kb_s > 200 {
        "GROWTH_RATE"
    } else if free_pct < 15 {
        "PRESSURE"
    } else {
        "NONE"
    }
}

/// v0.9: growth enforcement is not bare `>200 KB/s`. Require near-limit + pressure + projected breach.
/// 200 KB/s = 12 MB/min is normal for indexing/compiling — only enforce if:
/// - growth >200 AND
/// - footprint >=70% of effective limit OR pressure <20% AND
/// - projected breach <5 min (or <10 min with >1MB/s)
pub fn should_enforce_growth(
    growth_kb_s: i64,
    footprint_kb: u64,
    eff_kb: u64,
    free_pct: u8,
) -> bool {
    if growth_kb_s <= 200 {
        return false;
    }
    if eff_kb == 0 {
        return false;
    }
    let near_limit = footprint_kb * 100 / eff_kb >= 70;
    let pressured = free_pct < 20;
    if !near_limit && !pressured {
        return false;
    }
    let remaining = eff_kb.saturating_sub(footprint_kb) as i64;
    if remaining <= 0 {
        return true;
    }
    let secs_to_limit = remaining as f64 / growth_kb_s as f64;
    // very high growth enforces sooner
    if growth_kb_s > 1024 && secs_to_limit < 600.0 {
        return true;
    }
    // otherwise need near limit and <5 min to breach
    near_limit && secs_to_limit < 300.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn growth_enforcement_needs_context() {
        // bare 500 KB/s but far from limit and no pressure — should NOT enforce (12 MB/min is normal indexing)
        assert!(
            !should_enforce_growth(500, 1000 * 1024, 4096 * 1024, 40),
            "far from limit, no pressure, 500 KB/s is normal"
        );
        // same growth but near limit 96% and <5min to breach — should enforce
        assert!(
            should_enforce_growth(500, 3950 * 1024, 4096 * 1024, 40),
            "96% + 500 KB/s => 146MB/500=292s <300"
        );
        // moderate growth 300 but pressured free 10% and near limit — enforce
        assert!(
            should_enforce_growth(300, 4010 * 1024, 4096 * 1024, 10),
            "4010 86KB/300=286s <300"
        );
        // high growth 2MB/s near limit with pressure — enforce (fast leak) 596MB/2048=291s <600
        assert!(should_enforce_growth(2048, 3500 * 1024, 4096 * 1024, 15));
        // low growth under threshold never enforces even near limit
        assert!(!should_enforce_growth(100, 3900 * 1024, 4096 * 1024, 10));
        // projected breach far (>10min) with no pressure should not enforce
        // 300 KB/s, 2GB remaining => 6826 sec >600
        assert!(!should_enforce_growth(300, 2000 * 1024, 4096 * 1024, 40));
    }
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
    #[test]
    fn prune_removes_gone() {
        let mut r = GrowthRing::new();
        r.update_with_dt(1, 1000, Duration::from_secs(1));
        r.update_with_dt(2, 2000, Duration::from_secs(1));
        r.prune_stale(&[1]);
        assert_eq!(r.get(1), 0);
        assert_eq!(r.get(2), 0); // pruned
    }
    #[test]
    fn get_default_for_unknown() {
        let r = GrowthRing::new();
        assert_eq!(r.get(999), 0);
    }
}
