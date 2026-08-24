#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAP="$ROOT/bin/caproom"
TMP="/tmp/caproom-edge-pty"
mkdir -p "$TMP"
pass=0; fail=0
ok(){ echo "✓ $1"; pass=$((pass+1)); }
bad(){ echo "✗ $1"; fail=$((fail+1)); }

# helper: run caproom and capture exit + stderr
run_cap() { "$CAP" "$@" 2> "$TMP/cap.err" > "$TMP/cap.out"; echo $?; }

echo "== edge cases =="

# 1. is_known_tui detection via pty bypass (tty) vs piped
echo "--- 1. is_known_tui + tty ---"
mkdir -p /tmp/edge-pty-bin
cat > /tmp/edge-pty-bin/opencode <<'EOS'
#!/usr/bin/env bash
exec node /Users/kalinovdameus/Developer/caproom/test/fake-tui.js tui-leak-check
EOS
cat > /tmp/edge-pty-bin/myapp <<'EOS'
#!/usr/bin/env bash
exec node /Users/kalinovdameus/Developer/caproom/test/fake-tui.js tui-leak-check
EOS
chmod +x /tmp/edge-pty-bin/opencode /tmp/edge-pty-bin/myapp
export PATH="/tmp/edge-pty-bin:$PATH"

# tty opencode should bypass (detached watch) — use direct script without extra bash -c to preserve pty
if script -q /dev/null bin/caproom --limit 512 -- opencode > /tmp/edge1.log 2> /tmp/edge1.err; then
  if grep -q "tty TUI detected" /tmp/edge1.log /tmp/edge1.err 2>/dev/null; then ok "tty opencode → bypass"; else bad "tty opencode not bypass"; cat /tmp/edge1.err | head -n 20; cat /tmp/edge1.log | head -n 10; fi
else
  if grep -q "tty TUI detected" /tmp/edge1.log /tmp/edge1.err 2>/dev/null; then ok "tty opencode → bypass (exit non-zero)"; else bad "tty opencode bypass fail"; cat /tmp/edge1.err | head -n 20; cat /tmp/edge1.log | head -n 20; fi
fi

# tty myapp (non-TUI) should NOT bypass, should be watchdog
if script -q /dev/null bin/caproom --limit 512 -- myapp > /tmp/edge2.log 2> /tmp/edge2.err; then
  if grep -q "watchdog backend" /tmp/edge2.log /tmp/edge2.err 2>/dev/null; then ok "tty myapp → watchdog (not TUI)"; else bad "tty myapp not watchdog"; cat /tmp/edge2.err | head -n 20; cat /tmp/edge2.log | head -n 10; fi
else
  if grep -q "watchdog backend" /tmp/edge2.err; then ok "tty myapp → watchdog"; else bad "tty myapp fail"; cat /tmp/edge2.err | head -n 10; fi
fi

# piped opencode should be watchdog (not tty, so not bypass)
if bin/caproom --limit 512 -- opencode > /tmp/edge3.log 2> /tmp/edge3.err; then
  if grep -q "watchdog backend" /tmp/edge3.err; then ok "piped opencode → watchdog (not tty)"; else bad "piped opencode not watchdog"; cat /tmp/edge3.err | head -n 10; fi
else
  if grep -q "watchdog backend" /tmp/edge3.err; then ok "piped opencode → watchdog"; else bad "piped opencode fail"; cat /tmp/edge3.err | head -n 10; fi
fi

# 2. CAPROOM_BYPASS_TTY=1 forces bypass even for non-TUI piped
echo "--- 2. CAPROOM_BYPASS_TTY=1 ---"
if CAPROOM_BYPASS_TTY=1 bin/caproom --limit 512 -- myapp > /tmp/edge4.log 2> /tmp/edge4.err; then
  if grep -q "bypass-tty" /tmp/edge4.err; then ok "BYPASS_TTY=1 forces bypass"; else bad "BYPASS not forced"; cat /tmp/edge4.err | head -n 10; fi
else
  if grep -q "bypass-tty" /tmp/edge4.err; then ok "BYPASS_TTY=1 forces bypass"; else bad "BYPASS fail"; cat /tmp/edge4.err | head -n 10; fi
fi

# 3. --no-intercept-tty flag
echo "--- 3. --no-intercept-tty flag ---"
if bin/caproom --no-intercept-tty --limit 512 -- myapp > /tmp/edge5.log 2> /tmp/edge5.err; then
  if grep -q "bypass-tty" /tmp/edge5.err; then ok "--no-intercept-tty bypass"; else bad "--no-intercept-tty not bypass"; cat /tmp/edge5.err | head -n 10; fi
