# aicap

Memory-cap any command — AI coding agents, builds, background jobs — on macOS/Linux.

## Why

macOS has no working per-process memory limit in userspace. Verified empirically:

- `setrlimit(RLIMIT_AS, ...)`, `RLIMIT_DATA`, `RLIMIT_RSS` all return `EINVAL` on modern macOS — the kernel rejects them outright, `ulimit -v` included.
- launchd's own `HardResourceLimits.ResidentSetSize` is a no-op — a job capped at 100MB was observed running past 590MB, still `state = active`.

So a runaway process — a leaking build tool, an AI agent that gets stuck in a loop, a stray `find /` — has nothing standing between it and system OOM on macOS. `aicap` fills that gap with the two mechanisms that actually work.

## What it does

```
aicap --limit <mb> -- <command> [args...]
```

Two backends, auto-selected:

1. **Docker cgroup** (`--memory`) — used when Docker is installed and running. Hard cap, real kernel enforcement, zero race window.
2. **Polling watchdog** (`ps` RSS + `SIGKILL`) — fallback when Docker isn't available. No dependencies, works anywhere `ps` exists. Has a small race window bounded by `--interval` (default 200ms) — a process can spike briefly past the cap between polls before being killed.

Both are honest about their mechanism: neither claims kernel-enforced rlimit, because that doesn't exist on macOS for arbitrary processes.

## Install

```
npm install -g aicap
```

or clone and symlink `bin/aicap` onto your `PATH`.

## Usage

```bash
# cap a build at 2GB
aicap --limit 2048 -- npm run build

# cap an AI coding agent run at 512MB
aicap --limit 512 -- claude -p "refactor this module"

# force the watchdog even if Docker is available
aicap --limit 1024 --force-watchdog -- ./some-script.sh

# use a different docker image for the docker backend (default: node:22-slim)
aicap --limit 4096 --image python:3.12-slim -- python train.py
```

Env var overrides: `AICAP_LIMIT_MB`, `AICAP_IMAGE`, `AICAP_INTERVAL`.

On cap breach, `aicap` kills the process and exits `137` (same convention as Docker's own OOM-kill exit code).

## Limitations

- Docker backend mounts `$PWD` into the container at `/work` and runs there — paths outside `$PWD` aren't visible to the command.
- Watchdog backend has a real (if small) race window; for a hard guarantee, use the Docker backend.
- Neither backend can cap a process that immediately forks and hides children under a different watched PID tree in unusual ways — the watchdog only tracks the direct child.

## License

MIT
