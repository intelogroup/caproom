# caproom

[![npm](https://img.shields.io/npm/v/caproom.svg)](https://www.npmjs.com/package/caproom)
[![license](https://img.shields.io/npm/l/caproom.svg)](LICENSE)

Memory-cap any command — AI coding agents, builds, background jobs — on macOS/Linux.

## Why

macOS has no reliable way to cap a process's memory from userspace. A runaway process — a leaking build tool, an AI agent stuck in a loop, a stray `find /` — has nothing standing between it and system OOM. `caproom` fills that gap with mechanisms that actually enforce a limit.

## What it does

```
caproom --limit <mb> -- <command> [args...]
```

Two backends, auto-selected:

1. **Docker cgroup** (`--memory`) — used when Docker is installed and running. Hard cap, real kernel enforcement, zero race window.
2. **Polling watchdog** (`ps` RSS + `SIGKILL`) — fallback when Docker isn't available. No dependencies, works anywhere `ps` exists. Has a small race window bounded by `--interval` (default 200ms) — a process can spike briefly past the cap between polls before being killed.

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

No daemon, no tracking file, no dependency — just `SIGSTOP`/`SIGCONT` wrapped with a friendly CLI. Any script or agent can call `caproom park <pid>` / `caproom wake <pid>` directly.

**Caveat**: a parked process does zero work while stopped — no CPU, no I/O, no timers firing. Only park something actually idle (a background watcher, a finished subprocess kept around for reuse) — never park the process an agent is actively waiting on a response from, or you'll hang the agent, not save it memory.

## Limitations

- Docker backend mounts `$PWD` into the container at `/work` and runs there — paths outside `$PWD` aren't visible to the command.
- Watchdog backend has a real (if small) race window; for a hard guarantee, use the Docker backend.
- Neither backend can cap a process that immediately forks and hides children under a different watched PID tree in unusual ways — the watchdog only tracks the direct child.

## Contributing

Issues and PRs welcome at [github.com/intelogroup/caproom](https://github.com/intelogroup/caproom).

## License

MIT