else
  if grep -q "bypass-tty" /tmp/edge5.err; then ok "--no-intercept-tty bypass"; else bad "fail"; cat /tmp/edge5.err | head -n 10; fi
fi

# 4. --pty flag
echo "--- 4. --pty flag ---"
if timeout 5 bin/caproom --pty --limit 512 -- echo hello > /tmp/edge6.log 2> /tmp/edge6.err; then
  if grep -q "pty mode" /tmp/edge6.err && grep -q "hello" /tmp/edge6.log; then ok "--pty echo hello"; else bad "--pty echo"; cat /tmp/edge6.err | head -n 10; cat /tmp/edge6.log | head -n 10; fi
else
  if grep -q "pty mode" /tmp/edge6.err; then ok "--pty mode triggered"; else bad "--pty fail"; cat /tmp/edge6.err | head -n 10; fi
fi

# piped --pty should still allocate pty (even if not tty)
if timeout 5 bin/caproom --pty --limit 512 -- bash -c 'tty' > /tmp/edge7.log 2> /tmp/edge7.err; then
  if grep -q "/dev/ttys" /tmp/edge7.log; then ok "--pty piped still allocates pty"; else bad "--pty piped not pty"; cat /tmp/edge7.log | head -n 10; cat /tmp/edge7.err | head -n 10; fi
else
  if grep -q "/dev/ttys" /tmp/edge7.log; then ok "--pty piped pty"; else bad "fail"; cat /tmp/edge7.log | head -n 10; fi
fi

# 5. CAPROOM_PTY=1
echo "--- 5. CAPROOM_PTY=1 ---"
if timeout 5 bash -c 'CAPROOM_PTY=1 bin/caproom --limit 512 -- echo hello' > /tmp/edge8.log 2> /tmp/edge8.err; then
  if grep -q "pty mode" /tmp/edge8.err; then ok "CAPROOM_PTY=1 triggers pty"; else bad "CAPROOM_PTY not trigger"; cat /tmp/edge8.err | head -n 10; fi
else
  if grep -q "pty mode" /tmp/edge8.err; then ok "CAPROOM_PTY triggers"; else bad "fail"; cat /tmp/edge8.err | head -n 10; fi
fi

# 6. --no-pty disables
echo "--- 6. --no-pty disables ---"
if CAPROOM_PTY=1 bin/caproom --no-pty --limit 512 -- echo hello > /tmp/edge9.log 2> /tmp/edge9.err; then
  if grep -q "watchdog backend" /tmp/edge9.err && ! grep -q "pty mode" /tmp/edge9.err; then ok "--no-pty disables pty"; else bad "--no-pty not disabled"; cat /tmp/edge9.err | head -n 10; fi
else
  if grep -q "watchdog backend" /tmp/edge9.err; then ok "--no-pty disables"; else bad "fail"; cat /tmp/edge9.err | head -n 10; fi
fi

# 7. pty exit code propagation
echo "--- 7. pty exit code ---"
timeout 5 bash -c 'python3 scripts/pty_wrapper.py bash -c "exit 42"; echo code=$?' > /tmp/edge10.log 2>&1
if grep -q "code=42" /tmp/edge10.log; then ok "pty_wrapper exit 42"; else bad "pty_wrapper exit 42"; cat /tmp/edge10.log | head -n 10; fi
if timeout 5 bin/caproom --pty --limit 512 -- bash -c 'exit 42' > /tmp/edge11.log 2> /tmp/edge11.err; then
  bad "pty exit 42 should be non-zero"
else
  ec=$?
  # bin/caproom exit code is from wait, should be 42
  # we need to capture correctly without pipe
  timeout 5 bash -c 'bin/caproom --pty --limit 512 -- bash -c "exit 42"; echo ec=$?' > /tmp/edge11b.log 2>&1
  if grep -q "ec=42" /tmp/edge11b.log; then ok "pty caproom exit 42 propagates"; else bad "pty exit not 42"; cat /tmp/edge11b.log | head -n 10; fi
fi

# 8. pty tree hog still caps
echo "--- 8. pty tree hog ---"
if timeout 15 bin/caproom --pty --limit 64 --grace 2 -- node test/tree-hog.js > /tmp/edge12.log 2>&1; then
  bad "pty tree-hog should be killed"
else
  if grep -q "exceeded" /tmp/edge12.log && grep -q "killing pty tree\|killing tree" /tmp/edge12.log; then ok "pty tree-hog capped"; else bad "pty tree-hog not capped"; cat /tmp/edge12.log | tail -n 20; fi
  # orphans
  sleep 1
  surv=$( (pgrep -f 'Buffer.alloc' 2>/dev/null || true) | wc -l | tr -d ' '); surv=${surv:-0}
  if [ "$surv" = "0" ]; then ok "pty tree no orphans"; else bad "pty tree orphans $surv"; pgrep -f 'Buffer.alloc' 2>/dev/null | head -n 5; fi
