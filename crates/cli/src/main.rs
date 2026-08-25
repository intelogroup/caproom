use caproom_core::{
    collector, growth,
    policy::{is_idle_subtree, TreeView},
    pressure,
    process_tree::Tree,
    CpuRing,
};
use clap::{Parser, Subcommand};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

/// Poll interval seconds — negative or zero would panic Duration::from_millis
fn parse_interval(s: &str) -> Result<f64, String> {
    let v: f64 = s.parse().map_err(|e| format!("invalid interval: {}", e))?;
    if v <= 0.0 {
        return Err("interval must be > 0".into());
    }
    Ok(v)
}

#[derive(Parser)]
#[command(
    name = "caproom",
    version,
    about = "Memory-cap any command — Rust CLI-first v1"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// memory cap in MB (default 4096)
    #[arg(long, default_value = "4096", global = true)]
    limit: u64,
    /// poll interval seconds
    #[arg(long, default_value="0.2", global=true, value_parser=parse_interval)]
    interval: f64,
    /// grace seconds after SIGTERM before SIGKILL
    #[arg(long, default_value = "5", global = true)]
    grace: u64,
    /// offload sink for parked trees: --offload=headroom (opt-in, default off)
    #[arg(long, global = true)]
    offload: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    Freemem,
    Top {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        pid: Option<i32>,
        #[arg(long, default_value = "512")]
        park_min_mb: u64,
    },
    Park {
        pid: i32,
    },
    #[command(name = "park-tree")]
    ParkTree {
        pid: i32,
    },
    Wake {
        pid: Option<i32>,
        #[arg(long)]
        headroom: Option<String>,
        #[arg(long)]
        retrieve: Option<String>,
    },
    #[command(name = "wake-tree")]
    WakeTree {
        pid: i32,
    },
    Status {
        pid: i32,
    },
    /// Retrieve headroom stash by hash (byte-exact, then filter self)
    Retrieve {
        hash: String,
        #[arg(long)]
        out: Option<String>,
    },
    /// Calibrate: suggest limit from current footprint vs total RAM (24GB→14G, 8GB→4G)
    Calibrate {
        #[arg(long, default_value = "30")]
        duration: u64,
    },
    /// Run command under cap: caproom run --limit 2048 -- cmd args
    Run {
        #[arg(long)]
        offload: Option<String>,
        #[arg(last = true)]
        cmd: Vec<String>,
    },
}

fn main() {
    // bare-exec compat: `caproom --limit N -- cmd args` — clap can't model
    // trailing positional args without a subcommand, so split at the first
    // `--` ourselves and parse only the flag side.
    let raw: Vec<String> = std::env::args().collect();
    const SUBCMDS: [&str; 11] = [
        "freemem",
        "top",
        "park",
        "park-tree",
        "wake",
        "wake-tree",
        "status",
        "calibrate",
        "run",
        "retrieve",
        "help",
    ];
    let has_subcmd = raw.iter().skip(1).any(|a| SUBCMDS.contains(&a.as_str()));
    if !has_subcmd {
        if let Some(idx) = raw.iter().position(|a| a == "--") {
            let cmd = raw[idx + 1..].to_vec();
            if !cmd.is_empty() {
                let cli = Cli::parse_from(&raw[..idx]);
                let off = cli.offload.clone();
                cmd_run(cmd, cli.limit, cli.interval, cli.grace, off);
                return;
            }
        }
    }
    let cli = Cli::parse(); // allow `caproom --limit 2048 -- npm run build` compat: treat remaining args as run
                            // clap handles subcommand; for direct exec without `run` keyword, fallback handled below
    // resolve global offload vs run-local
    let global_offload = cli.offload.clone();
    match cli.cmd {
        Some(Cmd::Freemem) => println!("{}", pressure::free_mem_pct()),
        Some(Cmd::Top {
            json,
            pid,
            park_min_mb,
        }) => cmd_top(json, pid, park_min_mb),
        Some(Cmd::Park { pid }) => cmd_park(pid),
        Some(Cmd::ParkTree { pid }) => cmd_park_tree(pid),
        Some(Cmd::Wake { pid, headroom, retrieve }) => cmd_wake(pid, headroom, retrieve),
        Some(Cmd::WakeTree { pid }) => cmd_wake_tree(pid),
        Some(Cmd::Status { pid }) => cmd_status(pid),
        Some(Cmd::Calibrate { duration }) => cmd_calibrate(duration),
        Some(Cmd::Retrieve { hash, out }) => cmd_retrieve(hash, out),
        Some(Cmd::Run { offload, cmd }) => {
            let off = offload.or(global_offload);
            cmd_run(cmd, cli.limit, cli.interval, cli.grace, off)
        }
        None => {
            // parse raw args for `caproom -- cmd` compat
            let args: Vec<String> = std::env::args().collect();
            if let Some(idx) = args.iter().position(|a| a == "--") {
                let cmd = args[idx + 1..].to_vec();
                if !cmd.is_empty() {
                    cmd_run(cmd, cli.limit, cli.interval, cli.grace, global_offload);
                    return;
                }
            }
            eprintln!("usage: caproom [--limit <mb>] [--interval <sec>] -- <command> [args...]");
            eprintln!("       caproom top [--json] [--pid <pid>] | freemem | park <pid> | park-tree <pid> | wake <pid> | wake-tree <pid>");
            std::process::exit(1);
        }
    }
}

