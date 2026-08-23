#!/usr/bin/env node
// caproom-mcp — MCP server wrapping the caproom CLI so coding agents can
// discover, freeze, and restore memory-heavy process trees natively.
//
// Tools:
//   top    {pid?, park_min_mb?}     read-only tree inventory (stable schema)
//   park   {pid}                    SIGSTOP an idle tree's root
//   wake   {pid}                    SIGCONT it back
//   watch_start  {pids[], threshold_mb?, auto_park?, auto_wake_free_pct?, interval?}
//   watch_events {id}               drain NDJSON events from a running watcher
//   watch_stop   {id}
//   run    {command[], limit_mb?, grace?, image?, docker?}
//                  cap a command end-to-end; returns exit code + stderr tail
//
// Hand-rolled MCP stdio transport (newline-delimited JSON-RPC 2.0): zero
// dependencies, same rule as the CLI itself.

'use strict';

const { spawn, spawnSync } = require('child_process');
const os = require('os');
const path = require('path');

const CAPROOM = path.join(__dirname, process.platform === 'win32' ? 'caproom.ps1' : 'caproom');
if (process.platform !== 'win32') {
  try { require('fs').chmodSync(CAPROOM, 0o755); } catch (_) { /* already exec */ }
}

const watchers = new Map();
let watcherSeq = 0;

function caproom(args, opts = {}) {
  const exe = process.platform === 'win32' ? 'powershell.exe' : 'bash';
  const argv = process.platform === 'win32' ? ['-NoProfile', '-File', CAPROOM].concat(args) : [CAPROOM].concat(args);
  return spawnSync(exe, argv, { encoding: 'utf8', timeout: opts.timeout || 30000 });
}

function text(s) {
  return { content: [{ type: 'text', text: String(s) }] };
}

function startWatcher(p) {
  const id = 'w' + (++watcherSeq);
  const args = ['watch', '--json'];
  if (p.threshold_mb != null) args.push('--threshold-mb', String(p.threshold_mb));
  if (p.interval != null) args.push('--interval', String(p.interval));
  if (p.auto_park) args.push('--auto-park');
  if (p.auto_wake_free_pct != null) args.push('--auto-wake-free-pct', String(p.auto_wake_free_pct));
  const pids = Array.isArray(p.pids) ? p.pids : [];
  if (!pids.length) throw new Error('pids[] required');
  for (const x of pids) args.push(String(x));

  const exe = process.platform === 'win32' ? 'powershell.exe' : 'bash';
  const argv = process.platform === 'win32' ? ['-NoProfile', '-File', CAPROOM].concat(args) : [CAPROOM].concat(args);
  const child = spawn(exe, argv, { stdio: ['ignore', 'pipe', 'pipe'], detached: false });
  const rec = {
    id, child, pids,
    events: [], buf: '', stderr: [],
  };
  child.stdout.on('data', d => {
    rec.buf += d.toString();
    let i;
    while ((i = rec.buf.indexOf('\n')) >= 0) {
      const line = rec.buf.slice(0, i).trim();
      rec.buf = rec.buf.slice(i + 1);
      if (!line) continue;
      try { rec.events.push(JSON.parse(line)); } catch (_) { /* partial */ }
      if (rec.events.length > 1000) rec.events.splice(0, rec.events.length - 1000);
    }
  });
  child.stderr.on('data', d => {
    rec.stderr.push(d.toString());
    if (rec.stderr.length > 50) rec.stderr.splice(0, rec.stderr.length - 50);
  });
  child.on('exit', () => { rec.exited = true; });
  watchers.set(id, rec);
  return { id, pids };
}

function drainWatcher(id, clear) {
  const rec = watchers.get(id);
  if (!rec) throw new Error('unknown watcher id: ' + id);
  const out = rec.events.slice();
  if (clear) rec.events = [];
  return { events: out, exited: !!rec.exited, stderr_tail: rec.stderr.join('').slice(-2000) };
}

