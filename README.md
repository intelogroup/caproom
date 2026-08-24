# caproom

[![npm](https://img.shields.io/npm/v/caproom.svg)](https://www.npmjs.com/package/caproom)
[![license](https://img.shields.io/npm/l/caproom.svg)](LICENSE)

Prevent RAM OOM for long-running terminal coding agents, builds, and background jobs — memory caps plus idle-process parking, for macOS, Linux, and Windows.

## Why

macOS has no reliable way to cap a process's memory from userspace. A runaway process — a leaking build tool, an AI agent stuck in a loop, a stray `find /` — has nothing standing between it and system OOM. `caproom` fills that gap with a host-native polling watchdog that actually enforces a limit on the whole process tree.

## What it does (Rust-only v0.9.0)

```
caproom --limit <mb> -- <command> [args...]
# or explicit subcommand
caproom run --limit 2048 -- npm run build
```

Single binary `bin/caproom-rs` (`1.2M`, `target/release/caproom` copy) — no bash, no JS wrapper, no PowerShell shim. The 1K-line bash watchdog (`bin/caproom`), `bin/caproom.js`, `bin/caproom-mcp.js`, `bin/caproom.ps1`, `scripts/postinstall.js` were removed in `f017051` (0.8.1). Install via `cargo`, `brew`, or `npm` (npm unpacks prebuilt `caproom-darwin-arm64/x64`, `caproom-linux-x64`, `caproom-win32-x64.exe`).

Core mechanism:

- **Collector:** `proc_listallpids` + `proc_pidinfo PROC_PIDTBSDINFO/TASKINFO` + `proc_pid_rusage ri_phys_footprint` (libproc, not `ps`). Measures **phys_footprint** (Mach `TASK_VM_INFO`, Jetsam/Activity Monitor truth) — not RSS. RSS overcounts shared libs. R/S state comes from TASKINFO `pti_numrunning`, because `pbi_status` reports SRUN for processes blocked in `time.sleep`.
- **Tree walk:** whole process tree per poll (agents keep memory in children — MCP servers, bundler daemons, headless browsers — while parent RSS stays flat). Ancestry walk depth 64, reparented to `launchd`/`init` (ppid 1) escapes.
- **Parking predicate `is_idle_subtree`:** `still_in_tree` + `footprint >=512KB` + `!is_session_leader` (`pid == sid` primary, `pgid == pid` fallback — foreground group never parks) + `cpu_delta <0.02` (2% over 500ms window via `CpuRing` from `ri_user_time+ri_system_time` delta). Dropped `pbi_status S|I` gate in 0.8.0 — multithreaded Python/Node reports `R` while sleeping in `time.sleep`.
- **Escalation:** park idle subtrees (`SIGSTOP`) → resample after 1s → only `TERM` → grace `5s` → `KILL` if still over `effective_limit`. `effective_limit = limit * (80 + 20*free_pct/15)/100` (free_pct from `vm_stat`/`MemAvailable`), so low headroom lowers trigger but doesn't escalate without park.
- **Polling-only v1:** `vm_stat`/`free_mem_pct` every `200ms`. `dispatch_source_memorypressure` event-driven (`0% idle`) is **not built** — `pressure::try_init_pressure_source() -> false` and logs `poll fallback 200ms (dispatch unavailable, v1.1 daemon will use GCD source)`. Deferred to daemon v1.1 single Mach source.
- **Docker backend dropped:** no `--docker`/`--image` in Rust (native only). `--pty`/`--no-intercept-tty`/`--force-watchdog`/`init`/`setup`/`SAFE_RUN` also dropped — they were bash-only.

## Install

```bash
# from source (requires Rust stable)
cargo install --locked --path crates/cli
# or
cargo install caproom  # once published to crates.io

# Homebrew (tap exists, formula builds from source)
brew tap intelogroup/caproom
brew install caproom

# npm (unpacks prebuilt binaries, no wrapper)
npm install -g caproom
# clean up old global npm shim if you migrated from bash
npm rm -g caproom; nvm use system; hash -r; which caproom # should be ~/.cargo/bin/caproom
```

Binary: `~/.cargo/bin/caproom` `v0.9.0`, `caproom --help` prints `Memory-cap any command — Rust CLI-first v1`.

## Usage

```bash
# cap a build at 2GB
caproom --limit 2048 -- npm run build
caproom run --limit 2048 -- npm run build

# cap an AI coding agent run at 512MB
caproom --limit 512 -- claude -p "refactor this module"

# inspect trees
caproom freemem                # prints free %
caproom top                    # human table sorted by TREE_MB
caproom top --json             # machine schema 1
caproom top --json --pid 45057 # one subtree
caproom top --json --park-min-mb 1024

# park / wake
caproom park <pid>   # SIGSTOP — pages eligible for reclaim under pressure
caproom wake <pid>   # SIGCONT
caproom status <pid> # ps stat/rss

# calibrate limit from current footprint (24GB->14G, 8GB->4G clamp)
caproom calibrate
caproom calibrate --duration 30
```

### Flags

| Flag | Default | Meaning |
|---|---|---|
| `--limit <mb>` | `4096` | memory cap in MB (phys_footprint) |
| `--interval <sec>` | `0.2` | watchdog poll interval |
| `--grace <sec>` | `5` | seconds to wait after `SIGTERM` before `SIGKILL` |

On breach: `SIGTERM` whole tree → wait `grace` → `SIGKILL` grace survivors from breach snapshot. Exit `143` if TERM honored, `137` if KILL (same as Docker OOM convention, but now native).

## Backends compared (POSIX)

| | Watchdog (bash 0.7.6) | Watchdog (Rust v1) | Watchdog (Windows) |
|---|---|---|---|
| Measures | RSS of tree | **phys_footprint** (`ri_phys_footprint`) | working set |
| Children | tree walked each poll | libproc tree walked each poll | tree walked |
| Enforcement | TERM → grace → KILL | TERM → grace → KILL | `taskkill /T /F` |
| Race window | bounded by `interval` | bounded by `interval` (polling-only; event-driven deferred) | bounded |
| Interactive | full tty | full tty | temp-file tail-follow (~50ms) |

Rust is a faster polling loop with better metrics, not yet event-driven.

## `caproom top` — process-tree inventory

Read-only snapshot of every tree you own, sorted by tree footprint.

```json
{ "schema": 1, "ts": 1755950000, "limit_mb_default": 4096,
  "processes": [
    { "pid": 45057, "cmd": "node /tmp/hog.mjs", "tree_rss_kb": 455136, "footprint_kb": 455136,
      "tree_pids": [45057, 45060], "state": "running", "reason_code": "PARK_IDLE|GROWTH_RATE|PRESSURE|NONE",
      "growth_kb_s": 0, "free_pct": 33, "park_candidate": true,
      "reason": "root sleeping + tree_rss 455136KB >= 524288KB park threshold" } ] }
```

One row per tree root; `park_candidate` is heuristic (`state running` + sleeping + footprint ≥ threshold) with `reason` spelled out.

## park / wake — reclaim idle memory without killing

```
caproom park <pid>        # SIGSTOP — single PID, pages eligible for kernel compressor
caproom park-tree <pid>   # SIGSTOP whole tree (recursive Tree::build) — PID reuse guarded (pid,start)
caproom wake <pid>        # SIGCONT single
caproom wake-tree <pid>   # SIGCONT whole tree
```

Verified on macOS: parked RSS dropped `~90%` (345MB → 37MB) once real pressure hit. No reclaim while system idle/unpressured — rides kernel compressor, doesn't force. **Caveat:** parked process does zero work while stopped — only park something actually idle (watcher, finished subprocess), never the process an agent is waiting on.

Predicate `is_idle_subtree` protects: `cpu_delta <0.02` (`CpuRing` 500ms window, first sample assumes busy `1.0` so active watcher not parked on first sight) and `is_session_leader` (`pid == sid` primary, `pgid == pid` fallback) never parks foreground. Tested with real `python3 -c "while True: pass"` busy vs `sleep` idle — `park_does_not_hang_active_watcher` proves.

## `caproom calibrate` — suggest limit

Prints total RAM, free %, top 3 trees footprint, and `suggested --limit` (60% total, clamp 4G min / 80% max). Migration note `old RSS --limit 6144 ≈ footprint ~4.8G` (≈80% due shared overcount).

## MCP — typed surface

`crates/caproom-mcp` is hand-rolled `serde` JSON with strictly typed enums (`state: parked|running|zombie`, `reason_code: PARK_IDLE|GROWTH_RATE|PRESSURE|NONE`), no `message` string. Tools v1 (8): `top`, `park`, `park_tree`, `wake`, `wake_tree`, `run`, `freemem`, `status`. Unknown tools return proper JSON-RPC error objects (`-32601`). `watch_*` + `rmcp` deferred to v1.1 with tokio isolated to MCP crate.

## Windows

Same `run`/`top`/`park`/`wake` via `cfg(unix)`/`cfg(windows)` guards (`cargo check --target x86_64-pc-windows-msvc` passes). `park`/`wake` are stubs (`park not implemented on Windows`) — `EmptyWorkingSet` not wired in Rust v1. Windows watchdog `taskkill /T /F` for TERM/KILL, no grace, `--grace` ignored. No Job Object hard cap — Rust is watchdog-only.

## Rust port status (current `main` 0.9.0)

- ✅ `phys_footprint` via libproc, typed `top --json` with `growth_kb_s`/`reason_code`/`free_pct`
- ✅ `is_idle_subtree` ancestry walk + `cpu_delta` (`CpuRing` first-sample busy) + `is_session_leader` (`pid==sid` + `pid==pgid` fallback) wired, `S|I` gate dropped, `28` tests incl. `park_does_not_hang_active_watcher`
- ✅ R/S via TASKINFO `pti_numrunning` (`pbi_status` reports SRUN for sleepers); pid-list buffer grows past 8192 instead of silent truncation
- ✅ `growth` contextual `should_enforce_growth` (70% + pressure + <5min breach, 600s for >1MB/s) not bare `>200 KB/s`; `pressure` cached `hw.memsize OnceLock` + `free 800ms/200ms`
- ✅ `park-tree`/`wake-tree` tree-aware + `(pid,start_time)` reuse guard (`pbi_start` / `/proc/starttime` via `getsid`), `is_session_leader` fixes `Ghostty→tmux` conflation
- ✅ MCP: 8 tools, invalid-pid guards (kill(0)/kill(1) rejected), park_tree reuse guard, JSON-RPC error objects, EPIPE-safe writes
- ✅ `effective_limit = limit * (80 + 20*free_pct/15)/100` pressure-aware threshold
- ❌ Not yet: `dispatch_source_memorypressure` event-driven (polling-only v1), `rmcp` port, daemon+arbiter. See `docs/plan-caproom-rust.md` for reality vs pitch.

## Limitations

- Watchdog has bounded `interval` race window; for hard guarantee use cgroups outside caproom (Docker dropped in Rust v1).
- Tree walk follows live `ppid` edges. Daemonized double-fork reparented to 1 escapes by design — prefer missing outside lineage over interfering with user processes.
- Parking only makes pages *eligible*; kernel compresses lazily under real pressure. Parking 2GB on idle machine may stay ~2GB for hours.

## License

MIT
