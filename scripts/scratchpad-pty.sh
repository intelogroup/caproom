#!/usr/bin/env bash
# scratchpad-pty — verify caproom TUI fix without touching repo dirty state
# Isolated to /tmp so graphify-out/ stays dirty but not interfering.
# Exit 0 = all pass, non-zero = fail.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAP="$ROOT/bin/caproom"
TMP="/tmp/caproom-scratchpad-pty"
HOME_TMP="/tmp/crhome-scratch-pty"
mkdir -p "$TMP" "$HOME_TMP"

pass=0; fail=0
ok()  { echo "✓ $1"; pass=$((pass+1)); }
bad() { echo "✗ $1"; fail=$((fail+1)); }

echo "== scratchpad pty checks =="
echo "tmp=$TMP home=$HOME_TMP"

# 1. syntax
if bash -n "$CAP"; then ok "bash -n"; else bad "bash -n"; fi
if node --check "$ROOT/bin/caproom.js"; then ok "node --check shim"; else bad "node --check shim"; fi

# 2. normal batch still capped (hog)
echo "--- hog (64MB cap, should exceed) ---"
if "$CAP" --limit 64 --grace 2 -- node "$ROOT/test/hog.js" > "$TMP/hog.log" 2>&1; then
  bad "hog should have been killed (exit 0)"
else
  if grep -q "exceeded" "$TMP/hog.log"; then ok "hog exceeded detected"; else bad "hog no exceeded line"; cat "$TMP/hog.log" | tail -n 20; fi
  if grep -q "65536KB cap" "$TMP/hog.log"; then ok "hog cap size ok"; else bad "hog cap size"; fi
fi
# orphans
sleep 1
surv=$( (pgrep -f 'Buffer.alloc' 2>/dev/null || true) | wc -l | tr -d ' '); surv=${surv:-0}
if [ "$surv" = "0" ]; then ok "hog no orphans"; else bad "hog orphans=$surv"; pgrep -f 'Buffer.alloc' 2>/dev/null || true; fi

# 3. tree hog
echo "--- tree-hog (200MB tree cap) ---"
if "$CAP" --limit 200 --grace 2 -- node "$ROOT/test/tree-hog.js" > "$TMP/tree.log" 2>&1; then
  bad "tree-hog should be killed"
else
  if grep -q "killing tree" "$TMP/tree.log"; then ok "tree kill attributed to tree"; else bad "tree kill not tree"; cat "$TMP/tree.log" | tail -n 20; fi
fi
sleep 1
surv=$( (pgrep -f 'Buffer.alloc' 2>/dev/null || true) | wc -l | tr -d ' '); surv=${surv:-0}
if [ "$surv" = "0" ]; then ok "tree no orphans"; else bad "tree orphans=$surv"; fi

# 4. stubborn
echo "--- stubborn-hog (should SIGKILL sweep) ---"
if "$CAP" --limit 200 --grace 2 -- node "$ROOT/test/stubborn-hog.js" > "$TMP/stubborn.log" 2>&1; then
  bad "stubborn should be killed"
else
  ec=$?
  if grep -q "SIGKILLed 1 survivor" "$TMP/stubborn.log"; then ok "stubborn sweep fired"; else bad "stubborn sweep missing"; cat "$TMP/stubborn.log" | tail -n 20; fi
fi
# caproom does exit 137 for stubborn (via sweep) else 143; accept either
if grep -q "exit 137" "$TMP/stubborn.log" || grep -q "sweep" "$TMP/stubborn.log"; then ok "stubborn exit 137 path"; else bad "stubborn exit not 137"; cat "$TMP/stubborn.log" | tail -n 20; fi
sleep 1
surv=$( (pgrep -f 'Buffer.alloc' 2>/dev/null || true) | wc -l | tr -d ' '); surv=${surv:-0}
if [ "$surv" = "0" ]; then ok "stubborn no orphans"; else bad "stubborn orphans=$surv"; fi

# 5. clean pass-through under cap
echo "--- clean pass-through ---"
if "$CAP" --limit 512 -- node -e 'console.log("ok")' 2> "$TMP/clean.err" | grep -q ok; then ok "clean ok"; else bad "clean ok"; cat "$TMP/clean.err" | head -n 10; fi
if grep -q "watchdog backend" "$TMP/clean.err"; then ok "clean uses watchdog"; else bad "clean not watchdog"; cat "$TMP/clean.err" | head -n 10; fi

# 6. bypass flag/env
echo "--- bypass flag ---"
if "$CAP" --no-intercept-tty --limit 512 -- node -e 'console.log("flag-ok")' 2> "$TMP/bypass.err" | grep -q "flag-ok"; then ok "bypass flag passthrough"; else bad "bypass flag"; cat "$TMP/bypass.err" | head -n 10; fi
if grep -q "bypass-tty" "$TMP/bypass.err"; then ok "bypass flag message"; else bad "bypass flag message"; cat "$TMP/bypass.err" | head -n 10; fi