function stopWatcher(id) {
  const rec = watchers.get(id);
  if (!rec) throw new Error('unknown watcher id: ' + id);
  if (!rec.exited) {
    try { rec.child.kill(process.platform === 'win32' ? undefined : 'SIGTERM'); } catch (_) {}
  }
  const out = drainWatcher(id, true);
  watchers.delete(id);
  return out;
}

function callTool(name, args) {
  switch (name) {
    case 'top': {
      const a = ['top', '--json'];
      if (args.pid != null) a.push('--pid', String(args.pid));
      if (args.park_min_mb != null) a.push('--park-min-mb', String(args.park_min_mb));
      const r = caproom(a);
      if (r.status !== 0) return text(r.stderr || 'top failed');
      return text(r.stdout.trim());
    }
    case 'park':
    case 'wake': {
      if (args.pid == null) return text('pid required');
      const r = caproom([name, String(args.pid)]);
      return text((r.stderr || '').trim() + (r.status === 0 ? '' : `\nexit=${r.status}`));
    }
    case 'watch_start': {
      const { id, pids } = startWatcher(args);
      return text(JSON.stringify({ id, pids, note: 'poll watch_events{ id } to drain NDJSON events' }));
    }
    case 'watch_events': {
      return text(JSON.stringify(drainWatcher(String(args.id), args.clear !== false)));
    }
    case 'watch_stop': {
      return text(JSON.stringify(stopWatcher(String(args.id))));
    }
    case 'run': {
      const cmd = Array.isArray(args.command) ? args.command : null;
      if (!cmd || !cmd.length) return text('command[] required');
      const a = [];
      a.push('--limit', String(args.limit_mb != null ? args.limit_mb : 4096));
      if (args.grace != null) a.push('--grace', String(args.grace));
      if (args.docker) { a.push('--docker'); if (args.image) a.push('--image', String(args.image)); }
      a.push('--');
      const r = caproom(a.concat(cmd), { timeout: Math.max(60000, (args.timeout_ms || 300000)) });
      const errText = (r.stderr || '');
      const capped = /exceeded .* cap/.test(errText);
      let verdict;
      if (r.status === 137 || r.signal === 'SIGKILL') verdict = 'RESULT: KILLED BY CAP (exit 137)';
      else if (capped && (r.status === 143 || r.signal === 'SIGTERM')) verdict = 'RESULT: CAPPED — tree killed during grace (SIGTERM honored, exit 143)';
      else if (r.status === 143 || r.signal === 'SIGTERM') verdict = 'RESULT: terminated during grace (SIGTERM honored)';
      else verdict = 'RESULT: exit=' + r.status;
      return text(verdict + '\n--- stderr ---\n' + (errText.slice(-4000) || '(empty)'));
    }
    default:
      throw new Error('unknown tool: ' + name);
  }
}