static CLI_GROWTH: std::sync::OnceLock<std::sync::Mutex<growth::GrowthRing>> =
    std::sync::OnceLock::new();
fn cli_growth() -> &'static std::sync::Mutex<growth::GrowthRing> {
    CLI_GROWTH.get_or_init(|| std::sync::Mutex::new(growth::GrowthRing::new()))
}
static CLI_CPU: std::sync::OnceLock<std::sync::Mutex<CpuRing>> = std::sync::OnceLock::new();
fn cli_cpu() -> &'static std::sync::Mutex<CpuRing> {
    CLI_CPU.get_or_init(|| std::sync::Mutex::new(CpuRing::new()))
}
/// Truncate for display on char boundary — byte-slicing panics on multibyte UTF-8.
fn truncate_cmd(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn cmd_top(json: bool, filter_pid: Option<i32>, park_min_mb: u64) {
    let snap = collector::snapshot_current_user();
    let ppid_map = snap.ppid_map();
    let roots = if let Some(pid) = filter_pid {
        if snap.by_pid(pid).is_none() {
            eprintln!("caproom: no such pid {}", pid);
            std::process::exit(1);
        }
        vec![pid]
    } else {
        Tree::roots(&snap)
    };
    let park_min_kb = park_min_mb * 1024;
    let free_pct = pressure::free_mem_pct();
    #[derive(serde::Serialize)]
    struct ProcJson {
        pid: i32,
        cmd: String,
        tree_rss_kb: u64,
        footprint_kb: u64,
        tree_pids: Vec<i32>,
        state: String,
        reason_code: String,
        growth_kb_s: i64,
        free_pct: u8,
        park_candidate: bool,
        reason: String,
    }
    let mut out: Vec<ProcJson> = Vec::new();
    let mut ring = cli_growth().lock().unwrap();
    for r in roots {
        if let Some(t) = Tree::build(r, &snap) {
            let state = match t.state {
                'T' => "parked",
                'Z' => "zombie",
                _ => "running",
            };
            let is_sleeping = matches!(t.state, 'S' | 'I');
            let cand = state == "running" && is_sleeping && t.footprint_kb >= park_min_kb;
            let growth_kb_s = ring.update(r, t.footprint_kb);
            let reason_code = growth::reason_code(cand, growth_kb_s, free_pct).to_string();
            let reason = if cand {
                format!(
                    "root sleeping + tree_rss {}KB >= {}KB park threshold",
                    t.footprint_kb, park_min_kb
                )
            } else {
                String::new()
            };
            let _attached = t
                .pids
                .iter()
                .all(|p| caproom_core::policy::still_in_tree(*p, r, &ppid_map));
            out.push(ProcJson {
                pid: r,
                cmd: t.cmd.clone(),
                tree_rss_kb: t.footprint_kb,
                footprint_kb: t.footprint_kb,
                tree_pids: t.pids,
                state: state.to_string(),
                reason_code,
                growth_kb_s,
                free_pct,
                park_candidate: cand,
                reason,
            });
        }
    }
    let ids: Vec<i32> = out.iter().map(|p| p.pid).collect();
    ring.prune_stale(&ids);
    drop(ring);
    out.sort_by_key(|a| std::cmp::Reverse(a.tree_rss_kb));
    if json {
        let v = serde_json::json!({"schema":1,"ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), "limit_mb_default": 4096, "processes": out});
        println!("{}", serde_json::to_string(&v).unwrap());
    } else {
        println!(
            "{:<8} {:>10} {:<8} {:<9} COMMAND",
            "PID", "TREE_MB", "STATE", "PIDS"
        );
        for p in &out {
            println!(
                "{:<8} {:>10} {:<8} {:<9} {}",
                p.pid,
                p.tree_rss_kb / 1024,
                p.state,
                p.tree_pids.len(),
                truncate_cmd(&p.cmd, 60)
            );
        }
    }
}

/// kill(0) signals our own process group, kill(1) targets init — reject both.
fn valid_pid(pid: i32) -> bool {
    pid > 1
}

#[cfg(unix)]
fn cmd_park(pid: i32) {
    if !valid_pid(pid) {
        eprintln!("caproom: invalid pid {} — must be > 1", pid);
        std::process::exit(1);
    }
    if unsafe { libc::kill(pid, libc::SIGSTOP) } != 0 {
        eprintln!("caproom: no such pid {}", pid);
        std::process::exit(1);
    }
    eprintln!("caproom: pid {} parked (SIGSTOP) — pages eligible for reclaim under pressure; wake with: caproom wake {}", pid, pid);
}
#[cfg(windows)]
fn cmd_park(pid: i32) {
    // Windows: use taskkill /T equivalent via suspend (stub — proper impl via NT suspend)
    eprintln!("caproom: park not implemented on Windows for pid {}", pid);
    std::process::exit(1);
}
#[cfg(unix)]
fn cmd_wake(pid: Option<i32>, headroom: Option<String>, retrieve: Option<String>) {
    // headroom/retrieve path: bare retrieve, byte-exact, then filter self (caller filters)
    if let Some(h) = retrieve.or(headroom) {
        let hash = h.trim().to_string();
        // also accept headroom:hash prefix
        let bare = hash.strip_prefix("headroom:").unwrap_or(&hash).to_string();
        match caproom_core::offload::retrieve(&bare) {
            Ok(data) => {
                // write to stdout bare (byte-exact), stderr logs hash
                use std::io::Write;
                let _ = std::io::stdout().write_all(&data);
                eprintln!("caproom: retrieved headroom:{} ({} bytes, byte-exact)", &bare[..bare.len().min(8)], data.len());
                // if offload was a parked tree, also SIGCONT the pid if known from payload?
                // try to wake pid if provided, otherwise just retrieve
                if let Some(p) = pid {
                    if valid_pid(p) {
                        unsafe { libc::kill(p, libc::SIGCONT); }
                    }
                }
            }
            Err(e) => {
                eprintln!("caproom: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
    let Some(p) = pid else {
        eprintln!("caproom: wake requires <pid> or --headroom <hash> / --retrieve <hash>");
        std::process::exit(1);
    };
    if !valid_pid(p) {
        eprintln!("caproom: invalid pid {} — must be > 1", p);
        std::process::exit(1);
    }
    if unsafe { libc::kill(p, libc::SIGCONT) } != 0 {
        eprintln!("caproom: no such pid {}", p);
        std::process::exit(1);
    }
    eprintln!("caproom: pid {} woken (SIGCONT)", p);
}
#[cfg(windows)]
fn cmd_wake(pid: Option<i32>, headroom: Option<String>, retrieve: Option<String>) {
    if headroom.is_some() || retrieve.is_some() {
        let h = headroom.or(retrieve).unwrap();
        match caproom_core::offload::retrieve(h.strip_prefix("headroom:").unwrap_or(&h)) {
            Ok(data) => {
                use std::io::Write;
                let _ = std::io::stdout().write_all(&data);
                eprintln!("caproom: retrieved headroom ({} bytes)", data.len());
            }
            Err(e) => {
                eprintln!("caproom: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(p) = pid {
        eprintln!("caproom: wake not implemented on Windows for pid {}", p);
    } else {
        eprintln!("caproom: wake requires pid on Windows");
    }
    std::process::exit(1);
}

#[cfg(unix)]
fn cmd_park_tree(pid: i32) {
    if !valid_pid(pid) {
        eprintln!("caproom: invalid pid {} — must be > 1", pid);
        std::process::exit(1);
    }
    let snap = collector::snapshot_current_user();
    if let Some(tree) = caproom_core::process_tree::Tree::build(pid, &snap) {
        // PID reuse guard: verify start_time matches snapshot
        let mut stopped = 0;
        for p in &tree.pids {
            if let Some(proc) = snap.by_pid(*p) {
                // re-read current start_time for guard
                if let Some(cur) = collector::snapshot_current_user().by_pid(*p) {
                    if cur.start_time != 0
                        && proc.start_time != 0
                        && cur.start_time != proc.start_time
                    {
                        eprintln!(
                            "caproom: pid {} reused (start {} != {}), skip",
                            p, proc.start_time, cur.start_time
                        );
                        continue;
                    }
                }
                if unsafe { libc::kill(*p, libc::SIGSTOP) } == 0 {
                    stopped += 1;
                }
            }
        }
        eprintln!(
            "caproom: tree {} pids parked (SIGSTOP) — wake with: caproom wake-tree {}",
            stopped, pid
        );
        if stopped == 0 {
            std::process::exit(1);
        }
    } else {
        eprintln!("caproom: no such pid {}", pid);
        std::process::exit(1);
    }
}
#[cfg(windows)]
fn cmd_park_tree(pid: i32) {
    eprintln!(
        "caproom: park-tree not implemented on Windows for pid {}",
        pid
    );
    std::process::exit(1);
}

#[cfg(unix)]
fn cmd_wake_tree(pid: i32) {
    if !valid_pid(pid) {
        eprintln!("caproom: invalid pid {} — must be > 1", pid);
        std::process::exit(1);
    }
    let snap = collector::snapshot_current_user();
    if let Some(tree) = caproom_core::process_tree::Tree::build(pid, &snap) {
        let mut woken = 0;
        for p in &tree.pids {
            if unsafe { libc::kill(*p, libc::SIGCONT) } == 0 {
                woken += 1;
            }
        }
        eprintln!("caproom: tree {} pids woken (SIGCONT)", woken);
        if woken == 0 {
            std::process::exit(1);
        }
    } else {
        // pid may be stopped and not in snapshot? try single wake as fallback
        if unsafe { libc::kill(pid, libc::SIGCONT) } == 0 {
            eprintln!("caproom: pid {} woken (SIGCONT) fallback", pid);
        } else {
            eprintln!("caproom: no such pid {}", pid);
            std::process::exit(1);
        }
    }
}
#[cfg(windows)]
fn cmd_wake_tree(pid: i32) {
    eprintln!(
        "caproom: wake-tree not implemented on Windows for pid {}",
        pid
    );
    std::process::exit(1);
}
fn cmd_status(pid: i32) {
    let out = std::process::Command::new("ps")
        .args(["-o", "pid,stat,rss,etime,command=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    print!("{}", String::from_utf8_lossy(&out.stdout));
    if !out.status.success() {
        eprintln!("caproom: no such pid {}", pid);
        std::process::exit(1);
    }
    // headroom offload sink: print hash if present (caproom status <pid> prints headroom:hash)
    let hline = caproom_core::offload::status_line(pid);
    println!("{}", hline);
}

fn cmd_retrieve(hash: String, out: Option<String>) {
    let bare = hash.strip_prefix("headroom:").unwrap_or(&hash).to_string();
    match caproom_core::offload::retrieve(&bare) {
        Ok(data) => {
            if let Some(path) = out {
                std::fs::write(&path, &data).unwrap_or_else(|e| {
                    eprintln!("caproom: write {} failed: {}", path, e);
                    std::process::exit(1);
                });
                eprintln!("caproom: retrieved headroom:{} → {} ({} bytes)", &bare[..bare.len().min(8)], path, data.len());
            } else {
                use std::io::Write;
                let _ = std::io::stdout().write_all(&data);
                eprintln!("caproom: retrieved headroom:{} ({} bytes, byte-exact — filter self after bare retrieve)", &bare[..bare.len().min(8)], data.len());
            }
        }
        Err(e) => {
            eprintln!("caproom: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_calibrate(duration: u64) {
    use std::process::Command;
    let total_kb = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
                .map(|b| b / 1024)
        })
        .unwrap_or(24 * 1024 * 1024);
    let total_gb = total_kb as f64 / (1024.0 * 1024.0);
    let snap = collector::snapshot_current_user();
    // measure top 3 trees to estimate agent envelope
    let mut trees: Vec<_> = Tree::roots(&snap)
        .into_iter()
        .filter_map(|r| Tree::build(r, &snap))
        .collect();
    trees.sort_by_key(|t| std::cmp::Reverse(t.footprint_kb));
    let top_mb: u64 = trees.iter().take(3).map(|t| t.footprint_kb / 1024).sum();
    // suggestion: 60% of total for 24GB→14G, clamp to 4G min, 80% max
    let suggested = ((total_kb as f64 * 0.6) as u64)
        .clamp(4096 * 1024 / 1024, (total_kb as f64 * 0.8) as u64)
        / 1024;
    // migration note: old RSS limit 6144 ≈ footprint ~80% due to shared overcount
    let rss_equiv = (suggested as f64 * 1.25) as u64;
    println!("caproom calibrate ({}s canary)", duration);
    println!(" total RAM: {:.1}GB ({}MB)", total_gb, total_kb / 1024);
    println!(
        " free mem: {}% ({}MB avail)",
        pressure::free_mem_pct(),
        (total_kb as f64 * pressure::free_mem_pct() as f64 / 100.0) as u64 / 1024
    );
    println!(
        " top trees footprint: {}MB ({} trees)",
        top_mb,
        trees.len().min(3)
    );
    for t in trees.iter().take(3) {
        println!(
            "   pid {} {}MB {}",
            t.root_pid,
            t.footprint_kb / 1024,
            t.cmd.chars().take(50).collect::<String>()
        );
    }
    println!(
        " suggested --limit: {}MB ({}G)",
        suggested,
        suggested / 1024
    );
    println!(" migration: old RSS --limit {} ≈ footprint {} (footprint ~80% of RSS due to shared overcount)", rss_equiv, suggested);
    println!(
        " usage: caproom run --limit {} -- claude  (or CAPROOM_LIMIT_MB={} claude)",
        suggested, suggested
    );
    if duration > 0 {
        println!(
            " canary: run `caproom run --limit {} -- <your build>` for {}s to validate",
            suggested, duration
        );
    }
}

fn cmd_run(cmd: Vec<String>, limit_mb: u64, interval: f64, grace: u64, offload: Option<String>) {
    if cmd.is_empty() {
        eprintln!("caproom: no command");
        std::process::exit(1);
    }
    let free0 = pressure::free_mem_pct();
    let eff = pressure::effective_limit(limit_mb, free0);
    #[cfg(target_os = "macos")]
    let pressure_note = if pressure::try_init_pressure_source() {
        "event-driven"
    } else {
        "poll fallback 200ms (dispatch unavailable, v1.1 daemon will use GCD source)"
    };
    #[cfg(not(target_os = "macos"))]
    let pressure_note = "poll";
    let offload_note = if offload.as_deref() == Some("headroom") { " offload=headroom" } else { "" };
    eprintln!("caproom: watchdog limit={}MB effective={}MB (free {}%) {} poll={}s grace={}s{} — phys_footprint", limit_mb, eff, free0, pressure_note, interval, grace, offload_note);
    // fork exec
    let mut child = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .spawn()
        .expect("spawn failed");
    let pid = child.id() as i32;
    let limit_kb = eff * 1024;
    let interval_d = std::time::Duration::from_millis((interval * 1000.0) as u64);
    loop {
        std::thread::sleep(interval_d);
        if let Ok(Some(status)) = child.try_wait() {
            #[cfg(unix)]
            let code = status
                .code()
                .or_else(|| status.signal().map(|s| 128 + s))
                .unwrap_or(0);
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

                rg.update(pid, tree.footprint_kb)
            };
            let growth_trigger = caproom_core::growth::should_enforce_growth(
                growth_kb_s,
                tree.footprint_kb,
                eff_kb,
                free,
            );
            if tree.footprint_kb >= eff_kb || growth_trigger {
                // fix #2: park idle subtrees → resample → only TERM if still over
                let states: HashMap<i32, char> =
                    snap.procs.iter().map(|p| (p.pid, p.state)).collect();
                let foot: HashMap<i32, u64> =
                    snap.procs.iter().map(|p| (p.pid, p.footprint_kb)).collect();
                // v0.9: is_session_leader via pid == sid (true session leader), fallback pgid==pid if sid unknown
                // protects Ghostty->tmux->shell->claude foreground; pgid alone conflates group vs session
                let leaders: HashMap<i32, bool> = snap
                    .procs
                    .iter()
                    .map(|p| {
                        let is_leader = if p.sid != 0 {
                            p.pid == p.sid
                        } else {
                            p.pid == p.pgid
                        };
                        (p.pid, is_leader)
                    })
                    .collect();
                let cpu: HashMap<i32, f32> = {
                    let mut ring = cli_cpu().lock().unwrap();
                    snap.procs
                        .iter()
                        .map(|p| {
                            // cpu_time_ns == 0 means unknown (ps fallback path) — treat as
                            // busy so an unmeasured process is never parked on bad data
                            let d = if p.cpu_time_ns == 0 {
                                1.0
                            } else {
                                ring.update(p.pid, p.cpu_time_ns)
                            };
                            (p.pid, d)
                        })
                        .collect()
                };
                let view = TreeView {
                    root: pid,
                    pids: &tree.pids,
                    states: &states,
                    footprints: &foot,
                    is_session_leader: &leaders,
                    cpu_delta: &cpu,
                };
                let mut idle: Vec<i32> = tree
                    .pids
                    .iter()
                    .copied()
                    .filter(|p| is_idle_subtree(*p, pid, &view, &ppid_map))
                    .collect();
                let offload_enabled = offload.as_deref() == Some("headroom");
                // If offload is opt-in and idle empty on first sample (cpu 1.0 first-sight busy),
                // give CpuRing 600ms to settle — true idle will drop <0.02, busy stays >0.02.
                // This prevents immediate TERM before offload can stash park_candidate=true trees.
                if idle.is_empty() && offload_enabled {
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    let snap2 = collector::snapshot_current_user();
                    let ppid_map2 = snap2.ppid_map();
                    let states2: HashMap<i32, char> = snap2.procs.iter().map(|p| (p.pid, p.state)).collect();
                    let foot2: HashMap<i32, u64> = snap2.procs.iter().map(|p| (p.pid, p.footprint_kb)).collect();
                    let leaders2: HashMap<i32, bool> = snap2.procs.iter().map(|p| {
                        let is_leader = if p.sid != 0 { p.pid == p.sid } else { p.pid == p.pgid };
                        (p.pid, is_leader)
                    }).collect();
                    let cpu2: HashMap<i32, f32> = {
                        let mut ring = cli_cpu().lock().unwrap();
                        snap2.procs.iter().map(|p| {
                            let d = if p.cpu_time_ns == 0 { 1.0 } else { ring.update(p.pid, p.cpu_time_ns) };
                            (p.pid, d)
                        }).collect()
                    };
                    if let Some(tree2) = Tree::build(pid, &snap2) {
                        let view2 = TreeView {
                            root: pid,
                            pids: &tree2.pids,
                            states: &states2,
                            footprints: &foot2,
                            is_session_leader: &leaders2,
                            cpu_delta: &cpu2,
                        };
                        let idle2: Vec<i32> = tree2.pids.iter().copied().filter(|p| is_idle_subtree(*p, pid, &view2, &ppid_map2)).collect();
                        if !idle2.is_empty() {
                            idle = idle2;
                        }
                    }
                }
                if !idle.is_empty() {
                    // headroom offload sink: freeze then stash byte-exact snapshot (log+snapshot)
                    // ~67% on log-shaped text, retrieve bare then filter self (headroom bug note)
                    // Dedup: if already stashed for this root, reuse hash instead of spamming new files
                    let mut stash_hash: Option<String> = None;
                    let already = if offload_enabled { caproom_core::offload::hash_for_pid(pid) } else { None };
                    if offload_enabled && already.is_none() {
                        // will stash after freeze; keep None until after SIGSTOP
                    } else if let Some(h) = already {
                        stash_hash = Some(h);
                    }
                    #[cfg(unix)]
                    for p in &idle {
                        unsafe {
                            libc::kill(*p, libc::SIGSTOP);
                        }
                    }
                    #[cfg(windows)]
                    for _ in &idle {}
                    if offload_enabled && stash_hash.is_none() {
                        let payload = format!("idle {} pids footprint {}KB log+snapshot", idle.len(), tree.footprint_kb);
                        match caproom_core::offload::compress_snapshot(pid, payload.as_bytes()) {
                            Ok(h) => {
                                stash_hash = Some(h.clone());
                                eprintln!("caproom: headroom stash {} (67% on log-shaped) — retrieve: caproom wake --headroom {}", &h[..h.len().min(8)], h);
                            }
                            Err(e) => eprintln!("caproom: headroom stash failed: {}", e),
                        }
                    }
                    eprint!("\x07"); // bell — visible park signal
                    eprintln!(
                        "caproom: parked idle {} pids (wake: caproom wake {})",
                        idle.len(),
                        pid
                    );
                    if let Some(ref h) = stash_hash {
                        eprintln!("caproom: offload headroom:{} (park kept)", &h[..h.len().min(8)]);
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    if let Some(cur) = Tree::build(pid, &collector::snapshot_current_user()) {
                        let cur_eff =
                            pressure::effective_limit(limit_mb, pressure::free_mem_pct()) * 1024;
                        if cur.footprint_kb < cur_eff {
                            if let Some(ref h) = stash_hash {
                                eprintln!(
                                    "caproom: park relieved {}KB -> {}KB, TERM skipped — headroom:{}",
                                    tree.footprint_kb, cur.footprint_kb, h
                                );
                            } else {
                                eprintln!(
                                    "caproom: park relieved {}KB -> {}KB, TERM skipped",
                                    tree.footprint_kb, cur.footprint_kb
                                );
                            }
                            continue;
                        }
                        // offload sink: keep STOP + keep hash for `wake --retrieve <hash>` / `wake --headroom <hash>`
                        // don't TERM — offload is the reclaim sink, idle stays parked
                        if offload_enabled && stash_hash.is_some() {
                            let h = stash_hash.clone().unwrap();
                            eprintln!(
                                "caproom: offloaded {}MB → headroom:{}... (park kept, TERM skipped — wake --retrieve {})",
                                tree.footprint_kb / 1024,
                                &h[0..h.len().min(8)],
                                h
                            );
                            // also surface via status file already indexed; continue watching without TERM
                            continue;
                        }
                    }
                }
                eprintln!(
                    "caproom: pid {} tree {}KB exceeded {}KB cap — TERM grace {}s",
                    pid, tree.footprint_kb, limit_kb, grace
                );
                // PID reuse guard: don't signal pid that has been recycled (pid, start_time)
                let start_map: std::collections::HashMap<i32, u64> = snap
                    .procs
                    .iter()
                    .map(|pr| (pr.pid, pr.start_time))
                    .collect();
                let cur_snap_for_term = collector::snapshot_current_user();
                #[cfg(unix)]
                for p in &tree.pids {
                    if let Some(&orig_start) = start_map.get(p) {
                        if orig_start != 0 {
                            if let Some(cur) = cur_snap_for_term.by_pid(*p) {
                                if cur.start_time != orig_start {
                                    eprintln!(
                                        "caproom: pid {} reused ({} != {}), skip TERM",
                                        p, cur.start_time, orig_start
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                    unsafe {
                        libc::kill(*p, libc::SIGTERM);
                    }
                }
                #[cfg(windows)]
                for p in &tree.pids {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &p.to_string(), "/T"])
                        .output();
                }
                let mut waited = 0;
                let mut child_exit: Option<i32> = None;
                while waited < grace {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    waited += 1;
                    if let Ok(Some(s)) = child.try_wait() {
                        // root may die before stragglers — record its code but
                        // fall through to the KILL sweep so no orphan survives
                        #[cfg(unix)]
                        {
                            child_exit = Some(
                                s.code()
                                    .or_else(|| s.signal().map(|sig| 128 + sig))
                                    .unwrap_or(0),
                            );
                        }
                        #[cfg(windows)]
                        {
                            child_exit = Some(s.code().unwrap_or(0));
                        }
                        break;
                    }
                }
                #[cfg(unix)]
                for p in &tree.pids {
                    if let Some(&orig_start) = start_map.get(p) {
                        if orig_start != 0 {
                            if let Some(cur) = collector::snapshot_current_user().by_pid(*p) {
                                if cur.start_time != orig_start {
                                    eprintln!("caproom: pid {} reused, skip KILL", p);
                                    continue;
                                }
                            }
                        }
                    }
                    unsafe {
                        libc::kill(*p, libc::SIGKILL);
                    }
                }
                #[cfg(windows)]
                for p in &tree.pids {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &p.to_string(), "/F", "/T"])
                        .output();
                }
                let status = child.wait().unwrap();
                #[cfg(unix)]
                let code = status
                    .code()
                    .or_else(|| status.signal().map(|sig| 128 + sig))
                    .or(child_exit)
                    .unwrap_or(137);
                #[cfg(windows)]
                let code = status.code().or(child_exit).unwrap_or(137);
                std::process::exit(code);
            }
        } else {
            #[cfg(unix)]
            if let Ok(Some(s)) = child.try_wait() {
                let code = s
                    .code()
                    .or_else(|| s.signal().map(|sig| 128 + sig))
                    .unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncate_cmd_never_panics_on_multibyte() {
        let emoji_cmd = "node 🦀🦀🦀🦀🦀 --flag".repeat(10);
        let _ = truncate_cmd(&emoji_cmd, 60);
        assert!(truncate_cmd(&emoji_cmd, 60).chars().count() <= 60);
        assert_eq!(truncate_cmd("short", 60), "short");
    }
    #[test]
    fn parse_interval_validation() {
        assert!(parse_interval("0.5").is_ok());
        assert!(parse_interval("1.0").is_ok());
        assert!(parse_interval("0").is_err());
        assert!(parse_interval("-1").is_err());
        assert!(parse_interval("abc").is_err());
    }
}
