use std::collections::HashMap;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

/// Tracks per-pid cpu_time_ns and computes fractional CPU usage (1.0 = 100% one core).
/// Same pattern as GrowthRing but for cpu. v1 CLI keeps this in-process; daemon v1.1 shares via UDS.
#[derive(Debug, Default)]
pub struct CpuRing {
    history: HashMap<i32, (u64, Instant, f32)>,
}

impl CpuRing {
    pub fn new() -> Self {
        Self {
            history: HashMap::new(),
        }
    }

    /// Update with current total cpu_time_ns, return fractional CPU (1.0 = 100% one core).
    /// First sample returns 1.0 (assume busy) to avoid parking active watcher on first sight;
    /// after 500ms second sample yields true idle (<0.02) vs busy (>0.02).
    /// If elapsed < 500ms, return prior value to avoid jitter.
    pub fn update(&mut self, pid: i32, cpu_ns: u64) -> f32 {
        let now = Instant::now();
        if let Some((prev_cpu, prev_t, prev_f)) = self.history.get(&pid).copied() {
            let dt = now.duration_since(prev_t).as_secs_f64();
            if dt < 0.5 {
                return prev_f;
            }
            let delta = cpu_ns.saturating_sub(prev_cpu) as f64;
            let frac = (delta / 1e9) / dt;
            let clamped = frac.clamp(0.0, 8.0) as f32;
            self.history.insert(pid, (cpu_ns, now, clamped));
            clamped
        } else {
            // first sight: assume busy until proven idle 500ms later
            self.history.insert(pid, (cpu_ns, now, 1.0));
            1.0
        }
    }

    pub fn get(&self, pid: i32) -> f32 {
        self.history.get(&pid).map(|(_, _, f)| *f).unwrap_or(0.0)
    }

    pub fn prune(&mut self, alive: &[i32]) {
        let set: std::collections::HashSet<i32> = alive.iter().copied().collect();
        self.history.retain(|k, _| set.contains(k));
    }

    #[cfg(test)]
    pub fn update_with_dt(&mut self, pid: i32, cpu_ns: u64, dt: Duration) -> f32 {
        let now = Instant::now();
        if let Some((prev_cpu, _, _)) = self.history.get(&pid).copied() {
            let delta = cpu_ns.saturating_sub(prev_cpu) as f64;
            let frac = (delta / 1e9) / dt.as_secs_f64();
            let clamped = frac.clamp(0.0, 8.0) as f32;
            self.history.insert(pid, (cpu_ns, now, clamped));
            clamped
        } else {
            self.history.insert(pid, (cpu_ns, now, 1.0));
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_idle_vs_busy() {
        let mut r = CpuRing::new();
        assert_eq!(r.update_with_dt(100, 0, Duration::from_secs(1)), 1.0); // first sight busy
                                                                           // 10ms cpu in 1s = 1%
        let f1 = r.update_with_dt(100, 10_000_000, Duration::from_secs(1));
        assert!((f1 - 0.01).abs() < 0.001, "got {f1}");
        // 500ms in 1s = 50%
        let f2 = r.update_with_dt(100, 510_000_000, Duration::from_secs(1));
        assert!((f2 - 0.5).abs() < 0.01, "got {f2}");
    }
    #[test]
    fn cpu_below_threshold() {
        let mut r = CpuRing::new();
        assert_eq!(r.update_with_dt(1, 0, Duration::from_secs(1)), 1.0);
        let f = r.update_with_dt(1, 15_000_000, Duration::from_secs(1)); // 1.5% < 2%
        assert!(f < 0.02);
        let f2 = r.update_with_dt(1, 115_000_000, Duration::from_secs(1)); // +100ms =10% >2%
        assert!(f2 >= 0.02);
    }
    #[test]
    fn prune_drops_history() {
        let mut r = CpuRing::new();
        r.update_with_dt(1, 0, Duration::from_secs(1));
        r.update_with_dt(2, 0, Duration::from_secs(1));
        r.prune(&[1]);
        assert_eq!(r.get(1), 1.0);
        assert_eq!(r.get(2), 0.0); // pruned -> default
    }
    #[test]
    fn get_missing_defaults_zero() {
        let r = CpuRing::new();
        assert_eq!(r.get(999), 0.0);
    }
}
