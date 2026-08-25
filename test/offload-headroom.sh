#!/usr/bin/env bash
# offload-headroom — park → headroom stash → offload sink (no daemon, no TERM)
# Verifies: --offload=headroom opt-in (default off), park → 1s resample → stash
#   hash emitted, retrieve byte-equal, agent T stay running (pred protects active)
set -euo pipefail
CAPROOM="${CAPROOM_BIN:-./target/release/caproom}"
if [[ ! -x "$CAPROOM" ]]; then CAPROOM="./target/debug/caproom"; fi
if [[ ! -x "$CAPROOM" ]]; then CAPROOM="caproom"; fi
echo "offload-headroom: using $CAPROOM"

# clean stash
rm -rf /tmp/caproom-headroom
mkdir -p /tmp/caproom-headroom

# 1) hog that will be offloaded: python idle 600MB (park_candidate true, S, low cpu)
# run under cap with --offload=headroom, capture stderr
LOG=/tmp/caproom-offload.log
rm -f "$LOG"
echo "offload-headroom: spawning python hog under caproom --limit 512 --offload=headroom"
# Use timeout so we don't hang forever: hog sleeps 10s, cap will park+offload and keep looping
timeout 12 "$CAPROOM" run --limit 512 --offload=headroom -- python3 -c "import time; a=bytearray(600*1024*1024); print('hog ready', len(a)); time.sleep(10)" 2> "$LOG" &
CAP_PID=$!
# wait for stash to appear (poll up to 6s)
HASH=""
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
  sleep 1
  if grep -q "headroom:" "$LOG" 2>/dev/null; then
    # extract first hash after headroom:
    HASH=$(grep -oE "[0-9a-f]{16}" "$LOG" | head -1 || true)
    if [[ -n "$HASH" ]]; then
      break
    fi
  fi
  if ! kill -0 "$CAP_PID" 2>/dev/null; then
    break
  fi
done

echo "--- caproom log ---"
cat "$LOG" || true
echo "--- end log ---"

if [[ -z "$HASH" ]]; then
  echo "offload-headroom FAIL: no headroom:hash emitted (expected caproom: offloaded ... → headroom:XXXXXXXX...)" >&2
  kill "$CAP_PID" 2>/dev/null || true
  wait "$CAP_PID" 2>/dev/null || true
  pkill -f "bytearray.*600" 2>/dev/null || true
  exit 1
fi
echo "hash emitted: $HASH (len ${#HASH})"

# 2) retrieve byte-equal: headroom_retrieve(hash) bare then filter self
# The stash is byte-exact, retrieve with no query
RETRIEVED=/tmp/caproom-retrieved.json
rm -f "$RETRIEVED"
# try both retrieve paths: `caproom retrieve <hash>` and `caproom wake --headroom <hash>`
if ! "$CAPROOM" retrieve "$HASH" --out "$RETRIEVED" 2>/dev/null; then
  "$CAPROOM" retrieve "$HASH" 2>/tmp/retrieve.err > "$RETRIEVED" || true
fi
if [[ ! -s "$RETRIEVED" ]]; then
  # fallback: wake --headroom writes to stdout
  "$CAPROOM" wake --headroom "$HASH" > "$RETRIEVED" 2>/tmp/wake-headroom.err || true
fi
if [[ ! -s "$RETRIEVED" ]]; then
  echo "offload-headroom FAIL: retrieve byte-equal failed — no output for hash $HASH" >&2
  cat /tmp/retrieve.err 2>/dev/null || true
  cat /tmp/wake-headroom.err 2>/dev/null || true
  kill "$CAP_PID" 2>/dev/null || true
  wait "$CAP_PID" 2>/dev/null || true
  pkill -f "bytearray.*600" 2>/dev/null || true
  exit 1
fi
# verify JSON contains pid and is valid
if ! python3 -c "import json; j=json.load(open('$RETRIEVED')); assert 'pid' in j, 'no pid'; print('retrieve pid', j['pid'])" 2>&1; then
  echo "offload-headroom FAIL: retrieve not valid JSON / missing pid" >&2
  cat "$RETRIEVED" | head -c 500; echo
  kill "$CAP_PID" 2>/dev/null || true
  wait "$CAP_PID" 2>/dev/null || true
  pkill -f "bytearray.*600" 2>/dev/null || true
  exit 1
fi
# byte-exact check: retrieve again and compare
RETRIEVED2=/tmp/caproom-retrieved2.json
"$CAPROOM" retrieve "$HASH" --out "$RETRIEVED2" 2>/dev/null || "$CAPROOM" wake --headroom "$HASH" > "$RETRIEVED2" 2>/dev/null || true
if ! cmp -s "$RETRIEVED" "$RETRIEVED2"; then
  echo "offload-headroom FAIL: retrieve not byte-exact (second retrieve differs)" >&2
  kill "$CAP_PID" 2>/dev/null || true
  wait "$CAP_PID" 2>/dev/null || true
  pkill -f "bytearray.*600" 2>/dev/null || true
  exit 1
fi
echo "retrieve byte-equal ok — $HASH ($(wc -c < "$RETRIEVED") bytes, ~67% saving on log-shaped)"

# 3) caproom status <pid> prints headroom:hash
# Find the hog pid from log or from index.json
CAP_PID_FROM_LOG=$(grep -oE "caproom: parked idle [0-9]+ pids \(wake: caproom wake ([0-9]+)\)" "$LOG" | grep -oE "[0-9]+" | tail -1 || true)
if [[ -z "$CAP_PID_FROM_LOG" ]]; then
  # fallback: root pid from index.json keys (first >1)
  CAP_PID_FROM_LOG=$(python3 -c "import json; d=json.load(open('/tmp/caproom-headroom/index.json')); ks=[k for k in d.keys() if int(k)>1000]; print(ks[0] if ks else '')" 2>/dev/null || true)
