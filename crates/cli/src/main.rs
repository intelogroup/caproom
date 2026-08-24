use caproom_core::{collector, growth, pressure, process_tree::Tree, policy::{is_idle_subtree, TreeView}, CpuRing};
use clap::{Parser, Subcommand};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

#[derive(Parser)]
#[command(name="caproom", version, about="Memory-cap any command — Rust CLI-first v1")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// memory cap in MB (default 4096)
    #[arg(long, default_value="4096", global=true)]
    limit: u64,
    /// poll interval seconds
    #[arg(long, default_value="0.2", global=true)]
    interval: f64,
    /// grace seconds after SIGTERM before SIGKILL
    #[arg(long, default_value="5", global=true)]
    grace: u64,
}

#[derive(Subcommand)]
enum Cmd {
    Freemem,
    Top { #[arg(long)] json: bool, #[arg(long)] pid: Option<i32>, #[arg(long, default_value="512")] park_min_mb: u64 },
    Park { pid: i32 },
    Wake { pid: i32 },
    Status { pid: i32 },
    /// Calibrate: suggest limit from current footprint vs total RAM (24GB→14G, 8GB→4G)
    Calibrate { #[arg(long, default_value="30")] duration: u64 },
    /// Run command under cap: caproom run --limit 2048 -- cmd args
    Run { #[arg(last=true)] cmd: Vec<String> },
}

fn main() {
    let cli = Cli::parse();
    // allow `caproom --limit 2048 -- npm run build` compat: treat remaining args as run
    // clap handles subcommand; for direct exec without `run` keyword, fallback handled below
    match cli.cmd {
        Some(Cmd::Freemem) => println!("{}", pressure::free_mem_pct()),
        Some(Cmd::Top { json, pid, park_min_mb }) => cmd_top(json, pid, park_min_mb),
        Some(Cmd::Park { pid }) => cmd_park(pid),
        Some(Cmd::Wake { pid }) => cmd_wake(pid),
        Some(Cmd::Status { pid }) => cmd_status(pid),
        Some(Cmd::Calibrate { duration }) => cmd_calibrate(duration),
        Some(Cmd::Run { cmd }) => cmd_run(cmd, cli.limit, cli.interval, cli.grace),
        None => {
            // parse raw args for `caproom -- cmd` compat
            let args: Vec<String> = std::env::args().collect();
            if let Some(idx) = args.iter().position(|a| a=="--") {
                let cmd = args[idx+1..].to_vec();
                if !cmd.is_empty() { cmd_run(cmd, cli.limit, cli.interval, cli.grace); return; }
            }
            eprintln!("usage: caproom [--limit <mb>] [--interval <sec>] -- <command> [args...]");
            eprintln!("       caproom top [--json] [--pid <pid>] | freemem | park <pid> | wake <pid>");
            std::process::exit(1);
        }
    }
}

static CLI_GROWTH: std::sync::OnceLock<std::sync::Mutex<growth::GrowthRing>> = std::sync::OnceLock::new();
fn cli_growth() -> &'static std::sync::Mutex<growth::GrowthRing> {
    CLI_GROWTH.get_or_init(|| std::sync::Mutex::new(growth::GrowthRing::new()))
}
static CLI_CPU: std::sync::OnceLock<std::sync::Mutex<CpuRing>> = std::sync::OnceLock::new();
fn cli_cpu() -> &'static std::sync::Mutex<CpuRing> {
    CLI_CPU.get_or_init(|| std::sync::Mutex::new(CpuRing::new()))
}
fn cmd_top(json: bool, filter_pid: Option<i32>, park_min_mb: u64) {
    let snap = collector::snapshot_current_user();
    let ppid_map = snap.ppid_map();
    let roots = if let Some(pid) = filter_pid {
        if snap.by_pid(pid).is_none() { eprintln!("caproom: no such pid {}", pid); std::process::exit(1); }
        vec![pid]
    } else {
        Tree::roots(&snap)
    };
    let park_min_kb = park_min_mb * 1024;
    let free_pct = pressure::free_mem_pct();
    #[derive(serde::Serialize)]
    struct ProcJson { pid: i32, cmd: String, tree_rss_kb: u64, footprint_kb: u64, tree_pids: Vec<i32>, state: String, reason_code: String, growth_kb_s: i64, free_pct: u8, park_candidate: bool, reason: String }
    let mut out: Vec<ProcJson> = Vec::new();
    let mut ring = cli_growth().lock().unwrap();
    for r in roots {
        if let Some(t) = Tree::build(r, &snap) {
            let state = match t.state { 'T' => "parked", 'Z' => "zombie", _ => "running" };
            let is_sleeping = matches!(t.state, 'S' | 'I');
            let cand = state=="running" && is_sleeping && t.footprint_kb >= park_min_kb;
            let growth_kb_s = ring.update(r, t.footprint_kb);
            let reason_code = growth::reason_code(cand, growth_kb_s, free_pct).to_string();
            let reason = if cand { format!("root sleeping + tree_rss {}KB >= {}KB park threshold", t.footprint_kb, park_min_kb) } else { String::new() };
            let _attached = t.pids.iter().all(|p| caproom_core::policy::still_in_tree(*p, r, &ppid_map));
            out.push(ProcJson{ pid: r, cmd: t.cmd.clone(), tree_rss_kb: t.footprint_kb, footprint_kb: t.footprint_kb, tree_pids: t.pids, state: state.to_string(), reason_code, growth_kb_s, free_pct, park_candidate: cand, reason });
        }
    }
    let ids: Vec<i32> = out.iter().map(|p| p.pid).collect();
    ring.prune_stale(&ids);
    drop(ring);
    out.sort_by(|a,b| b.tree_rss_kb.cmp(&a.tree_rss_kb));
    if json {
        let v = serde_json::json!({"schema":1,"ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), "limit_mb_default": 4096, "processes": out});
        println!("{}", serde_json::to_string(&v).unwrap());
    } else {
        println!("{:<8} {:>10} {:<8} {:<9} {}", "PID", "TREE_MB", "STATE", "PIDS", "COMMAND");
        for p in &out { println!("{:<8} {:>10} {:<8} {:<9} {}", p.pid, p.tree_rss_kb/1024, p.state, p.tree_pids.len(), &p.cmd[..p.cmd.len().min(60)]); }
    }
}

#[cfg(unix)]
fn cmd_park(pid: i32) {
    if unsafe { libc::kill(pid, libc::SIGSTOP) } != 0 { eprintln!("caproom: no such pid {}", pid); std::process::exit(1); }
    eprintln!("caproom: pid {} parked (SIGSTOP) — pages eligible for reclaim under pressure; wake with: caproom wake {}", pid, pid);
}
#[cfg(windows)]
fn cmd_park(pid: i32) {
    // Windows: use taskkill /T equivalent via suspend (stub — proper impl via NT suspend)
    eprintln!("caproom: park not implemented on Windows for pid {}", pid);
    std::process::exit(1);
}
#[cfg(unix)]
fn cmd_wake(pid: i32) {
    if unsafe { libc::kill(pid, libc::SIGCONT) } != 0 { eprintln!("caproom: no such pid {}", pid); std::process::exit(1); }
    eprintln!("caproom: pid {} woken (SIGCONT)", pid);
}
#[cfg(windows)]
fn cmd_wake(pid: i32) {
    eprintln!("caproom: wake not implemented on Windows for pid {}", pid);
    std::process::exit(1);
}
fn cmd_status(pid: i32) {
    let out = std::process::Command::new("ps").args(["-o","pid,stat,rss,etime,command=","-p",&pid.to_string()]).output().unwrap();
    print!("{}", String::from_utf8_lossy(&out.stdout));
    if !out.status.success() { eprintln!("caproom: no such pid {}", pid); std::process::exit(1); }
}

fn cmd_calibrate(duration: u64) {
    use std::process::Command;
    let total_kb = Command::new("sysctl").args(["-n","hw.memsize"]).output().ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok().map(|b| b/1024))
        .unwrap_or(24*1024*1024);
    let total_gb = total_kb as f64 / (1024.0*1024.0);
    let snap = collector::snapshot_current_user();
    // measure top 3 trees to estimate agent envelope
    let mut trees: Vec<_> = Tree::roots(&snap).into_iter().filter_map(|r| Tree::build(r, &snap)).collect();
    trees.sort_by(|a,b| b.footprint_kb.cmp(&a.footprint_kb));
    let top_mb: u64 = trees.iter().take(3).map(|t| t.footprint_kb/1024).sum();
    // suggestion: 60% of total for 24GB→14G, clamp to 4G min, 80% max
    let suggested = ((total_kb as f64 * 0.6) as u64).clamp(4096*1024/1024, (total_kb as f64 * 0.8) as u64) /1024;
    // migration note: old RSS limit 6144 ≈ footprint ~80% due to shared overcount
    let rss_equiv = (suggested as f64 * 1.25) as u64;
    println!("caproom calibrate ({}s canary)", duration);
    println!(" total RAM: {:.1}GB ({}MB)", total_gb, total_kb/1024);
    println!(" free mem: {}% ({}MB avail)", pressure::free_mem_pct(), (total_kb as f64 * pressure::free_mem_pct() as f64 /100.0) as u64 /1024);
    println!(" top trees footprint: {}MB ({} trees)", top_mb, trees.len().min(3));
    for t in trees.iter().take(3) { println!("   pid {} {}MB {}", t.root_pid, t.footprint_kb/1024, t.cmd.chars().take(50).collect::<String>()); }
    println!(" suggested --limit: {}MB ({}G)", suggested, suggested/1024);
    println!(" migration: old RSS --limit {} ≈ footprint {} (footprint ~80% of RSS due to shared overcount)", rss_equiv, suggested);
    println!(" usage: caproom run --limit {} -- claude  (or CAPROOM_LIMIT_MB={} claude)", suggested, suggested);
    if duration > 0 {
        println!(" canary: run `caproom run --limit {} -- <your build>` for {}s to validate", suggested, duration);
    }
}

fn cmd_run(cmd: Vec<String>, limit_mb: u64, interval: f64, grace: u64) {
    if cmd.is_empty() { eprintln!("caproom: no command"); std::process::exit(1); }
    let free0 = pressure::free_mem_pct();
    let eff = pressure::effective_limit(limit_mb, free0);
    #[cfg(target_os = "macos")]
    let pressure_note = if pressure::try_init_pressure_source() { "event-driven" } else { "poll fallback 200ms (dispatch unavailable, v1.1 daemon will use GCD source)" };
    #[cfg(not(target_os = "macos"))]
    let pressure_note = "poll";
    eprintln!("caproom: watchdog limit={}MB effective={}MB (free {}%) {} poll={}s grace={}s — phys_footprint", limit_mb, eff, free0, pressure_note, interval, grace);
    // fork exec
    let mut child = std::process::Command::new(&cmd[0]).args(&cmd[1..]).spawn().expect("spawn failed");
    let pid = child.id() as i32;
    let limit_kb = eff * 1024;
    let interval_d = std::time::Duration::from_millis((interval*1000.0) as u64);
    loop {
        std::thread::sleep(interval_d);
        if let Ok(Some(status)) = child.try_wait() {
            #[cfg(unix)]
            let code = status.code().or_else(|| status.signal().map(|s| 128 + s)).unwrap_or(0);
            #[cfg(windows)]
            let code = status.code().unwrap_or(0);
            std::process::exit(code);
        }
        let snap = collector::snapshot_current_user();
        let ppid_map = snap.ppid_map();
        if let Some(tree) = Tree::build(pid, &snap) {
            let free = pressure::free_mem_pct();
            let eff_kb = pressure::effective_limit(limit_mb, free) * 1024;
            // growth trigger via shared history ring (same as MCP): >200 KB/s sustained
            let growth_kb_s = {
                let mut rg = cli_growth().lock().unwrap();
                let g = rg.update(pid, tree.footprint_kb);
                g
            };
            let growth_trigger = caproom_core::growth::should_enforce_growth(growth_kb_s, tree.footprint_kb, eff_kb, free);
            if tree.footprint_kb >= eff_kb || growth_trigger {
                // fix #2: park idle subtrees → resample → only TERM if still over
                let states: HashMap<i32,char> = snap.procs.iter().map(|p| (p.pid, p.state)).collect();
                let foot: HashMap<i32,u64> = snap.procs.iter().map(|p| (p.pid, p.footprint_kb)).collect();
                // wired 0.8.2: derive is_session_leader via pgid==pid (session/group leader, foregroup never parks)
                // and cpu_delta via CpuRing (mach task_info / proc stat delta, 2% threshold keeps active watcher alive)
                let leaders: HashMap<i32,bool> = snap.procs.iter().map(|p| (p.pid, p.pgid == p.pid)).collect();
                let cpu: HashMap<i32,f32> = {
                    let mut ring = cli_cpu().lock().unwrap();
                    snap.procs.iter().map(|p| (p.pid, ring.update(p.pid, p.cpu_time_ns))).collect()
                };
                let view = TreeView{ root: pid, pids: &tree.pids, states: &states, footprints: &foot, is_session_leader: &leaders, cpu_delta: &cpu };
                let idle: Vec<i32> = tree.pids.iter().copied().filter(|p| is_idle_subtree(*p, pid, &view, &ppid_map)).collect();
                if !idle.is_empty() {
#[cfg(unix)]
                    for p in &idle { unsafe { libc::kill(*p, libc::SIGSTOP); } }
                    #[cfg(windows)]
                    for _ in &idle {}
                    eprint!("\x07"); // bell — visible park signal
                    eprintln!("caproom: parked idle {} pids (wake: caproom wake {})", idle.len(), pid);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    if let Some(cur) = Tree::build(pid, &collector::snapshot_current_user()) {
                        let cur_eff = pressure::effective_limit(limit_mb, pressure::free_mem_pct()) * 1024;
                        if cur.footprint_kb < cur_eff {
                            eprintln!("caproom: park relieved {}KB -> {}KB, TERM skipped", tree.footprint_kb, cur.footprint_kb);
                            continue;
                        }
                    }
                }
                eprintln!("caproom: pid {} tree {}KB exceeded {}KB cap — TERM grace {}s", pid, tree.footprint_kb, limit_kb, grace);
#[cfg(unix)]
                for p in &tree.pids { unsafe { libc::kill(*p, libc::SIGTERM); } }
                #[cfg(windows)]
                for p in &tree.pids { let _ = std::process::Command::new("taskkill").args(["/PID", &p.to_string(), "/T"]).output(); }
                let mut waited = 0;
                while waited < grace {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    waited += 1;
                    if let Ok(Some(s)) = child.try_wait() {
#[cfg(unix)]
                        let code = s.code().or_else(|| s.signal().map(|sig| 128 + sig)).unwrap_or(0);
                        #[cfg(windows)]
                        let code = s.code().unwrap_or(0);
                        std::process::exit(code);
                    }
                }
                #[cfg(unix)]
                for p in &tree.pids { unsafe { libc::kill(*p, libc::SIGKILL); } }
                #[cfg(windows)]
                for p in &tree.pids { let _ = std::process::Command::new("taskkill").args(["/PID", &p.to_string(), "/F", "/T"]).output(); }
                let status = child.wait().unwrap();
                #[cfg(unix)]
                let code = status.code().or_else(|| status.signal().map(|sig| 128 + sig)).unwrap_or(137);
                #[cfg(windows)]
                let code = status.code().unwrap_or(137);
                std::process::exit(code);
            }
        } else {
#[cfg(unix)]
            if let Ok(Some(s)) = child.try_wait() {
                let code = s.code().or_else(|| s.signal().map(|sig| 128 + sig)).unwrap_or(0);
                std::process::exit(code);
            }
            #[cfg(windows)]
            if let Ok(Some(s)) = child.try_wait() {
                let code = s.code().unwrap_or(0);
                std::process::exit(code);
            }
            // root reparented / escaped — exit cleanly, don't kill unrelated
            break;
        }
    }
}