fi

# 9. pty stubborn hog
echo "--- 9. pty stubborn ---"
if timeout 15 bin/caproom --pty --limit 64 --grace 2 -- node test/stubborn-hog.js > /tmp/edge13.log 2>&1; then
  bad "pty stubborn should be killed"
else
  if grep -q "SIGKILLed" /tmp/edge13.log || grep -q "sweep" /tmp/edge13.log; then ok "pty stubborn SIGKILL"; else bad "pty stubborn no SIGKILL"; cat /tmp/edge13.log | tail -n 20; fi
  sleep 1
  surv=$( (pgrep -f 'Buffer.alloc' 2>/dev/null || true) | wc -l | tr -d ' '); surv=${surv:-0}
  if [ "$surv" = "0" ]; then ok "pty stubborn no orphans"; else bad "pty stubborn orphans $surv"; fi
fi

# 10. normal hog still caps (no pty)
echo "--- 10. normal hog ---"
if timeout 15 bin/caproom --limit 64 --grace 2 -- node test/hog.js > /tmp/edge14.log 2>&1; then
  bad "normal hog should be killed"
else
  if grep -q "exceeded" /tmp/edge14.log; then ok "normal hog capped"; else bad "normal hog not capped"; cat /tmp/edge14.log | tail -n 20; fi
fi

# 11. init snippet still correct after pty changes
echo "--- 11. init snippets ---"
bin/caproom init opencode --limit 6144 --grace 10 > /tmp/edge15.log 2> /tmp/edge15.err
if grep -q "opencode_capped" /tmp/edge15.log && grep -q "CAPROOM_BYPASS_TTY" /tmp/edge15.log; then ok "init opencode snippet"; else bad "init opencode"; cat /tmp/edge15.log | head -n 20; fi
if grep -q "warning.*TUI" /tmp/edge15.err; then ok "init TUI warning"; else bad "init warning"; cat /tmp/edge15.err | head -n 10; fi
bin/caproom init npm --limit 2048 > /tmp/edge16.log 2> /tmp/edge16.err
if ! grep -q "warning.*TUI" /tmp/edge16.err && grep -q "npm_capped" /tmp/edge16.log; then ok "init npm no TUI warning"; else bad "init npm"; cat /tmp/edge16.err | head -n 10; fi

# 12. bypass + pty precedence: --pty should win over bypass when both set?
echo "--- 12. --pty vs --no-intercept-tty precedence ---"
if timeout 5 bin/caproom --pty --no-intercept-tty --limit 512 -- echo hello > /tmp/edge17.log 2> /tmp/edge17.err; then
  # pty is checked first, so pty mode should win
  if grep -q "pty mode" /tmp/edge17.err; then ok "--pty wins over --no-intercept-tty"; else bad "precedence fail"; cat /tmp/edge17.err | head -n 10; fi
else
  if grep -q "pty mode" /tmp/edge17.err; then ok "pty wins"; else bad "fail"; cat /tmp/edge17.err | head -n 10; fi
fi

# 13. pty allocation fallback when python missing (simulate by PATH without python)
echo "--- 13. pty fallback when no python ---"
mkdir -p /tmp/empty-path
# include /bin for bash, /usr/bin for script
if timeout 5 bash -c 'PATH=/tmp/empty-path:/usr/bin:/bin bin/caproom --pty --limit 512 -- echo hello' > /tmp/edge18.log 2> /tmp/edge18.err; then
  if grep -q "pty allocated\|watchdog backend\|bypass" /tmp/edge18.err; then ok "pty fallback without python"; else bad "fallback fail"; cat /tmp/edge18.err | head -n 10; cat /tmp/edge18.log | head -n 10; fi
else
  if grep -q "pty allocated\|watchdog" /tmp/edge18.err; then ok "fallback without python"; else bad "fail"; cat /tmp/edge18.err | head -n 10; cat /tmp/edge18.log | head -n 10; fi
fi

# 14. top --json still works under pty load
echo "--- 14. top --json ---"
if bin/caproom top --json > /tmp/edge19.log 2>&1; then
  if python3 -c "import json; json.load(open('/tmp/edge19.log')); print('ok')" 2>&1 | grep -q ok; then ok "top --json valid"; else bad "top --json invalid"; cat /tmp/edge19.log | head -n 20; fi
else
  bad "top --json exit non-zero"
fi

echo ""
echo "=== edge result: $pass passed, $fail failed ==="
if [ "$fail" -gt 0 ]; then exit 1; fi