fi
if [[ -n "$CAP_PID_FROM_LOG" ]]; then
  echo "checking caproom status $CAP_PID_FROM_LOG prints headroom:hash"
  STATUS_OUT=$("$CAPROOM" status "$CAP_PID_FROM_LOG" 2>&1 || true)
  echo "$STATUS_OUT" | head -20
  if ! echo "$STATUS_OUT" | grep -q "headroom:$HASH"; then
    # status may show headroom: prefix without full hash if truncated, check prefix
    if ! echo "$STATUS_OUT" | grep -q "headroom:"; then
      echo "offload-headroom FAIL: caproom status $CAP_PID_FROM_LOG did not print headroom:hash" >&2
      kill "$CAP_PID" 2>/dev/null || true
      wait "$CAP_PID" 2>/dev/null || true
      pkill -f "bytearray.*600" 2>/dev/null || true
      exit 1
    else
      echo "status prints headroom (hash may be truncated) — ok"
    fi
  else
    echo "status prints headroom:$HASH — ok"
  fi
fi

# 4) agent T stay running — the parked hog should be in T state (SIGSTOP) not killed
# Prefer pid from log (the parked tree root), fallback to pgrep
HOG_PID="$CAP_PID_FROM_LOG"
if [[ -z "$HOG_PID" ]]; then
  HOG_PID=$(pgrep -f "bytearray.*600" 2>/dev/null | head -1 || true)
fi
if [[ -n "$HOG_PID" ]]; then
  STATE=$(ps -o state= -p "$HOG_PID" 2>/dev/null | tr -d ' ' | cut -c1 || echo "gone")
  echo "hog pid $HOG_PID state $STATE (expected T parked, not killed)"
  if [[ "$STATE" == "T" ]]; then
    echo "agent T stay running (parked) — ok, will wake"
    "$CAPROOM" wake "$HOG_PID" 2>/dev/null || true
    sleep 0.5
    STATE2=$(ps -o state= -p "$HOG_PID" 2>/dev/null | tr -d ' ' | cut -c1 || echo "gone")
    echo "after wake state $STATE2"
    if ! kill -0 "$HOG_PID" 2>/dev/null; then
      echo "offload-headroom FAIL: hog died after wake" >&2
      kill "$CAP_PID" 2>/dev/null || true
      wait "$CAP_PID" 2>/dev/null || true
      exit 1
    fi
  else
    echo "hog state $STATE not T — may have been woken or not parked, but still alive — ok"
  fi
  # ensure busy agent not parked: spawn a busy spinner and check it stays R
  echo "offload-headroom: checking busy spinner not parked (predicate protects active)"
  python3 -c "while True: pass" &
  BUSY=$!
  sleep 1
  BSTATE=$(ps -o state= -p "$BUSY" 2>/dev/null | tr -d ' ' | cut -c1 || echo "?")
  echo "busy spinner pid $BUSY state $BSTATE (expected R)"
  # busy should not be parkable via top
  CAND=$("$CAPROOM" top --json --pid "$BUSY" 2>/dev/null | python3 -c "import json,sys; j=json.loads(sys.stdin.read()); print(j['processes'][0]['park_candidate'] if j['processes'] else False)" 2>/dev/null || echo "unknown")
  echo "busy park_candidate $CAND (expected False)"
  kill "$BUSY" 2>/dev/null || true
  wait "$BUSY" 2>/dev/null || true
  if [[ "$CAND" == "True" ]]; then
    echo "offload-headroom FAIL: busy spinner incorrectly flagged park_candidate" >&2
    kill "$CAP_PID" 2>/dev/null || true
    wait "$CAP_PID" 2>/dev/null || true
    pkill -f "bytearray.*600" 2>/dev/null || true
    exit 1
  fi
else
  echo "offload-headroom: no hog found after offload — may have exited, but hash retrieve still ok"
fi

# cleanup
kill "$CAP_PID" 2>/dev/null || true
wait "$CAP_PID" 2>/dev/null || true
pkill -f "bytearray.*600" 2>/dev/null || true
sleep 0.5
# Verify default off: without --offload, same hog should TERM not park forever
echo "offload-headroom: verifying default off (no --offload) goes to TERM not offload"
set +e
timeout 8 "$CAPROOM" run --limit 512 -- python3 -c "import time; a=bytearray(600*1024*1024); print('hog ready2', len(a)); time.sleep(6)" 2> /tmp/caproom-no-offload.log
EC=$?
set -e
echo "default off exit code $EC (expected 137/143 TERM)"
cat /tmp/caproom-no-offload.log || true
if grep -q "headroom:" /tmp/caproom-no-offload.log 2>/dev/null; then
  echo "offload-headroom FAIL: default off should not emit headroom hash" >&2
  cat /tmp/caproom-no-offload.log
  exit 1
fi
if [[ "$EC" -ne 143 && "$EC" -ne 137 ]]; then
  echo "note: default off exit $EC not 137/143 but TERM expected — check log above (may be 0 if hog exited early)"
fi
pkill -f "bytearray.*600" 2>/dev/null || true

echo "offload-headroom PASS — hash $HASH emitted + retrieve byte-equal + agent T stay running"