const TOOLS = [
  {
    name: 'top',
    description: 'Snapshot every process tree you own, sorted by tree RSS. Read-only. Returns caproom\'s stable schema-1 JSON: rows are tree roots with cmd, tree_rss_kb, tree_pids (blast radius), state (running|parked|zombie), park_candidate + reason heuristic.',
    inputSchema: {
      type: 'object',
      properties: {
        pid: { type: 'number', description: 'restrict to one subtree' },
        park_min_mb: { type: 'number', description: 'park-candidate threshold in MB (default 512)' },
      },
    },
  },
  {
    name: 'park',
    description: 'SIGSTOP an idle process (and only its root — use watch auto_park for whole trees). Pages become eligible for lazy kernel reclaim under pressure; not immediate RAM return. Only park genuinely idle processes — never one an agent awaits a reply from.',
    inputSchema: { type: 'object', properties: { pid: { type: 'number' } }, required: ['pid'] },
  },
  {
    name: 'wake',
    description: 'SIGCONT a parked process back to life, same state, same PID.',
    inputSchema: { type: 'object', properties: { pid: { type: 'number' } }, required: ['pid'] },
  },
  {
    name: 'watch_start',
    description: 'Start a caproom watch daemon on explicit pids. Naming pids is the opt-in; there is no system-wide mode. Emits NDJSON events (started/breach/parked/recovered/woke/all-exited). auto_park freezes whole breaching trees; auto_wake_free_pct restores its own parks when free memory recovers.',
    inputSchema: {
      type: 'object',
      properties: {
        pids: { type: 'array', items: { type: 'number' }, description: 'explicit pids to watch' },
        threshold_mb: { type: 'number', description: 'per-tree breach threshold (default 2048)' },
        auto_park: { type: 'boolean' },
        auto_wake_free_pct: { type: 'number' },
        interval: { type: 'number', description: 'poll seconds (default 5)' },
      },
      required: ['pids'],
    },
  },
  {
    name: 'watch_events',
    description: 'Drain accumulated NDJSON events from a watcher (clears them unless clear=false). Also reports whether the watcher exited.',
    inputSchema: { type: 'object', properties: { id: { type: 'string' }, clear: { type: 'boolean' } }, required: ['id'] },
  },
  {
    name: 'watch_stop',
    description: 'Stop a watcher and return its final drained events.',
    inputSchema: { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] },
  },
  {
    name: 'run',
    description: 'Run a command under a caproom memory cap (default watchdog backend; docker:true opts into the container cgroup backend). Kills the whole tree on breach. Returns verdict line (KILLED BY CAP at exit 137) plus captured stderr.',
    inputSchema: {
      type: 'object',
      properties: {
        command: { type: 'array', items: { type: 'string' }, description: 'argv, e.g. ["npm","run","build"]' },
        limit_mb: { type: 'number' },
        grace: { type: 'number', description: 'seconds between SIGTERM and SIGKILL (default 5)' },
        docker: { type: 'boolean' },
        image: { type: 'string' },
        timeout_ms: { type: 'number', description: 'default 300000' },
      },
      required: ['command'],
    },
  },
];

process.stdin.setEncoding('utf8');
let inbuf = '';

function send(obj) {
  process.stdout.write(JSON.stringify(obj) + '\n');
}

function handle(msg) {
  if (msg.method === 'initialize') {
    send({
      jsonrpc: '2.0', id: msg.id,
      result: {
        protocolVersion: msg.params && msg.params.protocolVersion || '2024-11-05',
        capabilities: { tools: {} },
        serverInfo: { name: 'caproom-mcp', version: require('../package.json').version },
      },
    });
  } else if (msg.method === 'tools/list') {
    send({ jsonrpc: '2.0', id: msg.id, result: { tools: TOOLS } });
  } else if (msg.method === 'tools/call') {
    const { name, arguments: args } = msg.params || {};
    try {
      const res = callTool(name, args || {});
      send({ jsonrpc: '2.0', id: msg.id, result: res });
    } catch (e) {
      send({ jsonrpc: '2.0', id: msg.id, result: { content: [{ type: 'text', text: 'error: ' + e.message }], isError: true } });
    }
  } else if (msg.method === 'ping') {
    send({ jsonrpc: '2.0', id: msg.id, result: {} });
  } else if (msg.id !== undefined) {
    send({ jsonrpc: '2.0', id: msg.id, error: { code: -32601, message: 'method not found: ' + msg.method } });
  }
  // notifications (initialized, etc.) get no reply
}

process.stdin.on('data', chunk => {
  inbuf += chunk;
  let i;
  while ((i = inbuf.indexOf('\n')) >= 0) {
    const line = inbuf.slice(0, i).trim();
    inbuf = inbuf.slice(i + 1);
    if (!line) continue;
    let msg;
    try { msg = JSON.parse(line); } catch (_) { continue; }
    try { handle(msg); } catch (e) {
      if (msg && msg.id !== undefined) {
        send({ jsonrpc: '2.0', id: msg.id, error: { code: -32603, message: String(e.message || e) } });
      }
    }
  }
});

process.on('disconnect', () => {
  for (const id of Array.from(watchers.keys())) stopWatcher(id);
});
