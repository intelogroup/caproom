# Changelog

## 0.7.0 — 2026-08-23

### Added
- **Windows `top --json` and `watch` parity** for the PowerShell backend — the full agent interface (`caproom-mcp`) now works on win32. Same schema-1 envelope and NDJSON event contract as POSIX; whole-tree auto-park via ntdll `NtSuspendProcess`, auto-wake via `NtResumeProcess`. Honest divergence: win32 exposes no cheap sleep-state, so `state` is always `"running"` and the park reason says no sleep check was made.
- Windows CI now parse-checks the PowerShell backend (fails fast with line numbers) and exercises `top --json` plus a live watcher's started event.

## 0.6.0 — 2026-08-23

### Added
- `caproom watch` — RSS-threshold daemon on explicit pids: observer mode reports breaches; gated `--auto-park` freezes whole breaching trees (SIGSTOP every pid in the snapshot, one park per breach episode); `--auto-wake-free-pct N` restores its own parks when system free memory recovers. NDJSON events on stdout (`schema:1`).
- `caproom-mcp` — zero-dependency MCP server (stdio) exposing `top`, `park`, `wake`, `watch_start`/`watch_events`/`watch_stop`, and `run` as native agent tools. Same gates as the CLI: explicit pids only, auto-park opt-in per watcher.
- `caproom setup` / `bind` — terminal-agnostic shell integration: writes `~/.caproom/shell.{sh,fish}` (and `shell.ps1` on Windows, patching `$PROFILE`) and marker-patches your rc files (timestamped backups, idempotent, reversible). Headroom warning on every prompt via precmd/PROMPT_COMMAND/fish_prompt/prompt hooks — throttled, threshold via `CAPROOM_HEADROOM_WARN`. Opt-in auto-wrap: `CAPROOM_AUTO_WRAP=claude,codex` creates `<cmd>_capped` twins under `$CAPROOM_LIMIT_MB`; bare-name shadowing requires explicit `CAPROOM_AUTO_ALIAS=1`.
- `caproom setup --guard --threshold N` — installs a login daemon (LaunchAgent plist on macOS, systemd user unit on Linux) running `caproom guard` across all terminals.
- `caproom freemem` — prints free-memory percentage (backs the shell hooks).
- POSIX CI workflow (ubuntu + macOS matrix): syntax checks, single/tree/stubborn kill regressions with zero-orphan assertions, `top --json` schema validation, watch observer safety (must not park un-armed pids), MCP handshake and an end-to-end capped run through the MCP server.

### Fixed
- MCP `run` verdict now recognizes SIGTERM-honoring tree kills as CAPPED (exit 143 with a breach line), not a plain termination.

## 0.5.0 — 2026-08-23

### Breaking
- **POSIX default backend is now the host-native watchdog.** Docker no longer auto-selects when the daemon is up — it runs only with `--docker`, and fails loudly if the daemon is unreachable instead of silently switching backends. Rationale: container drift (native-module exec failures, missing toolchain, no TTY) could break working workflows outright; caproom prefers running your command unmodified and missing a hard-cap guarantee over impeding it. `--force-watchdog` stays as an accepted no-op.
- Windows Job Object now holds only the wrapped command, not caproom's own PowerShell runtime — the full `--limit` reaches your workload (previously ~100MB+ of PS commit ate the budget).

### Fixed
- **Watchdog kills the whole process tree on POSIX** (was root-only). Children that survived a root-only SIGTERM got orphaned and kept allocating past the cap after exit — verified leaking ~800MB across three orphan generations in testing. Escalation scans the breach-time pid snapshot, so a stubborn child outliving its root still gets SIGKILLed (exit 137).
- Kill message reports tree overshoot percentage (`exceeded 204800KB cap (+142%)`).
- `park` help/stderr no longer oversell: SIGSTOP makes pages *eligible* for reclaim; the kernel acts lazily under real pressure.

### Added
- `caproom top` / `caproom top --json` — read-only process-tree inventory for agents. One row per tree root with `tree_rss_kb`, `tree_pids` (blast radius), `state`, and `park_candidate` + `reason`. JSON contract pinned via `"schema":1`, additive changes only.
- `test/stubborn-hog.js` — regression fixture for TERM-ignoring children.

### Docs
- Backend comparison matrix now covers orphan safety per backend (watchdog snapshot sweep vs cgroup OOM vs Job Object atomic kill).

## 0.4.0 — 2026-08-23

- Windows support via PowerShell backend: Job Object enforcement plus polling watchdog fallback with whole-tree accounting, live output streaming, `--force-watchdog`, and `init` parity.
- Process-tree RSS measurement on all watchdog backends.
