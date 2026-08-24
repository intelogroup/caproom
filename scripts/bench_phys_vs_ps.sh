#!/usr/bin/env bash
# bench: phys_footprint (libproc) vs ps RSS — cheap before more feature work
set -euo pipefail
CAPROOM="./target/debug/caproom"
if [[ ! -x "$CAPROOM" ]]; then CAPROOM="./target/release/caproom"; fi
echo "bench phys_footprint (libproc) vs ps RSS"
echo "warming..."
for i in 1 2 3; do "$CAPROOM" top --json >/dev/null; ps -eo pid= >/dev/null; done
echo "timing libproc (caproom top --json) 20 runs"
time for i in $(seq 1 20); do "$CAPROOM" top --json >/dev/null; done
echo "timing ps (ps -eo pid,ppid,rss) 20 runs"
time for i in $(seq 1 20); do ps -eo pid=,ppid=,rss=,state=,command= >/dev/null; done
echo "single snapshot comparison"
echo "libproc:"
time "$CAPROOM" top --json >/dev/null
echo "ps:"
time bash -c 'ps -eo pid=,ppid=,pgid=,rss=,state=,command= >/dev/null'
echo "note: libproc should be faster (no spawn+parse) and more accurate (phys_footprint vs RSS shared overcount)"