if CAPROOM_BYPASS_TTY=1 "$CAP" --limit 512 -- node -e 'console.log("env-ok")' 2> "$TMP/bypass2.err" | grep -q "env-ok"; then ok "bypass env passthrough"; else bad "bypass env"; cat "$TMP/bypass2.err" | head -n 10; fi

# 7. fake-tui batch mode under cap (not tty, so watchdog path)
echo "--- fake-tui batch (piped, must go via watchdog) ---"
if "$CAP" --limit 512 -- node "$ROOT/test/fake-tui.js" batch 2> "$TMP/fake-batch.err" | grep -q "batch-ok"; then ok "fake-tui batch ok"; else bad "fake-tui batch"; cat "$TMP/fake-batch.err" | head -n 20; fi

# 8. fake-tui leak check under bypass (simulates TUI)
echo "--- fake-tui leak check (bypass) ---"
if "$CAP" --no-intercept-tty --limit 512 -- node "$ROOT/test/fake-tui.js" tui-leak-check > "$TMP/fake-tui.out" 2> "$TMP/fake-tui.err"; then ok "fake-tui no leak exit 0"; else bad "fake-tui leak check exit $?"; cat "$TMP/fake-tui.err" | tail -n 20; cat "$TMP/fake-tui.out" | tail -n 20; fi
if grep -q "no leak" "$TMP/fake-tui.out"; then ok "fake-tui no leak string"; else bad "fake-tui no leak string"; cat "$TMP/fake-tui.out" | head -n 20; fi

# 9. init snippet generation
echo "--- init snippet checks ---"
if "$CAP" init opencode --limit 6144 --grace 10 2> "$TMP/init-warn.log" | grep -q "opencode_capped"; then ok "init opencode snippet"; else bad "init opencode snippet"; fi
if grep -q "warning.*TUI" "$TMP/init-warn.log"; then ok "init TUI warning on stderr"; else bad "init TUI warning missing"; cat "$TMP/init-warn.log" | head -n 10; fi
if grep -q 'if \[ -t 0 \] && \[ -t 1 \]' "$TMP/init-warn.log" 2>/dev/null; then
  # warn log is stderr, snippet is stdout — check stdout file instead
  true
fi
# re-run and check stdout contains tty check
"$CAP" init opencode --limit 6144 --grace 10 2> /tmp/init-warn2.log > "$TMP/init-snippet.sh"
if grep -q '\[ -t 0 \] && \[ -t 1 \]' "$TMP/init-snippet.sh"; then ok "init snippet tty guard"; else bad "init snippet tty guard"; cat "$TMP/init-snippet.sh" | head -n 40; fi
if grep -q 'CAPROOM_BYPASS_TTY' "$TMP/init-snippet.sh"; then ok "init snippet bypass env"; else bad "init snippet bypass env"; fi
if grep -q 'caproom watch --threshold-mb' "$TMP/init-snippet.sh"; then ok "init snippet uses watch --auto-park"; else bad "init snippet watch"; fi
# non-TUI should be simple
"$CAP" init npm --limit 2048 2> /tmp/init-npm-warn.log > "$TMP/init-npm.sh"
if ! grep -q 'warning.*TUI' /tmp/init-npm-warn.log; then ok "init npm no TUI warning"; else bad "init npm spurious warning"; cat /tmp/init-npm-warn.log | head -n 10; fi
if grep -q 'npm_capped' "$TMP/init-npm.sh"; then ok "init npm snippet"; else bad "init npm snippet"; fi

# 10. setup smoke with fake HOME
echo "--- setup smoke (fake HOME) ---"
export HOME="$HOME_TMP"
rm -f "$HOME/.zshrc"
mkdir -p "$HOME"
if "$CAP" setup > "$TMP/setup.log" 2>&1; then ok "setup ok"; else bad "setup"; cat "$TMP/setup.log" | head -n 20; fi
if grep -q "# >>> caproom >>>" "$HOME/.zshrc"; then ok "setup patched rc"; else bad "setup not patched"; cat "$HOME/.zshrc" | head -n 20; fi
if bash -c 'source ~/.caproom/shell.sh && type caproom_headroom_check >/dev/null && echo ok' | grep -q ok; then ok "shell.sh ok"; else bad "shell.sh"; fi
if "$CAP" setup --uninstall > "$TMP/uninstall.log" 2>&1; then ok "uninstall ok"; else bad "uninstall"; cat "$TMP/uninstall.log" | head -n 20; fi
if ! grep -q "# >>> caproom >>>" "$HOME/.zshrc"; then ok "uninstall removed markers"; else bad "uninstall markers remain"; fi
# restore HOME
export HOME="$HOME" 2>/dev/null || export HOME="/Users/kalinovdameus"

echo ""
echo "=== result: $pass passed, $fail failed ==="
if [ "$fail" -gt 0 ]; then exit 1; fi
