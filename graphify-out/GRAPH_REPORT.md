# Graph Report - caproom  (2026-08-23)

## Corpus Check
- 7 files · ~4,019 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 67 nodes · 72 edges · 8 communities (7 shown, 1 thin omitted)
- Extraction: 100% EXTRACTED · 0% INFERRED · 0% AMBIGUOUS
- Token cost: 0 input · 0 output

## Graph Freshness
- Built from commit: `20b2e9ef`
- Run `git rev-parse HEAD` and compare to check if the graph is stale.
- Run `graphify update .` after code changes (no API cost).

## Community Hubs (Navigation)
- package.json
- caproom
- caproom script
- keywords
- caproom.ps1
- caproom.js
- files
- hog.js

## God Nodes (most connected - your core abstractions)
1. `caproom` - 12 edges
2. `keywords` - 10 edges
3. `files` - 4 edges
4. `Import-Native()` - 3 edges
5. `Invoke-Capped()` - 3 edges
6. `repository` - 3 edges
7. `Invoke-Park()` - 2 edges
8. `ConvertTo-ArgString()` - 2 edges
9. `bin` - 2 edges
10. `Usage` - 2 edges

## Surprising Connections (you probably didn't know these)
- None detected - all connections are within the same source files.

## Import Cycles
- None detected.

## Communities (8 total, 1 thin omitted)

### Community 0 - "package.json"
Cohesion: 0.14
Nodes (13): bin, caproom, description, license, name, os, repository, type (+5 more)

### Community 1 - "caproom"
Cohesion: 0.14
Nodes (13): caproom, Contributing, Flags, init — auto-cap a command on every launch, Install, License, Limitations, park / wake — reclaim idle memory without killing (+5 more)

### Community 2 - "caproom script"
Cohesion: 0.38
Nodes (9): caproom script, cmd_init(), cmd_park(), cmd_status(), cmd_wake(), docker_available(), run_docker(), run_watchdog() (+1 more)

### Community 3 - "keywords"
Cohesion: 0.20
Nodes (10): keywords, ai-agent, cgroup, idle-memory, macos, memory, oom, park (+2 more)

### Community 4 - "caproom.ps1"
Cohesion: 0.31
Nodes (4): ConvertTo-ArgString(), Import-Native(), Invoke-Capped(), Invoke-Park()

### Community 5 - "caproom.js"
Cohesion: 0.50
Nodes (3): args, { join }, { spawnSync }

### Community 6 - "files"
Cohesion: 0.50
Nodes (4): files, bin/caproom, bin/caproom.js, bin/caproom.ps1

## Knowledge Gaps
- **37 isolated node(s):** `{ spawnSync }`, `{ join }`, `args`, `name`, `version` (+32 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **1 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `keywords` connect `keywords` to `package.json`?**
  _High betweenness centrality (0.092) - this node is a cross-community bridge._
- **Why does `files` connect `files` to `package.json`?**
  _High betweenness centrality (0.035) - this node is a cross-community bridge._
- **What connects `{ spawnSync }`, `{ join }`, `args` to the rest of the system?**
  _37 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `package.json` be split into smaller, more focused modules?**
  _Cohesion score 0.14285714285714285 - nodes in this community are weakly interconnected._
- **Should `caproom` be split into smaller, more focused modules?**
  _Cohesion score 0.14285714285714285 - nodes in this community are weakly interconnected._