# caproom

[![npm](https://img.shields.io/npm/v/caproom.svg)](https://www.npmjs.com/package/caproom)
[![license](https://img.shields.io/npm/l/caproom.svg)](LICENSE)

Prevent RAM OOM for long-running terminal coding agents, builds, and background jobs — memory caps plus idle-process parking, for macOS, Linux, and Windows.

## Why

macOS has no reliable way to cap a process's memory from userspace. A runaway process — a leaking build tool, an AI agent stuck in a loop, a stray `find /` — has nothing standing between it and system OOM. `caproom` fills that gap with mechanisms that actually enforce a limit.

## What it does

```
caproom --limit <mb> -- <command> [args...]
```

Two backends (bash 0.7.6):

1. **Host-native polling watchdog** (`ps` RSS + `SIGKILL`) — the **default** on macOS/Linux. Runs in your real environment: same PATH, auth, native binaries, tty. Measures the **whole process tree** each poll (agents keep their memory in children — MCP servers, bundler daemons, headless browsers — while the parent's own RSS stays flat). Has a small race window bounded by `--interval` (default 200ms).
2. **Docker cgroup** (`--memory`) — opt-in with `--docker`. Hard cap, real kernel enforcement, zero race window — at the cost of running inside a Linux container (see caveats below). Fails loudly if the daemon isn't reachable rather than silently switching backends.

**Rust port (CLI-first v1, current `main`):** `phys_footprint` via `proc_pid_rusage` (libproc, not `ps`; bench 13ms vs 20ms) + `proc_listallpids` tree walk, polling-only 200ms (`vm_stat` free/inactive). `dispatch_source_memorypressure` event-driven (0% idle) is **not built** — current Rust is a faster polling loop with better metrics, not the event-driven daemon pitched. Deferred to daemon v1.1 global listener. Docker backend dropped in Rust (native only).

## Install

```bash
npm install -g caproom
```

or clone and symlink `bin/caproom` onto your `PATH`.

## Usage

```bash
# cap a build at 2GB (host-native watchdog, the default)
caproom --limit 2048 -- npm run build

# cap an AI coding agent run at 512MB
caproom --limit 512 -- claude -p "refactor this module"

# opt in to the Docker cgroup backend for a zero-race hard cap
caproom --limit 4096 --docker --image python:3.12-slim -- python train.py
```

### Flags

| Flag | Default | Meaning |
|---|---|---|
| `--limit <mb>` | `4096` | memory cap in MB |
| `--interval <sec>` | `0.2` | watchdog poll interval |
| `--grace <sec>` | `5` | seconds to wait after `SIGTERM` before `SIGKILL`, watchdog backend only — gives the process a chance to flush/save state before a hard kill |
| `--docker` | off | opt in to the Docker cgroup backend instead of the default host-native watchdog |
| `--image <name>` | `node:22-slim` | docker image used by the `--docker` backend |
| `--force-watchdog` | — | legacy no-op; the watchdog IS the default. Accepted so existing scripts and `init` snippets keep working |
| `--no-intercept-tty` | off | bypass stdio interposition for TUI/pty apps — `exec` directly and monitor via detached `watch --auto-park` (preserves `OSC 10/11`, `DSR CPR`, mouse `DEC 1003`) |
| `--pty` | off | allocate a real pty via `forkpty` (`scripts/pty_wrapper.py` python or `script`) and forward bytes verbatim — full terminal fidelity, watchdog enforces cap on pty tree |
| `--no-pty` | — | disable pty allocation (fallback to bypass/watchdog) |

Env var overrides: `CAPROOM_LIMIT_MB`, `CAPROOM_IMAGE`, `CAPROOM_INTERVAL`, `CAPROOM_GRACE`, `CAPROOM_BYPASS_TTY=1` (force bypass), `CAPROOM_PTY=1` (force pty, same as `--pty`).

On cap breach, the watchdog backend sends `SIGTERM` first and waits `--grace` seconds before `SIGKILL`. If the process exits cleanly during the grace window, `caproom` propagates its real exit code; only a hard `SIGKILL` (process ignored `SIGTERM`, or grace ran out) reports `137` (same convention as Docker's own OOM-kill exit code, which the docker backend always uses on breach since Docker itself sends the kill).

## Backends compared

The three enforcement mechanisms measure different quantities and cover children differently. Read this before reusing a `--limit` number across platforms or backends. The watchdog is the POSIX default; Docker is opt-in — caproom prefers to run your command unmodified in its real environment and *miss* an exotic memory spike over breaking a working workflow with container drift:

| | Docker cgroup (bash, dropped in Rust) | Windows Job Object | Watchdog (POSIX) bash | Watchdog (POSIX) Rust v1 | Watchdog (Windows) |
|---|---|---|---|---|---|
| Measures | cgroup memory | **committed virtual memory** | RSS of the process tree | **phys_footprint** (`ri_phys_footprint`) | working set of the process tree |
| Children counted | yes — whole container | yes — auto-inherited at spawn | yes — tree walked each poll | yes — libproc tree walked each poll | yes — tree walked each poll |
| Enforcement | kernel OOM-kill | allocation fails in-process | TERM → grace → KILL | TERM → grace → KILL (143 TERM, 137 KILL) | hard kill (`taskkill /T /F`) |
| Race window | none | none | bounded by `--interval` | bounded by `--interval` (polling-only v1; event-driven deferred) | bounded by `--interval` |
| Interactive/streaming output | degraded — no TTY (`-i` only) | full — streamed live | full — child inherits the tty | full — child inherits the tty | streamed live via temp-file tail-follow (~50ms cadence) |

**Committed vs RSS**: Node/V8 runtimes commit far more virtual memory than they touch, so a limit tuned against RSS on macOS will bite much earlier under the Job Object backend. Tune per platform.

### Docker backend caveats

Opt in with `--docker`. The command then runs inside `node:22-slim` with `$PWD` mounted at `/work` — a Linux container, not your host shell:

- Native modules built for macOS (`esbuild`, `swc`, `sharp`) fail with exec-format errors inside the container.
- Host toolchain, env vars, git credentials, and `~/.ssh` are not present.
- The image pins Node 22 regardless of your project's version (`--image` to override).
- No TTY is allocated, so interactive/TUI programs degrade; Docker Desktop's file-share layer slows large builds on macOS.

For capping an AI agent session you want to *interact* with, use the default watchdog: same host environment, streaming output, no container drift. Reach for `--docker` when you need the zero-race kernel guarantee and the command is container-safe.

Orphan safety differs too. The watchdog TERMs the whole measured tree and SIGKILLs grace-period survivors from a breach-time snapshot — but a process that detaches before being observed escapes. Inside the Docker backend, the kernel's cgroup OOM handling acts on every task in the container: nothing outlives it, though the OOM killer picks victims by badness (it may kill your hog rather than the whole container — either way the capped workload ends and `caproom` exits non-zero). The Windows Job Object kills the whole job atomically on breach.


## init — auto-cap a command on every launch

For a command you always want capped (e.g. an AI coding agent), don't type the wrapper every time — bake it into your shell so a new terminal tab is capped automatically:

```bash
caproom init claude --limit 6144 --grace 10 >> ~/.zshrc && source ~/.zshrc
```

This appends a shell function that wraps `claude` through the watchdog backend (host-native — no Docker isolation, so the wrapped command keeps its normal filesystem/auth/PATH access) and an alias so plain `claude` picks it up. Per-shell override without editing the rc file: `CAPROOM_LIMIT_MB=8192 claude ...`. Works for any command, not just `claude` — `caproom init npm --limit 2048` wraps `npm` the same way.

**TUI / pty note:** `caproom -- <TUI>` (opencode, claude, vim, htop, …) would break the pty — `OSC 10/11` (`^[]10;rgb:...`), `DSR CPR` (`^[[5;1R`), and mouse `^[[<35;` reports leak as text. Since 0.7.3, `caproom` detects `[ -t 0 ] && [ -t 1 ]` for known TUIs and **bypasses stdio interposition**: the TUI `exec`s directly and a detached `caproom watch --auto-park` monitors its pid tree (same cap, no pty corruption). For full pty fidelity, use `--pty` / `CAPROOM_PTY=1` which allocates a real pty via `forkpty` (`python3` `scripts/pty_wrapper.py` or `script`) and forwards bytes verbatim while the watchdog still enforces the cap on the pty tree. Piped/batch `caproom -- opencode run "task"` stays fully capped via the watchdog. Override: `CAPROOM_BYPASS_TTY=1` / `--no-intercept-tty` forces bypass, `CAPROOM_PTY=1` / `--pty` forces pty for any command. `caproom init <TUI>` prints a warning and emits the tty-aware wrapper.

## setup / bind — shell integration for every terminal

`caproom setup` binds headroom management to your shells at the **shell layer**, so it works in any terminal (Terminal.app, iTerm2, Ghostty, Windows Terminal) without touching terminal-specific config:

```bash
caproom setup                # bind zsh/bash + fish (Windows: PowerShell $PROFILE)
caproom setup --uninstall    # remove rc markers; backups and integration files stay
```

What it does:

- Writes one integration file per shell under `~/.caproom/` (`shell.sh`, `shell.fish`, `shell.ps1`) and adds a 3-line marker block (`# >>> caproom >>>`) to your rc files. Idempotent; timestamped `.bak` backups are made before first patch.
- Adds a **headroom warning on every prompt** — once per minute, only when free memory drops below `CAPROOM_HEADROOM_WARN` percent (default 20). Never blocks, never modifies your prompt text.
- Optional **auto-wrap**: `export CAPROOM_AUTO_WRAP="claude,codex,opencode"` in your rc creates `<cmd>_capped` twins that run through the watchdog with `CAPROOM_LIMIT_MB`. Bare command names are shadowed **only** if you also set `CAPROOM_AUTO_ALIAS=1` — caproom never hijacks an unwrapped command by default.
- Optional login daemon: `caproom setup --guard --threshold 15` installs a LaunchAgent (macOS) or systemd user unit (Linux) running `caproom guard` across all terminals.

npm install never touches your rc files — postinstall prints a hint, binding is always explicit. Undo any time: `caproom setup --uninstall`.

### `caproom top` — process-tree inventory for agents

Read-only snapshot of every process tree you own, sorted by tree RSS. `--json` output is a **stable contract**: `schema` version field, additive changes only.

```bash
caproom top                       # human table
caproom top --json                # machine output
caproom top --json --pid 45057    # one subtree only
caproom top --json --park-min-mb 1024   # park-candidate threshold (default 512MB)
```

```json
{ "schema": 1, "ts": 1755950000, "limit_mb_default": 4096,
  "processes": [
    { "pid": 45057,
      "cmd": "node /tmp/hog.mjs",
      "tree_rss_kb": 455136,
      "tree_pids": [45057, 45060],
      "state": "running" | "parked" | "zombie",
      "park_candidate": true,
      "reason": "root sleeping + tree_rss 455136KB >= 524288KB park threshold" } ] }
```

One row per **tree root**; members are listed in `tree_pids`. `park_candidate` is a heuristic (`state == running`, root sleeping/idle, tree RSS ≥ threshold) with the rule spelled out in `reason` so the agent never re-derives it — override freely using the raw fields. The intended loop: poll `top --json` → decide → `park <pid>` / `wake <pid>`. Note `park` makes pages *eligible* for reclaim; see the caveat under park/wake below before treating it as freed RAM.

## park / wake — reclaim idle memory without killing

Long-running agent sessions accumulate subprocesses that go idle but stay resident — old file watchers, finished tool-call children, stale servers. Killing them loses state; leaving them wastes RAM. `caproom park` freezes instead:

```bash
caproom park <pid>     # SIGSTOP — process stays alive, keeps its PID and state,
                        # just isn't scheduled. Its memory becomes eligible for
                        # the kernel's own compressor once real system memory
                        # pressure shows up.
caproom wake <pid>      # SIGCONT — resumes instantly, same state, no restart.
caproom status <pid>    # pid, state (T = parked, S = running), RSS, elapsed
```

Verified empirically on macOS: a parked process's RSS dropped ~90% (345MB → 37MB) once real memory pressure hit, and resumed correctly and stayed responsive after `SIGCONT`. No reclaim happens while the system is idle/unpressured — this rides the kernel's own compressor, it doesn't force anything.

No daemon, no tracking file, no dependency — just `SIGSTOP`/`SIGCONT` wrapped in a CLI. Any script or agent can call `caproom park <pid>` / `caproom wake <pid>` directly.

**Caveat**: a parked process does zero work while stopped — no CPU, no I/O, no timers firing. Only park something actually idle (a background watcher, a finished subprocess kept around for reuse) — never park the process an agent is actively waiting on a response from, or you'll hang the agent, not save it memory. Also: SIGSTOP only makes pages *eligible* for reclaim — the kernel compresses/evicts them lazily under real memory pressure. Park an idle 2GB agent on a quiet machine and it may stay ~2GB resident for hours. Park is insurance against OOM, not immediate RAM return.

### `caproom top` / `caproom watch` — agent interface

`caproom top --json` (above) is read-only discovery with a stable schema. `caproom watch` turns it into a daemon:

```bash
# observer: report tree-RSS breaches, touch nothing
caproom watch --threshold-mb 2000 --json <pid>

# arm auto-park: freeze breaching trees (SIGSTOP every pid in the snapshot)
caproom watch --threshold-mb 2000 --auto-park --json <pid>

# also restore automatically when system free memory recovers
caproom watch --threshold-mb 2000 --auto-park --auto-wake-free-pct 15 <pid>
```

Naming the pid IS the per-process opt-in — there is no system-wide mode, since stopping an unchosen process risks freezing it mid-write. Events are NDJSON on stdout (`started`, `breach`/`parked`, `recovered`, `woke`, `all-exited`). Auto-park freezes the whole measured tree, tracks exactly what *it* stopped, never re-parks within one breach episode (woken trees stay awake unless RSS drops back under threshold), and `--auto-wake-free-pct` undoes only watch's own parks.

Typical loop: `top --json` finds candidates → `watch --auto-park` babysits them during heavy builds → explicit or automatic wake restores them after.

## MCP server — native agent access (pull-only, typed)

`npm i -g caproom` also installs `caproom-mcp` (bash: zero-dependency stdio). Rust `crates/caproom-mcp` is hand-rolled `serde` JSON with strictly typed enums (`state: parked|running|zombie`, `reason_code: PARK_IDLE|GROWTH_RATE|PRESSURE|NONE`, `growth_kb_s`, `free_pct`) — no `message` string (injection surface closed). `rmcp` 3.1.4 **not integrated** (zero hits in `Cargo.toml`), `watch_start`/`watch_events`/`watch_stop` and `rmcp` port deferred to v1.1 with tokio isolated to MCP crate. `bin/caproom-mcp.js` shim stays for v1. Pull-only (see `skills/caproom-memory/SKILL.md`): agent calls `top` every 3 tool calls/30s, human ack before kill.

```json
{ "mcpServers": { "caproom": { "command": "caproom-mcp" } } }
```

Tools (v1): `top` (typed, stable schema), `park`/`wake`. `watch_*` + `run` deferred. Same gating: watch requires explicit pids; auto-park opt-in.

## What it never touches

caproom only watches OS-level RSS and sends signals (`SIGTERM`/`SIGKILL`/`SIGSTOP`/`SIGCONT`). The watchdog backend runs the wrapped command as a direct child with stdin/stdout/stderr passed straight through — no pipe, no buffering, no interception. The Docker backend passes stdio through the same way (`docker run -i`). caproom never reads, modifies, or truncates anything the wrapped process reads or writes — including an AI agent's own conversation/context stream. It manages RAM headroom only, nothing else.

## Windows

Windows uses a separate PowerShell backend, selected automatically. Same commands, but the semantics differ in three ways worth knowing before you reuse a `--limit` number across platforms.

**The cap is a Job Object** (`JOB_OBJECT_LIMIT_PROCESS_MEMORY`), enforced by the kernel at allocation time. Two things it does better than the POSIX watchdog: there is no poll-interval race window, and child processes are covered automatically — a process associated with a job passes that association to anything it spawns, so the whole tree is capped, not just the direct child.

**`--limit` means committed memory on Windows, RSS on macOS/Linux.** These are different quantities. The same number will bite at a different point, so tune it per platform rather than assuming it transfers.

**No grace period.** Windows console apps have no `SIGTERM` equivalent. Under the Job Object backend nothing is killed at all — the allocation just fails inside the process. Under the watchdog fallback, a breach kills the whole tree (`taskkill /T /F`) with no chance to flush state. `--grace` is accepted and ignored.

**Watchdog output streams, but through temp files.** stdout/stderr are captured to files and tail-followed (~50ms cadence) so logs and CI steps show progress live. Full-screen TUI redraws are not pixel-perfect over this path; plain streaming output (agents in non-interactive mode, builds) works normally.

**`park` does not suspend on Windows.** It calls `EmptyWorkingSet`, which trims the process's working set to the pagefile immediately and on demand — no waiting for system memory pressure, and **the process keeps running**. The macOS caveat about never parking a process an agent is waiting on does not apply here. `caproom wake` is therefore a no-op on Windows; trimmed pages fault back in on next access.

`init` emits a PowerShell function plus `Set-Alias` for your `$PROFILE`:

```powershell
caproom init claude --limit 6144 >> $PROFILE
```

Docker backend is not wired up on Windows — the Job Object path already gives kernel enforcement, so there is nothing for it to add.

## Rust port status (current `main`)

- `caproom` Rust binary: `cargo build --release` 1.2M, `phys_footprint` via libproc 13ms vs `ps` 20ms per `top --json` (`scripts/bench_phys_vs_ps.sh`), typed `top --json` with `growth_kb_s`/`reason_code`/`free_pct`, `caproom calibrate` suggests 14G on 24GB (60% total), `test/sum-oom.sh` gate now real (6×800M limit 400 → 6/6 `143` capped, was stub `exit 0` false green).
- Not yet: `dispatch_source_memorypressure` event-driven (polling-only v1), `rmcp` port, daemon+arbiter. See `docs/plan-caproom-rust.md:1` for reality vs pitch.

## Limitations

- Docker backend mounts `$PWD` into the container at `/work` and runs there — paths outside `$PWD` aren't visible to the command. **Dropped in Rust v1** (native `phys_footprint` only).
- Watchdog backends have a real (if small) race window; for a hard guarantee, opt into the Docker backend (`--docker`, bash only) on POSIX, or use the Job Object backend on Windows.
- The watchdog's tree walk follows live parent→child edges. A child that *daemonizes* (double-fork, reparented to init/launchd) leaves the tree and escapes the cap — as does any process spawned after its parent chain broke, or during the kill grace window. On breach the watchdog signals every pid in the measured tree and SIGKILLs survivors of the grace period from a breach-time snapshot, so children cannot outlive the root — but processes that detach *before* being observed are missed by design. The Windows Job Object backend does not have this gap. This is a deliberate trade: caproom prefers to **miss** memory outside the tracked lineage rather than risk interfering with processes the user didn't ask it to manage.
- On Windows, `Get-CimInstance` per poll makes the watchdog heavier than a plain RSS read; keep `--interval` at 0.2s or above there.
- On Windows, the Job Object holds only the wrapped command and its descendants — never caproom itself — so the full `--limit` reaches your workload. (Cost: a millisecond-scale window after spawn before assignment lands, where the child is not yet counted.)

## Contributing

Issues and PRs welcome at [github.com/intelogroup/caproom](https://github.com/intelogroup/caproom).

## License

MIT
