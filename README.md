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

Two backends, auto-selected:

1. **Docker cgroup** (`--memory`) — used when Docker is installed and running. Hard cap, real kernel enforcement, zero race window.
2. **Polling watchdog** (`ps` RSS + `SIGKILL`) — fallback when Docker isn't available. No dependencies, works anywhere `ps` exists. Measures the **whole process tree** each poll (agents keep their memory in children — MCP servers, bundler daemons, headless browsers — while the parent's own RSS stays flat). Has a small race window bounded by `--interval` (default 200ms) — memory can spike briefly past the cap between polls before the kill lands.

## Install

```bash
npm install -g caproom
```

or clone and symlink `bin/caproom` onto your `PATH`.

## Usage

```bash
# cap a build at 2GB
caproom --limit 2048 -- npm run build

# cap an AI coding agent run at 512MB
caproom --limit 512 -- claude -p "refactor this module"

# force the watchdog even if Docker is available
caproom --limit 1024 --force-watchdog -- ./some-script.sh

# use a different docker image for the docker backend (default: node:22-slim)
caproom --limit 4096 --image python:3.12-slim -- python train.py
```

### Flags

| Flag | Default | Meaning |
|---|---|---|
| `--limit <mb>` | `4096` | memory cap in MB |
| `--image <name>` | `node:22-slim` | docker image used by the docker backend |
| `--interval <sec>` | `0.2` | watchdog poll interval |
| `--grace <sec>` | `5` | seconds to wait after `SIGTERM` before `SIGKILL`, watchdog backend only — gives the process a chance to flush/save state before a hard kill |
| `--force-watchdog` | off | use the polling watchdog even if Docker is available |

Env var overrides: `CAPROOM_LIMIT_MB`, `CAPROOM_IMAGE`, `CAPROOM_INTERVAL`, `CAPROOM_GRACE`.

On cap breach, the watchdog backend sends `SIGTERM` first and waits `--grace` seconds before `SIGKILL`. If the process exits cleanly during the grace window, `caproom` propagates its real exit code; only a hard `SIGKILL` (process ignored `SIGTERM`, or grace ran out) reports `137` (same convention as Docker's own OOM-kill exit code, which the docker backend always uses on breach since Docker itself sends the kill).

## Backends compared

The three enforcement mechanisms measure different quantities and cover children differently. Read this before reusing a `--limit` number across platforms or backends:

| | Docker cgroup | Windows Job Object | Watchdog (POSIX) | Watchdog (Windows) |
|---|---|---|---|---|
| Measures | cgroup memory | **committed virtual memory** | RSS of the process tree | working set of the process tree |
| Children counted | yes — whole container | yes — auto-inherited at spawn | yes — tree walked each poll | yes — tree walked each poll |
| Enforcement | kernel OOM-kill | allocation fails in-process | TERM → grace → KILL | hard kill (`taskkill /T /F`) |
| Race window | none | none | bounded by `--interval` | bounded by `--interval` |
| Interactive/streaming output | degraded — no TTY (`-i` only) | full — streamed live | full — child inherits the tty | streamed live via temp-file tail-follow (~50ms cadence) |

**Committed vs RSS**: Node/V8 runtimes commit far more virtual memory than they touch, so a limit tuned against RSS on macOS will bite much earlier under the Job Object backend. Tune per platform.

### Docker backend caveats

The command runs inside `node:22-slim` with `$PWD` mounted at `/work` — it is a Linux container, not your host shell:

- Native modules built for macOS (`esbuild`, `swc`, `sharp`) fail with exec-format errors inside the container.
- Host toolchain, env vars, git credentials, and `~/.ssh` are not present.
- The image pins Node 22 regardless of your project's version (`--image` to override).
- No TTY is allocated, so interactive/TUI programs degrade; Docker Desktop's file-share layer slows large builds on macOS.

For capping an AI agent session you want to *interact* with, prefer the watchdog (`--force-watchdog`): same host environment, streaming output, no container drift.


## init — auto-cap a command on every launch

For a command you always want capped (e.g. an AI coding agent), don't type the wrapper every time — bake it into your shell so a new terminal tab is capped automatically:

```bash
caproom init claude --limit 6144 --grace 10 >> ~/.zshrc && source ~/.zshrc
```

This appends a shell function that wraps `claude` through the watchdog backend (host-native — no Docker isolation, so the wrapped command keeps its normal filesystem/auth/PATH access) and an alias so plain `claude` picks it up. Per-shell override without editing the rc file: `CAPROOM_LIMIT_MB=8192 claude ...`. Works for any command, not just `claude` — `caproom init npm --limit 2048` wraps `npm` the same way.

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

**Caveat**: a parked process does zero work while stopped — no CPU, no I/O, no timers firing. Only park something actually idle (a background watcher, a finished subprocess kept around for reuse) — never park the process an agent is actively waiting on a response from, or you'll hang the agent, not save it memory.

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

## Limitations

- Docker backend mounts `$PWD` into the container at `/work` and runs there — paths outside `$PWD` aren't visible to the command.
- Watchdog backends have a real (if small) race window; for a hard guarantee, use the Docker backend (POSIX) or the Job Object backend (Windows).
- The watchdog's tree walk follows live parent→child edges. A child that *daemonizes* (double-fork, reparented to init/launchd) leaves the tree and escapes the cap — as does any process spawned after its parent chain broke. The Windows Job Object backend does not have this gap.
- On Windows, `Get-CimInstance` per poll makes the watchdog heavier than a plain RSS read; keep `--interval` at 0.2s or above there.

## Contributing

Issues and PRs welcome at [github.com/intelogroup/caproom](https://github.com/intelogroup/caproom).

## License

MIT
