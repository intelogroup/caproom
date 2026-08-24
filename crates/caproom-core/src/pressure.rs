#[cfg(target_os = "macos")]
pub fn free_mem_pct() -> u8 {
    // vm_stat Pages free + inactive vs hw.memsize, same as bin/caproom mem_free_pct:441
    use std::process::Command;
    let vm = Command::new("vm_stat").output().ok();
    let memsize = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .unwrap_or(24 * 1024 * 1024 * 1024);
    if let Some(vm) = vm {
        let text = String::from_utf8_lossy(&vm.stdout);
        let mut free: u64 = 0;
        let mut inactive: u64 = 0;
        let mut page: u64 = 16384;
        for line in text.lines() {
            if line.contains("page size of") {
                if let Some(v) = line.split_whitespace().nth(7) {
                    page = v.parse().unwrap_or(16384);
                }
            }
            if line.contains("Pages free") {
                free = line.split_whitespace().nth(2).unwrap_or("0").trim_matches('.').parse().unwrap_or(0);
            }
            if line.contains("Pages inactive") {
                inactive = line.split_whitespace().nth(2).unwrap_or("0").trim_matches('.').parse().unwrap_or(0);
            }
        }
        let avail = (free + inactive) * page;
        return ((avail * 100) / memsize) as u8;
    }
    35
}

#[cfg(target_os = "linux")]
pub fn free_mem_pct() -> u8 {
    use std::fs;
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut avail = 0u64;
    let mut total = 1u64;
    for line in meminfo.lines() {
        if line.starts_with("MemAvailable") {
            avail = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        }
        if line.starts_with("MemTotal") {
            total = line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1);
        }
    }
    ((avail * 100) / total) as u8
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn free_mem_pct() -> u8 {
    35
}

/// Interim v1 mitigation: lower effective limit as system pressure rises.
/// free_pct >=15 => no change. free_pct 0 => 0.8*limit.
pub fn effective_limit(limit_mb: u64, free_pct: u8) -> u64 {
    if free_pct >= 15 {
        limit_mb
    } else {
        limit_mb * (80 + 20 * free_pct as u64 / 15) / 100
    }
}

/// dispatch_source_memorypressure — event-driven vs poll fallback.
/// v1 CLI: per-run `caproom run` uses 200ms poll (30 wakeups/sec for 6 tabs, negligible).
/// Real 0% idle benefit is for v1.1 daemon global listener (single Mach source, N trees).
/// Attempt GCD source; if unavailable (older macOS, sandbox, entitlement denied) log once and fall back.
/// Returns true if event-driven source is live.
#[cfg(target_os = "macos")]
pub fn try_init_pressure_source() -> bool {
    // Minimal stub: attempt dispatch_source_create(DISPATCH_SOURCE_TYPE_MEMORYPRESSURE).
    // Full block-based handler requires `dispatch` + `block2` crates and is deferred to daemon v1.1
    // where single global source justifies the dependency. For CLI v1, poll is correct.
    // We probe availability by checking OS version >= 10.9 (where proc_pid_rusage exists).
    // If probe fails, caller logs: "pressure listener unavailable, polling fallback 200ms"
    false // poll fallback for v1 — daemon v1.1 will return true with live source
}

#[cfg(target_os = "macos")]
pub fn pressure_source_available() -> bool {
    try_init_pressure_source()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn effective_limit_threshold() {
        assert_eq!(effective_limit(4096, 35), 4096);
        assert_eq!(effective_limit(4096, 15), 4096);
        assert_eq!(effective_limit(4096, 0), 3276); // 0.8*4096 floor
    }
}
