use std::io::{BufRead, Write};

/// EPIPE-safe write: a client that disconnects mid-session must not panic the
/// server — exit cleanly instead.
fn send(stdout: &mut std::io::Stdout, resp: &serde_json::Value) {
    if writeln!(stdout, "{}", resp).is_err() {
        std::process::exit(0);
    }
    let _ = stdout.flush();
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(j) => j,
            Err(e) => {
                let err = serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}});
                send(&mut stdout, &err);
                continue;
            }
        };
        let id = v.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let resp = match method {
            "initialize" => serde_json::json!({
                "jsonrpc":"2.0","id":id,
                "result":{
                    "protocolVersion":"2024-11-05",
                    "serverInfo":{"name":"caproom-mcp","version":env!("CARGO_PKG_VERSION")},
                    "capabilities":{"tools":{"listChanged":false}}
                }
            }),
            "notifications/initialized" => continue,
            "tools/list" => serde_json::json!({
                "jsonrpc":"2.0","id":id,
                "result":{
                    "tools":[
                        {"name":"top","description":"tree inventory — typed TopResponse","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"additionalProperties":false}},
                        {"name":"park","description":"SIGSTOP single PID","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}},
                        {"name":"park_tree","description":"SIGSTOP whole tree — PID reuse guarded","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}},
                        {"name":"wake","description":"SIGCONT single PID","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}},
                        {"name":"wake_tree","description":"SIGCONT whole tree","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}},
                        {"name":"run","description":"execute command under caproom run --limit","inputSchema":{"type":"object","properties":{"command":{"type":"array","items":{"type":"string"}},"limit_mb":{"type":"integer"}},"required":["command"]}},
                        {"name":"freemem","description":"free memory %","inputSchema":{"type":"object","properties":{}}},
                        {"name":"status","description":"ps stat for pid","inputSchema":{"type":"object","properties":{"pid":{"type":"integer"}},"required":["pid"]}}
                    ]
                }
            }),
            "tools/call" => {
                let params = v.get("params").cloned().unwrap_or(serde_json::Value::Null);
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                match name {
                    "top" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).map(|p| p as i32);
                        let r = caproom_mcp::handle_top(pid);
                        send(
                            &mut stdout,
                            &serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"content":[{"type":"text","text": serde_json::to_string(&r).unwrap()}]}
                            }),
                        );
                    }
                    "park" | "park_tree" | "wake" | "wake_tree" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).unwrap_or(0) as i32;
                        let r = match name {
                            "park" => caproom_mcp::handle_park(pid),
                            "park_tree" => caproom_mcp::handle_park_tree(pid),
                            "wake" => caproom_mcp::handle_wake(pid),
                            _ => caproom_mcp::handle_wake_tree(pid),
                        };
                        send(
                            &mut stdout,
                            &serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"content":[{"type":"text","text": r.to_string()}]}
                            }),
                        );
                    }
                    "run" => {
                        let cmd = args
                            .get("command")
                            .and_then(|c| c.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let limit = args
                            .get("limit_mb")
                            .and_then(|l| l.as_u64())
                            .unwrap_or(4096);
                        let r = caproom_mcp::handle_run(cmd, limit);
                        send(
                            &mut stdout,
                            &serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"content":[{"type":"text","text": r.to_string()}]}
                            }),
                        );
                    }
                    "freemem" => {
                        let pct = caproom_core::pressure::free_mem_pct();
                        let text =
                            serde_json::to_string(&serde_json::json!({"free_pct": pct})).unwrap();
                        send(
                            &mut stdout,
                            &serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"content":[{"type":"text","text": text}]}
                            }),
                        );
                    }
                    "status" => {
                        let pid = args.get("pid").and_then(|p| p.as_i64()).unwrap_or(0) as i32;
                        let snap = caproom_core::collector::snapshot_current_user();
                        let text = if let Some(pr) = snap.by_pid(pid) {
                            serde_json::to_string(&serde_json::json!({"pid": pr.pid, "state": pr.state.to_string(), "footprint_kb": pr.footprint_kb, "cmd": pr.cmd})).unwrap()
                        } else {
                            serde_json::to_string(
                                &serde_json::json!({"error": format!("no such pid {}", pid)}),
                            )
                            .unwrap()
                        };
                        send(
                            &mut stdout,
                            &serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "result":{"content":[{"type":"text","text": text}]}
                            }),
                        );
                    }
                    _ => {
                        // spec: unknown tool is a JSON-RPC error object, not a success result
                        send(
                            &mut stdout,
                            &serde_json::json!({
                                "jsonrpc":"2.0","id":id,
                                "error":{"code":-32601,"message": format!("unknown tool {}", name)}
                            }),
                        );
                    }
                }
                continue;
            }
            _ => {
                serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message": format!("unknown method {}", method)}})
            }
        };
        send(&mut stdout, &resp);
    }
}
