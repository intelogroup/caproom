# Changelog

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
- `caproom watch` — RSS-threshold daemon on explicit pids: observer mode, gated `--auto-park` (freezes whole breaching trees, one park per breach episode), `--auto-wake-free-pct N` restores its own parks when pressure clears. NDJSON events on stdout.
- `caproom-mcp` — zero-dependency MCP server (stdio) exposing top/park/wake/watch/run as native agent tools. Same gates as the CLI.
- `test/stubborn-hog.js` — regression fixture for TERM-ignoring children.

### Docs
- Backend comparison matrix now covers orphan safety per backend (watchdog snapshot sweep vs cgroup OOM vs Job Object atomic kill).

## 0.4.0 — 2026-08-23

- Windows support via PowerShell backend: Job Object enforcement plus polling watchdog fallback with whole-tree accounting, live output streaming, `--force-watchdog`, and `init` parity.
- Process-tree RSS measurement on all watchdog backends.
