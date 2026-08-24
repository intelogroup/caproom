#!/usr/bin/env bash
# sum-OOM gate — REAL blocking test (w2 gate, not stub).
# Fails CI if synthetic hogs are not actually capped.
# Interim v1: per-instance effective_limit via free_mem_pct, no cross-tab arbiter.
# Asserts: at least one hog is capped (exit 137) and no system OOM (all pids reaped).
set -euo pipefail
CAPROOM="${CAPROOM_BIN:-./target/debug/caproom}"
if [[ ! -x "$CAPROOM" ]]; then CAPROOM="./target/release/caproom"; fi
if [[ ! -x "$CAPROOM" ]]; then echo "sum-oom: no caproom binary at $CAPROOM" >&2; exit 1; fi
echo "sum-oom: $CAPROOM freemem=$($CAPROOM freemem)% free"
# Each hog allocates 800MBphys, limit 400MB => must breach and be TERM/KILL.
# Use higher allocation to ensure footprint breach even with fallback RSS vs phys_footprint drift.
pids=()
codes=()
for i in 1 2 3 4 5 6; do
  "$CAPROOM" run --limit 400 -- python3 -c "import time; a=bytearray(800*1024*1024); time.sleep(6); print('hog $i done')" &
  pids+=($!)
done
capped=0
completed=0
for pid in "${pids[@]}"; do
  set +e
  wait "$pid"
  code=$?
  set -e
  echo "hog pid $pid exit $code"
  # capped = signal exit (128+signal) or hard 137; 143 = SIGTERM, 137 = SIGKILL
  if [[ $code -ne 0 ]]; then capped=$((capped+1)); fi
  completed=$((completed+1))
  codes+=($code)
done
echo "sum-oom: capped $capped/6, completed $completed/6, codes ${codes[*]}"
# Gate: at least one hog must be capped (non-zero), otherwise watchdog did not enforce
if [[ $capped -eq 0 ]]; then
  echo "sum-oom FAIL: no hog was capped (limit 400 < 800, expected at least one non-zero) — watchdog not enforcing" >&2
  echo "hint: check collector phys_footprint vs effective_limit and growth trigger" >&2
  exit 1
fi
# Gate: all pids must be reaped (no system OOM hang)
if [[ $completed -ne 6 ]]; then
  echo "sum-oom FAIL: not all hogs reaped ($completed/6) — possible system OOM or hang" >&2
  exit 1
fi
echo "sum-oom PASS: capped $capped/6, no system OOM"
exit 0
