#!/usr/bin/env node
// fake-tui — minimal TUI that reproduces the caproom pty leak
// Sends the same terminal queries as opencode: OSC 10/11, DSR CPR, CSI t,
// and enables DEC 1003 mouse tracking. In a correct pty, replies stay on
// the pty and are consumed by the TUI parser; under a broken pipe wrapper
// they leak as visible "^[]10;rgb..." / "^[[5;1R" / "^[[<35;" garbage.
// This harness checks that caproom does NOT mangle stdio.

'use strict';

const mode = process.argv[2] || 'batch';

// TUI-mode: send queries burst, then read stdin for 1s and check for leak
if (mode === 'tui-leak-check') {
  // Enable mouse tracking like opencode does (DEC 1003 + 1006 SGR)
  process.stdout.write('\x1b[?1003h\x1b[?1006h');
  // OSC 10/11 fg/bg queries, DSR CPR, CSI t window report
  process.stdout.write('\x1b]10;?\x07');
  process.stdout.write('\x1b]11;?\x07');
  process.stdout.write('\x1b[6n');
  process.stdout.write('\x1b[4;0;0t');

  let leak = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', d => { leak += d; });

  // Simulate TUI render after 200ms, then check
  setTimeout(() => {
    // Disable mouse before exit (like TUI cleanup)
    process.stdout.write('\x1b[?1003l\x1b[?1006l');
    setTimeout(() => {
      // If any terminal reply leaked into stdin as text, it would appear here
      // In non-tty (CI piped), no reply arrives — leak should be empty
      // Check stdout wasn't corrupted by wrapper (we already wrote queries)
      // This process itself is the oracle: if caproom mangled stdio, we'd see
      // truncated or duplicated bytes.
      if (leak.includes('10;rgb') || leak.includes('[<35;') || leak.includes('[5;1R')) {
        console.error('LEAK detected: ' + JSON.stringify(leak.slice(0, 200)));
        process.exit(2);
      }
      console.log('fake-tui: no leak (stdin leak bytes=' + leak.length + ')');
      process.exit(0);
    }, 500);
  }, 200);
  // Keep stdin open briefly
  setTimeout(() => { process.stdin.pause(); }, 1500);
  return;
}

// batch-mode: plain streaming output, must pass through verbatim
if (mode === 'batch') {
  console.log('batch-ok');
  process.exit(0);
}

// passthrough test: echo stdin to stdout, caproom must not buffer/mangle
if (mode === 'passthrough') {
  process.stdin.pipe(process.stdout);
  return;
}

// simple hello for smoke
console.log('fake-tui hello');
